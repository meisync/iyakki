//! Wi-Fi and Bluetooth firmware for an Intel Mac, from Apple's own files, at the person's request (IYAKKI menu -> Mac drivers, or
//! Quick install -> "Mac Wi-Fi and Bluetooth firmware"). One detached job (`iyakki quick-install mac-firmware`), three sources:
//!
//! * **Download**: Apple's recovery image for this Mac model (`macrecovery.rs`; the board id is the DMI `board_name` of any Intel Mac,
//!   `iyakki_drivers::macboard::resolve`, the table there is only a cross-check), read with 7-Zip (`7zz`, installed from the CLI list) for `usr/share/firmware`.
//! * **Image**: a disk image file the person picked (a `.dmg` or an `.img` of a macOS volume on a stick or in the data folder),
//!   read the same way. A macOS volume is never mounted: the kernel has no APFS driver (and HFS+ is not what T2 Macs use), and
//!   no drive is ever written or read as a whole device.
//! * **Folder**: a folder or stick with the firmware: either the files `dist/drivers/mac-wifi-firmware-from-macos.sh` wrote (the
//!   Linux names already) or a copy of macOS's `usr/share/firmware`.
//!
//! A T2 Mac needs Apple's whole Wi-Fi firmware set and (BCM4364) a Bluetooth patch. A Mac without a T2 chip (BCM4350 / BCM43602 Wi-Fi,
//! UART Bluetooth) already has the generic firmware from linux-firmware: the job fetches only the NVRAM, CLM and txcap files named after
//! the DMI board type, and the `.hcd` for the controller's chip (`appfw` board mode); the T2 hold-back never applies to it.
//!
//! The firmware then goes through the Rust twin of that script (`iyakki_drivers::appfw`: the same names, the same rules), into two
//! driver packs on the data volume (`mac-wifi-brcmfmac` and `mac-bt-broadcom`, so they are linked into `/lib/firmware` at every
//! boot) and the drivers are loaded again; the job then looks for the Wi-Fi interface and the Bluetooth address and writes a plain
//! result (`<data>/mac-firmware/last.json`) the Mac drivers page shows.
//!
//! The files are Apple's: they stay on this machine, are never uploaded and are not part of IYAKKI ([`NOTICE`]). Temporary files go
//! to the data volume (never RAM) and are removed at the end; a partial download is kept for the next try. Nothing writes a drive
//! other than the data volume.
//!
//! The world is reached only through [`Env`]; the tests give it a fake (no network, no devices, no Apple files).

use std::path::{Path, PathBuf};
use std::time::Duration;

use iyakki_drivers::{appfw, apply, known, macboard, pack};

/// The notice shown before anything starts and again at the top of the log.
/// Said plainly (the job's lines, Settings -> Bluetooth, the Mac drivers page) when macOS has no patch file for the BCM4364 UART controller.
pub const NO_BCM4364_FILE: &str = "This Mac's Bluetooth chip has no firmware file in macOS: the controller runs in its ROM state (limited); a USB Bluetooth adapter is the reliable option";

/// The same sentence for any UART controller chip (`BCM4350C0`).
pub fn no_hcd_message(chip: &str) -> String {
    if chip == "BCM4364" {
        NO_BCM4364_FILE.to_string()
    } else {
        format!("This Mac's Bluetooth chip ({chip}) has no firmware file in macOS: the controller runs in its ROM state (limited); a USB Bluetooth adapter is the reliable option")
    }
}

/// What a Mac without a T2 chip is told before the job starts (the log's second line and the Mac drivers page).
pub const NON_T2_PLAN: &str = "This Mac has no T2 chip. It will fetch only the Wi-Fi NVRAM and CLM files for its BCM4350 / BCM43602 chip and the Bluetooth patch file for its UART chip. Wi-Fi works without them (the generic firmware is on the image), with limits: fewer channels and country settings. The Wi-Fi and Bluetooth drivers are loaded again at the end, so the network may drop for a moment";

/// What "Load the Bluetooth driver anyway" does, in one sentence (the button's explanation).
pub const LOAD_ANYWAY_HELP: &str = "Load the Bluetooth driver anyway starts the controller without a firmware file: it registers, but has no valid address, so it can be switched on and can scan, and cannot pair (pairing and audio need a Bluetooth stack, which IYAKKI does not have yet)";

pub const NOTICE: &str = "These files are Apple's. They stay on this machine, are never uploaded and are not part of IYAKKI.";

/// The ready-to-paste prompt for Claude Code, Copilot CLI or the IYAKKI assistant (the Mac drivers page copies it; the same text is in
/// docs/MAC-WIFI-BLUETOOTH-SETUP.md, and a test keeps the two equal).
pub const CHAT_PROMPT: &str = include_str!("macfw_prompt.txt");

pub const PACK_WIFI: &str = "mac-wifi-brcmfmac";
pub const PACK_BT: &str = "mac-bt-broadcom";
/// Free space wanted on the data volume for a download (the image is 1 to 2 GB, the unpacked part is small).
pub const NEEDED_FREE: u64 = 5 << 29;
/// Free space wanted for the fallback that reads the whole image (about 2.2 GB written next to the 0.9 GB image).
pub const NEEDED_FULL: u64 = 7 << 29;

/// Where the files come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Download,
    /// A disk image file.
    Image(PathBuf),
    /// A folder (a stick): the script's output or a copy of macOS's firmware folder.
    Folder(PathBuf),
}

impl Source {
    /// `--source download|image|folder [--path P]`.
    pub fn from_args(args: &[String]) -> Result<Source, String> {
        let flag = |n: &str| args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).cloned();
        let path = flag("--path");
        match flag("--source").as_deref().unwrap_or("download") {
            "download" => Ok(Source::Download),
            "image" => path.map(|p| Source::Image(p.into())).ok_or_else(|| "--source image needs --path <disk image file>".into()),
            "folder" => path.map(|p| Source::Folder(p.into())).ok_or_else(|| "--source folder needs --path <folder>".into()),
            other => Err(format!("unknown source `{other}` (download, image or folder)")),
        }
    }

    pub fn args(&self) -> Vec<String> {
        match self {
            Source::Download => vec!["--source".into(), "download".into()],
            Source::Image(p) => vec!["--source".into(), "image".into(), "--path".into(), p.display().to_string()],
            Source::Folder(p) => vec!["--source".into(), "folder".into(), "--path".into(), p.display().to_string()],
        }
    }

    pub fn words(&self) -> String {
        match self {
            Source::Download => "Apple's recovery image for this Mac".into(),
            Source::Image(p) => format!("the disk image {}", p.display()),
            Source::Folder(p) => format!("the folder {}", p.display()),
        }
    }
}

/// Where the job keeps its files on the data volume.
pub fn work_dir(data: &Path) -> PathBuf {
    data.join("mac-firmware")
}

pub fn result_path(data: &Path) -> PathBuf {
    work_dir(data).join("last.json")
}

/// What the machine shows after the drivers were loaded again.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Seen {
    /// Wireless interfaces (`wlan0`).
    pub wifi: Vec<String>,
    /// A Bluetooth controller (`hci0`) exists.
    pub bt_controller: bool,
    /// Its address when it has one (the controller started).
    pub bt_address: Option<String>,
}

/// Everything the job does to the world.
pub trait Env {
    fn say(&mut self, line: &str);
    /// The DMI product name (`Macmini8,1`).
    fn product(&self) -> String;
    /// The DMI board name (`Mac-7BA5B2DFE22DDD8C`): on an Intel Mac it is the board id Apple's recovery service wants. Empty when unknown.
    fn board_name(&self) -> String {
        String::new()
    }
    /// The DMI system vendor (`Apple Inc.`): brcmfmac names the board files `<vendor>-<product>`.
    fn vendor(&self) -> String {
        "Apple Inc.".into()
    }
    /// The kernel log about Bluetooth (`dmesg`): the UART controller's chip name and the firmware names btbcm tried. Empty when unknown.
    fn bt_log(&mut self) -> String {
        String::new()
    }
    fn free_bytes(&self, dir: &Path) -> Option<u64>;
    /// The data folder is on a RAM file system (a live session).
    fn data_in_memory(&self) -> bool;
    /// A `7zz` that runs: found, or installed from the CLI list (saying so).
    fn ensure_7zz(&mut self) -> Result<PathBuf, String>;
    /// Download the recovery image of a candidate (board id, serial part, `os`) into `dir` (resuming), checked against Apple's chunk
    /// list; returns the file and the image's identity (product and address). Says the host and the byte counts.
    fn fetch_image(&mut self, c: &Candidate, dir: &Path) -> Result<(PathBuf, String), String>;
    /// Which image Apple would send for a candidate, without downloading it.
    fn image_id(&mut self, c: &Candidate) -> Result<String, String>;
    /// Unpack the firmware folders of a disk image into `out` with 7-Zip (`full`: the whole image without its big folders, for an image
    /// where the name filter found nothing); returns what it said. 7-Zip's exit codes 1 and 2 (some items could not be written: links
    /// and special files of the macOS volume) are not errors here; whether the firmware is there decides.
    fn unpack(&mut self, seven: &Path, image: &Path, out: &Path, full: bool) -> Result<String, String>;
    /// Put a pack on the data volume, link it and load the drivers again.
    fn install(&mut self, id: &str, version: &str, source: &str, pack: &pack::Pack) -> Result<apply::Report, String>;
    /// What the machine shows now.
    fn look(&mut self) -> Seen;
    /// Kernel log lines about the drivers, newest last.
    fn kernel_lines(&mut self) -> Vec<String>;
    fn sleep(&mut self, d: Duration);
    fn now(&self) -> i64;
}

/// The plain result of a run, also written to `last.json` for the Mac drivers page.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Outcome {
    pub at: i64,
    pub source: String,
    /// Packs were installed (the job succeeded).
    pub installed: bool,
    pub wifi_files: usize,
    pub bt_files: usize,
    pub seen: Seen,
    /// One short line for the page.
    pub headline: String,
    /// The detail lines (what happened, what is known).
    pub lines: Vec<String>,
    pub kernel: Vec<String>,
}

pub fn read_outcome(data: &Path) -> Option<Outcome> {
    serde_json::from_str(&std::fs::read_to_string(result_path(data)).ok()?).ok()
}

fn write_outcome(data: &Path, o: &Outcome) {
    let _ = std::fs::create_dir_all(work_dir(data));
    let _ = std::fs::write(result_path(data), serde_json::to_string_pretty(o).unwrap_or_default());
}

/// "1.<day>.<minute>": versions only go up, so a second run is newer (the same scheme as the other local packs).
fn version_for(now: i64) -> String {
    format!("1.{}.{}", (now / 86_400).max(1), (now % 86_400) / 60)
}

/// The names (not paths) of the files of a folder the macOS script wrote, taken from up to two levels down (no links), when they
/// are names one of the two entries accepts and fit a driver pack.
pub fn collect_named(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let accepted: Vec<&str> = [PACK_WIFI, PACK_BT].iter().filter_map(|id| known::by_id(id)).flat_map(|k| k.accept.iter().map(|a| a.0)).collect();
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0u32)];
    while let Some((d, depth)) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?.flatten() {
            let (p, Ok(t)) = (e.path(), e.file_type()) else { continue };
            if t.is_symlink() {
                continue;
            }
            if t.is_dir() && depth < 2 {
                stack.push((p, depth + 1));
            } else if t.is_file() {
                let name = e.file_name().to_string_lossy().into_owned();
                if accepted.iter().any(|a| pack::glob(a, &name)) && !out.iter().any(|(n, _)| *n == name) {
                    let len = e.metadata().map(|m| m.len()).unwrap_or(0);
                    if len > 0 && len as usize <= pack::MAX_FILE_BYTES {
                        out.push((name, std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?));
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Turn the files into the Wi-Fi pack (when there are any) and the Bluetooth pack (when there are any). Refuses a Wi-Fi set that does
/// not fit whole, and a set with neither.
pub fn build_packs(files: &[(String, Vec<u8>)], from: &str) -> Result<(Option<pack::Pack>, Option<pack::Pack>, Vec<String>), String> {
    let wk = known::by_id(PACK_WIFI).ok_or("Wi-Fi entry missing")?;
    let bk = known::by_id(PACK_BT).ok_or("Bluetooth entry missing")?;
    let accepts = |k: &known::Known, n: &str| k.accept.iter().any(|(pat, _)| pack::glob(pat, n));
    let too_big: Vec<&str> = files.iter().filter(|(n, b)| (accepts(wk, n) || accepts(bk, n)) && b.len() > pack::MAX_FILE_BYTES).map(|f| f.0.as_str()).collect();
    if !too_big.is_empty() {
        return Err(format!("{} is larger than a driver pack file may be ({} MiB); nothing was installed", too_big[0], pack::MAX_FILE_BYTES >> 20));
    }
    let wifi = apply::pack_from_named(wk, files, from);
    let mut taken = Vec::new();
    let wifi = match wifi {
        Ok((p, t)) => {
            taken.extend(t);
            Some(p)
        }
        Err(e) if e.contains("larger than") => return Err(format!("{e}. The firmware of several Mac models was found and this Mac's model is not in IYAKKI's table yet: run dist/drivers/mac-wifi-firmware-from-macos.sh in macOS on this Mac instead")),
        Err(e) => {
            // a Bluetooth-only set is fine; nothing at all is the Wi-Fi error
            if apply::pack_from_named(bk, files, from).is_err() {
                return Err(e);
            }
            None
        }
    };
    let bt = match apply::pack_from_named(bk, files, from) {
        Ok((p, t)) => {
            taken.extend(t);
            Some(p)
        }
        Err(_) => None,
    };
    Ok((wifi, bt, taken))
}

/// The firmware files from a source tree: macOS-shaped (`wifi/`, `bluetooth/`) or already named for Linux.
fn files_from_tree(tree: &Path, opts: &appfw::Options, out: &mut Vec<String>) -> Result<Vec<(String, Vec<u8>)>, String> {
    if let Some(root) = appfw::find_firmware_root(tree, 8) {
        out.push(format!("found macOS's firmware folder at {}", root.strip_prefix(tree).unwrap_or(&root).display()));
        let x = appfw::extract(&root, opts)?;
        out.push(format!("{} Wi-Fi firmware set(s), {} Bluetooth file(s){}", x.wifi_sets, x.bt_files, if x.hcd { ", and BCM.hcd" } else { "" }));
        out.extend(x.notes.iter().cloned());
        return Ok(x.files);
    }
    let named = collect_named(tree)?;
    if named.is_empty() {
        return Err(format!("no firmware found in {}: neither macOS's firmware folder (usr/share/firmware with wifi and bluetooth) nor the files the macOS script wrote (brcmfmac*.bin, BCM.hcd, ...)", tree.display()));
    }
    out.push(format!("{} file(s) already named for Linux", named.len()));
    Ok(named)
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn remove_dir(p: &Path) {
    let _ = std::fs::remove_dir_all(p);
}

/// Run the job. Returns the outcome (already written to `last.json`); `Err` only when nothing could be installed.
pub fn run(env: &mut dyn Env, data: &Path, source: &Source) -> Result<Outcome, String> {
    let mut lines: Vec<String> = Vec::new();
    let r = run_inner(env, data, source, &mut lines);
    match r {
        Ok(o) => Ok(o),
        Err(e) => {
            // the download and the unpacked folder stay: a retry resumes, and the last failure can be looked at (Delete download clears them)
            let kept = leftovers(data);
            let mut lines = lines;
            if kept > 0 {
                lines.push(format!("kept for a retry or a look: {} MB in {} (the Mac drivers page has Delete download)", kept >> 20, work_dir(data).display()));
            }
            let cs = card_status(data);
            if !cs.is_empty() {
                lines.push(cs);
            }
            let o = Outcome { at: env.now(), source: source.words(), installed: false, headline: format!("Not done: {e}"), lines, ..Default::default() };
            write_outcome(data, &o);
            Err(e)
        }
    }
}

/// One line of the log (the job's output) and of the result.
fn note(env: &mut dyn Env, lines: &mut Vec<String>, l: String) {
    env.say(&l);
    lines.push(l);
}

/// Bytes the job keeps on the data volume between runs: a downloaded image (kept after a failure so a retry does not download again)
/// and the unpacked folder.
pub fn leftovers(data: &Path) -> u64 {
    fn size(p: &Path) -> u64 {
        let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
        if m.is_dir() {
            std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| size(&e.path())).sum()
        } else {
            m.len()
        }
    }
    ["download", "unpacked"].iter().map(|d| size(&work_dir(data).join(d))).sum()
}

/// Delete the downloaded image and the unpacked folder (the Mac drivers page's Delete download). Returns the bytes freed.
pub fn delete_leftovers(data: &Path) -> u64 {
    let n = leftovers(data);
    remove_dir(&work_dir(data).join("download"));
    remove_dir(&work_dir(data).join("unpacked"));
    n
}

/// An error of the tool or the disk (7-Zip failing, no room) rather than of one image: another image would fail the same way, so the
/// candidates loop stops and does not download more. Marked with a leading control character that [`unfatal`] removes.
fn fatal(e: String) -> String {
    format!("\u{1}{e}")
}

fn is_fatal(e: &str) -> bool {
    e.starts_with('\u{1}')
}

fn unfatal(e: String) -> String {
    e.trim_start_matches('\u{1}').to_string()
}

/// Unpack an image and name its firmware: first with the name filter (quick, a few MB written), and, when that finds no firmware
/// folder, the whole image without its big folders (about 2 GB, only with room for it). 7-Zip's non-fatal exit codes do not matter;
/// what is in the folder does. The unpacked folder stays when nothing is found.
#[allow(clippy::too_many_arguments)]
fn read_image(env: &mut dyn Env, lines: &mut Vec<String>, seven: &Path, image: &Path, x: &Path, opts: &appfw::Options, data: &Path, found: &mut Vec<String>) -> Result<Vec<(String, Vec<u8>)>, String> {
    for full in [false, true] {
        remove_dir(x);
        std::fs::create_dir_all(x).map_err(|e| format!("{}: {e}", x.display()))?;
        if full {
            if let Some(f) = env.free_bytes(data) {
                if f < NEEDED_FULL {
                    return Err(fatal(format!("the name filter found no firmware folder, and reading the whole image needs about {} MB free ({} MB are)", NEEDED_FULL >> 20, f >> 20)));
                }
            }
            note(env, lines, "  the name filter found no firmware folder: reading the whole image without its big folders (about 2 GB written)".into());
        }
        let said = env.unpack(seven, image, x, full).map_err(fatal)?;
        let tail: Vec<&str> = said.lines().filter(|l| !l.trim().is_empty()).collect();
        for l in tail.iter().skip(tail.len().saturating_sub(3)) {
            note(env, lines, format!("  7zz: {}", l.trim()));
        }
        if appfw::find_firmware_root(x, 8).is_some() {
            break;
        }
        if full {
            return Err(format!("the image has no usr/share/firmware folder with wifi in it (7-Zip unpacked: {}). The unpacked folder {} is kept for a look", appfw::describe_dir(x), x.display()));
        }
    }
    let mut files = files_from_tree(x, opts, found).map_err(|e| format!("{e}. The unpacked folder {} is kept for a look; the macOS script on the Mac itself always can", x.display()))?;
    if let Some(prefix) = opts.hcd_prefix.as_deref().filter(|_| !files.iter().any(|f| f.0 == opts.hcd_name) && !found.iter().any(|l| l.contains("not guessing"))) {
        let c = appfw::hcd_candidates_for(x, prefix);
        match c.len() {
            0 => {
                // macOS 11 keeps the patch of some controllers as Intel HEX text (IOBluetooth.framework/.../BCM4350-Updater.hex)
                let (h, bad) = appfw::hex_candidates_for(x, prefix);
                for b in bad {
                    found.push(format!("a hex file could not be converted: {b}"));
                }
                match h.len() {
                    0 => {}
                    1 => {
                        let data = iyakki_drivers::ihex::check_hcd(&h[0].1).unwrap_or(0);
                        found.push(format!("{} was made from {} (macOS keeps this patch as Intel HEX text; {} data bytes, {} bytes as a .hcd, sha256 {})", opts.hcd_name, h[0].0, data, h[0].1.len(), sha256_hex(&h[0].1)));
                        files.push((opts.hcd_name.clone(), h[0].1.clone()));
                    }
                    n => found.push(format!("found {n} different {prefix} hex patch files - not guessing which one; none was used")),
                }
            }
            1 => {
                found.push(format!("{} comes from {} (found outside the Bluetooth folder)", opts.hcd_name, c[0].0));
                files.push((opts.hcd_name.clone(), c[0].1.clone()));
            }
            n => found.push(format!("found {n} different {prefix} .hcd files - not guessing which one; none was used")),
        }
    }
    Ok(files)
}

// ------------------------------------------------------------------------------------------------ which macOS versions to look in

/// How many of Apple's images one run may look into (each is 0.5 to 1 GB).
pub const MAX_IMAGES: usize = 3;
/// Bump when the scanning rules change: a "nothing in it" result written under older rules is checked again (from the kept image,
/// never from the network).
pub const SCAN_VERSION: u32 = 2;
/// The board id under which Apple's recovery service serves macOS 11 (Big Sur, 11.6 build 20G165 with `os=default`, 11.5.2 with
/// `os=latest`; checked on the real service 2026-10-23). A board id only tells the service which Mac is asking: the image is the same
/// generic recovery system whatever Mac downloads it. The images of macOS 11 hold the Bluetooth patch of the BCM4350 as
/// `BCM4350-Updater.hex`; the newer ones do not.
pub const MACOS11_BOARD: &str = "Mac-BE0E8AC46FE800CC";

/// One macOS recovery image to ask Apple for: the board id and serial part the service is asked with, and `os`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// A folder name under `download/` and the key of the result kept in `state.json`.
    pub key: String,
    /// What the log and the page call it.
    pub label: String,
    pub board_id: String,
    pub mlb: String,
    pub os: String,
}

impl Candidate {
    pub fn new(label: &str, board_id: &str, mlb: &str, os: &str) -> Candidate {
        Candidate { key: format!("{board_id}-{os}-{}", if mlb == crate::macrecovery::MLB_ZERO { "0" } else { mlb }), label: label.into(), board_id: board_id.into(), mlb: mlb.into(), os: os.into() }
    }
}

/// Models whose own newest macOS (13, Ventura) was opened on a real run and has none of the files for their chips (MacBookPro14,1:
/// no `C-4350` Wi-Fi folder, no BCM4350C0 Bluetooth patch). Their own image is not downloaded again for nothing.
pub fn own_image_lacks_bcm4350(product: &str, chip: Option<&str>) -> bool {
    (product.starts_with("MacBookPro13,") || product.starts_with("MacBookPro14,")) && chip.is_some_and(|c| appfw::bt_family(c).as_deref() == Some("BCM4350"))
}

/// The images to look into for this Mac, in order, at most [`MAX_IMAGES`]: the version Apple sends for this Mac's own board, the other
/// version it offers for it (`os=latest`; often the same image, which is then skipped without a download), and, for a controller whose
/// patch only macOS 11 keeps as a hex file (BCM4350), macOS 11.
pub fn candidates(product: &str, own_board: &str, chip: Option<&str>) -> Vec<Candidate> {
    let z = crate::macrecovery::MLB_ZERO;
    let mut v = Vec::new();
    if !own_board.is_empty() && !own_image_lacks_bcm4350(product, chip) {
        v.push(Candidate::new("the macOS Apple sends for this Mac", own_board, z, "default"));
        v.push(Candidate::new("the other macOS version Apple offers for this Mac", own_board, z, "latest"));
    }
    if chip.is_some_and(|c| appfw::bt_family(c).as_deref() == Some("BCM4350")) {
        v.push(Candidate::new("macOS 11 (Big Sur)", MACOS11_BOARD, z, "default"));
    }
    v.truncate(MAX_IMAGES);
    v
}

/// What one image gave, kept in `state.json` so a retry continues at the next one.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CandState {
    pub key: String,
    pub label: String,
    /// Product and address Apple sent (the image's identity).
    pub image_id: String,
    pub bytes: u64,
    /// The scanning rules this result was made under ([`SCAN_VERSION`]).
    pub scan: u32,
    /// `empty` (nothing for this Mac in it), `found`, `failed` (could not be read; the note says why) or `same` (the same image as an earlier one).
    pub status: String,
    pub note: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct State {
    pub product: String,
    pub cands: Vec<CandState>,
}

impl State {
    fn get(&self, key: &str) -> Option<&CandState> {
        self.cands.iter().find(|c| c.key == key)
    }
    fn put(&mut self, c: CandState) {
        match self.cands.iter_mut().find(|x| x.key == c.key) {
            Some(x) => *x = c,
            None => self.cands.push(c),
        }
    }
}

pub fn state_path(data: &Path) -> PathBuf {
    work_dir(data).join("state.json")
}

pub fn read_state(data: &Path) -> State {
    std::fs::read_to_string(state_path(data)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_state(data: &Path, s: &State) {
    let _ = std::fs::create_dir_all(work_dir(data));
    let _ = std::fs::write(state_path(data), serde_json::to_string_pretty(s).unwrap_or_default());
}

/// The folder of one candidate's image under `download/`.
fn image_dir(dl: &Path, c: &Candidate) -> PathBuf {
    dl.join(&c.key)
}

/// A kept image that was complete and checked against Apple's chunk list when it was written: its size. The marker `verified` holds the
/// size; a file of another size (cut off, or grown) is not trusted.
pub fn kept_image(dir: &Path) -> Option<u64> {
    let len = std::fs::metadata(dir.join("BaseSystem.dmg")).ok()?.len();
    let said: u64 = std::fs::read_to_string(dir.join("verified")).ok()?.trim().parse().ok()?;
    (len > 0 && len == said).then_some(len)
}

fn mark_verified(dir: &Path, len: u64) {
    let _ = std::fs::write(dir.join("verified"), len.to_string());
}

/// The kept images: `(candidate key, label, bytes)`, complete ones only, and the partial ones' bytes in the same list with `complete` false.
pub fn kept_images(data: &Path) -> Vec<(String, String, u64, bool)> {
    let st = read_state(data);
    let mut v = Vec::new();
    for e in std::fs::read_dir(work_dir(data).join("download")).into_iter().flatten().flatten() {
        let dir = e.path();
        let key = e.file_name().to_string_lossy().into_owned();
        let Ok(m) = std::fs::metadata(dir.join("BaseSystem.dmg")) else { continue };
        let label = st.get(&key).map(|c| c.label.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| key.clone());
        v.push((key, label, m.len(), kept_image(&dir).is_some()));
    }
    v.sort();
    v
}

/// The plain status of the job for its Setup card and the Mac drivers page: the kept images (a complete image is not downloaded again),
/// what each one gave, and what Retry will do. Empty when there is nothing to say.
pub fn card_status(data: &Path) -> String {
    let st = read_state(data);
    let kept = kept_images(data);
    let mut parts: Vec<String> = Vec::new();
    for (key, label, bytes, complete) in &kept {
        let result = match st.get(key).map(|c| c.status.as_str()) {
            Some("empty") => ", nothing for this Mac in it",
            Some("found") => ", has files for this Mac",
            Some("failed") => ", could not be read",
            _ => "",
        };
        parts.push(format!("{label}: {} MB{}{result}", bytes >> 20, if *complete { "" } else { " (download not finished, it resumes)" }));
    }
    let checked: Vec<&CandState> = st.cands.iter().filter(|c| c.status == "empty" && !kept.iter().any(|k| k.0 == c.key)).collect();
    if kept.is_empty() && checked.is_empty() {
        return String::new();
    }
    let mut s = String::new();
    if !kept.is_empty() {
        s.push_str(&format!("Kept on this machine: {}. ", parts.join("; ")));
    }
    if !checked.is_empty() {
        s.push_str(&format!("Already looked at, nothing in them: {}. ", checked.iter().map(|c| c.label.as_str()).collect::<Vec<_>>().join("; ")));
    }
    s.push_str("Retry (no new download) reads what is kept and only asks Apple for a version it has not tried; Delete download removes everything.");
    s
}

/// Is there anything for the page to offer as "Retry (no new download)": a kept image or a recorded result.
pub fn has_cache(data: &Path) -> bool {
    !kept_images(data).is_empty() || read_state(data).cands.iter().any(|c| c.status == "empty")
}

/// Did the files wanted for this Mac turn up? A T2 Mac wants Wi-Fi files; a Mac with a UART controller of a known chip wants its patch
/// file; a Mac without a T2 chip gets by without Wi-Fi extras (the generic firmware works).
fn satisfied(files: &[(String, Vec<u8>)], t2: bool, chip: Option<&str>, hcd_name: &str) -> bool {
    let bt = chip.is_none() || files.iter().any(|f| f.0 == hcd_name);
    let wifi = !t2 || files.iter().any(|f| f.0.starts_with("brcmfmac"));
    bt && wifi
}

/// The plain end of a run in which no image had what this Mac needs.
pub fn nothing_found_message(t2: bool, chip: Option<&str>, looked: &[String]) -> String {
    let what = match (t2, chip) {
        (true, _) => "Wi-Fi firmware".to_string(),
        (false, Some(c)) => format!("the Wi-Fi extra files for this Mac's chip or the Bluetooth patch for its {c} controller"),
        (false, None) => "Wi-Fi extra files for this Mac's chip".to_string(),
    };
    let which = if looked.is_empty() { "no image could be read".to_string() } else { format!("looked in: {}", looked.join("; ")) };
    let works = if t2 { "Wi-Fi: the Mac's own firmware is needed (the macOS script on the Mac itself can copy it); Bluetooth: a USB Bluetooth adapter" } else { "Wi-Fi works with the generic firmware (fewer channels and country settings) and Bluetooth through a USB Bluetooth adapter" };
    format!("Apple's macOS versions for this Mac do not contain {what} ({which}). What works: {works}")
}

/// Look into Apple's recovery images for this Mac, one candidate at a time ([`candidates`]), until the files wanted are found or the
/// candidates are used up. Never downloads an image twice: a complete image from an earlier try is read from the data volume
/// (`download/<key>/`, checked against Apple's chunk list when it was written), a partial one resumes, an image whose scan found
/// nothing under the current rules is skipped, and an image Apple names the same as an earlier candidate's is skipped before any
/// download. Each result is written to `state.json` as it is known, so a retry continues at the next candidate. Images are deleted by
/// the caller after a successful install, or by Delete download; never here.
#[allow(clippy::too_many_arguments)]
fn look_in_apple_images(env: &mut dyn Env, lines: &mut Vec<String>, data: &Path, product: &str, board_id: &str, chip: Option<&str>, hcd_name: &str, t2: bool, opts: &appfw::Options, x: &Path, dl: &Path, found: &mut Vec<String>, notes: &mut Vec<String>, looked: &mut Vec<String>) -> Result<Vec<(String, Vec<u8>)>, String> {
    let cands = candidates(product, board_id, chip);
    let mut state = read_state(data);
    if state.product != product {
        state = State { product: product.to_string(), cands: Vec::new() };
    }
    // the layout of earlier versions kept one image straight in download/: it cannot be told which version it is, so it is not trusted
    if dl.join("BaseSystem.dmg").exists() {
        let n = std::fs::metadata(dl.join("BaseSystem.dmg")).map(|m| m.len()).unwrap_or(0);
        let _ = std::fs::remove_file(dl.join("BaseSystem.dmg"));
        note(env, lines, format!("removed a download of an older layout ({} MB; it could not be told which macOS version it was)", n >> 20));
    }
    if own_image_lacks_bcm4350(product, chip) {
        note(env, lines, format!("{product}: Apple's own macOS for this model was opened on a real run and has no Wi-Fi extra files and no Bluetooth patch for its BCM4350 chip, so it is not downloaded; macOS 11 is the version that keeps the Bluetooth patch"));
    }
    note(env, lines, format!("will look in at most {} macOS image(s): {}", cands.len(), cands.iter().map(|c| c.label.as_str()).collect::<Vec<_>>().join("; then ")));
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut seven: Option<PathBuf> = None;
    let mut fetched_now = false;
    let mut scanned_any = false;
    for (n, c) in cands.iter().enumerate() {
        // a Mac that wants nothing in particular still gets one image read (the Wi-Fi board files, when there are any)
        if scanned_any && satisfied(&files, t2, chip, hcd_name) {
            break;
        }
        let prev = state.get(&c.key).cloned();
        if let Some(p) = prev.as_ref().filter(|p| p.scan == SCAN_VERSION && matches!(p.status.as_str(), "empty" | "same")) {
            note(env, lines, format!("  {} (image {}): looked at in an earlier try, {}; not downloaded again", c.label, n + 1, if p.status == "same" { format!("the same image as another candidate ({})", p.note) } else { "nothing for this Mac in it".to_string() }));
            looked.push(format!("{} ({})", c.label, if p.status == "same" { "same image" } else { "nothing for this Mac" }));
            continue;
        }
        let dir = image_dir(dl, c);
        let img = dir.join("BaseSystem.dmg");
        let image_id;
        let bytes;
        if let Some(len) = kept_image(&dir) {
            note(env, lines, format!("[1/6] {}: using the image kept from an earlier try ({len} bytes, already checked against Apple's chunk list); no new download", c.label));
            image_id = prev.as_ref().map(|p| p.image_id.clone()).unwrap_or_default();
            bytes = len;
        } else {
            // the same image under another name is not fetched twice
            let partial = std::fs::metadata(&img).map(|m| m.len()).unwrap_or(0);
            if partial == 0 {
                match env.image_id(c) {
                    Ok(id) => {
                        if let Some(o) = state.cands.iter().find(|o| o.key != c.key && !o.image_id.is_empty() && o.image_id == id).cloned() {
                            note(env, lines, format!("  {}: Apple sends the same image as for \"{}\"; nothing more to try here", c.label, o.label));
                            state.put(CandState { key: c.key.clone(), label: c.label.clone(), image_id: id, scan: SCAN_VERSION, status: "same".into(), note: o.label.clone(), ..Default::default() });
                            write_state(data, &state);
                            looked.push(format!("{} (same image as {})", c.label, o.label));
                            continue;
                        }
                    }
                    Err(e) => {
                        let note_text = format!("Apple's recovery service did not give an image for {} ({e})", c.label);
                        note(env, lines, format!("  {note_text}"));
                        looked.push(format!("{} (not offered)", c.label));
                        state.put(CandState { key: c.key.clone(), label: c.label.clone(), scan: SCAN_VERSION, status: "failed".into(), note: note_text, ..Default::default() });
                        write_state(data, &state);
                        continue;
                    }
                }
            }
            if let Some(f) = env.free_bytes(data) {
                if f < NEEDED_FREE && partial == 0 {
                    return Err(format!("the data volume has {} MB free; the download of {} needs about {} MB", f >> 20, c.label, NEEDED_FREE >> 20));
                }
            }
            if n > 0 || fetched_now {
                note(env, lines, format!("  {}: another download of about 1 GB (a few minutes on a fast line, 10 to 30 on a slow one). The images already kept stay on the data volume", c.label));
            }
            note(env, lines, format!("[1/6] downloading {} for this Mac ({}; a stopped download resumes{})", c.label, if partial > 0 { "continuing a partial file" } else { "about 1 GB" }, if partial > 0 { format!(": {} MB are on disk", partial >> 20) } else { String::new() }));
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let (got, id) = env.fetch_image(c, &dir)?;
            let len = std::fs::metadata(&got).map(|m| m.len()).unwrap_or(0);
            mark_verified(&dir, len);
            note(env, lines, format!("  {}: {len} bytes on the data volume, kept until the files are installed or you press Delete download", c.label));
            image_id = id;
            bytes = len;
            fetched_now = true;
            state.put(CandState { key: c.key.clone(), label: c.label.clone(), image_id: image_id.clone(), bytes, scan: 0, status: "downloaded".into(), ..Default::default() });
            write_state(data, &state);
        }
        let sz = match &seven {
            Some(s) => s.clone(),
            None => {
                note(env, lines, "[2/6] getting 7-Zip (the program that reads the disk image)".into());
                let s = env.ensure_7zz()?;
                seven = Some(s.clone());
                s
            }
        };
        note(env, lines, format!("[3/6] reading the firmware folders out of {} with {}", c.label, sz.display()));
        note(env, lines, "[4/6] naming the files the way the kernel asks for them".into());
        let mut got_notes: Vec<String> = Vec::new();
        let scanned = read_image(env, lines, &sz, &img, x, opts, data, &mut got_notes);
        for l in got_notes {
            note(env, lines, format!("  {l}"));
        }
        scanned_any = scanned_any || scanned.is_ok();
        let (status, text) = match scanned {
            Ok(f) => {
                let before = files.len();
                for (name, b) in f {
                    if !files.iter().any(|o| o.0 == name) {
                        files.push((name, b));
                    }
                }
                let useful = files.len() > before;
                if useful {
                    ("found", format!("{} file(s) for this Mac", files.len() - before))
                } else {
                    ("empty", "nothing for this Mac".to_string())
                }
            }
            Err(e) if is_fatal(&e) => return Err(unfatal(e)),
            Err(e) => ("failed", e),
        };
        let short = text.split(". The unpacked folder").next().unwrap_or(&text).to_string();
        note(env, lines, format!("  {}: {short}", c.label));
        looked.push(format!("{} ({})", c.label, if status == "found" { "had files".to_string() } else if status == "empty" { "nothing for this Mac".to_string() } else { format!("could not be read: {short}") }));
        if status == "failed" {
            notes.push(format!("{}: {short}", c.label));
        }
        if let (Some(chip), false) = (chip, files.iter().any(|f| f.0 == hcd_name)) {
            if status != "failed" {
                note(env, lines, format!("  Bluetooth: {} has no patch file for the {chip} controller{}", c.label, if n + 1 < cands.len() { "; looking at the next macOS version" } else { "" }));
            }
        }
        state.put(CandState { key: c.key.clone(), label: c.label.clone(), image_id, bytes, scan: SCAN_VERSION, status: status.into(), note: text });
        write_state(data, &state);
    }
    let _ = found;
    if files.is_empty() && !state.cands.iter().any(|c| matches!(c.status.as_str(), "empty" | "same")) {
        // every image that was tried could not be read: say why instead of claiming Apple's macOS lacks the files
        if let Some(c) = state.cands.iter().find(|c| c.status == "failed") {
            return Err(format!("{}: {}", c.label, c.note));
        }
    }
    Ok(files)
}

fn run_inner(env: &mut dyn Env, data: &Path, source: &Source, lines: &mut Vec<String>) -> Result<Outcome, String> {
    note(env, lines, format!("[0/6] {NOTICE}"));
    let product = env.product();
    note(env, lines, format!("this machine: {}", if product.is_empty() { "unknown model".to_string() } else { product.clone() }));
    let wd = work_dir(data);
    std::fs::create_dir_all(&wd).map_err(|e| format!("{}: {e}", wd.display()))?;
    let (dl, x) = (wd.join("download"), wd.join("unpacked"));
    remove_dir(&x);
    let board_name = env.board_name();
    let res = macboard::resolve(&product, &board_name);
    // the UART Bluetooth controller: a T2 Mac's is known from its model; any other Mac's chip is read from the kernel log, and the
    // file name is the first one btbcm says it tried (BCM.hcd on an Intel Mac)
    let (uart_chip, hcd_name): (Option<String>, String) = if res.t2 {
        (macboard::uart_bluetooth(&product).then(|| "BCM4364".to_string()), "BCM.hcd".into())
    } else {
        let log = env.bt_log();
        (appfw::bt_chip_from_log(&log), appfw::hcd_serve_name(&appfw::bt_tried_from_log(&log)))
    };
    if !res.t2 {
        note(env, lines, NON_T2_PLAN.to_string());
        match &uart_chip {
            Some(c) => note(env, lines, format!("the UART Bluetooth controller reports itself as {c}; the patch file's name starts with that, and it is installed as brcm/{hcd_name} (the first name the kernel tried)")),
            None => note(env, lines, "no UART Bluetooth controller was seen in the kernel log: no Bluetooth patch file is looked for".into()),
        }
    }
    let opts = appfw::options_for_machine(&product, &env.vendor(), &board_name, uart_chip.as_deref(), &hcd_name);
    let mut notes: Vec<String> = Vec::new();
    let mut found: Vec<String> = Vec::new();
    let mut looked: Vec<String> = Vec::new();

    // ---- 1. get the files out of the source
    let files: Vec<(String, Vec<u8>)>;
    match source {
        Source::Folder(p) => {
            if !p.is_dir() {
                return Err(format!("{} is not a folder", p.display()));
            }
            note(env, lines, format!("[1/6] reading the folder {}", p.display()));
            note(env, lines, "[4/6] naming the files the way the kernel asks for them".into());
            files = files_from_tree(p, &opts, &mut found)?;
        }
        Source::Image(_) | Source::Download => {
            if env.data_in_memory() {
                return Err("the data volume is in memory (a live session): the download needs 1 to 2 GB and nothing would be kept after a restart. Start from the installed system, or use the folder route (a stick made by the macOS script)".into());
            }
            if let Source::Image(p) = source {
                if !p.is_file() {
                    return Err(format!("{} is not a file", p.display()));
                }
                note(env, lines, format!("[1/6] using the disk image {}", p.display()));
                note(env, lines, "[2/6] getting 7-Zip (the program that reads the disk image)".into());
                let seven = env.ensure_7zz()?;
                note(env, lines, format!("[3/6] reading the firmware folders out of the image with {}", seven.display()));
                note(env, lines, "[4/6] naming the files the way the kernel asks for them".into());
                files = read_image(env, lines, &seven, p, &x, &opts, data, &mut found).map_err(unfatal)?;
                if let (false, Some(chip)) = (files.iter().any(|f| f.0 == hcd_name), uart_chip.as_deref()) {
                    notes.push(format!("Bluetooth: this macOS image has no {chip} patch file (brcm/{hcd_name}). An older macOS may have it (macOS 11 keeps the BCM4350 one); the macOS script on this Mac's own macOS reads whatever it has"));
                }
            } else {
                let board_id = res.board_id.clone().ok_or_else(|| {
                    format!("this Mac's board id could not be read (DMI board_name \"{}\") and the model {} is not in IYAKKI's table, so Apple's recovery image cannot be chosen for it. Use the macOS script (dist/drivers/mac-wifi-firmware-from-macos.sh) or a disk image", board_name.trim(), if product.is_empty() { "unknown" } else { &product })
                })?;
                if let Some(m) = &res.mismatch {
                    note(env, lines, format!("note: {m}"));
                }
                if !res.verified {
                    let model = if product.is_empty() { "unknown model".to_string() } else { product.clone() };
                    if res.from_dmi {
                        note(env, lines, format!("note: {model} is not on IYAKKI's verified list (no run on that model is recorded). The board id {board_id} is this Mac's own (DMI board_name); Apple's service decides whether it has an image for it. If it refuses, use the macOS script"));
                    } else {
                        note(env, lines, format!("note: the board id {board_id} for {model} comes from a list, not from a run on that Mac; if Apple's service refuses it, use the macOS script"));
                    }
                }
                if let Some(f) = env.free_bytes(data) {
                    note(env, lines, format!("free space on the data volume: {} MB", f >> 20));
                } else {
                    note(env, lines, "note: the free space of the data volume could not be read".into());
                }
                files = look_in_apple_images(env, lines, data, &product, &board_id, uart_chip.as_deref(), &hcd_name, res.t2, &opts, &x, &dl, &mut found, &mut notes, &mut looked)?;
            }
        }
    }
    for l in found {
        note(env, lines, format!("  {l}"));
    }
    if let Some(chip) = uart_chip.as_deref().filter(|_| !files.iter().any(|f| f.0 == hcd_name)) {
        notes.push(no_hcd_message(chip));
    }
    let (wifi_pack, bt_pack, taken) = match build_packs(&files, &source.words()) {
        Ok(x) => x,
        Err(_) if matches!(source, Source::Download) && files.is_empty() => return Err(nothing_found_message(res.t2, uart_chip.as_deref(), &looked)),
        Err(e) => return Err(e),
    };
    for t in taken.iter().take(12) {
        note(env, lines, format!("  {t}"));
    }
    if taken.len() > 12 {
        note(env, lines, format!("  ... and {} more", taken.len() - 12));
    }
    let wifi_files = wifi_pack.as_ref().map_or(0, |p| p.files.len());
    let bt_files = bt_pack.as_ref().map_or(0, |p| p.files.len());
    if bt_pack.is_none() {
        notes.push("no Bluetooth firmware was found in these files".into());
    }

    // ---- 3. install
    note(env, lines, "[5/6] installing as driver packs on the data volume (linked at every start) and loading the drivers again".into());
    let version = version_for(env.now());
    let from = format!("Apple's firmware from {}", source.words());
    if let Some(wp) = &wifi_pack {
        let rep = env.install(PACK_WIFI, &version, &from, wp)?;
        for l in &rep.lines {
            note(env, lines, format!("  Wi-Fi: {l}"));
        }
    } else if !res.t2 {
        notes.push("Wi-Fi: no board file for this Mac was found (see the lines above); the generic firmware keeps working, with its limits".into());
    }
    if let Some(bp) = &bt_pack {
        let rep = env.install(PACK_BT, &version, &from, bp)?;
        for l in &rep.lines {
            note(env, lines, format!("  Bluetooth: {l}"));
        }
    }
    // success: the unpacked folder and the downloaded image have done their job (after a failure both stay, for a retry or a look)
    remove_dir(&x);
    if matches!(source, Source::Download) {
        remove_dir(&dl);
        let _ = std::fs::remove_file(state_path(data));
    }

    // ---- 4. look
    note(env, lines, "[6/6] looking for the Wi-Fi interface and the Bluetooth address".into());
    let mut seen = env.look();
    for _ in 0..20 {
        if (!seen.wifi.is_empty() || wifi_pack.is_none()) && (bt_pack.is_none() || seen.bt_address.is_some()) {
            break;
        }
        env.sleep(Duration::from_secs(1));
        seen = env.look();
    }
    let kernel = env.kernel_lines();
    let mut headline_parts = Vec::new();
    let mut detail = notes;
    if wifi_pack.is_none() {
        headline_parts.push(if seen.wifi.is_empty() { "no Wi-Fi board file was found".to_string() } else if res.t2 { format!("Wi-Fi: {} (no board file was found, the generic firmware is in use)", seen.wifi.join(", ")) } else { format!("Wi-Fi: no extra files for this chip in Apple's macOS, the generic firmware stays in use ({})", seen.wifi.join(", ")) });
    } else if seen.wifi.is_empty() {
        headline_parts.push("Wi-Fi firmware is installed, but no wireless interface has appeared yet".to_string());
        detail.push("No Wi-Fi interface yet: restart IYAKKI once (the firmware is linked in at every start); if it is still missing, the kernel lines below say which file the driver asked for.".into());
    } else {
        headline_parts.push(format!("Wi-Fi {} installed: {} appeared", if res.t2 { "firmware" } else { "extra files" }, seen.wifi.join(", ")));
    }
    match (&bt_pack, &seen.bt_address) {
        (None, _) => headline_parts.push(if matches!(source, Source::Download) { "Bluetooth: Apple's macOS versions for this Mac have no firmware for it, a USB Bluetooth adapter works".to_string() } else { "no Bluetooth firmware was in these files".to_string() }),
        (Some(_), Some(a)) => headline_parts.push(format!("Bluetooth firmware installed: the controller has an address ({a})")),
        (Some(_), None) if seen.bt_controller => {
            headline_parts.push("Bluetooth firmware is installed, but the controller still has no address".into());
            detail.push("The patch file is installed and the Bluetooth drivers were loaded again. If the controller still has no address, restart IYAKKI once from the IYAKKI menu (nothing restarts by itself); if it has none after that either, this controller does not accept the file (known limit F-NET-11). Settings \u{2192} Bluetooth shows what it reports. A USB Bluetooth adapter works.".into());
        }
        (Some(_), None) => {
            headline_parts.push("Bluetooth firmware is installed, but no Bluetooth controller appeared".into());
            detail.push("No Bluetooth controller yet: restart IYAKKI once. If it stays missing, a USB Bluetooth adapter works.".into());
        }
    }
    let o = Outcome { at: env.now(), source: source.words(), installed: true, wifi_files, bt_files, seen, headline: headline_parts.join(" · "), lines: lines.iter().cloned().chain(detail).collect(), kernel };
    write_outcome(data, &o);
    env.say(&format!("done: {}", o.headline));
    for l in o.lines.iter().rev().take(3).rev() {
        env.say(&format!("  {l}"));
    }
    Ok(o)
}

// ------------------------------------------------------------------------------------------------ the real machine

/// Install a pack and load the driver again. A Wi-Fi driver that is loaded already has its vendor module (`brcmfmac_wcc`) holding
/// it, so that goes first; the pack then removes and loads `brcmfmac`.
pub fn install_with_unload(fs: &apply::Fs, run: &dyn iyakki_drivers::run::Runner, id: &str, version: &str, source: &str, p: &pack::Pack) -> Result<apply::Report, String> {
    if id == PACK_WIFI {
        let loaded = std::fs::read_to_string(fs.proc.join("modules")).unwrap_or_default();
        for m in ["brcmfmac_wcc", "brcmfmac_cyw", "brcmfmac_bca"] {
            if loaded.lines().any(|l| l.split_whitespace().next() == Some(m)) {
                let _ = run.run("modprobe", &["-r", m]);
            }
        }
    }
    crate::fwpack::install_checked(fs, run, id, version, "LicenseRef-Apple-firmware-supplied-by-the-user", source, p)
}

/// The Bluetooth drivers of this machine are set aside (a T2 Mac without the firmware, `t2hold.rs`): "Load the Bluetooth driver anyway"
/// is then offered. `lib_modules` is `/lib/modules`.
pub fn bluetooth_held(lib_modules: &Path) -> bool {
    iyakki_drivers::t2hold::bluetooth_held(lib_modules)
}

/// "Load the Bluetooth driver anyway" (Mac drivers page, Devices): give the held Bluetooth drivers back and load them in dependency order,
/// remembered for the next starts. On the device image this is the real `/lib/modules`; elsewhere (a desktop window, tests) a sandbox folder
/// inside the data folder and no program runs. Returns the plain lines.
pub fn load_bluetooth_anyway(data: &Path) -> Vec<String> {
    let (fs, real) = apply::Fs::for_data(data);
    let run: Box<dyn iyakki_drivers::run::Runner> = if real { Box::new(iyakki_drivers::run::System) } else { Box::new(iyakki_drivers::run::Dry) };
    apply::load_bluetooth_anyway(&fs, run.as_ref())
}

/// The chip name of the UART Bluetooth controller of a Mac model (the kernel does not report it): `BCM4364` for the Mac mini 2018 and the
/// 2018-2019 MacBook Pros, nothing for other models.
pub fn uart_chip(dmi_product: &str) -> Option<&'static str> {
    macboard::uart_bluetooth(dmi_product).then_some("BCM4364")
}

/// What the program registers with `iyakki_devctl::set_bt_loader`: the real `/lib/modules` and data volume.
pub fn register_bluetooth_loader() {
    fn held() -> bool {
        bluetooth_held(Path::new("/lib/modules"))
    }
    fn load() -> Result<String, String> {
        let lines = load_bluetooth_anyway(Path::new("/var/lib/iyakki"));
        Ok(lines.join(". "))
    }
    iyakki_devctl::set_bt_loader(held, load);
}

/// The Wi-Fi interfaces and Bluetooth controller under a `/sys` folder.
pub fn look_in(sys: &Path) -> Seen {
    let names = |d: &str| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(sys.join(d)).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    };
    let wifi = names("class/net").into_iter().filter(|n| sys.join("class/net").join(n).join("wireless").exists() || sys.join("class/net").join(n).join("phy80211").exists()).collect();
    let hci = names("class/bluetooth").into_iter().find(|n| n.starts_with("hci") && !n.contains(':'));
    let bt_address = hci.as_ref().and_then(|h| std::fs::read_to_string(sys.join("class/bluetooth").join(h).join("address")).ok()).map(|a| a.trim().to_string()).filter(|a| !a.is_empty() && a != "00:00:00:00:00:00");
    Seen { wifi, bt_controller: hci.is_some(), bt_address }
}

/// The addresses of the Bluetooth controllers the kernel's management interface lists (needs root). Current kernels have no `address`
/// file under `/sys/class/bluetooth/hci0`, and a controller that did not finish its setup is not in this list, which is the sign
/// the page reports ("still has no address").
pub fn mgmt_addresses() -> Vec<String> {
    use iyakki_devctl::btmgmt::{ControlSocket, Mgmt};
    let Ok(sock) = ControlSocket::open() else { return Vec::new() };
    let mut m = Mgmt::new(sock);
    let idx = m.index_list().unwrap_or_default();
    idx.into_iter().filter_map(|i| m.info(i).ok()).map(|i| i.address).filter(|a| !a.is_empty() && a != "00:00:00:00:00:00").collect()
}

/// The lines of the kernel log that are about the drivers and the firmware they asked for.
pub fn interesting(log: &str) -> Vec<String> {
    let keys = ["brcmfmac", "Bluetooth", "hci_uart", "hci0", "BCM", "bcm4377", "firmware load"];
    let v: Vec<String> = log.lines().filter(|l| keys.iter().any(|k| l.contains(k))).map(|l| l.trim().to_string()).collect();
    v.iter().skip(v.len().saturating_sub(14)).cloned().collect()
}

/// Is `path` on a RAM file system, by the mount table text?
pub fn in_memory(mounts: &str, path: &Path) -> bool {
    let mut best: Option<(usize, &str)> = None;
    for l in mounts.lines() {
        let mut w = l.split_whitespace();
        let (Some(_), Some(mp), Some(fs)) = (w.next(), w.next(), w.next()) else { continue };
        let mp = mp.replace("\\040", " ");
        if (path == Path::new(&mp) || path.starts_with(&mp)) && best.is_none_or(|(n, _)| mp.len() >= n) {
            best = Some((mp.len(), fs));
        }
    }
    best.is_some_and(|(_, fs)| matches!(fs, "tmpfs" | "ramfs" | "rootfs"))
}

/// The 7-Zip command line: the firmware folders and any BCM4364 or BCM4350 patch file by name pattern (the DMG holds one volume, `4.hfs`, which
/// 7-Zip opens itself when extracting), or, with `full`, everything except the big system folders.
pub fn unpack_argv(seven: &Path, image: &Path, out: &Path, full: bool) -> Vec<String> {
    let mut v: Vec<String> = [seven.display().to_string().as_str(), "x", "-y", "-bd", "-aoa"].iter().map(|s| s.to_string()).collect();
    v.push(format!("-o{}", out.display()));
    v.push(image.display().to_string());
    if full {
        v.extend(["-xr!System", "-xr!Applications"].map(String::from));
    } else {
        v.extend(["-ir!*usr/share/firmware/wifi/*", "-ir!*usr/share/firmware/bluetooth/*", "-ir!*BCM4364*.hcd", "-ir!*BCM4350*.hcd", "-ir!*IOBluetooth.framework/Versions/A/Resources/BCM*-Updater.hex"].map(String::from));
    }
    v
}

/// This machine: the CLI list for 7-Zip, Apple's service over the network, the driver packs on the real data volume.
pub struct RealEnv {
    pub data: PathBuf,
    pub user: String,
    pub clis: std::sync::Arc<crate::clis::CliManager>,
    pub streamer: std::sync::Arc<dyn iyakki_proxy::Streamer>,
    pub sys: PathBuf,
}

impl RealEnv {
    pub fn new(data: &Path, user: &str, clis: std::sync::Arc<crate::clis::CliManager>, streamer: std::sync::Arc<dyn iyakki_proxy::Streamer>) -> RealEnv {
        RealEnv { data: data.to_path_buf(), user: user.into(), clis, streamer, sys: "/sys".into() }
    }

    fn find_7zz(&self) -> Option<PathBuf> {
        let own = self.clis.bin_dir().join("7zz");
        if own.exists() {
            return Some(own);
        }
        crate::player::find_program("7zz").map(PathBuf::from)
    }
}

impl Env for RealEnv {
    fn say(&mut self, line: &str) {
        use std::io::Write;
        let mut o = std::io::stdout();
        let _ = writeln!(o, "{line}");
        let _ = o.flush();
    }
    fn product(&self) -> String {
        std::fs::read_to_string(self.sys.join("class/dmi/id/product_name")).map(|s| s.trim().to_string()).unwrap_or_default()
    }
    fn board_name(&self) -> String {
        std::fs::read_to_string(self.sys.join("class/dmi/id/board_name")).map(|s| s.trim().to_string()).unwrap_or_default()
    }
    fn vendor(&self) -> String {
        std::fs::read_to_string(self.sys.join("class/dmi/id/sys_vendor")).map(|s| s.trim().to_string()).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "Apple Inc.".into())
    }
    fn bt_log(&mut self) -> String {
        crate::wsexec::run_argv(&self.data, &["dmesg".to_string()], &[], Duration::from_secs(10), 2 << 20).stdout
    }
    fn free_bytes(&self, dir: &Path) -> Option<u64> {
        crate::k3sdev::space(dir).map(|s| s.0)
    }
    fn data_in_memory(&self) -> bool {
        in_memory(&iyakki_storage::read_mounts().unwrap_or_default(), &self.data)
    }
    fn ensure_7zz(&mut self) -> Result<PathBuf, String> {
        if let Some(p) = self.find_7zz() {
            return Ok(p);
        }
        self.say("7-Zip is not installed: installing it from the CLI list (the project's static build from GitHub, not verified by a checksum: the project publishes none)");
        self.clis.install("7zip", &self.user).map_err(|e| format!("7-Zip could not be installed: {e}. You can put the official 7zz (7-zip.org, the Linux build 7z<version>-linux-x64.tar.xz) into {} and press Retry, or use the macOS script or a folder instead", self.clis.bin_dir().display()))?;
        let mut last = String::new();
        loop {
            let Some(j) = self.clis.job("7zip") else { return Err("the 7-Zip install did not start".into()) };
            if let Some(e) = j.error {
                return Err(format!("7-Zip could not be installed: {e}. You can put the official 7zz (7-zip.org, the Linux build 7z<version>-linux-x64.tar.xz) into {} and press Retry, or use the macOS script or a folder instead", self.clis.bin_dir().display()));
            }
            if j.finished {
                break;
            }
            if j.phase != last {
                self.say(&format!("  {}", j.phase));
                last = j.phase.clone();
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        self.find_7zz().ok_or_else(|| "7-Zip was installed but its 7zz command was not found".to_string())
    }
    fn fetch_image(&mut self, c: &Candidate, dir: &Path) -> Result<(PathBuf, String), String> {
        let st = self.streamer.clone();
        crate::macrecovery::fetch_recovery_image_for(&*st, crate::macrecovery::SERVICE, &crate::macrecovery::Policy::default(), &c.board_id, &c.mlb, &c.os, dir, &mut |l| self.say(&format!("  {l}")), &|| false, 5)
    }
    fn image_id(&mut self, c: &Candidate) -> Result<String, String> {
        crate::macrecovery::image_identity_for(&*self.streamer, crate::macrecovery::SERVICE, &crate::macrecovery::Policy::default(), &c.board_id, &c.mlb, &c.os)
    }
    fn unpack(&mut self, seven: &Path, image: &Path, out: &Path, full: bool) -> Result<String, String> {
        let r = crate::wsexec::run_argv(&self.data, &unpack_argv(seven, image, out, full), &[], Duration::from_secs(40 * 60), 256 * 1024);
        if r.missing {
            return Err("7zz could not be started".into());
        }
        let all = r.combined();
        match r.code {
            // 0 fine, 1 warning, 2 "sub items errors": a macOS volume has links and special files 7-Zip cannot write
            Some(0) => Ok(all),
            Some(c @ (1 | 2)) => Ok(format!("{all}\n7-Zip finished with exit code {c} (some items could not be written; the firmware files are what counts)")),
            other => {
                let mut tail: Vec<&str> = all.lines().filter(|l| !l.trim().is_empty()).rev().take(3).collect();
                tail.reverse();
                Err(format!("7-Zip could not read the disk image ({}): {}", other.map_or("it was stopped".to_string(), |c| format!("exit {c}")), tail.join(" | ")))
            }
        }
    }
    fn install(&mut self, id: &str, version: &str, source: &str, p: &pack::Pack) -> Result<apply::Report, String> {
        let (fs, real) = apply::Fs::for_data(&self.data);
        let run: Box<dyn iyakki_drivers::run::Runner> = if real { Box::new(iyakki_drivers::run::System) } else { Box::new(iyakki_drivers::run::Dry) };
        install_with_unload(&fs, run.as_ref(), id, version, source, p)
    }
    fn look(&mut self) -> Seen {
        let mut seen = look_in(&self.sys);
        if seen.bt_address.is_none() {
            if let Some(a) = mgmt_addresses().into_iter().next() {
                seen.bt_controller = true;
                seen.bt_address = Some(a);
            }
        }
        seen
    }
    fn kernel_lines(&mut self) -> Vec<String> {
        let r = crate::wsexec::run_argv(&self.data, &["dmesg".to_string()], &[], Duration::from_secs(10), 2 << 20);
        interesting(&r.stdout)
    }
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
    fn now(&self) -> i64 {
        crate::clock::now_unix()
    }
}

#[cfg(test)]
#[path = "macfw_tests.rs"]
mod tests;
