//! Camera firmware for the built-in FaceTime HD camera (Broadcom 14e4:1570, driver `facetimehd`) of a MacBook without the T2 chip,
//! from Apple's own files, at the person's request. One detached job (`iyakki quick-install mac-camera`):
//!
//! 1. Three byte ranges (about 17 MB in all) of Apple's macOS 10.12.6 combo update are fetched from Apple's update CDN over HTTPS,
//!    resumable, each checked for its size and SHA-256 (`iyakki_drivers::facetime::PARTS`). The whole 1.9 GB image is never downloaded:
//!    the update's payload is a series of xz streams stored as they are, so the camera's driver file and the helper that carries
//!    the sensor files are cut out by range, the way facetimehd-firmware's Makefile does it.
//! 2. The xz streams are decoded with the in-house reader (`xz.rs`), the driver file `AppleCameraInterface` is put together and
//!    checked (SHA-256), its gzip area is unpacked to `firmware.bin` and checked (SHA-256), and the eleven sensor files
//!    `<id>_01XX.dat` are cut from the helper and checked one by one. Anything that does not match is refused with the numbers found.
//! 3. The files become the driver pack `mac-facetimehd` on the data volume (linked into `/lib/firmware` at every start); the driver
//!    is loaded again and the job looks for the camera device (`/dev/video*`, name "Apple Facetime HD").
//!
//! The files are Apple's: they stay on this machine, are never uploaded and are not part of IYAKKI ([`NOTICE`]). Temporary files
//! go to the data volume (never RAM); a partial download is kept for the next try and removed at the end of a successful run.
//! Nothing writes a drive other than the data volume. The world is reached only through [`Env`]; the tests give it a fake.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use iyakki_drivers::{apply, facetime, pack};

pub const NOTICE: &str = "These files are Apple's, downloaded from Apple, stay on this machine, are never uploaded and are not part of IYAKKI.";
pub const PACK_ID: &str = "mac-facetimehd";
/// The camera's PCI id (vendor, device) as the sysfs files print it.
pub const PCI_VENDOR: &str = "0x14e4";
pub const PCI_DEVICE: &str = "0x1570";
/// Free space wanted on the data volume: the pieces are 17.6 MB, the packed files 2 MB; the rest is room to work.
pub const NEEDED_FREE: u64 = 200 << 20;
/// A decoded piece is 16 MiB; nothing larger is accepted.
const MAX_DECODED: u64 = 20 << 20;

/// Where the job keeps its files on the data volume.
pub fn work_dir(data: &Path) -> PathBuf {
    data.join("camera-firmware")
}

pub fn result_path(data: &Path) -> PathBuf {
    work_dir(data).join("last.json")
}

fn part_path(data: &Path, p: &facetime::Part) -> PathBuf {
    work_dir(data).join("download").join(format!("{}.xz", p.name))
}

/// What the machine shows after the driver was loaded again.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Seen {
    /// Video devices of the camera (`/dev/video0`).
    pub devices: Vec<String>,
}

/// The files made from the pieces.
#[derive(Debug, PartialEq, Eq)]
pub struct Built {
    pub firmware: facetime::Extracted,
    pub sets: Vec<(String, Vec<u8>)>,
}

/// Everything the job does to the world.
pub trait Env {
    fn say(&mut self, line: &str);
    /// The FaceTime HD camera (PCI 14e4:1570) is on this machine.
    fn camera_present(&self) -> bool;
    fn free_bytes(&self, dir: &Path) -> Option<u64>;
    /// The data folder is on a RAM file system (a live session).
    fn data_in_memory(&self) -> bool;
    /// Fetch one range of Apple's update into `dest` (resuming), complete and checked when this returns `Ok`.
    fn fetch_part(&mut self, part: &facetime::Part, dest: &Path) -> Result<(), String>;
    /// Decode the three downloaded pieces (`dir/<name>.xz`) and make the firmware and the sensor files, checked.
    fn build(&mut self, dir: &Path) -> Result<Built, String>;
    /// Put the pack on the data volume, link it and load the driver again.
    fn install(&mut self, version: &str, source: &str, pack: &pack::Pack) -> Result<apply::Report, String>;
    fn look(&mut self) -> Seen;
    /// Kernel log lines about the camera, newest last.
    fn kernel_lines(&mut self) -> Vec<String>;
    fn sleep(&mut self, d: Duration);
    fn now(&self) -> i64;
}

/// The plain result of a run, also written to `last.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Outcome {
    pub at: i64,
    pub installed: bool,
    pub firmware_version: String,
    pub sensor_files: usize,
    pub seen: Seen,
    pub headline: String,
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

fn version_for(now: i64) -> String {
    format!("1.{}.{}", (now / 86_400).max(1), (now % 86_400) / 60)
}

/// Bytes the job keeps between runs (the downloaded pieces).
pub fn leftovers(data: &Path) -> u64 {
    let d = work_dir(data).join("download");
    std::fs::read_dir(d).into_iter().flatten().flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()
}

/// Delete the downloaded pieces (the page's Delete download). Returns the bytes freed.
pub fn delete_leftovers(data: &Path) -> u64 {
    let n = leftovers(data);
    let _ = std::fs::remove_dir_all(work_dir(data).join("download"));
    n
}

fn note(env: &mut dyn Env, lines: &mut Vec<String>, l: String) {
    env.say(&l);
    lines.push(l);
}

/// Run the job. Returns the outcome (already written to `last.json`); `Err` only when nothing could be installed.
pub fn run(env: &mut dyn Env, data: &Path) -> Result<Outcome, String> {
    let mut lines: Vec<String> = Vec::new();
    match run_inner(env, data, &mut lines) {
        Ok(o) => Ok(o),
        Err(e) => {
            let kept = leftovers(data);
            if kept > 0 {
                lines.push(format!("kept for a retry: {} MB in {} (the page has Delete download)", kept >> 20, work_dir(data).join("download").display()));
            }
            let o = Outcome { at: env.now(), headline: format!("Not done: {e}"), lines, ..Default::default() };
            write_outcome(data, &o);
            Err(e)
        }
    }
}

fn run_inner(env: &mut dyn Env, data: &Path, lines: &mut Vec<String>) -> Result<Outcome, String> {
    note(env, lines, format!("[0/5] {NOTICE}"));
    if !env.camera_present() {
        return Err("no FaceTime HD camera (PCI 14e4:1570) was found on this computer; nothing was downloaded".into());
    }
    if env.data_in_memory() {
        return Err("the data volume is in memory (a live session): the firmware would be gone after a restart. Start from the installed system".into());
    }
    let wd = work_dir(data);
    let dl = wd.join("download");
    std::fs::create_dir_all(&dl).map_err(|e| format!("{}: {e}", dl.display()))?;
    match env.free_bytes(data) {
        Some(f) if f < NEEDED_FREE => return Err(format!("the data volume has {} MB free; this needs about {} MB", f >> 20, NEEDED_FREE >> 20)),
        Some(f) => note(env, lines, format!("free space on the data volume: {} MB", f >> 20)),
        None => note(env, lines, "note: the free space of the data volume could not be read".into()),
    }
    let total: u64 = facetime::PARTS.iter().map(|p| p.len()).sum();
    note(env, lines, format!("[1/5] downloading three pieces ({} MB) of Apple's macOS 10.12.6 update from Apple's update server (the camera's driver file and the sensor files; not the 1.9 GB image). A stopped download resumes", total >> 20));
    for p in facetime::PARTS.iter() {
        let dest = part_path(data, p);
        env.fetch_part(p, &dest)?;
        note(env, lines, format!("  {}: {} bytes, checksum correct", p.name, p.len()));
    }
    note(env, lines, "[2/5] unpacking: the driver file, its firmware and the sensor files, each checked against the published checksums".into());
    let built = env.build(&dl)?;
    note(env, lines, format!("  firmware {} (sha256 {}), made from {}", built.firmware.version, built.firmware.sha256, built.firmware.from));
    note(env, lines, format!("  {} sensor file(s): {}", built.sets.len(), built.sets.iter().map(|s| s.0.as_str()).collect::<Vec<_>>().join(", ")));

    note(env, lines, "[3/5] installing as the driver pack mac-facetimehd on the data volume (linked at every start) and loading the camera driver module (facetimehd, part of the Mac image)".into());
    let p = facetime::pack_with_setfiles(&built.firmware, &built.sets);
    let rep = env.install(&version_for(env.now()), "Apple's macOS 10.12.6 update, downloaded from Apple's update server", &p)?;
    for l in &rep.lines {
        note(env, lines, format!("  {l}"));
    }
    let no_driver = rep.lines.iter().any(|l| l.contains("did not load") && l.contains("facetimehd"));
    // success: the pieces have done their job (after a failure they stay, for a retry)
    let _ = std::fs::remove_dir_all(&dl);

    note(env, lines, "[4/5] looking for the camera device".into());
    let mut seen = env.look();
    for _ in 0..20 {
        if !seen.devices.is_empty() || no_driver {
            break;
        }
        env.sleep(Duration::from_secs(1));
        seen = env.look();
    }
    note(env, lines, "[5/5] reading the kernel log".into());
    let kernel = env.kernel_lines();
    let mut detail: Vec<String> = Vec::new();
    let headline = if !seen.devices.is_empty() {
        format!("The camera appeared as {}: open the CAMERA tile", seen.devices.join(", "))
    } else if no_driver {
        detail.push("This IYAKKI image does not contain the camera driver (facetimehd): the firmware is installed and waits for an image that has it. Update the image (Settings) and restart.".into());
        "The camera firmware is installed, but this image has no camera driver".to_string()
    } else {
        detail.push("No camera device yet: restart IYAKKI once (the firmware is linked in at every start). If it is still missing, send the kernel lines below: they say what the driver reported.".into());
        "The camera firmware is installed, but no camera device has appeared yet".to_string()
    };
    if built.sets.is_empty() {
        detail.push("No sensor calibration files were made: the picture may be off in colour and shading.".into());
    }
    let o = Outcome { at: env.now(), installed: true, firmware_version: built.firmware.version.to_string(), sensor_files: built.sets.len(), seen, headline, lines: lines.iter().cloned().chain(detail).collect(), kernel };
    write_outcome(data, &o);
    env.say(&format!("done: {}", o.headline));
    Ok(o)
}

// ------------------------------------------------------------------------------------------------ the download

fn sha256_file(p: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("{}: {e}", p.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Fetch `part` of `url` into `dest`, resuming from the bytes on disk, and check its size and SHA-256. Only `https` addresses on
/// `policy`'s hosts are used (redirects included); a server that answers the whole file instead of the range is refused without
/// reading it (the image is 1.9 GB). `source_bytes` is the size the server must name for the whole file.
#[allow(clippy::too_many_arguments)]
pub fn fetch_range(st: &dyn iyakki_proxy::Streamer, policy: &crate::macrecovery::Policy, url: &str, source_bytes: u64, part: &facetime::Part, dest: &Path, retries: u32, say: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool) -> Result<(), String> {
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let want = part.len();
    let mut last_err = String::new();
    for attempt in 0..=retries {
        if attempt > 0 {
            std::thread::sleep(policy.retry_pause * attempt);
        }
        let mut have = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        if have > want {
            let _ = std::fs::remove_file(dest);
            have = 0;
        }
        if have < want {
            match fetch_once(st, policy, url, source_bytes, part, dest, have, say, cancel) {
                Ok(()) => {}
                Err(Retry::Again(e)) => {
                    last_err = e;
                    continue;
                }
                Err(Retry::Stop(e)) => return Err(e),
            }
        }
        let got = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        if got != want {
            last_err = format!("got {got} of {want} bytes");
            continue;
        }
        let sum = sha256_file(dest)?;
        if sum != part.sha256 {
            let _ = std::fs::remove_file(dest);
            return Err(format!("the piece {} has sha256 {sum}, not the expected {}. It was deleted. Apple may have changed the file; nothing was installed", part.name, part.sha256));
        }
        return Ok(());
    }
    Err(format!("the download of {} did not finish: {last_err}", part.name))
}

enum Retry {
    Again(String),
    Stop(String),
}

#[allow(clippy::too_many_arguments)]
fn fetch_once(st: &dyn iyakki_proxy::Streamer, policy: &crate::macrecovery::Policy, url: &str, source_bytes: u64, part: &facetime::Part, dest: &Path, have: u64, say: &mut dyn FnMut(&str), cancel: &dyn Fn() -> bool) -> Result<(), Retry> {
    let want = part.len();
    let start = part.from + have;
    let mut url = url.to_string();
    let mut head = None;
    for _ in 0..=policy.max_redirects {
        if !url.starts_with("https://") || !policy.allows(&url) {
            return Err(Retry::Stop(format!("{url} is not an Apple https address; refused")));
        }
        let h = vec![("User-Agent".to_string(), "IYAKKI".to_string()), ("Range".to_string(), format!("bytes={start}-{}", part.to)), ("Accept-Encoding".to_string(), "identity".to_string())];
        let r = st.open(&url, &h).map_err(Retry::Again)?;
        if matches!(r.status, 301 | 302 | 303 | 307 | 308) {
            let loc = r.header("location").ok_or_else(|| Retry::Stop("a redirect without a place to go".into()))?.to_string();
            url = if loc.starts_with('/') {
                let rest = url.split_once("://").map_or("", |x| x.1);
                format!("https://{}{loc}", rest.split('/').next().unwrap_or(""))
            } else {
                loc
            };
            continue;
        }
        head = Some(r);
        break;
    }
    let head = head.ok_or_else(|| Retry::Stop("too many redirects".into()))?;
    match head.status {
        206 => {
            let (from, to, total) = head.header("content-range").and_then(crate::macrecovery::parse_content_range).ok_or_else(|| Retry::Stop("the server answered a partial download without saying which part".into()))?;
            if from != start || to != part.to {
                return Err(Retry::Stop(format!("the server sent bytes {from}-{to}, not {start}-{}", part.to)));
            }
            if let Some(t) = total.filter(|t| *t != source_bytes) {
                return Err(Retry::Stop(format!("Apple's file is {t} bytes, not the {source_bytes} this was made for: Apple changed it; nothing was downloaded")));
            }
        }
        200 => return Err(Retry::Stop("the server ignored the byte range and would send the whole 1.9 GB image; refused".into())),
        416 => {
            let _ = std::fs::remove_file(dest);
            return Err(Retry::Again("the part on disk does not fit what the server has; starting over".into()));
        }
        403 | 404 => return Err(Retry::Stop(format!("Apple's server refused the download (HTTP {}); the file may have moved", head.status))),
        s => return Err(Retry::Again(format!("HTTP {s}"))),
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(dest).map_err(|e| Retry::Stop(format!("{}: {e}", dest.display())))?;
    let mut body = head.body.take(want - have);
    let mut buf = vec![0u8; 128 * 1024];
    let mut done = have;
    let mut since = 0u64;
    loop {
        if cancel() {
            let _ = file.flush();
            return Err(Retry::Stop("stopped".into()));
        }
        match body.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                file.write_all(&buf[..n]).map_err(|e| Retry::Stop(format!("{}: {e} (is the disk full?)", dest.display())))?;
                done += n as u64;
                since += n as u64;
                if since >= 2 << 20 {
                    say(&format!("  {}: {} of {} KB", part.name, done >> 10, want >> 10));
                    since = 0;
                }
            }
            Err(e) => {
                let _ = file.flush();
                return Err(Retry::Again(format!("the connection dropped after {done} bytes: {e}")));
            }
        }
    }
    let _ = file.flush();
    Ok(())
}

// ------------------------------------------------------------------------------------------------ unpacking

fn unxz(p: &Path) -> Result<Vec<u8>, String> {
    let raw = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut out = Vec::new();
    crate::xz::decompress(&raw, &mut out, MAX_DECODED).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(out)
}

/// Make the firmware and the sensor files from the three downloaded pieces in `dir`.
pub fn build_from_dir(dir: &Path) -> Result<Built, String> {
    let get = |n: &str| unxz(&dir.join(format!("{n}.xz")));
    let driver = facetime::assemble_driver(&get("driver-1")?, &get("driver-2")?)?;
    let firmware = facetime::extract(&driver)?;
    let sets = facetime::setfiles_from_chunk(&get("assistant")?)?;
    Ok(Built { firmware, sets })
}

// ------------------------------------------------------------------------------------------------ the real machine

/// Is the FaceTime HD camera among the PCI devices under a `/sys` folder?
pub fn camera_in(sys: &Path) -> bool {
    let rd = |d: &Path, f: &str| std::fs::read_to_string(d.join(f)).map(|s| s.trim().to_ascii_lowercase()).unwrap_or_default();
    std::fs::read_dir(sys.join("bus/pci/devices")).into_iter().flatten().flatten().any(|e| {
        let d = e.path();
        rd(&d, "vendor") == PCI_VENDOR && rd(&d, "device") == PCI_DEVICE
    })
}

/// The video devices of the FaceTime HD camera: `video4linux` entries whose name says FaceTime or whose parent is a Broadcom 14e4 device.
pub fn video_devices_in(sys: &Path, dev: &Path) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for e in std::fs::read_dir(sys.join("class/video4linux")).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !n.starts_with("video") {
            continue;
        }
        let d = e.path();
        let name = std::fs::read_to_string(d.join("name")).unwrap_or_default().to_ascii_lowercase();
        let vendor = std::fs::read_to_string(d.join("device/vendor")).unwrap_or_default().trim().to_ascii_lowercase();
        if name.contains("facetime") || vendor == PCI_VENDOR {
            v.push(dev.join(&n).display().to_string());
        }
    }
    v.sort();
    v
}

/// The lines of the kernel log about the camera driver.
pub fn camera_lines(log: &str) -> Vec<String> {
    let keys = ["facetimehd", "fthd", "FaceTime", "Facetime", "firmware load", "videobuf2"];
    let v: Vec<String> = log.lines().filter(|l| keys.iter().any(|k| l.contains(k))).map(|l| l.trim().to_string()).collect();
    v.iter().skip(v.len().saturating_sub(40)).cloned().collect()
}

pub struct RealEnv {
    pub data: PathBuf,
    pub streamer: std::sync::Arc<dyn iyakki_proxy::Streamer>,
    pub sys: PathBuf,
}

impl RealEnv {
    pub fn new(data: &Path, streamer: std::sync::Arc<dyn iyakki_proxy::Streamer>) -> RealEnv {
        RealEnv { data: data.to_path_buf(), streamer, sys: "/sys".into() }
    }
}

impl Env for RealEnv {
    fn say(&mut self, line: &str) {
        let mut o = std::io::stdout();
        let _ = writeln!(o, "{line}");
        let _ = o.flush();
    }
    fn camera_present(&self) -> bool {
        camera_in(&self.sys)
    }
    fn free_bytes(&self, dir: &Path) -> Option<u64> {
        crate::k3sdev::space(dir).map(|s| s.0)
    }
    fn data_in_memory(&self) -> bool {
        crate::macfw::in_memory(&std::fs::read_to_string("/proc/mounts").unwrap_or_default(), &self.data)
    }
    fn fetch_part(&mut self, part: &facetime::Part, dest: &Path) -> Result<(), String> {
        let st = self.streamer.clone();
        let policy = crate::macrecovery::Policy { hosts: facetime::SOURCE_HOSTS.iter().map(|s| s.to_string()).collect(), ..Default::default() };
        fetch_range(&*st, &policy, facetime::SOURCE_URL, facetime::SOURCE_BYTES, part, dest, 5, &mut |l| self.say(l), &|| false)
    }
    fn build(&mut self, dir: &Path) -> Result<Built, String> {
        build_from_dir(dir)
    }
    fn install(&mut self, version: &str, source: &str, p: &pack::Pack) -> Result<apply::Report, String> {
        let (fs, real) = apply::Fs::for_data(&self.data);
        let run: Box<dyn iyakki_drivers::run::Runner> = if real { Box::new(iyakki_drivers::run::System) } else { Box::new(iyakki_drivers::run::Dry) };
        apply::install_pack(&fs, run.as_ref(), PACK_ID, version, "LicenseRef-Apple-firmware-supplied-by-the-user", source, "local", p)
    }
    fn look(&mut self) -> Seen {
        Seen { devices: video_devices_in(&self.sys, Path::new("/dev")) }
    }
    fn kernel_lines(&mut self) -> Vec<String> {
        let r = crate::wsexec::run_argv(&self.data, &["dmesg".to_string()], &[], Duration::from_secs(10), 2 << 20);
        camera_lines(&r.stdout)
    }
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
    fn now(&self) -> i64 {
        crate::clock::now_unix()
    }
}

#[cfg(test)]
#[path = "camfw_tests.rs"]
mod tests;
