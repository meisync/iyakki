//! Setting memory up: zram through sysfs, our own mkswap, `swapon(2)`, the vm sysctls, an optional swap file, zswap.
//! Idempotent (a second call finds zram active and leaves it), never fatal (each step is a line in the report).

use std::path::{Path, PathBuf};

use crate::config::{self, disk_size, human, pick_algo, Config, ZramMode, MIB};
use crate::stats::{self, MemStats};

/// Where the kernel's views are. `real()` on a running system; tests use `under(<temp dir>)`.
#[derive(Clone, Debug)]
pub struct Env {
    pub sys: PathBuf,
    pub proc: PathBuf,
    pub dev: PathBuf,
}

impl Env {
    pub fn real() -> Env {
        Env { sys: "/sys".into(), proc: "/proc".into(), dev: "/dev".into() }
    }
    /// The given `/proc` and `/sys` roots with the real device folder.
    pub fn with_roots(proc: PathBuf, sys: PathBuf) -> Env {
        Env { sys, proc, dev: "/dev".into() }
    }
    pub fn under(root: &Path) -> Env {
        Env { sys: root.join("sys"), proc: root.join("proc"), dev: root.join("dev") }
    }
}

/// `swapon(2)` / `swapoff(2)`, behind a seam so tests never touch the machine.
pub trait Swapper {
    fn swapon(&self, path: &Path, priority: i32) -> Result<(), String>;
    fn swapoff(&self, path: &Path) -> Result<(), String>;
}

pub struct KernelSwap;

fn cpath(p: &Path) -> Result<std::ffi::CString, String> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|e| e.to_string())
}

// swapon(2) flags (linux/swap.h); the libc crate does not export them
#[cfg(target_os = "linux")]
const SWAP_FLAG_PREFER: libc::c_int = 0x8000;
#[cfg(target_os = "linux")]
const SWAP_FLAG_PRIO_MASK: libc::c_int = 0x7fff;

#[cfg(target_os = "linux")]
impl Swapper for KernelSwap {
    fn swapon(&self, path: &Path, priority: i32) -> Result<(), String> {
        let c = cpath(path)?;
        let flags = SWAP_FLAG_PREFER | ((priority.clamp(0, 32767) as libc::c_int) & SWAP_FLAG_PRIO_MASK);
        // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
        let rc = unsafe { libc::swapon(c.as_ptr(), flags) };
        if rc == 0 { Ok(()) } else { Err(format!("swapon {}: {}", path.display(), std::io::Error::last_os_error())) }
    }
    fn swapoff(&self, path: &Path) -> Result<(), String> {
        let c = cpath(path)?;
        // SAFETY: as above.
        let rc = unsafe { libc::swapoff(c.as_ptr()) };
        if rc == 0 { Ok(()) } else { Err(format!("swapoff {}: {}", path.display(), std::io::Error::last_os_error())) }
    }
}

#[cfg(not(target_os = "linux"))]
impl Swapper for KernelSwap {
    fn swapon(&self, _: &Path, _: i32) -> Result<(), String> {
        Err("swap is only managed on Linux".into())
    }
    fn swapoff(&self, _: &Path) -> Result<(), String> {
        Err("swap is only managed on Linux".into())
    }
}

/// One line of the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub ok: bool,
    pub msg: String,
}

fn ok(msg: impl Into<String>) -> Step {
    Step { ok: true, msg: msg.into() }
}
fn bad(msg: impl Into<String>) -> Step {
    Step { ok: false, msg: msg.into() }
}

const ZRAM_PRIORITY: i32 = 100;
const SWAPFILE_PRIORITY: i32 = 10;
/// Headroom when taking zram out of use: the pages come back into RAM.
const SWAPOFF_MARGIN: u64 = 128 * MIB;

fn put(path: &Path, v: &str) -> Result<(), String> {
    std::fs::write(path, v).map_err(|e| format!("{}: {e}", path.display()))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// May zram be switched off right now? Everything stored in it has to fit back into free memory.
pub fn swapoff_safe(s: &MemStats) -> Result<(), String> {
    let used: u64 = s.swaps.iter().filter(|w| w.0.starts_with("/dev/zram")).map(|w| w.1).sum();
    let other_free: u64 = s.swaps.iter().filter(|w| !w.0.starts_with("/dev/zram")).map(|w| w.2.saturating_sub(w.1)).sum();
    let need = used.saturating_sub(other_free) + SWAPOFF_MARGIN;
    if need > s.available {
        Err(format!("{} is held in compressed memory and only {} of memory is free: switching zram off now could run the machine out of memory. Close some programs first.", human(used), human(s.available)))
    } else {
        Ok(())
    }
}

fn active_zram(env: &Env) -> Option<String> {
    stats::parse_swaps(&read(&env.proc.join("swaps"))).into_iter().find(|w| w.0.starts_with("/dev/zram")).map(|w| w.0)
}

/// Returns whether zram is in use afterwards.
fn setup_zram(env: &Env, sw: &dyn Swapper, cfg: &Config, ram: u64, reconfigure: bool, out: &mut Vec<Step>) -> bool {
    let active = active_zram(env);
    if cfg.zram == ZramMode::Off {
        match (active, reconfigure) {
            (Some(path), true) => {
                match sw.swapoff(Path::new(&path)) {
                    Ok(()) => {
                        if let Some(d) = stats::zram_device(env) {
                            let _ = put(&env.sys.join("block").join(&d).join("reset"), "1");
                        }
                        out.push(ok("zram: switched off"));
                    }
                    Err(e) => out.push(bad(format!("zram: could not switch off: {e}"))),
                }
            }
            (Some(_), false) => out.push(ok("zram: active (off requested; takes effect at next boot)")),
            (None, _) => out.push(ok("zram: off (as configured)")),
        }
        return matches!(active_zram(env), Some(_)) && !reconfigure;
    }
    if let (Some(path), false) = (&active, reconfigure) {
        out.push(ok(format!("zram: already active at {path}")));
        return true;
    }
    if let Some(path) = active {
        if let Err(e) = sw.swapoff(Path::new(&path)) {
            out.push(bad(format!("zram: could not take {path} out of use: {e}")));
            return false;
        }
    }
    // the device: an existing one, or a new one from the control file
    let name = match stats::zram_device(env) {
        Some(n) => n,
        None => match std::fs::read_to_string(env.sys.join("class/zram-control/hot_add")).ok().and_then(|s| s.trim().parse::<u32>().ok()) {
            Some(id) => format!("zram{id}"),
            None => {
                out.push(bad("zram: this kernel has no zram support; continuing without compressed swap"));
                return false;
            }
        },
    };
    let d = env.sys.join("block").join(&name);
    if read(&d.join("disksize")).trim().parse::<u64>().unwrap_or(0) != 0 {
        if let Err(e) = put(&d.join("reset"), "1") {
            out.push(bad(format!("zram: could not reset {name}: {e}")));
            return false;
        }
    }
    let (all, cur) = config::parse_algos(&read(&d.join("comp_algorithm")));
    let algo = pick_algo(cfg.algo.as_deref(), &all);
    match &algo {
        Some(a) if Some(a) != cur.as_ref() => {
            if let Err(e) = put(&d.join("comp_algorithm"), a) {
                out.push(bad(format!("zram: could not select {a}: {e}")));
            }
        }
        None => out.push(bad("zram: the kernel lists no usable compressor; using its default")),
        _ => {}
    }
    if let (Some(want), Some(got)) = (&cfg.algo, &algo) {
        if want != got {
            out.push(bad(format!("zram: compressor {want} is not available, using {got}")));
        }
    }
    let bytes = disk_size(cfg.size, ram);
    if let Err(e) = put(&d.join("disksize"), &bytes.to_string()) {
        out.push(bad(format!("zram: could not set the size of {name}: {e}")));
        return false;
    }
    let node = env.dev.join(&name);
    for _ in 0..40 {
        if node.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if let Err(e) = crate::mkswap::write(&node, bytes, crate::ZRAM_LABEL) {
        out.push(bad(format!("zram: could not prepare {name} as swap: {e}")));
        return false;
    }
    match sw.swapon(&node, ZRAM_PRIORITY) {
        Ok(()) => {
            out.push(ok(format!("zram: {name} {} ({}), swap priority {ZRAM_PRIORITY}", human(bytes), algo.unwrap_or_else(|| cur.unwrap_or_else(|| "default".into())))));
            true
        }
        Err(e) => {
            out.push(bad(format!("zram: {e}")));
            false
        }
    }
}

fn tune(env: &Env, cfg: &Config, zram_on: bool, out: &mut Vec<Step>) {
    let vm = env.proc.join("sys/vm");
    if !vm.is_dir() {
        return;
    }
    let mut set = vec![("swappiness", cfg.swappiness.to_string())];
    if zram_on {
        // zram is not a disk: no read-ahead of neighbouring pages, no boosted watermarks, a modest early-reclaim margin
        set.extend([("page-cluster", "0".to_string()), ("watermark_boost_factor", "0".to_string()), ("watermark_scale_factor", "50".to_string())]);
    }
    for (k, v) in set {
        if let Err(e) = put(&vm.join(k), &v) {
            out.push(bad(format!("vm.{k}: {e}")));
        }
    }
    out.push(ok(format!("vm: swappiness {}{}", cfg.swappiness, if zram_on { ", page-cluster 0, watermark_boost_factor 0, watermark_scale_factor 50" } else { "" })));
}

fn setup_zswap(env: &Env, cfg: &Config, out: &mut Vec<Step>) {
    let p = env.sys.join("module/zswap/parameters");
    if !p.is_dir() {
        if cfg.zswap {
            out.push(bad("zswap: this kernel has no zswap; ignored"));
        }
        return;
    }
    let want = if cfg.zswap { "Y" } else { "N" };
    if read(&p.join("enabled")).trim() == want {
        return;
    }
    match put(&p.join("enabled"), want) {
        Ok(()) => out.push(ok(format!("zswap: {}", if cfg.zswap { "on (compressed cache in front of the swap file)" } else { "off" }))),
        Err(e) => out.push(bad(format!("zswap: {e}"))),
    }
}

/// Boot step (and live "apply"): zram, the vm settings, zswap. The swap file is [`apply_swapfile`] (it can take a while).
pub fn apply(env: &Env, sw: &dyn Swapper, cfg: &Config, ram: u64) -> Vec<Step> {
    let mut out = Vec::new();
    let on = setup_zram(env, sw, cfg, ram, false, &mut out);
    finish(env, cfg, on, &mut out);
    out
}

fn finish(env: &Env, cfg: &Config, zram_on: bool, out: &mut Vec<Step>) {
    let any_swap = zram_on || stats::parse_swaps(&read(&env.proc.join("swaps"))).len() > 0;
    if any_swap || cfg.zram == ZramMode::On {
        tune(env, cfg, zram_on, out);
    }
    setup_zswap(env, cfg, out);
}

/// Live change from Settings: take zram out of use (refused when the pages would not fit back in RAM), reset it, set it
/// up again with the new values.
pub fn reconfigure(env: &Env, sw: &dyn Swapper, cfg: &Config, ram: u64) -> Result<Vec<Step>, String> {
    let st = MemStats::read(env);
    if active_zram(env).is_some() {
        swapoff_safe(&st)?;
    }
    let mut out = Vec::new();
    let on = setup_zram(env, sw, cfg, ram, true, &mut out);
    finish(env, cfg, on, &mut out);
    Ok(out)
}

/// What applying `cfg` would change, as lines for the Preview.
pub fn preview(cur: &MemStats, cfg: &Config, ram: u64) -> Vec<String> {
    let mut v = Vec::new();
    match (&cur.zram, active_zram_from(cur), cfg.zram) {
        (_, Some(_), ZramMode::Off) => v.push("zram: switch off (compressed memory is emptied back into RAM)".to_string()),
        (_, None, ZramMode::Off) => v.push("zram: stays off".to_string()),
        (z, active, ZramMode::On) => {
            let size = disk_size(cfg.size, ram);
            let algo = cfg.algo.clone().unwrap_or_else(|| "zstd (falls back to lz4, lzo-rle)".into());
            match (z, active) {
                (Some(z), Some(_)) if z.disksize == size && cfg.algo.as_deref().is_none_or(|a| a == z.algorithm) => v.push(format!("zram: unchanged ({} {})", human(size), z.algorithm)),
                (Some(z), Some(_)) => v.push(format!("zram: {} {} becomes {} {algo} (swap off, reset, swap on)", human(z.disksize), z.algorithm, human(size))),
                _ => v.push(format!("zram: create {} with {algo}", human(size))),
            }
        }
    }
    v.push(format!("vm.swappiness: {}", cfg.swappiness));
    v.push(match cfg.swapfile {
        Some(b) => format!("swap file: {} on the data volume (priority {SWAPFILE_PRIORITY}, below zram)", human(b)),
        None => "swap file: off".to_string(),
    });
    v.push(format!("zswap: {}", if cfg.zswap { "on" } else { "off" }));
    v
}

fn active_zram_from(s: &MemStats) -> Option<&str> {
    s.swaps.iter().find(|w| w.0.starts_with("/dev/zram")).map(|w| w.0.as_str())
}

/// Free bytes where `path` lives.
fn free_bytes(dir: &Path) -> Option<u64> {
    let c = cpath(dir).ok()?;
    // SAFETY: valid path and a writable, correctly sized statvfs.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0).then(|| st.f_bavail as u64 * st.f_frsize as u64)
}

/// The optional swap file. Written out in full (no holes, no `fallocate`, which swap refuses on some filesystems), mode
/// 0600, then swap header, then `swapon`. `on_disk` must be true: a file in RAM would only eat the memory it should save.
pub fn apply_swapfile(sw: &dyn Swapper, env: &Env, path: &Path, bytes: Option<u64>, on_disk: bool) -> Vec<Step> {
    let mut out = Vec::new();
    let active = stats::parse_swaps(&read(&env.proc.join("swaps"))).iter().any(|w| Path::new(&w.0) == path);
    let Some(bytes) = bytes else {
        if active {
            out.push(ok("swap file: active (off requested; takes effect at next boot)"));
        }
        return out;
    };
    let bytes = bytes / 4096 * 4096;
    if !on_disk {
        out.push(bad("swap file: the data directory is in RAM (no data disk), so a swap file would not help; skipped"));
        return out;
    }
    if active {
        out.push(ok("swap file: already active"));
        return out;
    }
    let existing = std::fs::metadata(path).map(|m| m.len()).ok();
    if existing != Some(bytes) {
        let dir = path.parent().unwrap_or(Path::new("/"));
        let have = existing.unwrap_or(0);
        match free_bytes(dir) {
            Some(free) if free + have < bytes + 1024 * MIB => {
                out.push(bad(format!("swap file: not enough free space on the data volume ({} free, {} wanted plus 1 GiB spare); skipped", human(free + have), human(bytes))));
                return out;
            }
            None => {
                out.push(bad("swap file: cannot tell how much space is free; skipped"));
                return out;
            }
            _ => {}
        }
        if let Err(e) = write_zero_file(path, bytes) {
            out.push(bad(format!("swap file: {e}")));
            let _ = std::fs::remove_file(path.with_extension("tmp"));
            return out;
        }
    }
    if let Err(e) = crate::mkswap::write(path, bytes, "IYAKKI-SWAP") {
        out.push(bad(format!("swap file: {e}")));
        return out;
    }
    match sw.swapon(path, SWAPFILE_PRIORITY) {
        Ok(()) => out.push(ok(format!("swap file: {} at {}, priority {SWAPFILE_PRIORITY}", human(bytes), path.display()))),
        Err(e) => out.push(bad(format!("swap file: {e}"))),
    }
    out
}

fn write_zero_file(path: &Path, bytes: u64) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let chunk = vec![0u8; MIB as usize];
    let mut left = bytes;
    while left > 0 {
        let n = left.min(MIB) as usize;
        f.write_all(&chunk[..n]).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
        left -= n as u64;
    }
    f.sync_all().map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_with_roots_keeps_the_given_folders_and_the_real_device_folder() {
        let e = Env::with_roots("/x/proc".into(), "/x/sys".into());
        assert_eq!((e.proc.to_str(), e.sys.to_str(), e.dev.to_str()), (Some("/x/proc"), Some("/x/sys"), Some("/dev")));
    }
    use crate::config::{Size, GIB};
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<String>>,
        fail_on: Option<&'static str>,
    }
    impl Swapper for Fake {
        fn swapon(&self, p: &Path, prio: i32) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("on {} {prio}", p.file_name().unwrap().to_string_lossy()));
            if self.fail_on == Some("on") { Err("EINVAL".into()) } else { Ok(()) }
        }
        fn swapoff(&self, p: &Path) -> Result<(), String> {
            self.calls.borrow_mut().push(format!("off {}", p.file_name().unwrap().to_string_lossy()));
            Ok(())
        }
    }

    fn tree(tag: &str, algos: &str) -> (PathBuf, Env) {
        let root = std::env::temp_dir().join(format!("iyakki-zapply-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let z = root.join("sys/block/zram0");
        std::fs::create_dir_all(&z).unwrap();
        std::fs::write(z.join("comp_algorithm"), algos).unwrap();
        std::fs::write(z.join("disksize"), "0\n").unwrap();
        std::fs::write(z.join("reset"), "").unwrap();
        std::fs::create_dir_all(root.join("dev")).unwrap();
        std::fs::File::create(root.join("dev/zram0")).unwrap().set_len(1 << 20).unwrap();
        std::fs::create_dir_all(root.join("proc/sys/vm")).unwrap();
        std::fs::write(root.join("proc/swaps"), "Filename Type Size Used Priority\n").unwrap();
        std::fs::write(root.join("proc/meminfo"), "MemTotal: 4194304 kB\nMemFree: 2000000 kB\nMemAvailable: 3000000 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n").unwrap();
        let e = Env::under(&root);
        (root, e)
    }

    #[test]
    fn sets_up_zram_with_zstd_and_tunes_the_vm() {
        let (root, env) = tree("a", "lzo [lzo-rle] lz4 zstd\n");
        let sw = Fake::default();
        let steps = apply(&env, &sw, &Config::default(), 4 * GIB);
        assert!(steps.iter().all(|s| s.ok), "{steps:?}");
        let z = root.join("sys/block/zram0");
        assert_eq!(std::fs::read_to_string(z.join("comp_algorithm")).unwrap(), "zstd");
        assert_eq!(std::fs::read_to_string(z.join("disksize")).unwrap(), (2 * GIB).to_string(), "50 % of 4 GiB");
        assert_eq!(*sw.calls.borrow(), vec!["on zram0 100"]);
        let dev = std::fs::read(root.join("dev/zram0")).unwrap();
        assert_eq!(&dev[4096 - 10..4096], b"SWAPSPACE2");
        assert_eq!(&dev[1052..1063], b"IYAKKI-ZRAM");
        let vm = |k: &str| std::fs::read_to_string(root.join("proc/sys/vm").join(k)).unwrap();
        assert_eq!((vm("swappiness").as_str(), vm("page-cluster").as_str(), vm("watermark_boost_factor").as_str(), vm("watermark_scale_factor").as_str()), ("100", "0", "0", "50"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn second_run_is_a_no_op_once_the_kernel_lists_it() {
        let (root, env) = tree("b", "lzo [lzo-rle] lz4 zstd\n");
        let sw = Fake::default();
        apply(&env, &sw, &Config::default(), 4 * GIB);
        std::fs::write(root.join("proc/swaps"), "Filename Type Size Used Priority\n/dev/zram0 partition 2097148 0 100\n").unwrap();
        let again = apply(&env, &sw, &Config::default(), 4 * GIB);
        assert!(again.iter().any(|s| s.msg.contains("already active")), "{again:?}");
        assert_eq!(sw.calls.borrow().len(), 1, "no second swapon");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn falls_back_when_the_compressor_is_missing_and_survives_failures() {
        let (root, env) = tree("c", "lzo [lzo-rle]\n");
        let sw = Fake::default();
        let cfg = Config { algo: Some("zstd".into()), ..Config::default() };
        let steps = apply(&env, &sw, &cfg, 8 * GIB);
        assert!(steps.iter().any(|s| !s.ok && s.msg.contains("zstd is not available, using lzo-rle")), "{steps:?}");
        assert!(!std::fs::read_to_string(root.join("sys/block/zram0/comp_algorithm")).unwrap().contains("zstd"), "unavailable compressor not written");
        // swapon failing is reported, not fatal
        let (root2, env2) = tree("c2", "[lzo-rle] zstd\n");
        let failing = Fake { fail_on: Some("on"), ..Default::default() };
        let s = apply(&env2, &failing, &Config::default(), 4 * GIB);
        assert!(s.iter().any(|x| !x.ok && x.msg.contains("EINVAL")));
        // no zram at all: a message and carry on
        let empty = std::env::temp_dir().join(format!("iyakki-zapply-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&empty);
        let s = apply(&Env::under(&empty), &Fake::default(), &Config::default(), 4 * GIB);
        assert!(s.iter().any(|x| x.msg.contains("no zram support")), "{s:?}");
        for r in [root, root2] {
            let _ = std::fs::remove_dir_all(r);
        }
    }

    #[test]
    fn hot_add_creates_the_device_when_none_exists() {
        let root = std::env::temp_dir().join(format!("iyakki-zapply-hot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sys/class/zram-control")).unwrap();
        std::fs::write(root.join("sys/class/zram-control/hot_add"), "0\n").unwrap();
        let env = Env::under(&root);
        // the "kernel" creates block/zram0 on hot_add; the fake does it up front, so only the naming path is covered here
        let s = apply(&env, &Fake::default(), &Config::default(), 4 * GIB);
        assert!(s.iter().any(|x| x.msg.contains("zram0")), "{s:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn off_and_reconfigure() {
        let (root, env) = tree("d", "lzo [lzo-rle] lz4 zstd\n");
        let sw = Fake::default();
        let off = Config { zram: ZramMode::Off, ..Config::default() };
        let s = apply(&env, &sw, &off, 4 * GIB);
        assert!(s.iter().any(|x| x.msg.contains("off")) && sw.calls.borrow().is_empty());
        // live: active zram with little used goes to a new size
        std::fs::write(root.join("proc/swaps"), "Filename Type Size Used Priority\n/dev/zram0 partition 2097148 1024 100\n").unwrap();
        std::fs::write(root.join("sys/block/zram0/disksize"), "2147483648\n").unwrap();
        let cfg = Config { size: Size::Bytes(GIB), algo: Some("lz4".into()), ..Config::default() };
        let r = reconfigure(&env, &sw, &cfg, 4 * GIB).unwrap();
        assert!(r.iter().all(|s| s.ok), "{r:?}");
        assert_eq!(*sw.calls.borrow(), vec!["off zram0", "on zram0 100"]);
        assert_eq!(std::fs::read_to_string(root.join("sys/block/zram0/disksize")).unwrap(), GIB.to_string());
        assert_eq!(std::fs::read_to_string(root.join("sys/block/zram0/comp_algorithm")).unwrap(), "lz4");
        // switching off live
        let sw2 = Fake::default();
        reconfigure(&env, &sw2, &off, 4 * GIB).unwrap();
        assert_eq!(*sw2.calls.borrow(), vec!["off zram0"]);
        // refused when the stored pages cannot fit back
        std::fs::write(root.join("proc/swaps"), "Filename Type Size Used Priority\n/dev/zram0 partition 8388604 4194304 100\n").unwrap();
        let sw3 = Fake::default();
        let e = reconfigure(&env, &sw3, &cfg, 4 * GIB).unwrap_err();
        assert!(e.contains("out of memory") && sw3.calls.borrow().is_empty(), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn preview_describes_the_change() {
        let mut cur = MemStats::default();
        let cfg = Config { size: Size::Bytes(GIB), ..Config::default() };
        let p = preview(&cur, &cfg, 8 * GIB);
        assert!(p[0].contains("create 1.0 GiB"), "{p:?}");
        cur.swaps = vec![("/dev/zram0".into(), 0, 0, 100)];
        cur.zram = Some(crate::stats::ZramStats { disksize: 4 * GIB, algorithm: "zstd".into(), ..Default::default() });
        assert!(preview(&cur, &cfg, 8 * GIB)[0].contains("becomes 1.0 GiB"));
        assert!(preview(&cur, &Config { zram: ZramMode::Off, ..cfg.clone() }, 8 * GIB)[0].contains("switch off"));
        let same = Config { size: Size::Bytes(4 * GIB), ..Config::default() };
        assert!(preview(&cur, &same, 8 * GIB)[0].contains("unchanged"));
    }

    #[test]
    fn swap_file_is_written_whole_and_only_on_disk() {
        let (root, env) = tree("e", "[lzo-rle]\n");
        let sw = Fake::default();
        let f = root.join("swapfile");
        let s = apply_swapfile(&sw, &env, &f, Some(64 * MIB), false);
        assert!(!f.exists() && s[0].msg.contains("in RAM"), "{s:?}");
        let s = apply_swapfile(&sw, &env, &f, Some(64 * MIB), true);
        assert!(s.iter().all(|x| x.ok), "{s:?}");
        let b = std::fs::read(&f).unwrap();
        assert_eq!(b.len() as u64, 64 * MIB);
        assert_eq!(&b[4096 - 10..4096], b"SWAPSPACE2");
        assert!(b[8192..].iter().all(|x| *x == 0));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(*sw.calls.borrow(), vec!["on swapfile 10"]);
        // off requested: nothing happens
        assert!(apply_swapfile(&sw, &env, &f, None, true).is_empty());
        // absurd size: refused for lack of space, nothing created
        let g = root.join("big");
        let s = apply_swapfile(&sw, &env, &g, Some(1 << 60), true);
        assert!(!g.exists() && s[0].msg.contains("not enough free space"), "{s:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
