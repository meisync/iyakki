//! `LocalOs`: the core API on this machine (docs/development/UI-OS-SPLIT-PLAN.md step 5, F-ARCH-05). It implements [`Sysinfo`] over `/proc` and
//! `/sys/class/dmi`, with the roots as fields so a test points it at a made-up tree. Reads only; nothing here writes, starts a process or opens a
//! device. The pure parsers live in `iyakki-core-api`; this crate is the one place that opens the files (step 7 moves the rest of the kernel access
//! of `iyakki-agent` and `iyakki-shell` here).

mod hardware;
pub mod appevents;
pub mod bootdisk;
pub mod homebrowse;
pub mod homefiles;
pub mod i2c;
pub mod resume;
pub mod cloudseed;
pub mod gadget;
pub mod bootconfirm;
pub mod net;
pub mod uistate;

/// The control of this machine (links, addresses, volume, DHCP).
pub fn system_control() -> std::sync::Arc<dyn iyakki_core_api::hardware::Control> {
    std::sync::Arc::new(iyakki_devctl::System)
}
mod memory;
pub mod sysinfo;
pub use hardware::LocalHardware;
pub use memory::LocalMemory;

use std::path::{Path, PathBuf};

use iyakki_core_api::{parse_loadavg, parse_meminfo, parse_mounts, parse_net_dev, parse_partitions, parse_uptime, ApiError, ApiResult, Audio, BlockDev, Disks, LoadAvg, MemInfo, Mount, Net, NetCounters, Os, PlaybackVolume, SoundCard, Sysinfo, Uptime};

/// Runs the mixer program with these arguments (its output, `None` when it failed). The real one starts `amixer`; a test gives its own.
pub type Mixer = Box<dyn Fn(&[&str]) -> Option<String> + Send + Sync>;

pub struct LocalOs {
    proc: PathBuf,
    sys: PathBuf,
    mixer: Mixer,
    mutes: iyakki_devctl::Mutes,
    memory: std::sync::Arc<LocalMemory>,
    hardware: std::sync::Arc<LocalHardware>,
}

fn amixer(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("amixer").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// The control of a made-up machine: it changes nothing.
struct NoControl;

impl iyakki_devctl::Control for NoControl {
    fn set_link(&self, _: &str, _: bool) -> Result<(), String> {
        Err("this is a made-up machine".into())
    }
    fn ipv4(&self) -> Vec<(String, String)> {
        vec![]
    }
    fn volume(&self, _: u32) -> Option<iyakki_devctl::Volume> {
        None
    }
    fn change_volume(&self, _: u32, _: Option<i32>) -> Result<iyakki_devctl::Volume, String> {
        Err("this is a made-up machine".into())
    }
    fn set_volume(&self, _: u32, _: u32) -> Result<iyakki_devctl::Volume, String> {
        Err("this is a made-up machine".into())
    }
    fn dhcp(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

impl LocalOs {
    /// This machine.
    pub fn system() -> LocalOs {
        LocalOs { proc: PathBuf::from("/proc"), sys: PathBuf::from("/sys"), mixer: Box::new(amixer), mutes: Default::default(), memory: std::sync::Arc::new(LocalMemory::system()), hardware: std::sync::Arc::new(LocalHardware::system()) }
    }

    /// Other places for `/proc` and `/sys` (a test tree, or a chroot that keeps them elsewhere).
    pub fn with_roots(proc: PathBuf, sys: PathBuf) -> LocalOs {
        let memory = std::sync::Arc::new(LocalMemory::with_roots(proc.clone(), sys.clone()));
        let hardware = std::sync::Arc::new(LocalHardware::new(iyakki_devctl::Roots { sys: sys.clone(), proc_: proc.clone() }, std::sync::Arc::new(NoControl)));
        LocalOs { proc, sys, mixer: Box::new(|_| None), mutes: Default::default(), memory, hardware }
    }

    /// A made-up machine under `root` (`root/proc`, `root/sys`).
    pub fn at(root: &Path) -> LocalOs {
        LocalOs { proc: root.join("proc"), sys: root.join("sys"), mixer: Box::new(|_| None), mutes: Default::default(), memory: std::sync::Arc::new(LocalMemory::under(root)), hardware: std::sync::Arc::new(LocalHardware::new(iyakki_devctl::Roots { sys: root.join("sys"), proc_: root.join("proc") }, std::sync::Arc::new(NoControl))) }
    }

    /// The same, with the mixer program replaced (tests).
    pub fn with_mixer(mut self, m: Mixer) -> LocalOs {
        self.mixer = m;
        self
    }

    fn read(&self, base: &Path, rel: &str) -> ApiResult<String> {
        std::fs::read_to_string(base.join(rel)).map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { ApiError::Unsupported(format!("{rel} is not there on this machine")) } else { ApiError::Io(format!("{rel}: {e}")) })
    }
}

impl Os for LocalOs {
    fn sysinfo(&self) -> &dyn Sysinfo {
        self
    }
    fn audio(&self) -> &dyn Audio {
        self
    }
    fn disks(&self) -> &dyn Disks {
        self
    }
    fn net(&self) -> &dyn Net {
        self
    }
    fn memory(&self) -> std::sync::Arc<dyn iyakki_core_api::memory::Memory> {
        self.memory.clone()
    }
    fn hardware(&self) -> std::sync::Arc<dyn iyakki_core_api::hardware::Hardware> {
        self.hardware.clone()
    }
    fn crashlog(&self) -> std::sync::Arc<dyn iyakki_core_api::crashlog::CrashLog> {
        std::sync::Arc::new(LocalCrashLog)
    }
    fn netscan(&self) -> std::sync::Arc<dyn iyakki_core_api::netscan::NetScan> {
        std::sync::Arc::new(LocalNetScan)
    }
    fn router(&self) -> std::sync::Arc<dyn iyakki_core_api::router::Router> {
        std::sync::Arc::new(LocalRouter)
    }
    fn fleet(&self) -> std::sync::Arc<dyn iyakki_core_api::fleet::Fleet> {
        std::sync::Arc::new(LocalFleet)
    }
}

/// The scan of the local network, run on a thread of its own.
pub struct LocalNetScan;

impl iyakki_core_api::netscan::NetScan for LocalNetScan {
    fn subnets(&self) -> Vec<iyakki_core_api::netscan::net::Subnet> {
        iyakki_core_api::netscan::net::local_subnets()
    }
    fn start(&self, targets: Vec<std::net::Ipv4Addr>, opts: iyakki_core_api::netscan::Options, cancel: std::sync::Arc<std::sync::atomic::AtomicBool>, tx: std::sync::mpsc::Sender<iyakki_core_api::netscan::Event>) -> Result<(), String> {
        std::thread::Builder::new().name("iyakki-netscan".into()).spawn(move || iyakki_netscan::scan(targets, opts, cancel, tx)).map(|_| ()).map_err(|e| format!("could not start the scan: {e}"))
    }
}

/// The kept kernel logs, read from the folder the caller names.
pub struct LocalCrashLog;

impl iyakki_core_api::crashlog::CrashLog for LocalCrashLog {
    fn read_unclean(&self, dir: &Path) -> Option<iyakki_core_api::crashlog::Unclean> {
        iyakki_storage::crashlog::read_unclean(dir)
    }
    fn latest_crash(&self, dir: &Path) -> Option<String> {
        iyakki_storage::crashlog::latest_crash(dir)
    }
    fn current_file(&self, dir: &Path) -> Option<String> {
        iyakki_storage::crashlog::current_file(dir)
    }
    fn has_file(&self, dir: &Path, name: &str) -> bool {
        !name.contains('/') && dir.join(name).exists()
    }
    fn last_lines(&self, dir: &Path, name: &str, n: usize) -> Vec<String> {
        if name.contains('/') || name.contains("..") {
            return Vec::new();
        }
        iyakki_storage::crashlog::last_lines(&dir.join(name), n)
    }
    fn mark_seen(&self, dir: &Path) {
        iyakki_storage::crashlog::mark_seen(dir)
    }
    fn copy_to_folder(&self, dir: &Path, name: &str, dest_dir: &Path, now_unix: i64) -> Result<std::path::PathBuf, String> {
        iyakki_storage::crashlog::copy_to_folder(dir, name, dest_dir, now_unix)
    }
}

impl Disks for LocalOs {
    fn space(&self, path: &str) -> ApiResult<(u64, u64)> {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(Path::new(path).as_os_str().as_bytes()).map_err(|_| ApiError::InvalidInput("the path has a NUL in it".into()))?;
        // SAFETY: `c` is a valid NUL-terminated path and `st` is a properly sized, writable statvfs.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
            return Err(ApiError::Io(format!("{path}: {}", std::io::Error::last_os_error())));
        }
        let bs = st.f_frsize as u64;
        Ok((st.f_bavail as u64 * bs, st.f_blocks as u64 * bs))
    }
    fn block_devices(&self) -> ApiResult<Vec<BlockDev>> {
        parse_partitions(&self.read(&self.proc, "partitions")?)
    }
    fn reword_failure(&self, msg: &str) -> String {
        iyakki_storage::loss::reword(msg)
    }
    fn mounts(&self) -> ApiResult<Vec<Mount>> {
        Ok(parse_mounts(&self.read(&self.proc, "mounts")?))
    }
}

impl Net for LocalOs {
    fn ipv4_addresses(&self) -> ApiResult<Vec<(String, String)>> {
        Ok(iyakki_devctl::ipv4_addresses())
    }
    fn interfaces(&self) -> ApiResult<Vec<NetCounters>> {
        parse_net_dev(&self.read(&self.proc, "net/dev")?)
    }
}

fn vol(v: iyakki_devctl::Volume) -> PlaybackVolume {
    PlaybackVolume { control: v.control, percent: v.percent, muted: v.muted }
}

impl Audio for LocalOs {
    fn sound_cards(&self) -> ApiResult<Vec<SoundCard>> {
        let r = iyakki_devctl::Roots { proc_: self.proc.clone(), sys: self.sys.clone(), ..iyakki_devctl::Roots::real() };
        Ok(iyakki_devctl::cards(&r).into_iter().map(|c| SoundCard { index: c.index, name: c.name, long_name: c.long_name }).collect())
    }
    fn volume(&self, card: u32) -> ApiResult<PlaybackVolume> {
        iyakki_devctl::volume_via(&*self.mixer, &self.mutes, card).map(vol).ok_or_else(|| ApiError::Unsupported("this card has no volume control".into()))
    }
    fn set_volume(&self, card: u32, percent: u32) -> ApiResult<PlaybackVolume> {
        iyakki_devctl::set_volume_via(&*self.mixer, &self.mutes, card, percent).map(vol).map_err(ApiError::Io)
    }
    fn change_volume(&self, card: u32, delta: Option<i32>) -> ApiResult<PlaybackVolume> {
        iyakki_devctl::change_volume_via(&*self.mixer, &self.mutes, card, delta).map(vol).map_err(ApiError::Io)
    }
}

impl Sysinfo for LocalOs {
    fn cmdline(&self) -> ApiResult<String> {
        Ok(self.read(&self.proc, "cmdline")?.trim_end_matches('\n').trim_end_matches('\0').to_string())
    }
    fn meminfo(&self) -> ApiResult<MemInfo> {
        parse_meminfo(&self.read(&self.proc, "meminfo")?)
    }
    fn uptime(&self) -> ApiResult<Uptime> {
        parse_uptime(&self.read(&self.proc, "uptime")?)
    }
    fn loadavg(&self) -> ApiResult<LoadAvg> {
        parse_loadavg(&self.read(&self.proc, "loadavg")?)
    }
    fn product_name(&self) -> ApiResult<String> {
        let t = self.read(&self.sys, "class/dmi/id/product_name")?;
        let t = t.trim();
        if t.is_empty() { Err(ApiError::Unsupported("the firmware gives no product name".into())) } else { Ok(t.to_string()) }
    }
    fn cpu_count(&self) -> ApiResult<u32> {
        // the kernel's own list of cpus that are online, like `nproc`: `0-3` or `0,2-5`
        if let Ok(t) = self.read(&self.sys, "devices/system/cpu/online") {
            let n: u32 = t.trim().split(',').filter_map(|r| match r.split_once('-') { Some((a, b)) => Some(b.parse::<u32>().ok()? + 1 - a.parse::<u32>().ok()?), None => r.parse::<u32>().ok().map(|_| 1) }).sum();
            if n > 0 {
                return Ok(n);
            }
        }
        // else the processor lines of the kernel's statistics, like the agent's reader always did
        if let Ok(t) = self.read(&self.proc, "stat") {
            let n = t.lines().filter(|l| l.strip_prefix("cpu").and_then(|r| r.chars().next()).is_some_and(|c| c.is_ascii_digit())).count() as u32;
            if n > 0 {
                return Ok(n);
            }
        }
        std::thread::available_parallelism().map(|n| n.get() as u32).map_err(|e| ApiError::Io(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-corelocal-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proc")).unwrap();
        std::fs::create_dir_all(d.join("sys/class/dmi/id")).unwrap();
        std::fs::create_dir_all(d.join("sys/devices/system/cpu")).unwrap();
        d
    }

    fn put(d: &Path, rel: &str, text: &str) {
        std::fs::write(d.join(rel), text).unwrap();
    }

    #[test]
    fn it_reads_the_made_up_machine_the_way_the_parsers_expect() {
        let d = tree("ok");
        put(&d, "proc/cmdline", "quiet iyakki.disk=sda\n");
        put(&d, "proc/meminfo", "MemTotal:        8000000 kB\nMemFree:          500000 kB\nMemAvailable:    6000000 kB\nSwapTotal:       1000000 kB\nSwapFree:         900000 kB\n");
        put(&d, "proc/uptime", "93784.55 180000.00\n");
        put(&d, "proc/loadavg", "0.52 0.40 0.30 1/200 999\n");
        put(&d, "sys/class/dmi/id/product_name", "MacBookPro14,1\n");
        put(&d, "sys/devices/system/cpu/online", "0-3\n");
        let os = LocalOs::at(&d);
        let s = os.sysinfo();
        assert_eq!(s.cmdline().unwrap(), "quiet iyakki.disk=sda");
        let m = s.meminfo().unwrap();
        assert_eq!((m.total_kb, m.available_kb, m.swap_total_kb), (8_000_000, 6_000_000, 1_000_000));
        assert_eq!(s.uptime().unwrap().up_secs, 93784.55);
        assert_eq!(s.loadavg().unwrap().one, 0.52);
        assert_eq!(s.product_name().unwrap(), "MacBookPro14,1");
        assert_eq!(s.cpu_count().unwrap(), 4);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_file_is_unsupported_a_damaged_one_is_an_error_and_the_cpu_list_reads_ranges() {
        let d = tree("bad");
        let os = LocalOs::at(&d);
        assert!(matches!(os.sysinfo().product_name(), Err(ApiError::Unsupported(_))));
        assert!(matches!(os.sysinfo().meminfo(), Err(ApiError::Unsupported(_))));
        put(&d, "proc/meminfo", "nothing useful\n");
        assert!(matches!(os.sysinfo().meminfo(), Err(ApiError::Io(_))));
        put(&d, "proc/uptime", "soon\n");
        assert!(os.sysinfo().uptime().is_err());
        put(&d, "sys/class/dmi/id/product_name", "  \n");
        assert!(matches!(os.sysinfo().product_name(), Err(ApiError::Unsupported(_))));
        put(&d, "sys/devices/system/cpu/online", "0,2-5\n");
        assert_eq!(os.sysinfo().cpu_count().unwrap(), 5);
        std::fs::remove_file(d.join("sys/devices/system/cpu/online")).unwrap();
        assert!(os.sysinfo().cpu_count().unwrap() >= 1, "falls back to what the program may use");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_real_machine_answers_what_every_linux_has() {
        let os = LocalOs::system();
        if std::path::Path::new("/proc/meminfo").exists() {
            assert!(os.sysinfo().meminfo().unwrap().total_kb > 0);
            assert!(os.sysinfo().uptime().unwrap().up_secs > 0.0);
        } else {
            assert!(os.sysinfo().meminfo().is_err(), "a machine without /proc says so, never panics");
        }
    }
}

#[cfg(test)]
mod slice6_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn tree(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-corelocal6-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proc/net")).unwrap();
        std::fs::create_dir_all(d.join("proc/asound")).unwrap();
        d
    }

    #[test]
    fn disks_and_net_are_read_through_the_same_parsers_as_before() {
        let d = tree("dn");
        std::fs::write(d.join("proc/partitions"), "major minor  #blocks  name\n\n   8        0   62500000 sda\n   8        1     524288 sda1\n").unwrap();
        std::fs::write(d.join("proc/mounts"), "/dev/sda1 /boot vfat rw,relatime 0 0\n/dev/sda2 / ext4 ro,relatime 0 0\n").unwrap();
        std::fs::write(d.join("proc/net/dev"), "Inter-|   Receive |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n  eth0: 123456 100 0 0 0 0 0 0 7890 50 0 0 0 0 0 0\n").unwrap();
        let os = LocalOs::at(&d);
        let b = os.disks().block_devices().unwrap();
        assert_eq!((b.len(), b[0].name.as_str(), b[0].size_bytes), (2, "sda", 62_500_000 * 1024));
        let m = os.disks().mounts().unwrap();
        assert!(m.iter().any(|x| x.target == "/" && x.read_only) && m.iter().any(|x| x.target == "/boot" && !x.read_only));
        let n = os.net().interfaces().unwrap();
        assert_eq!((n[0].name.as_str(), n[0].rx_bytes, n[0].tx_bytes), ("eth0", 123_456, 7_890));
        assert!(matches!(LocalOs::at(&d.join("nowhere")).disks().mounts(), Err(ApiError::Unsupported(_))));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_sound_cards_come_from_asound_and_the_volume_goes_through_the_mixer_the_way_the_tile_did() {
        let d = tree("snd");
        std::fs::write(d.join("proc/asound/cards"), " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0xf7f10000 irq 32\n").unwrap();
        let level = Arc::new(Mutex::new(40u32));
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let (l2, s2) = (level.clone(), seen.clone());
        let mixer: Mixer = Box::new(move |a: &[&str]| {
            s2.lock().unwrap().push(a.join(" "));
            if a.contains(&"sget") {
                return a.contains(&"Master").then(|| format!("Simple mixer control 'Master',0\n  Front Left: Playback 40 [{}%] [-20.00dB] [on]\n", l2.lock().unwrap()));
            }
            if let Some(p) = a.last().and_then(|x| x.strip_suffix('%')) {
                *l2.lock().unwrap() = p.parse().unwrap();
            }
            Some(String::new())
        });
        let os = LocalOs::at(&d).with_mixer(mixer);
        let c = os.audio().sound_cards().unwrap();
        assert_eq!((c.len(), c[0].index, c[0].name.as_str(), c[0].long_name.as_str()), (1, 0, "HDA-Intel", "HDA Intel PCH"));
        assert_eq!(os.audio().volume(0).unwrap(), PlaybackVolume { control: "Master".into(), percent: 40, muted: false });
        assert_eq!(os.audio().set_volume(0, 70).unwrap().percent, 70);
        assert!(seen.lock().unwrap().iter().any(|l| l == "-c 0 sset Master 70%"), "{:?}", seen.lock().unwrap());
        assert!(os.audio().set_volume(0, 250).unwrap().percent <= 100, "a level above 100 is cut");
        // a card without any control says so
        assert!(matches!(LocalOs::at(&d).audio().volume(3), Err(ApiError::Unsupported(_))));
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod space_tests {
    use super::*;

    #[test]
    fn the_space_of_a_real_folder_is_free_below_total_and_a_missing_one_is_an_error() {
        let d = std::env::temp_dir();
        let (free, total) = LocalOs::system().disks().space(&d.to_string_lossy()).unwrap();
        assert!(total > 0 && free <= total, "{free} of {total}");
        assert!(matches!(LocalOs::system().disks().space("/definitely/not/here"), Err(ApiError::Io(_))));
        assert!(matches!(LocalOs::system().disks().space("a\0b"), Err(ApiError::InvalidInput(_))));
    }

    #[test]
    fn the_address_list_of_this_computer_has_the_loopback() {
        let a = LocalOs::system().net().ipv4_addresses().unwrap();
        assert!(a.iter().any(|(_, ip)| ip == "127.0.0.1"), "{a:?}");
    }
}

/// The router on the device: the guard has asked and logged; there is nothing more for the machine to do.
pub struct LocalRouter;

impl iyakki_core_api::router::Router for LocalRouter {
    fn begin_run(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn ask_step_tool(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn allow_listen(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn allow_strategy(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn record(&self, _: &str, _: &str) {}
}

/// The fleet on the device: the guard has asked and logged; there is nothing more for the machine to do.
pub struct LocalFleet;

impl iyakki_core_api::fleet::Fleet for LocalFleet {
    fn read(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn join(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn manage(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn assign(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn receive(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn record(&self, _: &str, _: &str) {}
}
