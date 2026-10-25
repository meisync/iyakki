//! Apple's Wi-Fi and Bluetooth firmware, found in a copy of macOS's `/usr/share/firmware` and given the file names Linux
//! asks for. This is the on-device twin of `dist/drivers/mac-wifi-firmware-from-macos.sh` (which runs in macOS): the same
//! names, the same safety rules, and a test that runs the script on the same fixture and compares.
//!
//! The files are Apple's. IYAKKI never ships them: the person's own Mac (or Apple's recovery image, downloaded at their
//! request) is the only source, and the result stays on their machine (docs/MAC-WIFI-BLUETOOTH-SETUP.md).
//!
//! Rules (all copied from the script):
//! * only exact matches of the known name shapes; a file whose name does not fit is skipped with a note, never guessed;
//! * Apple's development builds (`*_DEV*`) are skipped;
//! * a Bluetooth `.hcd` for the BCM4364 becomes `BCM.hcd` only when exactly one exists;
//! * an output name that was written already is not written twice.
//!
//! Chips: BCM4364 (B2: Mac mini 2018, MacBookPro15,1/15,2/15,3, iMac Pro; B3: MacBookPro16,x), BCM4377 (B3), BCM4355 (C1).
//! The naming follows the t2linux / Asahi Linux firmware script and what brcmfmac and hci_bcm4377 request in Linux 6.12.
//!
//! Intel Macs without a T2 chip (BCM4350 in the 2016-2017 MacBook Pros, BCM43602 in the 2015 iMacs and others) are a second
//! mode ([`BoardTarget`]): brcmfmac asks there for `brcmfmac<chip>-pcie.<DMI sys_vendor>-<DMI product_name>.<ext>`
//! (`firmware.c`/`dmi.c` of Linux 6.12: no module/vendor/antenna parts, they come from Apple's device tree only), the `.bin` comes from
//! linux-firmware (not taken from macOS), and only the NVRAM (`.txt`), `.clm_blob` and `.txcap_blob` are taken from macOS. Which
//! platform's NVRAM belongs to this Mac is read from the `*PlatformConfig.plist` files of the wifi folder (the Mac's board id must be
//! in the plist of exactly one platform); when that cannot be determined nothing is taken and the notes say so. This mapping and the
//! 4350/43602 folder names are NOT verified against a real Apple image (docs/MAC-WIFI-BLUETOOTH-SETUP.md).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The Wi-Fi chip folders looked at (`C-<chip>__s-<revision>`).
const WIFI_CHIPS: [&str; 3] = ["4364", "4377", "4355"];

/// The Wi-Fi chip folders of Intel Macs without a T2 chip (`C-<chip>__s-<revision>`): BCM4350 (stepping C2) and BCM43602.
const BOARD_CHIPS: [&str; 2] = ["4350", "43602"];

/// The Mac the board-named mode is for: what brcmfmac builds from the DMI and the board id that Apple's plists list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardTarget {
    /// `<sys_vendor>-<product_name>`: `Apple Inc.-MacBookPro14,1`.
    pub board_type: String,
    /// `Mac-B4831CEBD52A0C4C` (the DMI board name).
    pub board_id: String,
}

/// What to take. Empty lists mean everything (what the script does).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Apple's platform names (`lanai`), lower case. Wi-Fi sets of other platforms are left out. Empty: all.
    pub platforms: Vec<String>,
    /// Chip numbers (`4364`). Wi-Fi sets of other chips are left out. Empty: all.
    pub chips: Vec<String>,
    /// Set for a Mac without a T2 chip: the board-named mode (only BCM4350 / BCM43602 folders, only NVRAM and blobs).
    pub board: Option<BoardTarget>,
    /// The UART Bluetooth controller's chip name (`BCM4364`, `BCM4350C0`): the `.hcd` file must start with it. `None`: no `.hcd` is taken.
    pub hcd_prefix: Option<String>,
    /// The file name the `.hcd` is written under (what btbcm asks for first; `BCM.hcd`).
    pub hcd_name: String,
}

impl Default for Options {
    fn default() -> Options {
        Options { platforms: Vec::new(), chips: Vec::new(), board: None, hcd_prefix: Some("BCM4364".into()), hcd_name: "BCM.hcd".into() }
    }
}

/// The result: Linux file names with their bytes, and what was seen but not taken.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extracted {
    /// `(file name, bytes)`: `brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin`, `BCM.hcd`, `brcmbt4377b3-apple,formosa-m.bin`.
    pub files: Vec<(String, Vec<u8>)>,
    /// Wi-Fi sets written (each is a `.bin` with its `.txt` and, when present, `.clm_blob` and `.txcap_blob`).
    pub wifi_sets: usize,
    /// PCIe Bluetooth files written (`brcmbt*`).
    pub bt_files: usize,
    /// `BCM.hcd` was written.
    pub hcd: bool,
    /// Things skipped or not found, in plain words.
    pub notes: Vec<String>,
    /// Every platform an NVRAM file was seen for (with or without an antenna in its name): for the message when the Mac's own is missing.
    pub nvram_platforms: BTreeSet<String>,
}

impl Extracted {
    pub fn names(&self) -> Vec<&str> {
        self.files.iter().map(|f| f.0.as_str()).collect()
    }
    pub fn total_bytes(&self) -> usize {
        self.files.iter().map(|f| f.1.len()).sum()
    }
}

fn lower(s: &str) -> String {
    s.to_ascii_lowercase()
}

// ------------------------------------------------------------------------------------------------ Wi-Fi

/// The parts of an Apple NVRAM file name `P-<platform>-<antenna>_M-<module>_V-<vendor>__m-<version>` (without `.txt`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NvramName {
    pub platform: String,
    pub antenna: String,
    pub module: String,
    pub vendor: String,
    pub version: String,
}

/// Split an NVRAM base name the way the script's sed expressions do. `None` when it does not have the shape.
pub fn parse_nvram_name(n: &str) -> Option<NvramName> {
    let rest = n.strip_prefix("P-")?;
    // platform: the characters up to the first '-' or '_', which must be a '-'
    let end = rest.find(['-', '_'])?;
    if end == 0 || rest.as_bytes()[end] != b'-' {
        return None;
    }
    let platform = &rest[..end];
    let after = &rest[end + 1..];
    // antenna: everything up to the "_M-" (it has no '_')
    let m = after.find("_M-")?;
    let antenna = &after[..m];
    if antenna.is_empty() || antenna.contains('_') {
        return None;
    }
    let tail = &after[m + 3..];
    // module up to "_V-", vendor up to "__m-", version the rest (the script's greedy `.*` takes the last marker)
    let v = tail.find("_V-")?;
    let module = &tail[..v];
    let tail = &tail[v + 3..];
    let mm = tail.rfind("__m-")?;
    let vendor = &tail[..mm];
    let version = &tail[mm + 4..];
    if module.is_empty() || module.contains('_') || vendor.is_empty() || vendor.contains('_') || version.is_empty() {
        return None;
    }
    Some(NvramName { platform: platform.into(), antenna: antenna.into(), module: module.into(), vendor: vendor.into(), version: version.into() })
}

/// The name brcmfmac asks for first, without extension: `brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0`.
pub fn wifi_base(chip: &str, rev: &str, n: &NvramName) -> String {
    format!("brcmfmac{chip}{rev}-pcie.apple,{}-{}-{}-{}-{}", n.platform, n.module, n.vendor, n.version, n.antenna)
}

/// brcmfmac does not like spaces around `=` in Apple's NVRAM files, and blank lines are dropped (what the script's sed and
/// grep do; the t2linux script strips them too).
pub fn clean_nvram(text: &[u8]) -> Vec<u8> {
    let is_space = |b: u8| matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r');
    let mut out = Vec::with_capacity(text.len());
    let mut lines: Vec<&[u8]> = text.split(|b| *b == b'\n').collect();
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    for line in lines {
        if line.iter().all(|b| is_space(*b)) {
            continue;
        }
        let mut i = 0;
        while i < line.len() && is_space(line[i]) {
            i += 1;
        }
        let key_start = i;
        while i < line.len() && line[i] != b'=' && !is_space(line[i]) {
            i += 1;
        }
        let key_end = i;
        while i < line.len() && is_space(line[i]) {
            i += 1;
        }
        if key_end > key_start && i < line.len() && line[i] == b'=' {
            out.extend_from_slice(&line[key_start..key_end]);
            out.extend_from_slice(&line[i..]);
        } else {
            out.extend_from_slice(line);
        }
        out.push(b'\n');
    }
    out
}

fn sorted_names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// Follow symbolic links (macOS's firmware folders are mostly links: `lanai.trx -> ekans.trx`, `lanai-X0.clmb -> lanai.clmb`) to the
/// regular file they end at, and read it. Every step must stay inside `root` (links are relative; an absolute link, a `..` that
/// leaves the tree, or a link inside a directory path is refused), chains are followed up to 16 steps. `None` when it is not a
/// regular, non-empty file in the tree.
pub fn read_in_tree(root: &Path, p: &Path) -> Option<Vec<u8>> {
    let target = resolve_in_tree(root, p)?;
    std::fs::read(target).ok().filter(|b| !b.is_empty())
}

pub fn resolve_in_tree(root: &Path, p: &Path) -> Option<PathBuf> {
    fn normalize(path: &Path) -> Option<PathBuf> {
        let mut out = PathBuf::new();
        for c in path.components() {
            match c {
                std::path::Component::ParentDir => {
                    out.pop();
                }
                std::path::Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        Some(out)
    }
    let root = normalize(root)?;
    let mut cur = normalize(p)?;
    for _ in 0..16 {
        let rel = cur.strip_prefix(&root).ok()?;
        // no directory on the way may itself be a link
        let mut walk = root.clone();
        let comps: Vec<_> = rel.components().collect();
        for (k, c) in comps.iter().enumerate() {
            walk.push(c.as_os_str());
            let m = std::fs::symlink_metadata(&walk).ok()?;
            if k + 1 < comps.len() && m.file_type().is_symlink() {
                return None;
            }
        }
        let m = std::fs::symlink_metadata(&cur).ok()?;
        if m.file_type().is_symlink() {
            let t = std::fs::read_link(&cur).ok()?;
            if t.is_absolute() {
                return None;
            }
            cur = normalize(&cur.parent()?.join(t))?;
            continue;
        }
        return m.is_file().then_some(cur);
    }
    None
}

/// A platform-only NVRAM name (`P-lanai_M-HRPN_V-u__m-7.5`): the original that the antenna-named links point to.
fn nvram_platform_only(stem: &str) -> Option<String> {
    let rest = stem.strip_prefix("P-")?;
    let end = rest.find(['-', '_'])?;
    (end > 0 && rest[end..].starts_with("_M-")).then(|| rest[..end].to_string())
}

/// What a folder holds, in a few words: for the message when nothing fits.
pub fn describe_dir(dir: &Path) -> String {
    let names = sorted_names(dir);
    let links = names.iter().filter(|n| std::fs::symlink_metadata(dir.join(n)).is_ok_and(|m| m.file_type().is_symlink())).count();
    let broken = names.iter().filter(|n| std::fs::symlink_metadata(dir.join(n)).is_ok_and(|m| m.file_type().is_symlink()) && resolve_in_tree(dir, &dir.join(n)).is_none()).count();
    format!("{} entries ({} symbolic links, {} of them not resolvable inside the folder); first: {}", names.len(), links, broken, names.iter().take(6).cloned().collect::<Vec<_>>().join(", "))
}

/// Chip folders such as `C-4364__s-B2` under `wifi/`: `(folder name, chip, revision lower case)`.
fn chip_dirs(wifi: &Path) -> Vec<(String, String, String)> {
    chip_dirs_of(wifi, &WIFI_CHIPS)
}

fn chip_dirs_of(wifi: &Path, chips: &[&str]) -> Vec<(String, String, String)> {
    let mut v = Vec::new();
    for chip in chips {
        let prefix = format!("C-{chip}__s-");
        for n in sorted_names(wifi) {
            if let Some(rev) = n.strip_prefix(&prefix) {
                if !rev.is_empty() && wifi.join(&n).is_dir() {
                    v.push((n.clone(), chip.to_string(), lower(rev)));
                }
            }
        }
    }
    v
}

fn extract_wifi(root: &Path, o: &Options, out: &mut Extracted) {
    if let Some(b) = &o.board {
        return extract_wifi_board(root, o, b, out);
    }
    let wifi = root.join("wifi");
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (dname, chip, rev) in chip_dirs(&wifi) {
        if !o.chips.is_empty() && !o.chips.iter().any(|c| c == &chip) {
            continue;
        }
        let dir = wifi.join(&dname);
        for nv in sorted_names(&dir) {
            let Some(stem) = nv.strip_suffix(".txt") else { continue };
            if !stem.starts_with("P-") {
                continue;
            }
            let Some(parts) = parse_nvram_name(stem) else {
                match nvram_platform_only(stem) {
                    Some(pl) => {
                        out.nvram_platforms.insert(pl);
                    }
                    None => out.notes.push(format!("skip {stem}: the name does not have the shape P-<platform>-<antenna>_M-<module>_V-<vendor>__m-<version>")),
                }
                continue;
            };
            out.nvram_platforms.insert(parts.platform.clone());
            if !o.platforms.is_empty() && !o.platforms.iter().any(|p| p.eq_ignore_ascii_case(&parts.platform)) {
                continue;
            }
            let base = wifi_base(&chip, &rev, &parts);
            let Some(trx) = read_in_tree(root, &dir.join(format!("{}.trx", parts.platform))) else {
                out.notes.push(format!("skip {stem}: no {}.trx (also not through a link)", parts.platform));
                continue;
            };
            let Some(nvram) = read_in_tree(root, &dir.join(&nv)) else {
                out.notes.push(format!("skip {stem}: the file is empty or a link that leaves the folder"));
                continue;
            };
            if !seen.insert(base.clone()) {
                continue;
            }
            out.files.push((format!("{base}.bin"), trx));
            out.files.push((format!("{base}.txt"), clean_nvram(&nvram)));
            for (ext, out_ext) in [("clmb", "clm_blob"), ("txcb", "txcap_blob")] {
                for cand in [format!("{}-{}.{ext}", parts.platform, parts.antenna), format!("{}.{ext}", parts.platform)] {
                    if let Some(b) = read_in_tree(root, &dir.join(&cand)) {
                        out.files.push((format!("{base}.{out_ext}"), b));
                        break;
                    }
                }
            }
            out.wifi_sets += 1;
        }
    }
}


/// The platforms (lower case) whose `<platform>-PlatformConfig.plist` mentions `board_id`, and how many plists were read. The plists
/// are searched under `wifi/` (3 levels, no links); a plist without a platform in its name is counted but cannot name one.
pub fn platforms_for_board(wifi: &Path, board_id: &str) -> (BTreeSet<String>, usize) {
    fn walk(dir: &Path, depth: u32, board_id: &str, hit: &mut BTreeSet<String>, read: &mut usize) {
        for n in sorted_names(dir) {
            let p = dir.join(&n);
            let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
            if m.is_dir() && depth > 0 {
                walk(&p, depth - 1, board_id, hit, read);
            } else if m.is_file() && n.ends_with("PlatformConfig.plist") && m.len() <= 4 << 20 {
                *read += 1;
                let platform = n.strip_suffix("PlatformConfig.plist").unwrap_or("").trim_end_matches(['-', '_']);
                if platform.is_empty() {
                    continue;
                }
                if let Ok(b) = std::fs::read(&p) {
                    if b.windows(board_id.len()).any(|w| w == board_id.as_bytes()) {
                        hit.insert(lower(platform));
                    }
                }
            }
        }
    }
    let (mut hit, mut read) = (BTreeSet::new(), 0);
    walk(wifi, 3, board_id, &mut hit, &mut read);
    (hit, read)
}

/// The kernel's name for the generic part of the files of a chip folder (`brcmfmac4350c2-pcie`), or why not. BCM4350 stepping C2 is
/// the one `pcie.c` maps to `brcmfmac4350c2-pcie` (revisions 0-7); other steppings are not guessed.
pub fn board_file_base(chip: &str, rev: &str) -> Result<String, String> {
    match (chip, rev) {
        ("4350", "c2") => Ok("brcmfmac4350c2-pcie".into()),
        ("4350", r) => Err(format!("C-4350__s-{}: only stepping C2 is known to be what brcmfmac4350c2-pcie is for; this one is not taken", r.to_ascii_uppercase())),
        ("43602", _) => Ok("brcmfmac43602-pcie".into()),
        _ => Err(format!("C-{chip}: not a chip of the board-named mode")),
    }
}

/// Exactly one distinct content among `candidates`, else why not (`what` names the kind for the note).
fn only_one(mut c: Vec<(String, Vec<u8>)>, what: &str) -> Result<(String, Vec<u8>), String> {
    c.sort();
    let mut distinct: Vec<(String, Vec<u8>)> = Vec::new();
    for x in c {
        if !distinct.iter().any(|d| d.1 == x.1) {
            distinct.push(x);
        }
    }
    match distinct.len() {
        0 => Err(format!("no {what} file")),
        1 => Ok(distinct.remove(0)),
        n => Err(format!("{n} different {what} files ({}): not guessing which one", distinct.iter().take(4).map(|d| d.0.clone()).collect::<Vec<_>>().join(", "))),
    }
}

fn extract_wifi_board(root: &Path, o: &Options, bt: &BoardTarget, out: &mut Extracted) {
    let wifi = root.join("wifi");
    if bt.board_id.is_empty() {
        out.notes.push("this Mac's board id (DMI board_name) could not be read: the platform of its Wi-Fi files cannot be told, nothing was taken".into());
        return;
    }
    let (listed, plists) = platforms_for_board(&wifi, &bt.board_id);
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let dirs = chip_dirs_of(&wifi, &BOARD_CHIPS);
    if dirs.is_empty() {
        out.notes.push(format!("no BCM4350 / BCM43602 folder (C-4350__s-C2, C-43602__s-*) under {}: this macOS has no Wi-Fi files for those chips", wifi.display()));
    }
    for (dname, chip, rev) in dirs {
        if !o.chips.is_empty() && !o.chips.iter().any(|c| c == &chip) {
            continue;
        }
        let base = match board_file_base(&chip, &rev) {
            Ok(b) => b,
            Err(e) => {
                out.notes.push(e);
                continue;
            }
        };
        let dir = wifi.join(&dname);
        // every platform with an NVRAM file in this folder
        let mut here: BTreeSet<String> = BTreeSet::new();
        let mut nv_files: Vec<(String, String)> = Vec::new();
        for nv in sorted_names(&dir) {
            let Some(stem) = nv.strip_suffix(".txt") else { continue };
            let pl = parse_nvram_name(stem).map(|p| p.platform).or_else(|| nvram_platform_only(stem));
            if let Some(pl) = pl {
                out.nvram_platforms.insert(pl.clone());
                here.insert(lower(&pl));
                nv_files.push((lower(&pl), nv));
            }
        }
        let hits: Vec<&String> = here.iter().filter(|p| listed.contains(*p)).collect();
        let platform = match hits.len() {
            1 => hits[0].clone(),
            0 => {
                out.notes.push(format!(
                    "{dname}: cannot tell which platform's files belong to board {}: {} PlatformConfig.plist file(s) read under wifi/, none of the folder's platforms ({}) lists it. Nothing was taken from this folder and nothing was guessed",
                    bt.board_id,
                    plists,
                    if here.is_empty() { "no NVRAM file".to_string() } else { here.iter().cloned().collect::<Vec<_>>().join(", ") }
                ));
                continue;
            }
            _ => {
                out.notes.push(format!("{dname}: board {} is listed for several platforms ({}): not guessing", bt.board_id, hits.iter().map(|h| h.as_str()).collect::<Vec<_>>().join(", ")));
                continue;
            }
        };
        out.notes.push(format!("{dname}: board {} is listed for the platform {platform}", bt.board_id));
        let mut any = false;
        let mut put = |ext: &str, bytes: Vec<u8>, out: &mut Extracted| {
            let name = format!("{base}.{}.{ext}", bt.board_type);
            if seen.insert(name.clone()) {
                out.files.push((name, bytes));
                any = true;
            }
        };
        // NVRAM: all of the platform's variants (antenna links share bytes) must be one content
        let cands: Vec<(String, Vec<u8>)> = nv_files.iter().filter(|(p, _)| *p == platform).filter_map(|(_, n)| read_in_tree(root, &dir.join(n)).map(|b| (n.clone(), clean_nvram(&b)))).collect();
        match only_one(cands, "NVRAM") {
            Ok((n, b)) => {
                out.notes.push(format!("NVRAM comes from {n}"));
                put("txt", b, out);
            }
            Err(e) => out.notes.push(format!("{dname}: {e}")),
        }
        for (ext, out_ext, what) in [("clmb", "clm_blob", "CLM"), ("txcb", "txcap_blob", "txcap")] {
            let plain = read_in_tree(root, &dir.join(format!("{platform}.{ext}")));
            let got = match plain {
                Some(b) => Ok((format!("{platform}.{ext}"), b)),
                None => {
                    let pre = format!("{platform}-");
                    let c: Vec<(String, Vec<u8>)> = sorted_names(&dir).into_iter().filter(|n| n.starts_with(&pre) && n.ends_with(&format!(".{ext}"))).filter_map(|n| read_in_tree(root, &dir.join(&n)).map(|b| (n, b))).collect();
                    only_one(c, what)
                }
            };
            match got {
                Ok((n, b)) => {
                    out.notes.push(format!("{what} comes from {n}"));
                    put(out_ext, b, out);
                }
                Err(e) => out.notes.push(format!("{dname}: {e}")),
            }
        }
        if any {
            out.wifi_sets += 1;
        }
    }
}

// ------------------------------------------------------------------------------------------------ Bluetooth

/// The Linux name of a PCIe Bluetooth firmware file from Apple's name (`BCM4377B3_<...>_PCIE_macOS_<Board>_<...>_MUR_<...>.bin`),
/// or why it is skipped. `ext` is `bin` or `ptb`.
pub fn bt_pcie_name(file: &str) -> Result<String, String> {
    let (root, ext) = file.rsplit_once('.').ok_or_else(|| format!("skip {file}: no extension"))?;
    if !matches!(ext, "bin" | "ptb") {
        return Err(format!("skip {file}: not a .bin or .ptb"));
    }
    if root.contains("_DEV") {
        return Err(format!("skip {file}: a development build"));
    }
    let first = lower(root.split('_').next().unwrap_or(""));
    let b = first.as_bytes();
    let shape = b.len() == 9 && first.starts_with("bcm43") && b[5].is_ascii_digit() && b[6].is_ascii_digit() && b[7].is_ascii_lowercase() && b[8].is_ascii_digit();
    if !shape {
        return Err(format!("skip {file}: the name does not start with BCM43xx<stepping>"));
    }
    let chip = &first[3..7];
    let step = &first[7..];
    // the board type: the field after PCIE, or after PCIE_macOS; trailing E, S and 2 characters dropped
    let fields: Vec<&str> = root.split('_').collect();
    let mut board = "";
    for i in 0..fields.len().saturating_sub(1) {
        if fields[i] == "PCIE" {
            board = if fields[i + 1] == "macOS" { fields.get(i + 2).copied().unwrap_or("") } else { fields[i + 1] };
            break;
        }
    }
    let board = lower(board.trim_end_matches(['E', 'S', '2']));
    if board.is_empty() {
        return Err(format!("skip {file}: no board type"));
    }
    let has = |v: &str| fields.contains(&v);
    let vendors = [has("MUR"), has("USI"), has("GEN")];
    if vendors.iter().filter(|x| **x).count() != 1 {
        return Err(format!("skip {file}: vendor not clear"));
    }
    let v = if has("MUR") { "-m" } else if has("USI") { "-u" } else { "" };
    Ok(format!("brcmbt{chip}{step}-apple,{board}{v}.{ext}"))
}

fn extract_bluetooth(root: &Path, o: &Options, out: &mut Extracted) {
    let bt = root.join("bluetooth");
    let bt = &bt;
    let names = sorted_names(bt);
    let mut written: BTreeSet<String> = BTreeSet::new();
    // .bin first, then .ptb (the script's glob order); the PCIe Bluetooth files are the T2 Macs' (a Mac without a T2 chip has none)
    for ext in if o.board.is_some() { &[][..] } else { &["bin", "ptb"][..] } {
        for n in names.iter().filter(|n| n.ends_with(&format!(".{ext}"))) {
            if resolve_in_tree(root, &bt.join(n)).is_none() {
                continue;
            }
            match bt_pcie_name(n) {
                Ok(name) => {
                    if !written.insert(name.clone()) {
                        out.notes.push(format!("skip {n}: {name} already written"));
                        continue;
                    }
                    if let Some(b) = read_in_tree(root, &bt.join(n)) {
                        out.files.push((name, b));
                        out.bt_files += 1;
                    }
                }
                Err(e) => {
                    // names that are simply not PCIe Bluetooth files stay quiet unless they look like one
                    if n.to_ascii_lowercase().starts_with("bcm43") {
                        out.notes.push(e);
                    }
                }
            }
        }
    }
    // UART Bluetooth: exactly one patch-RAM file whose name starts with the controller's chip name becomes the .hcd btbcm asks for
    let Some(prefix) = o.hcd_prefix.as_deref() else {
        out.notes.push("the UART Bluetooth controller's chip name is not known: no .hcd file was taken".into());
        return;
    };
    let starts = |n: &str, p: &str| n.len() >= p.len() && n.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes());
    let hcd_names: Vec<&String> = names.iter().filter(|n| n.to_ascii_lowercase().ends_with(".hcd") && resolve_in_tree(root, &bt.join(n)).is_some()).collect();
    let mut cands: Vec<(&String, Vec<u8>)> = Vec::new();
    for n in hcd_names.iter().filter(|n| starts(n, prefix)) {
        if let Some(b) = read_in_tree(root, &bt.join(n)) {
            if !cands.iter().any(|c| c.1 == b) {
                cands.push((n, b));
            }
        }
    }
    let family = &prefix[..prefix.len().min(7)];
    let others: Vec<&str> = hcd_names.iter().filter(|n| starts(n, family) && !starts(n, prefix)).map(|n| n.as_str()).take(4).collect();
    match cands.len() {
        0 => {
            out.notes.push(format!("no {prefix} UART Bluetooth patch file in macOS"));
            if !others.is_empty() {
                out.notes.push(format!("patch files of other steppings exist ({}): not used, they are for a different controller revision", others.join(", ")));
            }
        }
        1 => {
            out.notes.push(format!("{} comes from {}", o.hcd_name, cands[0].0));
            out.files.push((o.hcd_name.clone(), cands[0].1.clone()));
            out.hcd = true;
        }
        n => out.notes.push(format!("found {n} different {prefix} .hcd files - not guessing which one; none was used")),
    }
}


// ------------------------------------------------------------------------------------------------ what btbcm asks for

/// The firmware names `btbcm_initialize` (Linux 6.12, drivers/bluetooth/btbcm.c) tries, in its order. `hw_name` is the entry of its
/// subversion table (`BCM4345C5`; `None` for a controller it does not list, such as this MacBook's `BCM (003.001.134)`); `board` is
/// the device-tree board name (always `None` on an Intel Mac: there is no DMI or ACPI source); `usb` is `(vid, pid)` for a USB
/// controller (the UART one has no postfix).
pub fn btbcm_names(hw_name: Option<&str>, board: Option<&str>, usb: Option<(u16, u16)>) -> Vec<String> {
    let postfix = usb.map(|(v, p)| format!("-{v:04x}-{p:04x}")).unwrap_or_default();
    let mut v = Vec::new();
    if let Some(hw) = hw_name {
        if let Some(b) = board {
            v.push(format!("brcm/{hw}{postfix}.{b}.hcd"));
        }
        v.push(format!("brcm/{hw}{postfix}.hcd"));
    }
    if let Some(b) = board {
        v.push(format!("brcm/BCM{postfix}.{b}.hcd"));
    }
    v.push(format!("brcm/BCM{postfix}.hcd"));
    v
}

/// The names the kernel log says btbcm tried (`Bluetooth: hci0: BCM: 'brcm/BCM.hcd'` after `firmware Patch file not found, tried:`),
/// in order.
pub fn bt_tried_from_log(log: &str) -> Vec<String> {
    let mut v = Vec::new();
    for l in log.lines() {
        if let Some(i) = l.find("BCM: 'brcm/") {
            let rest = &l[i + 6..];
            if let Some(end) = rest.find('\'') {
                let n = &rest[..end];
                if n.ends_with(".hcd") && !v.iter().any(|x| x == n) {
                    v.push(n.to_string());
                }
            }
        }
    }
    v
}

/// The file name (under `brcm/`) to serve: the first name btbcm tried; `BCM.hcd` when the log has none (the last name of its list,
/// which a UART controller on an Intel Mac always ends with). A name with anything but plain characters is not used.
pub fn hcd_serve_name(tried: &[String]) -> String {
    tried
        .first()
        .and_then(|t| t.strip_prefix("brcm/"))
        .filter(|n| n.ends_with(".hcd") && !n.contains('/') && crate::pack::safe_rel_path(n))
        .map(str::to_string)
        .unwrap_or_else(|| "BCM.hcd".into())
}

/// The chip name a UART controller reports as its local name (`Bluetooth: hci0: BCM4350C0 UART 37.4 MHz Gamay USI UHE`): the last such
/// line of the log. `None` when there is none.
pub fn bt_chip_from_log(log: &str) -> Option<String> {
    let shape = |w: &str| {
        let b = w.as_bytes();
        b.len() >= 8 && b.len() <= 10 && w.starts_with("BCM") && b[3..b.len() - 2].iter().all(u8::is_ascii_digit) && b[b.len() - 2].is_ascii_uppercase() && b[b.len() - 1].is_ascii_digit()
    };
    log.lines().rev().find_map(|l| {
        let after = l.split("hci0: ").nth(1)?;
        let mut w = after.split_whitespace();
        let (first, second) = (w.next()?, w.next()?);
        (shape(first) && second == "UART").then(|| first.to_string())
    })
}

// ------------------------------------------------------------------------------------------------ finding and running

/// Where `wifi/` and `bluetooth/` are inside something macOS-shaped: `dir` itself, `dir/firmware`, or a folder ending in
/// `usr/share/firmware` up to `depth` levels down (a mounted macOS volume, or an image unpacked by 7-Zip, where the volume
/// folder is first). Symbolic links are not followed.
pub fn find_firmware_root(dir: &Path, depth: u32) -> Option<PathBuf> {
    fn has_wifi(d: &Path) -> bool {
        d.join("wifi").is_dir() && (!chip_dirs(&d.join("wifi")).is_empty() || !chip_dirs_of(&d.join("wifi"), &BOARD_CHIPS).is_empty())
    }
    if has_wifi(dir) {
        return Some(dir.to_path_buf());
    }
    for rel in ["usr/share/firmware", "firmware"] {
        if has_wifi(&dir.join(rel)) {
            return Some(dir.join(rel));
        }
    }
    if depth == 0 {
        return None;
    }
    for n in sorted_names(dir) {
        let p = dir.join(&n);
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) {
            if let Some(f) = find_firmware_root(&p, depth - 1) {
                return Some(f);
            }
        }
    }
    None
}

/// Read `src/wifi` and `src/bluetooth` (as in macOS `/usr/share/firmware`) and name what is found the way Linux asks.
/// Fails only when no Wi-Fi firmware of a known chip is there at all.
pub fn extract(src: &Path, o: &Options) -> Result<Extracted, String> {
    let wifi = src.join("wifi");
    if !wifi.is_dir() {
        return Err(format!("there is no wifi folder in {} (is this the firmware folder of macOS, /usr/share/firmware?)", src.display()));
    }
    let mut out = Extracted::default();
    extract_wifi(src, o, &mut out);
    if out.wifi_sets == 0 && o.board.is_none() {
        let chips: Vec<String> = chip_dirs(&wifi).into_iter().map(|c| c.0).collect();
        let seen = if out.nvram_platforms.is_empty() { "no NVRAM file names of the expected shape".to_string() } else { format!("NVRAM files exist for the platforms {}", out.nvram_platforms.iter().cloned().collect::<Vec<_>>().join(", ")) };
        let first_chip = chip_dirs(&wifi).into_iter().next().map(|c| describe_dir(&wifi.join(c.0))).unwrap_or_default();
        let want = [o.chips.join("/"), o.platforms.join("/")].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ");
        return Err(format!(
            "no Wi-Fi firmware {}under {} (chip folders: {}{}; {}; {}{})",
            if want.is_empty() { String::new() } else { format!("for this Mac ({want}) ") },
            wifi.display(),
            if chips.is_empty() { "none".into() } else { chips.join(", ") },
            if chips.is_empty() { " - is this a T2 Mac?" } else { "" },
            seen,
            if first_chip.is_empty() { String::new() } else { format!("first chip folder: {first_chip}; ") },
            if out.notes.is_empty() { "nothing was skipped".to_string() } else { format!("skipped: {}", out.notes.iter().take(3).cloned().collect::<Vec<_>>().join(" | ")) }
        ));
    }
    let bt = src.join("bluetooth");
    if bt.is_dir() {
        extract_bluetooth(src, o, &mut out);
    } else {
        out.notes.push("this macOS has no bluetooth firmware folder (macOS Big Sur and older)".into());
    }
    Ok(out)
}

/// Files named `BCM4364*.hcd` anywhere under `tree` (depth 16, no links followed): the Bluetooth patch files of the BCM4364 UART
/// controller, wherever an older macOS keeps them. Returns the distinct contents (by bytes), each with the name it was found under.
pub fn hcd_candidates(tree: &Path) -> Vec<(String, Vec<u8>)> {
    hcd_candidates_for(tree, "BCM4364")
}

/// Like [`hcd_candidates`] for any controller chip name (`BCM4350C0`): files whose name starts with it (any case) and ends in `.hcd`.
pub fn hcd_candidates_for(tree: &Path, prefix: &str) -> Vec<(String, Vec<u8>)> {
    let prefix = prefix.to_ascii_lowercase();
    fn walk(dir: &Path, depth: u32, prefix: &str, out: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let Ok(t) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            if t.is_dir() && depth > 0 {
                walk(&e.path(), depth - 1, prefix, out);
            } else if t.is_file() && name.to_ascii_lowercase().starts_with(prefix) && name.to_ascii_lowercase().ends_with(".hcd") {
                if let Ok(b) = std::fs::read(e.path()) {
                    if !b.is_empty() && !out.iter().any(|o| o.1 == b) {
                        out.push((name, b));
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, 16, &prefix, &mut out);
    out.sort();
    out
}

/// The controller family of a chip name: `BCM4350C0` -> `BCM4350` (the letters and digit after the number are the stepping).
pub fn bt_family(chip: &str) -> Option<String> {
    let rest = chip.strip_prefix("BCM")?;
    let n = rest.chars().take_while(char::is_ascii_digit).count();
    (n >= 4).then(|| format!("BCM{}", &rest[..n]))
}

/// The Bluetooth patch of a controller where macOS 11 keeps it: Intel HEX text named `<family>-Updater.hex` (`BCM4350-Updater.hex` in
/// `IOBluetooth.framework/Versions/A/Resources`), turned into a `.hcd` by [`crate::ihex::to_hcd`]. Looks for files whose name starts with
/// the family (`BCM4350`) and ends in `-Updater.hex` (any case; the `-MiniDriver-uart.hex` is not used), depth 16, no links followed.
/// Returns the distinct converted contents with the name each was found under; a file that does not convert is skipped and named in
/// the second list.
pub fn hex_candidates_for(tree: &Path, chip: &str) -> (Vec<(String, Vec<u8>)>, Vec<String>) {
    let (mut out, mut bad) = (Vec::new(), Vec::new());
    let Some(family) = bt_family(chip).map(|f| f.to_ascii_lowercase()) else { return (out, bad) };
    // a chip name with its stepping (`BCM4364B3`) narrows the files to that stepping (`BCM4364B3ID2-Updater.hex`, not
    // `BCM4364B0ID1-...`) when there are such files; `BCM4350-Updater.hex` has no stepping in its name, so the family is the fallback
    let stepped = {
        let rest = &chip.to_ascii_lowercase()[family.len().min(chip.len())..];
        let mut it = rest.chars();
        match (it.next(), it.next()) {
            (Some(l), Some(d)) if l.is_ascii_alphabetic() && d.is_ascii_digit() => Some(format!("{family}{l}{d}")),
            _ => None,
        }
    };
    fn walk(dir: &Path, depth: u32, family: &str, out: &mut Vec<(String, Vec<u8>)>, bad: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let Ok(t) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            let low = name.to_ascii_lowercase();
            if t.is_dir() && depth > 0 {
                walk(&e.path(), depth - 1, family, out, bad);
            } else if t.is_file() && low.starts_with(family) && low.ends_with("-updater.hex") {
                match std::fs::read(e.path()).map_err(|e| e.to_string()).and_then(|b| crate::ihex::to_hcd(&b)) {
                    Ok(h) if !out.iter().any(|o| o.1 == h) => out.push((name, h)),
                    Ok(_) => {}
                    Err(why) => bad.push(format!("{name}: {why}")),
                }
            }
        }
    }
    if let Some(st) = &stepped {
        walk(tree, 16, st, &mut out, &mut bad);
    }
    if out.is_empty() {
        walk(tree, 16, &family, &mut out, &mut bad);
    }
    out.sort();
    (out, bad)
}

/// The options for this machine. A T2 Mac (by its product name) gets [`options_for_model`] with the BCM4364 `.hcd` rule, as before. Any
/// other Mac gets the board-named mode with the board id of its DMI (`Mac-` + 16 hex digits; without one the board id is empty and
/// no Wi-Fi file is taken) and, when the UART Bluetooth controller's chip name is known (from the kernel log), that chip's `.hcd` rule written under
/// `hcd_name`. `vendor` is the DMI `sys_vendor` (`Apple Inc.`).
pub fn options_for_machine(dmi_product: &str, vendor: &str, board_name: &str, bt_chip: Option<&str>, hcd_name: &str) -> Options {
    if crate::macboard::is_t2(dmi_product) {
        return options_for_model(dmi_product);
    }
    let board = Some(BoardTarget { board_type: format!("{}-{}", vendor.trim(), dmi_product.trim()), board_id: crate::macboard::board_id_from_dmi(board_name).unwrap_or_default() });
    Options { board, hcd_prefix: bt_chip.map(str::to_string), hcd_name: hcd_name.into(), ..Options::default() }
}

/// What the Mac model says about the platform and the chip, for the Macs where it was seen on a real one. Everything else
/// returns empty lists (take all, and the size limits of a driver pack say when that is too much).
/// Only `Macmini8,1` -> `lanai` is verified (a Mac mini 2018); see docs/MAC-WIFI-BLUETOOTH-SETUP.md.
pub fn options_for_model(dmi_product: &str) -> Options {
    match dmi_product.trim() {
        "Macmini8,1" => Options { platforms: vec!["lanai".into()], chips: vec!["4364".into()], ..Options::default() },
        // MacBookPro16,2 (13-inch 2020): brcmfmac on the machine asked for `brcmfmac4364b3-pcie.apple,trinidad-HRPN-u-7.7-X0.bin`
        // (chip BCM4364 stepping B3, board `apple,trinidad`); Apple's own recovery image has `wifi/C-4364__s-B3/` with the
        // `trinidad` set. The Bluetooth controller is the BCM4364B3 on a UART (`hci_uart_bcm`, asks for `brcm/BCM.hcd`).
        "MacBookPro16,2" => Options { platforms: vec!["trinidad".into()], chips: vec!["4364".into()], hcd_prefix: Some("BCM4364B3".into()), ..Options::default() },
        _ => Options::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = PathBuf::from(std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())).join(format!("iyakki-appfw-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn put(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    /// A fake `/usr/share/firmware` with names that mirror a real T2 Mac's (the Wi-Fi names and the Mac mini 2018 were seen
    /// on a real machine; the Bluetooth `.bin` names are the shapes the script documents, not seen on a real Mac).
    pub(crate) fn fixture(root: &Path) {
        let w = "wifi/C-4364__s-B2";
        put(root, &format!("{w}/P-lanai-X0_M-HRPN_V-u__m-7.5.txt"), b"sromrev=11\nboardtype = 0x8a0\n  macaddr =00:90:4c:c5:12:38\n\n# comment\ncc=X0\r\n");
        put(root, &format!("{w}/P-lanai-X0_M-HRPN_V-m__m-7.5.txt"), b"sromrev=11\nboardrev=0x1104\n");
        put(root, &format!("{w}/P-lanai-X3_M-HRPN_V-u__m-7.5.txt"), b"sromrev=11\nboardrev=0x1105\n");
        put(root, &format!("{w}/lanai.trx"), b"TRX-LANAI-FIRMWARE");
        put(root, &format!("{w}/lanai-X0.clmb"), b"CLM-LANAI-X0");
        put(root, &format!("{w}/lanai.clmb"), b"CLM-LANAI");
        put(root, &format!("{w}/lanai.txcb"), b"TXCAP-LANAI");
        put(root, &format!("{w}/P-kauai-X1_M-MWAY_V-m__m-9.1.txt"), b"sromrev=11\n");
        put(root, &format!("{w}/kauai.trx"), b"TRX-KAUAI");
        put(root, &format!("{w}/P-nofirmware-X0_M-ABCD_V-u__m-1.0.txt"), b"sromrev=11\n");
        put(root, &format!("{w}/P-broken_name.txt"), b"x=1\n");
        let w3 = "wifi/C-4377__s-B3";
        put(root, &format!("{w3}/P-formosa-X3_M-YSBC_V-m__m-4.0.txt"), b"sromrev = 11\n");
        put(root, &format!("{w3}/formosa.trx"), b"TRX-FORMOSA");
        put(root, &format!("{w3}/formosa.clmb"), b"CLM-FORMOSA");
        put(root, "wifi/C-9999__s-A0/P-x-X0_M-A_V-u__m-1.txt", b"x=1\n");
        put(root, "wifi/C-9999__s-A0/x.trx", b"nope");
        let b = "bluetooth";
        put(root, &format!("{b}/BCM4377B3_003.001.025.0103.0000_PCIE_macOS_Formosa_E_MUR_5.5.bin"), b"BT-FORMOSA-MUR-BIN");
        put(root, &format!("{b}/BCM4377B3_003.001.025.0103.0000_PCIE_macOS_Formosa_E_MUR_5.5.ptb"), b"BT-FORMOSA-MUR-PTB");
        put(root, &format!("{b}/BCM4377B3_003.001.025.0103.0000_PCIE_macOS_Formosa_E_USI_5.5.bin"), b"BT-FORMOSA-USI-BIN");
        put(root, &format!("{b}/BCM4377B3_003.001.025.0103.0000_PCIE_macOS_Formosa_E_USI_5.5_DEV.bin"), b"dev build");
        put(root, &format!("{b}/BCM4377B3_003.001.025.0103.0000_PCIE_macOS_Formosa_E_MUR_USI_5.5.bin"), b"two vendors");
        put(root, &format!("{b}/BCM4377B3_003.001.025.0103.0000_PCIE_macOS_Formosa_E_GEN_5.5.ptb"), b"BT-FORMOSA-GEN-PTB");
        put(root, &format!("{b}/BCM4378B1_003.001.025.0103.0000_PCIE_macOS_Hawaii_S2_GEN_1.0.bin"), b"BT-HAWAII-GEN-BIN");
        put(root, &format!("{b}/BCM4377B3_nothing.bin"), b"no board");
        put(root, &format!("{b}/other.bin"), b"not bluetooth firmware");
        put(root, &format!("{b}/BCM4364B3_1234.hcd"), b"HCD-4364-B3");
    }

    #[test]
    fn nvram_names_split_into_platform_antenna_module_vendor_and_version() {
        let n = parse_nvram_name("P-lanai-X0_M-HRPN_V-u__m-7.5").unwrap();
        assert_eq!((n.platform.as_str(), n.antenna.as_str(), n.module.as_str(), n.vendor.as_str(), n.version.as_str()), ("lanai", "X0", "HRPN", "u", "7.5"));
        assert_eq!(wifi_base("4364", "b2", &n), "brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0", "the name the Mac mini 2018 asks for");
        for bad in ["lanai-X0_M-A_V-u__m-1", "P-lanai_X0_M-A_V-u__m-1", "P--X0_M-A_V-u__m-1", "P-lanai-X0_M-A_V-u_m-1", "P-lanai-X0_M-A_B_V-u__m-1", "P-lanai-X0_M-_V-u__m-1", "P-lanai-X0_M-A_V-u__m-"] {
            assert!(parse_nvram_name(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn nvram_text_loses_spaces_around_equals_and_blank_lines_and_nothing_else() {
        let got = clean_nvram(b"sromrev=11\nboardtype = 0x8a0\n  macaddr =00:90\n\n   \n# comment = not a key?\nnoequals\nlast=1");
        assert_eq!(String::from_utf8(got).unwrap(), "sromrev=11\nboardtype= 0x8a0\nmacaddr=00:90\n# comment = not a key?\nnoequals\nlast=1\n");
        assert_eq!(clean_nvram(b""), b"");
        assert_eq!(clean_nvram(b"a = b = c\r\n"), b"a= b = c\r\n", "only the first equals sign is touched, CR stays");
    }

    #[test]
    fn pcie_bluetooth_names_follow_chip_stepping_board_and_vendor() {
        assert_eq!(bt_pcie_name("BCM4377B3_003.001.025.0103.0000_PCIE_macOS_Formosa_E_MUR_5.5.bin").unwrap(), "brcmbt4377b3-apple,formosa-m.bin");
        assert_eq!(bt_pcie_name("BCM4377B3_x_PCIE_macOS_Formosa_E_USI_5.5.ptb").unwrap(), "brcmbt4377b3-apple,formosa-u.ptb");
        assert_eq!(bt_pcie_name("BCM4378B1_x_PCIE_macOS_Hawaii_S2_GEN_1.0.bin").unwrap(), "brcmbt4378b1-apple,hawaii.bin", "trailing S and 2 dropped, GEN has no vendor letter");
        assert_eq!(bt_pcie_name("BCM4377B3_x_PCIE_Taurus_MUR_1.bin").unwrap(), "brcmbt4377b3-apple,taurus-m.bin", "board right after PCIE");
        for (bad, why) in [("BCM4377B3_x_PCIE_macOS_Formosa_E_USI_DEV.bin", "development"), ("BCM4377B3_x_PCIE_macOS_Formosa_E_MUR_USI.bin", "vendor"), ("BCM4377B3_nothing.bin", "board"), ("other.bin", "BCM43"), ("BCM4377B3_x_PCIE_macOS_Formosa_E_MUR.txt", ".bin"), ("BCM4377_x_PCIE_macOS_F_MUR.bin", "BCM43"), ("BCM4377B3_x_PCIE_macOS_E_MUR.bin", "board")] {
            let e = bt_pcie_name(bad).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
        // a vendor word only counts as a whole field
        assert!(bt_pcie_name("BCM4377B3_x_PCIE_macOS_Formosa_E_MURX_5.bin").is_err());
    }

    #[test]
    fn a_fake_macos_firmware_folder_gives_the_names_linux_asks_for() {
        let root = tmp("all");
        fixture(&root);
        let x = extract(&root, &Options::default()).unwrap();
        let names = x.names();
        let want_lanai = "brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0";
        for ext in ["bin", "txt", "clm_blob", "txcap_blob"] {
            assert!(names.contains(&format!("{want_lanai}.{ext}").as_str()), "{ext}: {names:?}");
        }
        let get = |n: &str| x.files.iter().find(|f| f.0 == n).map(|f| f.1.clone()).unwrap();
        assert_eq!(get(&format!("{want_lanai}.bin")), b"TRX-LANAI-FIRMWARE");
        assert_eq!(get(&format!("{want_lanai}.clm_blob")), b"CLM-LANAI-X0", "the antenna-specific blob wins");
        assert_eq!(get(&format!("{want_lanai}.txcap_blob")), b"TXCAP-LANAI");
        assert_eq!(String::from_utf8(get(&format!("{want_lanai}.txt"))).unwrap(), "sromrev=11\nboardtype= 0x8a0\nmacaddr=00:90:4c:c5:12:38\n# comment\ncc=X0\r\n");
        // the other vendor letter and antenna get their own sets
        assert!(names.contains(&"brcmfmac4364b2-pcie.apple,lanai-HRPN-m-7.5-X0.bin".into()) && names.contains(&"brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X3.bin".into()));
        assert!(names.contains(&"brcmfmac4364b2-pcie.apple,kauai-MWAY-m-9.1-X1.bin".into()));
        assert!(names.contains(&"brcmfmac4377b3-pcie.apple,formosa-YSBC-m-4.0-X3.bin".into()));
        assert_eq!(get("brcmfmac4377b3-pcie.apple,formosa-YSBC-m-4.0-X3.txt"), b"sromrev= 11\n");
        // no trx -> skipped with a note; unknown chip folder and badly shaped names are ignored
        assert!(!names.iter().any(|n| n.contains("nofirmware") || n.contains("9999")));
        assert!(x.notes.iter().any(|n| n.contains("nofirmware") && n.contains("no nofirmware.trx")));
        assert!(x.notes.iter().any(|n| n.contains("P-broken_name")));
        assert_eq!(x.wifi_sets, 5);
        // Bluetooth
        assert_eq!(get("brcmbt4377b3-apple,formosa-m.bin"), b"BT-FORMOSA-MUR-BIN");
        assert_eq!(get("brcmbt4377b3-apple,formosa-m.ptb"), b"BT-FORMOSA-MUR-PTB");
        assert_eq!(get("brcmbt4377b3-apple,formosa-u.bin"), b"BT-FORMOSA-USI-BIN");
        assert_eq!(get("brcmbt4377b3-apple,formosa.ptb"), b"BT-FORMOSA-GEN-PTB");
        assert_eq!(get("brcmbt4378b1-apple,hawaii.bin"), b"BT-HAWAII-GEN-BIN");
        assert_eq!(x.bt_files, 5, "development builds, two-vendor names, nameless boards and other files are not taken");
        assert!(x.notes.iter().any(|n| n.contains("_DEV")) && x.notes.iter().any(|n| n.contains("vendor not clear")) && x.notes.iter().any(|n| n.contains("no board type")));
        assert!(x.hcd && get("BCM.hcd") == b"HCD-4364-B3");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn two_hcd_files_are_not_guessed_and_a_missing_one_is_said() {
        let root = tmp("hcd2");
        fixture(&root);
        put(&root, "bluetooth/BCM4364B0_5678.hcd", b"HCD-4364-B0");
        let x = extract(&root, &Options::default()).unwrap();
        assert!(!x.hcd && !x.names().contains(&"BCM.hcd"));
        assert!(x.notes.iter().any(|n| n.contains("not guessing")));
        std::fs::remove_file(root.join("bluetooth/BCM4364B0_5678.hcd")).unwrap();
        std::fs::remove_file(root.join("bluetooth/BCM4364B3_1234.hcd")).unwrap();
        let x = extract(&root, &Options::default()).unwrap();
        assert!(!x.hcd && x.notes.iter().any(|n| n.contains("no BCM4364 UART")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_model_filter_keeps_only_that_macs_platform_and_chip() {
        let root = tmp("filter");
        fixture(&root);
        let x = extract(&root, &options_for_model("Macmini8,1")).unwrap();
        assert!(x.names().iter().all(|n| !n.contains("kauai") && !n.contains("4377") || n.starts_with("brcmbt")), "{:?}", x.names());
        assert_eq!(x.wifi_sets, 3, "lanai: u/X0, m/X0, u/X3");
        assert!(extract(&root, &Options { platforms: vec!["nosuch".into()], ..Options::default() }).unwrap_err().contains("nosuch"));
        assert_eq!(options_for_model("MacBookPro15,1"), Options::default(), "unknown models take everything");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_folder_without_wifi_firmware_is_a_plain_error() {
        let root = tmp("empty");
        assert!(extract(&root, &Options::default()).unwrap_err().contains("no wifi folder"));
        std::fs::create_dir_all(root.join("wifi")).unwrap();
        assert!(extract(&root, &Options::default()).unwrap_err().contains("is this a T2 Mac"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn bcm4364_patch_files_are_found_anywhere_in_the_tree_once_per_distinct_content() {
        let root = tmp("hcd");
        put(&root, "a/b/usr/share/firmware/bluetooth/BCM4364B3_1.hcd", b"ONE");
        put(&root, "c/BCM4364B3_copy.hcd", b"ONE");
        put(&root, "d/bcm4364b0_2.hcd", b"TWO");
        put(&root, "d/BCM4377B3.hcd", b"OTHER");
        link(&root, "d/BCM4364_link.hcd", "bcm4364b0_2.hcd");
        let c = hcd_candidates(&root);
        assert_eq!(c.len(), 2, "{:?}", c.iter().map(|x| &x.0).collect::<Vec<_>>());
        assert!(c.iter().any(|x| x.1 == b"ONE") && c.iter().any(|x| x.1 == b"TWO"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_firmware_folder_is_found_inside_a_volume_or_an_unpacked_image() {
        let root = tmp("find");
        fixture(&root.join("2.hfs/Macintosh HD/usr/share/firmware"));
        assert_eq!(find_firmware_root(&root, 5), Some(root.join("2.hfs/Macintosh HD/usr/share/firmware")));
        assert_eq!(find_firmware_root(&root, 1), None, "too deep");
        let direct = tmp("find2");
        fixture(&direct);
        assert_eq!(find_firmware_root(&direct, 0), Some(direct.clone()));
        let empty = tmp("find3");
        assert_eq!(find_firmware_root(&empty, 5), None);
        for p in [root, direct, empty] {
            std::fs::remove_dir_all(p).unwrap();
        }
    }

    #[test]
    fn a_symbolic_link_is_never_followed_or_read() {
        let root = tmp("link");
        fixture(&root);
        std::fs::remove_file(root.join("wifi/C-4364__s-B2/lanai.trx")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("wifi/C-4364__s-B2/lanai.trx")).unwrap();
        let x = extract(&root, &options_for_model("Macmini8,1")).unwrap_err();
        assert!(x.contains("lanai"), "{x}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn link(root: &Path, from: &str, to: &str) {
        let p = root.join(from);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(to, p).unwrap();
    }

    /// The shapes of `wifi/C-4364__s-B2` in a recent macOS recovery image (names and link targets from a real listing, contents made up).
    fn device_shapes(root: &Path, with_lanai_nvram: bool) {
        let w = "wifi/C-4364__s-B2";
        put(root, &format!("{w}/ekans.trx"), b"TRX-EKANS");
        put(root, &format!("{w}/ekans.clmb"), b"CLM-EKANS");
        put(root, &format!("{w}/ekans.txcb"), b"TXCAP-EKANS");
        put(root, &format!("{w}/lanai.clmb"), b"CLM-LANAI");
        put(root, &format!("{w}/lanai.txcb"), b"TXCAP-LANAI");
        link(root, &format!("{w}/lanai.trx"), "ekans.trx");
        for a in ["ID", "X0", "X2", "X3"] {
            for e in ["trx", "clmb", "txcb"] {
                link(root, &format!("{w}/lanai-{a}.{e}"), &format!("lanai.{e}"));
            }
        }
        for (v, ver) in [("m", "7.7"), ("u", "7.5")] {
            put(root, &format!("{w}/P-ekans_M-HRPN_V-{v}__m-{ver}.txt"), b"sromrev=11\nplatform=ekans\n");
            link(root, &format!("{w}/P-ekans-X0_M-HRPN_V-{v}__m-{ver}.txt"), &format!("P-ekans_M-HRPN_V-{v}__m-{ver}.txt"));
            if with_lanai_nvram {
                put(root, &format!("{w}/P-lanai_M-HRPN_V-{v}__m-{ver}.txt"), format!("sromrev= 11\nplatform=lanai-{v}\n").as_bytes());
                for a in ["ID", "X0", "X2", "X3"] {
                    link(root, &format!("{w}/P-lanai-{a}_M-HRPN_V-{v}__m-{ver}.txt"), &format!("P-lanai_M-HRPN_V-{v}__m-{ver}.txt"));
                }
            }
        }
    }

    #[test]
    fn the_real_shape_of_a_recovery_images_firmware_folder_with_chains_of_links_gives_the_kernels_names() {
        let root = tmp("shapes");
        device_shapes(&root, true);
        let x = extract(&root, &options_for_model("Macmini8,1")).unwrap();
        let get = |n: &str| x.files.iter().find(|f| f.0 == n).map(|f| f.1.clone()).unwrap_or_else(|| panic!("{n} missing in {:?}", x.names()));
        let want = "brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0";
        assert_eq!(get(&format!("{want}.bin")), b"TRX-EKANS", "lanai-X0.trx -> lanai.trx -> ekans.trx is followed to the bytes");
        assert_eq!(get(&format!("{want}.txt")), b"sromrev= 11\nplatform=lanai-u\n", "the antenna-named NVRAM link gives the platform's file");
        assert_eq!(get(&format!("{want}.clm_blob")), b"CLM-LANAI");
        assert_eq!(get(&format!("{want}.txcap_blob")), b"TXCAP-LANAI");
        for a in ["ID", "X0", "X2", "X3"] {
            assert!(x.names().contains(&format!("brcmfmac4364b2-pcie.apple,lanai-HRPN-m-7.7-{a}.bin").as_str()));
        }
        assert_eq!(x.wifi_sets, 8, "two vendors by four antennas, only lanai");
        assert!(x.names().iter().all(|n| !n.contains("ekans")), "the other platform's sets are left out");
        assert_eq!(extract(&root, &Options::default()).unwrap().wifi_sets, 10);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_nvram_for_the_macs_platform_is_reported_with_what_was_seen_and_never_guessed() {
        let root = tmp("nolanai");
        device_shapes(&root, false);
        let e = extract(&root, &options_for_model("Macmini8,1")).unwrap_err();
        assert!(e.contains("no Wi-Fi firmware for this Mac (4364 lanai)") && e.contains("platforms ekans") && e.contains("C-4364__s-B2") && e.contains("symbolic links"), "{e}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn links_that_leave_the_tree_loop_or_point_to_nothing_are_never_followed() {
        let root = tmp("escape");
        let outside = tmp("escape-out");
        std::fs::write(outside.join("secret"), b"SECRET").unwrap();
        let tree = root.join("firmware");
        put(&tree, "wifi/x", b"1");
        link(&tree, "wifi/up", "../../../escape-out/secret");
        link(&tree, "wifi/abs", &outside.join("secret").display().to_string());
        link(&tree, "wifi/a", "b");
        link(&tree, "wifi/b", "a");
        link(&tree, "wifi/dangling", "nothing");
        link(&tree, "wifi/dirlink", "../wifi");
        for n in ["up", "abs", "a", "b", "dangling", "dirlink/x"] {
            assert!(read_in_tree(&tree, &tree.join("wifi").join(n)).is_none(), "{n}");
        }
        link(&tree, "wifi/sib", "../wifi/x");
        assert_eq!(read_in_tree(&tree, &tree.join("wifi/sib")).unwrap(), b"1");
        assert!(describe_dir(&tree.join("wifi")).contains("symbolic links"));
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    /// The macOS-side script and this code must name the same files with the same bytes for the same fixture.
    /// Runs only where bash, sed, awk and tr exist.
    #[test]
    fn the_macos_script_and_the_rust_extractor_produce_the_same_names_and_bytes() {
        let have = |p: &str| std::process::Command::new(p).arg("--version").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok();
        if !have("bash") || !have("sed") || !have("awk") {
            eprintln!("skipped: bash, sed or awk not available");
            return;
        }
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/drivers/mac-wifi-firmware-from-macos.sh");
        let root = tmp("parity");
        let src = root.join("firmware");
        fixture(&src);
        let out = root.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let st = std::process::Command::new("bash").arg(&script).arg(&out).env("IYAKKI_FW_SRC", &src).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().unwrap();
        assert!(st.success());
        let dest = out.join("iyakki-wifi-firmware");
        let mut from_script: Vec<(String, Vec<u8>)> = std::fs::read_dir(&dest).unwrap().flatten().filter(|e| e.path().is_file()).map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap())).collect();
        from_script.sort();
        let mut mine = extract(&src, &Options::default()).unwrap().files;
        mine.sort();
        let names = |v: &Vec<(String, Vec<u8>)>| v.iter().map(|f| f.0.clone()).collect::<Vec<_>>();
        assert_eq!(names(&mine), names(&from_script));
        for (a, b) in mine.iter().zip(&from_script) {
            assert_eq!(a.1, b.1, "bytes of {}", a.0);
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    // ---- Macs without a T2 chip (BCM4350, BCM43602): board-named files

    const BOARD: &str = "Mac-B4831CEBD52A0C4C";

    fn target() -> BoardTarget {
        BoardTarget { board_type: "Apple Inc.-MacBookPro14,1".into(), board_id: BOARD.into() }
    }

    fn board_opts() -> Options {
        Options { board: Some(target()), hcd_prefix: Some("BCM4350C0".into()), ..Options::default() }
    }

    /// The folder shapes of a recent macOS for the two chips (the names follow the T2 chips' folders: `C-<chip>__s-<stepping>`, `P-...txt`,
    /// `<platform>.clmb`, `<platform>-PlatformConfig.plist`; contents and platform names made up).
    fn board_fixture(root: &Path) {
        let w = "wifi/C-4350__s-C2";
        for (v, ver) in [("m", "1.1"), ("u", "1.2")] {
            // the same bytes under both vendor names: one content
            put(root, &format!("{w}/P-alpha_M-AAAA_V-{v}__m-{ver}.txt"), b"sromrev=11\nboardtype = 0x1\n");
        }
        put(root, &format!("{w}/P-bravo_M-BBBB_V-m__m-2.1.txt"), b"sromrev=11\nboardtype=0x2\n");
        put(root, &format!("{w}/alpha.clmb"), b"CLM-ALPHA");
        put(root, &format!("{w}/alpha.txcb"), b"TXCAP-ALPHA");
        put(root, &format!("{w}/bravo.clmb"), b"CLM-BRAVO");
        put(root, "wifi/alpha-PlatformConfig.plist", format!("<plist><array><string>Mac-AAAAAAAAAAAAAAAA</string><string>{BOARD}</string></array></plist>").as_bytes());
        put(root, "wifi/bravo-PlatformConfig.plist", b"<plist><array><string>Mac-1111111111111111</string></array></plist>");
        let w = "wifi/C-43602__s-C1";
        put(root, &format!("{w}/P-charlie_M-CCCC_V-u__m-3.0.txt"), b"sromrev=12\n");
        put(root, &format!("{w}/charlie.clmb"), b"CLM-CHARLIE");
        put(root, "wifi/charlie-PlatformConfig.plist", b"Mac-FFEEDDCCBBAA9988");
        // a T2 chip folder is in the same macOS
        put(root, "wifi/C-4364__s-B2/P-lanai-X0_M-HRPN_V-u__m-7.5.txt", b"sromrev=11\n");
        put(root, "wifi/C-4364__s-B2/lanai.trx", b"TRX-LANAI");
        put(root, "bluetooth/BCM4350C0_003.001.013.0100.hcd", b"HCD-4350C0");
        put(root, "bluetooth/BCM4350C5_other.hcd", b"HCD-4350C5");
        put(root, "bluetooth/BCM4364B3_1234.hcd", b"HCD-4364");
        put(root, "bluetooth/BCM4377B3_x_PCIE_macOS_Formosa_E_MUR_5.5.bin", b"BT-PCIE");
    }

    #[test]
    fn a_mac_without_t2_gets_nvram_and_blobs_named_after_its_dmi_board_type_and_never_a_bin() {
        let root = tmp("board");
        board_fixture(&root);
        let x = extract(&root, &board_opts()).unwrap();
        let get = |n: &str| x.files.iter().find(|f| f.0 == n).map(|f| f.1.clone()).unwrap_or_else(|| panic!("{n} missing in {:?}", x.names()));
        let base = "brcmfmac4350c2-pcie.Apple Inc.-MacBookPro14,1";
        assert_eq!(get(&format!("{base}.txt")), b"sromrev=11\nboardtype= 0x1\n", "alpha's NVRAM (both vendor files hold the same bytes), spaces around = removed");
        assert_eq!(get(&format!("{base}.clm_blob")), b"CLM-ALPHA");
        assert_eq!(get(&format!("{base}.txcap_blob")), b"TXCAP-ALPHA");
        assert_eq!(get("BCM.hcd"), b"HCD-4350C0", "only the file that starts with the controller's chip name");
        // nothing of bravo (another board), charlie (43602 folder, its plist lists another board), the T2 chip or the PCIe Bluetooth files
        assert_eq!(x.names().len(), 4, "{:?}", x.names());
        assert!(!x.names().iter().any(|n| n.ends_with(".bin")), "the .bin comes from linux-firmware");
        assert_eq!((x.wifi_sets, x.bt_files, x.hcd), (1, 0, true));
        assert!(x.notes.iter().any(|n| n.contains("listed for the platform alpha")));
        assert!(x.notes.iter().any(|n| n.contains("BCM4350C0_003.001.013.0100.hcd")) || x.notes.iter().any(|n| n.contains("BCM.hcd comes from BCM4350C0_")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_43602_folder_gives_the_unrevised_chip_name_when_its_platform_is_listed_for_the_board() {
        let root = tmp("board43602");
        board_fixture(&root);
        let o = Options { board: Some(BoardTarget { board_type: "Apple Inc.-iMac15,1".into(), board_id: "Mac-FFEEDDCCBBAA9988".into() }), hcd_prefix: None, ..Options::default() };
        let x = extract(&root, &o).unwrap();
        let mut names = x.names();
        names.sort();
        assert_eq!(names, vec!["brcmfmac43602-pcie.Apple Inc.-iMac15,1.clm_blob", "brcmfmac43602-pcie.Apple Inc.-iMac15,1.txt"]);
        assert!(x.notes.iter().any(|n| n.contains("chip name is not known")), "{:?}", x.notes);
        assert!(!x.notes.iter().any(|n| n.contains("txcap")) || x.notes.iter().any(|n| n.contains("no txcap file")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_t2_mode_ignores_the_non_t2_chip_folders_and_the_board_mode_ignores_the_t2_ones() {
        let root = tmp("modes");
        board_fixture(&root);
        let t2 = extract(&root, &Options::default()).unwrap();
        assert!(t2.names().iter().all(|n| !n.contains("4350") && !n.contains("43602")), "{:?}", t2.names());
        assert!(t2.names().iter().any(|n| n.contains("lanai")));
        let b = extract(&root, &board_opts()).unwrap();
        assert!(b.names().iter().all(|n| !n.contains("lanai") && !n.contains("4364")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn when_the_platform_cannot_be_told_nothing_is_taken_and_the_notes_say_why() {
        // the board id is in no plist
        let root = tmp("noplist");
        board_fixture(&root);
        let o = Options { board: Some(BoardTarget { board_type: "Apple Inc.-MacBookPro99,9".into(), board_id: "Mac-0123456789ABCDEF".into() }), hcd_prefix: None, ..Options::default() };
        let x = extract(&root, &o).unwrap();
        assert!(x.files.is_empty() && x.wifi_sets == 0);
        assert!(x.notes.iter().any(|n| n.contains("cannot tell which platform") && n.contains("nothing was guessed") && n.contains("alpha, bravo")), "{:?}", x.notes);
        // no plist files at all
        std::fs::remove_file(root.join("wifi/alpha-PlatformConfig.plist")).unwrap();
        std::fs::remove_file(root.join("wifi/bravo-PlatformConfig.plist")).unwrap();
        std::fs::remove_file(root.join("wifi/charlie-PlatformConfig.plist")).unwrap();
        let x = extract(&root, &board_opts()).unwrap();
        assert!(x.files.iter().all(|f| f.0 == "BCM.hcd"), "{:?}", x.names());
        assert!(x.notes.iter().any(|n| n.contains("0 PlatformConfig.plist")), "{:?}", x.notes);
        // two platforms list the board
        put(&root, "wifi/bravo-PlatformConfig.plist", BOARD.as_bytes());
        put(&root, "wifi/alpha-PlatformConfig.plist", BOARD.as_bytes());
        let x = extract(&root, &board_opts()).unwrap();
        assert!(x.notes.iter().any(|n| n.contains("listed for several platforms (alpha, bravo)")), "{:?}", x.notes);
        assert_eq!(x.wifi_sets, 0);
        // an unreadable board id
        let o = Options { board: Some(BoardTarget { board_type: "Apple Inc.-X".into(), board_id: String::new() }), ..Options::default() };
        assert!(extract(&root, &o).unwrap().notes.iter().any(|n| n.contains("board id (DMI board_name) could not be read")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn different_nvram_files_for_one_platform_are_not_guessed_between_and_other_steppings_are_not_taken() {
        let root = tmp("variants");
        board_fixture(&root);
        put(&root, "wifi/C-4350__s-C2/P-alpha_M-AAAA_V-x__m-9.9.txt", b"sromrev=11\nother=1\n");
        let x = extract(&root, &board_opts()).unwrap();
        assert!(!x.names().iter().any(|n| n.ends_with(".txt")), "{:?}", x.names());
        assert!(x.notes.iter().any(|n| n.contains("2 different NVRAM files") && n.contains("not guessing")), "{:?}", x.notes);
        assert!(x.names().iter().any(|n| n.ends_with(".clm_blob")), "the blobs do not depend on the NVRAM choice");
        assert_eq!(x.wifi_sets, 1);
        // a stepping other than C2 of the BCM4350
        let other = tmp("variants2");
        put(&other, "wifi/C-4350__s-C5/P-alpha_M-AAAA_V-m__m-1.0.txt", b"x=1\n");
        put(&other, "wifi/alpha-PlatformConfig.plist", BOARD.as_bytes());
        let y = extract(&other, &board_opts()).unwrap();
        assert!(y.files.iter().all(|f| f.0 == "BCM.hcd") && y.notes.iter().any(|n| n.contains("only stepping C2")), "{:?}", y.notes);
        // clm blobs by antenna: one content is taken, two are not guessed
        let c = tmp("variants3");
        put(&c, "wifi/C-4350__s-C2/P-alpha_M-AAAA_V-m__m-1.0.txt", b"x=1\n");
        put(&c, "wifi/C-4350__s-C2/alpha-X0.clmb", b"CLM1");
        put(&c, "wifi/C-4350__s-C2/alpha-X1.clmb", b"CLM2");
        put(&c, "wifi/alpha-PlatformConfig.plist", BOARD.as_bytes());
        let z = extract(&c, &board_opts()).unwrap();
        assert!(!z.names().iter().any(|n| n.ends_with(".clm_blob")) && z.notes.iter().any(|n| n.contains("2 different CLM files")), "{:?}", z.notes);
        for p in [root, other, c] {
            std::fs::remove_dir_all(p).unwrap();
        }
    }

    #[test]
    fn a_hcd_for_the_controller_chip_is_taken_only_when_exactly_one_exists() {
        let root = tmp("hcdboard");
        board_fixture(&root);
        put(&root, "bluetooth/BCM4350C0_003.001.013.0200.hcd", b"HCD-4350C0-NEWER");
        let x = extract(&root, &board_opts()).unwrap();
        assert!(!x.hcd && x.notes.iter().any(|n| n.contains("found 2 different BCM4350C0 .hcd files")), "{:?}", x.notes);
        std::fs::remove_file(root.join("bluetooth/BCM4350C0_003.001.013.0200.hcd")).unwrap();
        std::fs::remove_file(root.join("bluetooth/BCM4350C0_003.001.013.0100.hcd")).unwrap();
        let x = extract(&root, &board_opts()).unwrap();
        assert!(!x.hcd && x.notes.iter().any(|n| n.contains("no BCM4350C0 UART")) && x.notes.iter().any(|n| n.contains("other steppings") && n.contains("BCM4350C5_other.hcd")), "{:?}", x.notes);
        // the chip's name decides, not a hard-wired one; a different file name for the result when btbcm asks for another
        let o = Options { board: Some(target()), hcd_prefix: Some("BCM4350C5".into()), hcd_name: "BCM4350C5.hcd".into(), ..Options::default() };
        let x = extract(&root, &o).unwrap();
        assert_eq!(x.files.iter().find(|f| f.0 == "BCM4350C5.hcd").map(|f| f.1.clone()), Some(b"HCD-4350C5".to_vec()));
        // and a copy outside the bluetooth folder is found for any chip name
        let c = hcd_candidates_for(&root, "BCM4350C5");
        assert_eq!(c.len(), 1);
        assert!(hcd_candidates_for(&root, "bcm4364").len() == 1 && hcd_candidates(&root).len() == 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_options_follow_the_machine_t2_or_not() {
        assert_eq!(options_for_machine("Macmini8,1", "Apple Inc.", "Mac-7BA5B2DFE22DDD8C", Some("BCM4364"), "BCM.hcd"), options_for_model("Macmini8,1"));
        let o = options_for_machine("MacBookPro14,1", "Apple Inc.", "Mac-B4831CEBD52A0C4C\n", Some("BCM4350C0"), "BCM.hcd");
        assert_eq!(o.board, Some(target()));
        assert_eq!((o.hcd_prefix.as_deref(), o.hcd_name.as_str()), (Some("BCM4350C0"), "BCM.hcd"));
        let o = options_for_machine("MacBookPro14,1", "Apple Inc.", "garbage", None, "BCM.hcd");
        assert_eq!(o.board.unwrap().board_id, "", "a board id that is not Apple's shape is no board id");
        assert!(o.hcd_prefix.is_none());
    }

    /// The search order of `btbcm_initialize` in Linux 6.12, case by case (btbcm.c lines 650-670): the hardware name from the subversion
    /// table, the board name from the device tree, and the USB postfix.
    #[test]
    fn the_names_btbcm_asks_for_follow_its_search_order_and_the_job_serves_the_first() {
        type Case = (Option<&'static str>, Option<&'static str>, Option<(u16, u16)>, Vec<&'static str>);
        let cases: Vec<Case> = vec![
            // this MacBook: subversion 003.001.134 is not in the UART table, no device-tree board name -> only the generic name
            (None, None, None, vec!["brcm/BCM.hcd"]),
            // a UART controller the table knows
            (Some("BCM4345C5"), None, None, vec!["brcm/BCM4345C5.hcd", "brcm/BCM.hcd"]),
            // with a device-tree board name (Arm boards)
            (Some("BCM4345C5"), Some("apple,j274"), None, vec!["brcm/BCM4345C5.apple,j274.hcd", "brcm/BCM4345C5.hcd", "brcm/BCM.apple,j274.hcd", "brcm/BCM.hcd"]),
            (None, Some("b"), None, vec!["brcm/BCM.b.hcd", "brcm/BCM.hcd"]),
            // a USB controller (the Mac mini 2014)
            (Some("BCM20702A1"), None, Some((0x05ac, 0x8289)), vec!["brcm/BCM20702A1-05ac-8289.hcd", "brcm/BCM-05ac-8289.hcd"]),
            (None, None, Some((0x0a5c, 0x0306)), vec!["brcm/BCM-0a5c-0306.hcd"]),
        ];
        for (hw, board, usb, want) in cases {
            assert_eq!(btbcm_names(hw, board, usb), want, "{hw:?} {board:?} {usb:?}");
        }
        // the job serves the first name the kernel log says it tried, else BCM.hcd
        let log = "[5.0] Bluetooth: hci0: BCM: firmware Patch file not found, tried:\n[5.0] Bluetooth: hci0: BCM: 'brcm/BCM.hcd'\n";
        assert_eq!(bt_tried_from_log(log), vec!["brcm/BCM.hcd"]);
        assert_eq!(hcd_serve_name(&bt_tried_from_log(log)), "BCM.hcd");
        let log2 = "Bluetooth: hci0: BCM: 'brcm/BCM4345C5.hcd'\nBluetooth: hci0: BCM: 'brcm/BCM.hcd'\nBluetooth: hci0: BCM: 'brcm/BCM4345C5.hcd'\n";
        assert_eq!(bt_tried_from_log(log2), vec!["brcm/BCM4345C5.hcd", "brcm/BCM.hcd"]);
        assert_eq!(hcd_serve_name(&bt_tried_from_log(log2)), "BCM4345C5.hcd");
        assert_eq!(hcd_serve_name(&[]), "BCM.hcd");
        for bad in ["brcm/../x.hcd", "brcm/a/b.hcd", "other/BCM.hcd", "brcm/BCM.bin", "brcm/.hcd"] {
            assert_eq!(hcd_serve_name(&[bad.to_string()]), "BCM.hcd", "{bad}");
        }
        // the first name of the table-driven list for this MacBook is what the log shows
        assert_eq!(btbcm_names(None, None, None)[0], bt_tried_from_log(log)[0]);
    }

    #[test]
    fn the_controllers_chip_name_is_read_from_its_local_name_line() {
        let log = "[4.1] Bluetooth: hci0: BCM: chip id 63\n[4.2] Bluetooth: hci0: BCM: features 0x2f\n[4.3] Bluetooth: hci0: BCM4350C0 UART 37.4 MHz Gamay USI UHE\n[4.4] Bluetooth: hci0: BCM (003.001.134) build 1532\n";
        assert_eq!(bt_chip_from_log(log).as_deref(), Some("BCM4350C0"));
        assert_eq!(bt_chip_from_log("Bluetooth: hci0: BCM43430A1 UART x").as_deref(), Some("BCM43430A1"));
        for none in ["", "Bluetooth: hci0: BCM (003.001.134) build 1532", "Bluetooth: hci0: BCM4350C0 USB", "Bluetooth: hci0: BCMX350C0 UART", "Bluetooth: hci0: BCM4350 UART"] {
            assert_eq!(bt_chip_from_log(none), None, "{none}");
        }
    }

    fn ihex_rec(t: u8, addr: u16, data: &[u8]) -> String {
        let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, t];
        b.extend_from_slice(data);
        let sum = b.iter().fold(0u8, |a, x| a.wrapping_add(*x));
        b.push(0u8.wrapping_sub(sum));
        format!(":{}", b.iter().map(|x| format!("{x:02X}")).collect::<String>())
    }

    #[test]
    fn the_macos_11_hex_patch_is_found_by_chip_family_and_converted() {
        let root = tmp("hexfam");
        let res = "macOS Base System/System/Library/Frameworks/IOBluetooth.framework/Versions/A/Resources";
        let hex = |d: u8| format!("$\n{}\n{}\n", ihex_rec(4, 0, &[0xFF, 0]), ihex_rec(0, 0x4000, &[d, d, d]));
        put(&root, &format!("{res}/BCM4350-Updater.hex"), hex(1).as_bytes());
        put(&root, &format!("{res}/BCM4350-MiniDriver-uart.hex"), hex(2).as_bytes());
        put(&root, &format!("{res}/BCM4355-Updater.hex"), hex(3).as_bytes());
        put(&root, &format!("{res}/BCM4350-Broken-Updater.hex"), b":01zz\n");
        let (c, bad) = hex_candidates_for(&root, "BCM4350C0");
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].0, "BCM4350-Updater.hex");
        assert_eq!(crate::ihex::check_hcd(&c[0].1), Ok(3));
        assert_eq!(&c[0].1[..7], &[0x4C, 0xFC, 7, 0x00, 0x40, 0x00, 0xFF]);
        assert!(bad.len() == 1 && bad[0].starts_with("BCM4350-Broken-Updater.hex"), "{bad:?}");
        // another family, no chip, an odd name: nothing
        assert_eq!(hex_candidates_for(&root, "BCM4355A0").0.len(), 1);
        assert!(hex_candidates_for(&root, "").0.is_empty() && hex_candidates_for(&root, "BCM43").0.is_empty());
        assert_eq!(bt_family("BCM4350C0").as_deref(), Some("BCM4350"));
        assert_eq!(bt_family("BCM43430A1").as_deref(), Some("BCM43430"));
        // two different patches of one family (ID1 and ID2 builds): the caller sees both and must not guess
        put(&root, &format!("{res}/BCM4364B0ID1-Updater.hex"), hex(4).as_bytes());
        put(&root, &format!("{res}/BCM4364B0ID2-Updater.hex"), hex(5).as_bytes());
        assert_eq!(hex_candidates_for(&root, "BCM4364B0").0.len(), 2);
    }
}

#[cfg(test)]
mod model_tests {
    use super::*;

    #[test]
    fn the_macbookpro16_2_takes_only_the_trinidad_set_of_the_4364_and_its_bluetooth_chip_is_the_uart_one() {
        let o = options_for_model("MacBookPro16,2");
        assert_eq!((o.platforms.as_slice(), o.chips.as_slice()), (&["trinidad".to_string()][..], &["4364".to_string()][..]));
        assert!(crate::macboard::uart_bluetooth("MacBookPro16,2"));
        // other models are as before: take everything
        assert!(options_for_model("MacBookPro15,4").platforms.is_empty());
        // the stepping of the chip narrows the macOS 11 hex patches (ID2/ID3 of B3, not ID1/ID2 of B0)
        let d = std::env::temp_dir().join(format!("hexstep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let hex = ":020000040000FA\n:0400000001020304F2\n:00000001FF\n";
        for n in ["BCM4364B0ID1-Updater.hex", "BCM4364B3ID2-Updater.hex"] {
            std::fs::write(d.join(n), hex).unwrap();
        }
        let (all, _) = hex_candidates_for(&d, "BCM4364");
        let (b3, _) = hex_candidates_for(&d, "BCM4364B3");
        assert!(all.len() <= 2 && b3.len() <= 1, "{} {}", all.len(), b3.len());
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod live_tree {
    use super::*;

    /// Run by hand on a real macOS firmware folder (`IYAKKI_LIVE_FW=<.../usr/share/firmware>`): the MacBookPro16,2 gets the Wi-Fi set
    /// brcmfmac asks for. Skipped when the variable is not set.
    #[test]
    fn a_real_macos_15_firmware_folder_gives_the_macbookpro16_2_its_trinidad_wifi_set() {
        let Some(root) = std::env::var_os("IYAKKI_LIVE_FW").map(PathBuf::from) else { return };
        let o = options_for_model("MacBookPro16,2");
        let x = extract(&root, &o).unwrap();
        let names = x.names();
        println!("{} files, {} bytes", names.len(), x.total_bytes());
        // `IYAKKI_LIVE_OUT=<dir>`: also write the Wi-Fi files there (to try them on the real Mac)
        if let Some(out) = std::env::var_os("IYAKKI_LIVE_OUT").map(PathBuf::from) {
            std::fs::create_dir_all(&out).unwrap();
            for (n, b) in x.files.iter().filter(|f| f.0.starts_with("brcmfmac")) {
                std::fs::write(out.join(n), b).unwrap();
            }
        }
        for n in &names {
            println!("  {n}");
        }
        assert!(names.contains(&"brcmfmac4364b3-pcie.apple,trinidad-HRPN-u-7.7-X0.bin"), "{names:?}");
        assert!(names.contains(&"brcmfmac4364b3-pcie.apple,trinidad-HRPN-u-7.7-X0.txt"));
        assert!(names.contains(&"brcmfmac4364b3-pcie.apple,trinidad-HRPN-u-7.7-X0.clm_blob"));
        assert!(names.contains(&"brcmfmac4364b3-pcie.apple,trinidad-HRPN-u-7.7-X0.txcap_blob"));
        // the first name brcmfmac asks for is there, with the NVRAM text, the CLM blob and the txcap blob next to it; only trinidad sets
        assert!(names.iter().filter(|n| n.starts_with("brcmfmac")).all(|n| n.starts_with("brcmfmac4364b3-pcie.apple,trinidad-")), "{names:?}");
    }
}
