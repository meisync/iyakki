//! Early userspace for running as PID 1 on bare metal: mount the pseudo filesystems,
//! populate /dev, load drivers for the hardware that is present (coldplug), set up
//! hotplug, and bring up networking. Everything is best-effort and logged — a failed
//! step must never panic PID 1 (a kernel panic would follow).

use std::path::Path;
use std::process::Command;

/// (source, target, fstype, flags, data)
pub const MOUNTS: &[(&str, &str, &str, u64, &str)] = &[
    ("proc", "/proc", "proc", 0, ""),
    ("sysfs", "/sys", "sysfs", 0, ""),
    ("devtmpfs", "/dev", "devtmpfs", 0, "mode=0755"),
    ("devpts", "/dev/pts", "devpts", 0, "mode=0620,gid=5"),
    ("tmpfs", "/run", "tmpfs", 0, "mode=0755"),
    ("tmpfs", "/tmp", "tmpfs", 0, "mode=1777"),
    ("cgroup2", "/sys/fs/cgroup", "cgroup2", 0, ""),
    ("efivarfs", "/sys/firmware/efi/efivars", "efivarfs", 0, ""),
];

/// Mounting is Linux only (the boot code runs as init there); elsewhere the window runs as an ordinary program.
#[cfg(not(target_os = "linux"))]
fn mount(_src: &str, _target: &str, _fstype: &str, _flags: u64, _data: &str) -> Result<(), String> {
    Err("mount is not available on this OS".into())
}

#[cfg(target_os = "linux")]
fn mount(src: &str, target: &str, fstype: &str, flags: u64, data: &str) -> Result<(), String> {
    use std::ffi::CString;
    let _ = std::fs::create_dir_all(target);
    let (s, t, f, d) = (CString::new(src), CString::new(target), CString::new(fstype), CString::new(data));
    let (Ok(s), Ok(t), Ok(f), Ok(d)) = (s, t, f, d) else { return Err("nul in mount args".into()) };
    // SAFETY: all pointers are valid NUL-terminated strings for the duration of the call.
    let rc = unsafe { libc::mount(s.as_ptr(), t.as_ptr(), f.as_ptr(), flags, d.as_ptr().cast()) };
    if rc == 0 { Ok(()) } else { Err(std::io::Error::last_os_error().to_string()) }
}

/// Collect `modalias` strings from sysfs devices; each names the driver module to load.
/// How deep under `/sys/devices` the scan looks for `modalias` files.
pub const MAX_SYSFS_DEPTH: u32 = 16;

pub fn collect_modaliases(sys_devices: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>, depth: u32) {
        // A device behind a bridge chain and the T2's virtual USB host is 10 levels down (PCI root, bridge, bridge, BCE, apple-bce,
        // bce-vhci, usb5, port, interface, HID device): the old limit of 8 never reached the Mac's keyboard.
        if depth > MAX_SYSFS_DEPTH {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                walk(&p, out, depth + 1);
            } else if e.file_name() == "modalias" {
                if let Ok(s) = std::fs::read_to_string(&p) {
                    let s = s.trim();
                    if !s.is_empty() {
                        out.push(s.to_string());
                    }
                }
            }
        }
    }
    let mut v = Vec::new();
    walk(sys_devices, &mut v, 0);
    v.sort();
    v.dedup();
    v
}

/// Report boot progress to the splash screen (a no-op in builds without the shell).
#[cfg(feature = "shell")]
pub fn progress(percent: u32, stage: &str) {
    iyakki_shell::splash::progress(percent, stage);
}
#[cfg(not(feature = "shell"))]
pub fn progress(_percent: u32, _stage: &str) {}

/// Boot option `iyakki.install=1`: start the installer instead of the desktop.
pub fn install_mode(cmdline: &str) -> bool {
    cmdline_param(cmdline, "iyakki.install") == Some("1")
}

/// The installer on the console (`/dev/tty0`, the kernel's text console on the screen, with the keyboard). Returns when the
/// person is done; the caller restarts the machine.
#[cfg(target_os = "linux")]
pub fn run_install_console() -> i32 {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    // the node appears when mdev has run; wait a little for the console and the keyboard driver
    let mut tty = None;
    for _ in 0..100 {
        tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty0").ok();
        if tty.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let Some(tty) = tty else {
        log::error!("install mode: no /dev/tty0");
        return 1;
    };
    // SAFETY: dup2 onto the standard descriptors; KDSETMODE (0x4B3A) with KD_TEXT (0) takes an integer argument.
    unsafe {
        libc::dup2(tty.as_raw_fd(), 0);
        libc::dup2(tty.as_raw_fd(), 1);
        libc::dup2(tty.as_raw_fd(), 2);
        libc::ioctl(tty.as_raw_fd(), 0x4B3A as _, 0);
    }
    // no screen blanking while someone reads the plan
    print!("\x1b[9;0]\x1b[2J\x1b[H");
    let _ = std::io::stdout().flush();
    let code = iyakki_usb::plan_cli::run_boot();
    println!("\nRemove the installation USB stick, then press Enter to restart.");
    let mut l = String::new();
    let _ = std::io::stdin().read_line(&mut l);
    code
}

#[cfg(not(target_os = "linux"))]
pub fn run_install_console() -> i32 {
    eprintln!("install mode needs the IYAKKI image");
    1
}

/// Show the boot screen on the framebuffer the kernel started with, until [`finish_boot_screen`].
#[cfg(feature = "shell")]
fn start_boot_screen() {
    let cmd = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    // install mode draws text on the console: the boot screen would paint over it
    if cmdline_param(&cmd, "iyakki.shell") == Some("0") || install_mode(&cmd) {
        return;
    }
    iyakki_shell::splash::mark_started();
    let _ = BOOT_SCREEN_SINCE.set(std::time::Instant::now());
    let spawn = std::thread::Builder::new().name("iyakki-splash".into()).stack_size(crate::console::THREAD_STACK).spawn(|| {
        use iyakki_shell::splash::{self, note};
        let (dev, sys, started) = (Path::new("/dev"), Path::new("/sys"), std::time::Instant::now());
        note(&format!("boot screen requested (minimum {} s); waiting for a framebuffer. {}", splash_minimum(&std::fs::read_to_string("/proc/cmdline").unwrap_or_default()).as_secs(), splash::graphics_summary(sys)));
        // a GPU driver that is a module only creates the screen after it loads, and a node can be missing even when
        // the kernel has the screen: wait for either, never longer than boot itself
        let mut told = 0u64;
        let path = loop {
            if splash::is_done() {
                note(&format!("boot finished after {} ms before any framebuffer appeared; no boot screen was shown. {}", started.elapsed().as_millis(), splash::graphics_summary(sys)));
                splash::mark_stopped();
                return;
            }
            if let Some(p) = splash::find_framebuffer(dev) {
                break p;
            }
            let made = splash::make_missing_nodes(&sys.join("class/graphics"), dev);
            if !made.is_empty() {
                note(&format!("the kernel has {} but /dev had no node; made it", made.join(", ")));
                continue;
            }
            let s = started.elapsed().as_secs();
            if s >= 1 && s > told && (s == 1 || s % 5 == 0) {
                told = s;
                note(&format!("still no framebuffer after {s} s. {}", splash::graphics_summary(sys)));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        note(&format!("{} appeared {} ms after the boot screen was requested", path.display(), started.elapsed().as_millis()));
        // a screen that fails (or vanishes while the GPU driver takes over) is tried again until boot ends
        let mut attempt = 0;
        while !splash::is_done() {
            attempt += 1;
            let p = splash::find_framebuffer(dev).unwrap_or_else(|| path.clone());
            let r = std::panic::catch_unwind(|| iyakki_shell::backend::run_splash(&p.to_string_lossy()));
            if matches!(r, Ok(Ok(()))) {
                return;
            }
            note(&format!("stopped early (attempt {attempt}): {}", match &r {
                Ok(Err(e)) => e.clone(),
                Err(_) => "the drawing code panicked".to_string(),
                Ok(Ok(())) => String::new(),
            }));
            if attempt >= 20 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        splash::mark_stopped();
    });
    if spawn.is_err() {
        iyakki_shell::splash::note("could not start the boot screen thread");
        iyakki_shell::splash::mark_stopped();
    }
}
#[cfg(not(feature = "shell"))]
fn start_boot_screen() {}

/// A monitor turned up after a probe: the boot screen (or the desktop) writes its whole picture again on it.
pub fn screen_found() {
    #[cfg(feature = "shell")]
    iyakki_shell::splash::request_repaint();
}

static BOOT_SCREEN_SINCE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Seconds the boot screen must stay up at least (`iyakki.splash=N`, default 0): for looking at it on a fast machine.
pub fn splash_minimum(cmdline: &str) -> std::time::Duration {
    std::time::Duration::from_secs(cmdline_param(cmdline, "iyakki.splash").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).min(30))
}

/// Boot is over: stop the boot screen and wait until it has let go of the display.
pub fn finish_boot_screen() {
    #[cfg(feature = "shell")]
    {
        let min = splash_minimum(&std::fs::read_to_string("/proc/cmdline").unwrap_or_default());
        if let Some(t) = BOOT_SCREEN_SINCE.get() {
            std::thread::sleep(min.saturating_sub(t.elapsed()));
        }
        iyakki_shell::splash::finish();
        iyakki_shell::splash::wait_until_stopped(3000);
    }
}

/// Load the drivers for these device aliases (`modprobe -b` honors blacklists). `from..to` is the progress range.
fn load_aliases(aliases: &[String], what: &str, from: u32, to: u32) {
    let chunks = aliases.chunks(64).count().max(1);
    for (i, chunk) in aliases.chunks(64).enumerate() {
        progress(from + ((to - from) as usize * i / chunks) as u32, &format!("{what} ({}/{})", i + 1, chunks));
        let _ = Command::new("modprobe").arg("-b").arg("-a").args(chunk).stderr(std::process::Stdio::null()).status();
    }
}

/// Repeat the coldplug scan after a batch of drivers loaded: aliases of new devices that are needed at once are loaded here, the
/// ones that wait for the sign-in screen are appended to `deferred`. Logs one line when something was found.
fn coldplug_settle(seen: &mut crate::coldplug::Seen, deferred: &mut Vec<String>) {
    use crate::coldplug as cp;
    let res = cp::settle(Path::new("/sys/devices"), seen, cp::MAX_EXTRA_PASSES, cp::BUDGET, |new| {
        let (later, now): (Vec<String>, Vec<String>) = new.iter().cloned().partition(|a| is_deferred(a));
        deferred.extend(later);
        if !now.is_empty() {
            load_aliases(&now, "Starting hardware drivers", 66, 67);
        }
    }, || std::thread::sleep(cp::PAUSE));
    if res.passes > 0 {
        log::info!("coldplug: {} more device aliases appeared after drivers loaded ({} extra passes), e.g. {}", res.found.len(), res.passes, res.found.first().map(String::as_str).unwrap_or(""));
    }
}

/// The Apple SPI keyboard and trackpad (MacBook8,1 .. MacBookPro14,3): load the chain of drivers by name as well (see
/// `iyakki_drivers::applespi`). Any other machine: one DMI read.
fn apple_spi_keyboard() {
    let product = std::fs::read_to_string("/sys/class/dmi/id/product_name").unwrap_or_default();
    let n = iyakki_drivers::applespi::load(&product, |m| {
        let _ = Command::new("modprobe").arg("-b").arg(m).stderr(std::process::Stdio::null()).status();
    });
    if n > 0 {
        log::info!("Apple SPI keyboard: loaded {}", iyakki_drivers::applespi::MODULES.join(", "));
    }
}

/// Load drivers for all present hardware, in one go.
pub fn coldplug() -> usize {
    let aliases = collect_modaliases(Path::new("/sys/devices"));
    load_aliases(&aliases, "Starting hardware drivers", 25, 70);
    aliases.len()
}

/// MacBookPro13,x/14,x and 2017-2019 iMacs: the Cirrus CS8409 codec needs the image's snd-hda-codec-cs8409 module (package
/// snd-hda-macbookpro), or the generic driver binds and the speakers stay silent. The module is normally loaded by the codec's
/// alias during the coldplug above; this checks, and when the codec ended up on the generic driver loads the module and re-binds the
/// sound controller. One log line. Any other machine: one DMI read.
fn cs8409_sound() {
    use iyakki_drivers::cs8409::{self, RealSys, Roots};
    let r = Roots::real();
    if !cs8409::model_applies(&std::fs::read_to_string("/sys/class/dmi/id/product_name").unwrap_or_default()) {
        return;
    }
    // the sound controller is probed in the background: wait (at most 5 s) for its codec to be listed
    for _ in 0..25 {
        if cs8409::status(&r).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    if let Some(l) = cs8409::ensure(&r, &iyakki_drivers::run::System, &RealSys, &|| std::thread::sleep(std::time::Duration::from_millis(500))) {
        log::info!("{l}");
    }
}

/// Mandatory "always load" modules that have no modalias (crypto for WiFi/WPA, RNG, etc). `thunderbolt` and `thunderbolt_net` are
/// here as well as being found by their aliases (PCI ids, `tbsvc:`): a one-cable link to another computer (`iyakki dev usb`) must not
/// depend on the alias table. A kernel without them (not a Mac or PC image) ignores the failed modprobe.
pub const ALWAYS_LOAD: &[&str] = &["af_packet", "cfg80211", "mac80211", "ccm", "gcm", "cmac", "arc4", "ctr", "sha256_generic", "tun", "vfat", "nls_cp437", "nls_iso8859_1", "thunderbolt", "thunderbolt_net"];
/// The part of [`ALWAYS_LOAD`] that login needs (reading the boot partition); the rest is for the network.
pub const LOGIN_LOAD: &[&str] = &["vfat", "nls_cp437", "nls_iso8859_1"];

/// Hardware that login does not need: network, WiFi, sound, Bluetooth, video capture. Its drivers load after the sign-in
/// screen is up. Everything else (storage, keyboard, mouse, display, the platform) loads first, so a class we did not
/// think of is never left out of the critical path.
pub fn is_deferred(modalias: &str) -> bool {
    let a = modalias.to_ascii_lowercase();
    let class = |tag: &str| a.contains(tag);
    if a.starts_with("pci:") {
        // base class 02 network, 04 multimedia (sound), 0d wireless
        return class("bc02") || class("bc04") || class("bc0d");
    }
    if a.starts_with("usb:") {
        // interface class 01 audio, 02/0a communications (modems, USB network), 0e video capture, e0 wireless (Bluetooth)
        return class("ic01") || class("ic02") || class("ic0a") || class("ic0e") || class("ice0");
    }
    ["hdaudio:", "sdio:", "snd", "bluetooth", "mdio:", "ieee80211", "mei:"].iter().any(|p| a.starts_with(p))
}

/// Where boot time goes, logged once at the end ("boot: kernel 2.1 s, then mounts 20 ms, ...").
struct Timeline {
    last: std::time::Instant,
    marks: Vec<(&'static str, u128)>,
}

impl Timeline {
    fn new() -> Timeline {
        Timeline { last: std::time::Instant::now(), marks: Vec::new() }
    }
    fn mark(&mut self, what: &'static str) {
        self.marks.push((what, self.last.elapsed().as_millis()));
        self.last = std::time::Instant::now();
    }
    fn summary(&self, kernel_secs: Option<f64>) -> String {
        let parts: Vec<String> = self.marks.iter().map(|(w, ms)| format!("{w} {ms} ms")).collect();
        let total: u128 = self.marks.iter().map(|m| m.1).sum();
        format!("boot: {}iyakki took {total} ms ({})", kernel_secs.map_or(String::new(), |k| format!("kernel {k:.1} s, then ")), parts.join(", "))
    }
}

/// /proc, /sys, /dev and the like. Safe to call again: what is already mounted is left alone. Needed before the kernel
/// command line (`/proc/cmdline`) can be read.
pub fn mount_pseudo_filesystems() {
    for (src, tgt, fs, flags, data) in MOUNTS {
        if let Err(e) = mount(src, tgt, fs, *flags, data) {
            // already mounted (EBUSY) or unsupported (e.g. no EFI) is fine
            log::debug!("mount {tgt}: {e}");
        }
    }
}

/// What is left to do after login is possible.
pub struct LatePlan {
    deferred: Vec<String>,
    /// Every alias handed to a loader so far (the repeated coldplug only loads what is new).
    seen: crate::coldplug::Seen,
    tl: Timeline,
}

/// Everything at once (self-tests and anything that needs the network up when this returns).
pub fn early_boot() {
    let plan = early_boot_critical();
    late_boot(plan);
}

/// The part of boot that login needs: pseudo filesystems, device nodes, the boot screen, the drivers for storage, input
/// and display, and the data volume. Returns what remains for [`late_boot`], which may run in the background.
pub fn early_boot_critical() -> LatePlan {
    let mut tl = Timeline::new();
    let kernel_secs = std::fs::read_to_string("/proc/uptime").ok().and_then(|u| u.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()));
    KERNEL_SECS.get_or_init(|| kernel_secs);
    // The kernel starts PID 1 with no PATH; without this, modprobe/ip/udhcpc/mdev are never found.
    std::env::set_var("PATH", "/usr/sbin:/usr/bin:/sbin:/bin");
    mount_pseudo_filesystems();
    // The kernel drops all but about 10 lines per 5 seconds that a program writes to its log (`printk.devkmsg=ratelimit`, the default), so the
    // boot lines that follow (and the timeline `lean-check.sh` reads) went missing. Switch the limit off; the lean first stage does the same.
    let _ = std::fs::write("/proc/sys/kernel/printk_devkmsg", "on\n");
    tl.mark("mounts");
    // A clock that starts in the past (a Mac's RTC reads 2024) is lifted to the image's build time first, so nothing
    // that makes or checks certificates sees a date before the image existed; the saved time follows the data mount.
    clock_floor_early();
    // Before anything can load the camera or USB sound driver by alias (mdev below, the coldplug): IYAKKI loads those
    // itself, so a camera that needs a uvcvideo quirk gets it on its first probe (see camera.rs).
    crate::camera::block_autoload();
    // A T2 Mac's Wi-Fi and Bluetooth drivers fail noisily without Apple's firmware (and a Bluetooth chip started that way keeps no
    // address), so they wait: the driver pack that brings the firmware loads them by name in `late_boot`, once its files are linked.
    let held = iyakki_drivers::t2hold::apply(Path::new("/sys/class/dmi/id/product_name"), Path::new("/lib/firmware"), Path::new("/etc/modprobe.d"), Path::new("/lib/modules"));
    if !held.is_empty() {
        log::info!("T2 Mac: {} wait for Apple's firmware (Mac drivers page)", held.join(", "));
    }
    iyakki_drivers::t2input::apply(Path::new("/sys/class/dmi/id/product_name"), Path::new("/etc/modprobe.d"));
    // Hotplug: let the kernel call mdev for new devices (creates nodes, loads modules).
    let _ = std::fs::write("/proc/sys/kernel/hotplug", "/sbin/mdev\n");
    let _ = Command::new("mdev").arg("-s").status();
    // mdev makes device nodes but loads no drivers; this loads them for every device that appears from now on (a T2 Mac's keyboard
    // behind apple-bce, a USB network adapter plugged in later) and sets up a new network interface
    crate::hotplug::start();
    tl.mark("device nodes");
    start_boot_screen();
    // a monitor the GPU driver does not see at first (a DP->HDMI converter, a monitor still waking up) is probed again
    crate::display::start(screen_found);
    progress(8, "Preparing the system");
    for m in LOGIN_LOAD {
        let _ = Command::new("modprobe").arg("-b").arg(m).stderr(std::process::Stdio::null()).status();
    }
    tl.mark("core modules");
    let all = collect_modaliases(Path::new("/sys/devices"));
    let (deferred, critical): (Vec<String>, Vec<String>) = all.into_iter().partition(|a| is_deferred(a));
    log::info!("coldplug: {} device aliases now, {} (network, sound, ...) after the sign-in screen", critical.len(), deferred.len());
    load_aliases(&critical, "Starting hardware drivers", 12, 66);
    let mut seen = crate::coldplug::Seen::new();
    seen.add_all(&critical);
    seen.add_all(&deferred);
    let mut deferred = deferred;
    // Drivers that create child devices (Intel LPSS: the SPI/I2C controllers; MFD cells) leave devices the first scan could not
    // see: scan again, load what is new (the keyboard and trackpad of a 2016-2017 MacBook sit behind such a child).
    coldplug_settle(&mut seen, &mut deferred);
    apple_spi_keyboard();
    tl.mark("drivers");
    progress(68, "Setting up devices");
    let _ = Command::new("mdev").arg("-s").status(); // nodes for devices whose drivers just loaded
    progress(72, "Opening your storage");
    mount_data_volume();
    clock_floor_after_data();
    // crash forensics: follow the kernel log into /var/lib/iyakki/log from here on (normal and lean boot alike; docs/TROUBLESHOOTING.md)
    if dir_is_mount_point(&std::fs::read_to_string("/proc/mounts").unwrap_or_default(), DATA_DIR) {
        if let Some(u) = crate::crashlog::start(Path::new(DATA_DIR)) {
            log::warn!("crashlog: the last session ended unexpectedly (up {} s, last line: {}); its kernel log is kept as {}", u.uptime_s, u.last_line, u.file);
        }
    }
    progress(86, HOME_STAGE);
    bind_home_on_data();
    // watch the drive the data lives on: a lost USB drive is noticed, explained and (when safe) reconnected (docs/STORAGE-LOSS.md)
    crate::storageloss::start();
    tl.mark("storage");
    memory_setup();
    tl.mark("memory");
    if let Some(h) = configured_hostname() {
        let _ = std::fs::write("/etc/hostname", format!("{h}\n"));
    }
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            // SAFETY: pointer/len describe a valid byte slice.
            unsafe { libc::sethostname(h.as_ptr().cast(), h.len() as _) };
        }
    }
    progress(94, "Almost ready");
    log::info!("{}", tl.summary(kernel_secs).replace("boot:", "boot (to login):"));
    LatePlan { deferred, seen, tl: Timeline::new() }
}

static KERNEL_SECS: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();

/// The rest of boot: drivers for network, sound and Bluetooth, the network itself, remote diagnostics.
pub fn late_boot(mut plan: LatePlan) {
    // The firewall goes up before any network service listens: the interfaces are not configured yet (the network is
    // brought up further down), so nothing is reachable without it. A failure is loud (top bar, audit log), never silent.
    firewall_boot();
    swapfile_setup();
    // Driver packs from the data volume (firmware links, modprobe options, matching prebuilt modules) go in before the
    // network, sound and Bluetooth drivers load, so those drivers find their firmware the first time.
    for l in iyakki_drivers::apply_boot(&iyakki_drivers::Fs::real(), &iyakki_drivers::run::System) {
        log::info!("{l}");
    }
    for m in ALWAYS_LOAD.iter().filter(|m| !LOGIN_LOAD.contains(m)) {
        let _ = Command::new("modprobe").arg("-b").arg(m).stderr(std::process::Stdio::null()).status();
    }
    // Cameras known to need a quirk lose their microphone and are held back, then uvcvideo and snd-usb-audio are loaded
    // by name (they are blacklisted for alias loading); the quirk binds and later hot plugs run in a background thread.
    crate::camera::prepare_boot();
    load_aliases(&plan.deferred, "Starting network and sound", 70, 90);
    // children created by the network, sound and Bluetooth drivers (a Wi-Fi chip's Bluetooth function, a codec behind a bridge)
    {
        let mut more = Vec::new();
        coldplug_settle(&mut plan.seen, &mut more);
        load_aliases(&more, "Starting network and sound", 88, 90);
    }
    cs8409_sound();
    let _ = Command::new("mdev").arg("-s").status();
    crate::camera::start();
    plan.tl.mark("late drivers");
    match crate::net::bring_up(Path::new("/etc/iyakki/net.conf"), Path::new("/sys/class/net")) {
        Ok(errs) => errs.iter().for_each(|e| log::warn!("net: {e}")),
        Err(e) => log::error!("net config: {e}"),
    }
    // addresses exist now: look host names up and apply again
    iyakki_fw::service::network_changed();
    crate::hotplug::boot_done();
    touchbar_retry();
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    if cmdline_param(&cmdline, "iyakki.ssh") == Some("1") {
        start_remote_diagnostics();
    }
    plan.tl.mark("network");
    log::info!("{}", plan.tl.summary(None).replace("boot: iyakki took", "boot (after login): iyakki took"));
}

/// A T2 MacBook's Touch Bar often times out when the kernel configures it at boot (`can't set config #1, error -110`). One background
/// thread brings it up with the careful sequence of `iyakki_drivers::touchbar::bring_up` (a first try of 2 then 1, then at most one reset of
/// that one device and one more write, with long waits; never a loop). Any other machine: one directory read.
#[cfg(not(target_os = "linux"))]
fn touchbar_retry() {}

#[cfg(target_os = "linux")]
fn touchbar_retry() {
    let usb = Path::new("/sys/bus/usb/devices");
    let Some(found) = iyakki_drivers::touchbar::find_devices(usb).into_iter().find(|f| f.configured.is_none()) else { return };
    let _ = std::thread::Builder::new().name("iyakki-touchbar".into()).spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        let mut port = iyakki_drivers::usbfs::SysPort::new(found, "/dev".into());
        match iyakki_drivers::touchbar::bring_up(&mut port, &iyakki_drivers::touchbar::Policy::default()) {
            iyakki_drivers::touchbar::Outcome::Configured { rounds } => log::info!("Touch Bar: configured (round {rounds})"),
            iyakki_drivers::touchbar::Outcome::AlreadyConfigured => {}
            iyakki_drivers::touchbar::Outcome::Absent => log::info!("Touch Bar: the device went away"),
            iyakki_drivers::touchbar::Outcome::GaveUp { rounds } => log::info!("Touch Bar: the T2 did not accept its USB configuration after {rounds} rounds (left alone until a restart; the keyboard's Fn row is not affected)"),
        }
    });
}

/// Apply the firewall from the machine settings (`iyakki.firewall=off` skips it for this boot).
pub fn firewall_boot() {
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    iyakki_fw::service::boot(&cmdline);
    let st = iyakki_fw::service::engine().status();
    match st {
        iyakki_fw::Status::On | iyakki_fw::Status::Pending { .. } => log::info!("firewall: on (incoming connections blocked unless allowed)"),
        iyakki_fw::Status::Off => log::warn!("firewall: off (switched off in Settings)"),
        iyakki_fw::Status::OffThisBoot => log::warn!("firewall: OFF for this boot (iyakki.firewall=off)"),
        iyakki_fw::Status::NotProtected(e) => log::error!("firewall: NOT PROTECTED: {e}"),
        iyakki_fw::Status::Unavailable(e) => log::error!("firewall: NOT PROTECTED: {e}"),
    }
}

/// Where the image keeps the public keys allowed to log in for remote diagnostics (installed by the image build).
pub const SSH_KEYS_SRC: &str = "/etc/iyakki/authorized_keys";
/// Written once an address is known, for the sign-in screen to show.
pub const SSH_NOTE: &str = "/run/iyakki-ssh";

/// Where the SSH host key lives when the data volume is on a disk, so it is the same on every boot (a key made anew each boot
/// makes every SSH client warn "host key changed").
pub const SSH_HOST_KEY: &str = "/var/lib/iyakki/ssh/dropbear_ed25519_host_key";

/// Read by login shells (the image's `/etc/profile` sources `/etc/profile.d/*.sh`): the programs installed from the CLI catalogue
/// (k3s, node, python3, ...) and the managed cluster's kubeconfig, so an SSH session finds them like the terminal tile does.
pub const TOOLS_PROFILE: &str = "/etc/profile.d/iyakki-tools.sh";

/// The text of [`TOOLS_PROFILE`] for a data folder.
pub fn tools_profile(data: &str) -> String {
    format!("export PATH=\"{data}/cli/bin:$PATH\"\n[ -r {data}/k3s-dev/kubeconfig ] && export KUBECONFIG={data}/k3s-dev/kubeconfig\n[ -e {data}/cli/bin/k3s ] && [ ! -e {data}/cli/bin/kubectl ] && alias kubectl='k3s kubectl'\n")
}

/// The SSH server command for remote diagnostics: keys only (no passwords). With `host_key` (a key kept on the data volume)
/// the server uses that key; without one (a session in RAM) it makes host keys on demand, which change every boot.
pub fn ssh_server_command(host_key: Option<&str>) -> Vec<String> {
    let mut c: Vec<String> = ["dropbear", "-s", "-E", "-p", "22"].iter().map(|s| s.to_string()).collect();
    match host_key {
        Some(k) => c.extend(["-r".to_string(), k.to_string()]),
        None => c.insert(1, "-R".to_string()),
    }
    c
}

/// The command that makes the kept host key the first time (Dropbear's own key tool).
pub fn ssh_host_key_command(path: &str) -> Vec<String> {
    ["dropbearkey", "-t", "ed25519", "-f", path].iter().map(|s| s.to_string()).collect()
}

/// The host key to use: the kept one on a disk data volume (made with `make` when missing: 0700 folder, 0600 file), or
/// `None` when the data lives in RAM or the key cannot be made (the server then makes a fresh key for this boot).
pub fn persistent_host_key(path: &Path, data_on_disk: bool, make: &mut dyn FnMut(&Path) -> bool) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    if !data_on_disk {
        return None;
    }
    let dir = path.parent()?;
    std::fs::create_dir_all(dir).ok()?;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    let usable = |p: &Path| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() > 0);
    if !usable(path) {
        let _ = std::fs::remove_file(path); // an empty file left by an interrupted first try
        if !make(path) || !usable(path) {
            return None;
        }
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok()?;
    Some(path.to_path_buf())
}

/// First global IPv4 address in `ip -4 -o addr show scope global` output.
pub fn first_ipv4(ip_output: &str) -> Option<String> {
    ip_output.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        it.position(|t| t == "inet")?;
        it.next().map(|a| a.split('/').next().unwrap_or(a).to_string())
    })
}

/// Only when the boot entry asks for it (`iyakki.ssh=1`): allow the keys shipped in the image to log in over SSH,
/// and tell the sign-in screen the address once DHCP has given one. Passwords are never accepted.
fn start_remote_diagnostics() {
    let keys = match std::fs::read_to_string(SSH_KEYS_SRC) {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            log::error!("ssh: no keys in {SSH_KEYS_SRC} (build the image with IYAKKI_SSH_AUTHORIZED_KEYS=<file>); not starting");
            return;
        }
    };
    use std::os::unix::fs::PermissionsExt;
    let setup = std::fs::create_dir_all("/root/.ssh")
        .and_then(|_| std::fs::set_permissions("/root", std::fs::Permissions::from_mode(0o700)))
        .and_then(|_| std::fs::set_permissions("/root/.ssh", std::fs::Permissions::from_mode(0o700)))
        .and_then(|_| std::fs::write("/root/.ssh/authorized_keys", keys))
        .and_then(|_| std::fs::set_permissions("/root/.ssh/authorized_keys", std::fs::Permissions::from_mode(0o600)));
    if let Err(e) = setup {
        log::error!("ssh: could not install the keys: {e}");
        return;
    }
    let _ = std::fs::create_dir_all("/var/run/dropbear");
    if let Err(e) = std::fs::write(TOOLS_PROFILE, tools_profile(DATA_DIR)) {
        log::warn!("ssh: could not write {TOOLS_PROFILE}: {e}");
    }
    let on_disk = matches!(iyakki_storage::data_location(&iyakki_storage::Roots::real(), Path::new(DATA_DIR)), iyakki_storage::DataLocation::Disk(_));
    let key = persistent_host_key(Path::new(SSH_HOST_KEY), on_disk, &mut |p| {
        let c = ssh_host_key_command(&p.to_string_lossy());
        Command::new(&c[0]).args(&c[1..]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
    });
    match &key {
        Some(k) => log::info!("ssh: host key {} (kept on the data volume)", k.display()),
        None => log::warn!("ssh: data is in RAM (or the key could not be made); the host key is new this boot"),
    }
    let key = key.map(|k| k.to_string_lossy().to_string());
    let cmd = ssh_server_command(key.as_deref());
    match Command::new(&cmd[0]).args(&cmd[1..]).spawn() {
        Ok(_) => log::warn!("ssh: remote diagnostics ON (keys only, port 22)"),
        Err(e) => {
            log::error!("ssh: could not start {}: {e}", cmd[0]);
            return;
        }
    }
    std::thread::spawn(|| {
        let mut shown = String::new();
        loop {
            let out = Command::new("ip").args(["-4", "-o", "addr", "show", "scope", "global"]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
            if let Some(ip) = first_ipv4(&out) {
                if ip != shown {
                    log::warn!("ssh: connect with  ssh root@{ip}");
                    let _ = std::fs::write(SSH_NOTE, format!("ssh root@{ip}"));
                    shown = ip;
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(if shown.is_empty() { 3 } else { 30 }));
        }
    });
}

/// The memory settings in force: kernel command line over `system.conf` over the defaults.
pub fn memory_settings() -> iyakki_zram::Resolved {
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    let conf = std::fs::read_to_string(iyakki_zram::CONF_PATH).unwrap_or_default();
    iyakki_zram::config::resolve(&cmdline, &conf)
}

/// Total RAM in bytes from `/proc/meminfo`.
pub fn total_ram() -> u64 {
    iyakki_zram::stats::parse_meminfo(&std::fs::read_to_string("/proc/meminfo").unwrap_or_default()).0
}

fn log_steps(steps: Vec<iyakki_zram::apply::Step>) {
    for s in steps {
        if s.ok {
            log::info!("{}", s.msg);
        } else {
            log::warn!("{}", s.msg);
        }
    }
}

/// The host name chosen in the installer (`hostname=` in `system.conf`), if valid.
fn configured_hostname() -> Option<String> {
    // the boot option wins over the file (and locks the page in Settings)
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    if let Some(n) = cmdline_param(&cmdline, "iyakki.hostname").filter(|h| iyakki_usb::install::valid_hostname(h)) {
        return Some(n.to_string());
    }
    let conf = iyakki_zram::config::parse_conf(&std::fs::read_to_string(iyakki_zram::CONF_PATH).ok()?);
    conf.get("hostname").filter(|h| iyakki_usb::install::valid_hostname(h)).cloned()
}

/// zram swap and the vm settings, early so even a small machine has room while the rest starts. Never fatal.
pub fn memory_setup() {
    if cmdline_param(&std::fs::read_to_string("/proc/cmdline").unwrap_or_default(), "iyakki.env").is_some_and(|e| e == "docker") {
        return;
    }
    let r = memory_settings();
    let steps = iyakki_zram::apply(&iyakki_zram::Env::real(), &iyakki_zram::KernelSwap, &r.config, total_ram());
    // the serial console shows the outcome too (boot tests read it there)
    for st in steps.iter().filter(|s| s.msg.starts_with("zram:")) {
        crate::console::out_write(&format!("IYAKKI-BOOT {}\n", st.msg));
    }
    log_steps(steps);
}

/// The optional swap file on the data volume, in the background (writing it can take a while). Off unless asked for.
fn swapfile_setup() {
    let r = memory_settings();
    let Some(bytes) = r.config.swapfile else { return };
    std::thread::spawn(move || {
        let on_disk = matches!(iyakki_storage::data_location(&iyakki_storage::Roots::real(), Path::new(DATA_DIR)), iyakki_storage::DataLocation::Disk(_));
        log_steps(iyakki_zram::apply::apply_swapfile(&iyakki_zram::KernelSwap, &iyakki_zram::Env::real(), Path::new(iyakki_zram::SWAPFILE_PATH), Some(bytes), on_disk));
    });
}

pub const DATA_DIR: &str = "/var/lib/iyakki";

/// The clock floor (docs/K3S-DEV.md, "wrong date at boot"): see `iyakki_agent::clockfloor`. Only as the first process.
fn clock_floor_early() {
    #[cfg(feature = "shell")]
    if std::process::id() == 1 {
        iyakki_agent::clockfloor::apply_early();
    }
}

fn clock_floor_after_data() {
    #[cfg(feature = "shell")]
    if std::process::id() == 1 {
        let on_disk = matches!(iyakki_storage::data_location(&iyakki_storage::Roots::real(), Path::new(DATA_DIR)), iyakki_storage::DataLocation::Disk(_));
        iyakki_agent::clockfloor::apply_after_data(on_disk);
    }
}

/// Stop the speakers before the final sync (the T2 sound chip loops its buffer until the power is gone): see `iyakki_agent::quiet`.
fn quiet_sound() {
    #[cfg(feature = "shell")]
    iyakki_agent::quiet::quiet_for_shutdown();
}

/// Save the time for the next boot's floor (before the disks are flushed at shutdown).
fn clock_floor_save() {
    #[cfg(feature = "shell")]
    iyakki_agent::clockfloor::save_now();
}

/// `/home` lives on the data volume (see `homebind`): before the sign-in, so nothing writes user files into RAM.
fn bind_home_on_data() {
    #[cfg(target_os = "linux")]
    for l in crate::homebind::bind_home_real(Path::new(DATA_DIR), &|src, target| mount(&src.display().to_string(), &target.display().to_string(), "", libc::MS_BIND, "")) {
        log::info!("{l}");
    }
}

/// Mount the IYAKKI-DATA volume (if any) over the data directory so accounts, settings, vaults,
/// documents and chats survive power-off. Without one, IYAKKI keeps running from RAM.
/// `iyakki.data=ram` on the kernel command line skips the lookup.
pub fn mount_data_volume() {
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    if cmdline_param(&cmdline, "iyakki.data") == Some("ram") {
        log::info!("data: kept in RAM (iyakki.data=ram)");
        return;
    }
    // The lean boot layout (docs/BOOT-SPEED.md): the first stage read the system from the data volume and left it mounted here.
    if dir_is_mount_point(&std::fs::read_to_string("/proc/mounts").unwrap_or_default(), DATA_DIR) {
        log::info!("data: the volume under {DATA_DIR} was mounted by the first stage (lean boot)");
        crate::console::out_write(&format!("IYAKKI-BOOT data: persistent volume mounted at {DATA_DIR} (by the first stage)\n"));
        // a copy of the kernel log on the volume, so a start that stops before the sign-in screen can be read on another computer
        crate::bootlog::start(Path::new(DATA_DIR).join("boot-lean.log"));
        // the same private mode `iyakki_storage::mount_data` gives a volume it mounts itself
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(DATA_DIR, std::fs::Permissions::from_mode(0o700));
        }
        return;
    }
    let roots = iyakki_storage::Roots::real();
    // USB disks (the stick we booted from, most of all) show up some time after the drivers load: wait for them
    let start = std::time::Instant::now();
    let mut announced = false;
    loop {
        let found = iyakki_storage::find_data(&roots).is_some() || iyakki_storage::find_pending(&roots).is_some();
        let has_disk = iyakki_storage::list(&roots).iter().any(|d| d.parent.is_none());
        let usb = usb_storage_present(Path::new("/sys"));
        // a GPT that names the data partition whose device has not appeared yet: give the kernel a few more seconds
        let announced_by_gpt = !found && start.elapsed() < std::time::Duration::from_secs(10) && !iyakki_storage::gpt_hints(&roots).is_empty();
        if !keep_waiting_for_storage(start.elapsed(), found, has_disk, usb) && !announced_by_gpt {
            break;
        }
        if !announced {
            announced = true;
            progress(78, "Waiting for the storage device");
            log::info!("data: waiting for the disk (usb storage seen: {usb}, disk listed: {has_disk})");
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
        let _ = Command::new("mdev").arg("-s").status();
    }
    log::info!("data: storage search took {} ms", start.elapsed().as_millis());
    if iyakki_storage::find_data(&roots).is_none() && iyakki_storage::find_pending(&roots).is_none() {
        for h in iyakki_storage::gpt_hints(&roots) {
            log::warn!("data: the partition table of {} lists IYAKKI-DATA as partition {} ({}) but that device is missing or holds no IYAKKI volume", h.disk, h.number, h.part);
        }
    }
    // first boot after an install: the installer left a marked, unformatted data partition (and the choices made in the installer)
    let install_conf = iyakki_storage::find_pending(&roots).and_then(|d| iyakki_storage::read_pending_conf(&d.path));
    if iyakki_storage::find_data(&roots).is_none() {
        if iyakki_storage::find_pending(&roots).is_some() {
            // once, on the first start after an install: the boot screen says why this start takes a little longer
            progress(80, FIRST_START_FORMAT_STAGE);
        }
        let t_format = std::time::Instant::now();
        let mke2fs = ["/sbin/mke2fs", "/usr/sbin/mke2fs"].iter().map(std::path::PathBuf::from).find(|p| p.exists());
        match mke2fs.map(|m| iyakki_storage::format_pending(&roots, &m)) {
            Some(Ok(Some(dev))) => log::info!("data: first boot, formatted {dev} as the IYAKKI data volume in {} ms", t_format.elapsed().as_millis()),
            Some(Ok(None)) | None => {}
            Some(Err(e)) => log::error!("data: could not format the new data partition: {e}"),
        }
    }
    let t_mount = std::time::Instant::now();
    let mounted = iyakki_storage::mount_data(&roots, &iyakki_storage::SysMounter, Path::new(DATA_DIR));
    log::info!("data: mount took {} ms", t_mount.elapsed().as_millis());
    match mounted {
        Ok(Some(dev)) => {
            log::info!("data: persistent volume {dev} mounted at {DATA_DIR}");
            if let Some(conf) = install_conf {
                let p = Path::new(iyakki_zram::CONF_PATH);
                if !p.exists() {
                    match std::fs::write(p, conf) {
                        Ok(()) => log::info!("data: installer settings saved to {}", p.display()),
                        Err(e) => log::warn!("data: could not save the installer settings: {e}"),
                    }
                }
            }
            crate::console::out_write(&format!("IYAKKI-BOOT data: persistent volume {dev} mounted at {DATA_DIR}\n"));
        }
        Ok(None) => log::info!("data: no IYAKKI-DATA volume; this session is kept in RAM"),
        Err(e) => log::error!("data: {e}"),
    }
}

/// Is `dir` the mount point of something in this `/proc/mounts` text?
pub fn dir_is_mount_point(mounts: &str, dir: &str) -> bool {
    mounts.lines().any(|l| l.split_whitespace().nth(1) == Some(dir))
}

/// The boot screen's line while /home is set up on the data volume.
pub const HOME_STAGE: &str = "Setting up your home folder…";
/// The boot screen's line while the desktop is built, until its first picture.
pub const DESKTOP_STAGE: &str = "Starting your desktop…";

/// The boot screen's line while the data partition an install left is formatted (first start only).
pub const FIRST_START_FORMAT_STAGE: &str = "Formatting the data partition (first start only)…";

/// A USB mass-storage interface (class 08) is plugged in, whether or not its disk has appeared yet.
pub fn usb_storage_present(sys: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(sys.join("bus/usb/devices")) else { return false };
    rd.flatten().any(|e| std::fs::read_to_string(e.path().join("bInterfaceClass")).is_ok_and(|c| c.trim() == "08"))
}

/// Should boot go on waiting for the data disk? Never once it is found. A disk that is listed but whose partitions
/// have not appeared gets a few seconds; a USB storage device whose disk has not appeared yet gets longer; with
/// neither there is nothing to wait for.
pub fn keep_waiting_for_storage(elapsed: std::time::Duration, found: bool, has_disk: bool, usb_storage: bool) -> bool {
    if found {
        return false;
    }
    let limit = if has_disk { 4 } else if usb_storage { 15 } else { 0 };
    elapsed < std::time::Duration::from_secs(limit)
}

/// Kernel command line parameter lookup: `key=value` or bare `key` (value "").
pub fn cmdline_param<'a>(cmdline: &'a str, key: &str) -> Option<&'a str> {
    cmdline.split_whitespace().find_map(|t| match t.split_once('=') {
        Some((k, v)) if k == key => Some(v),
        None if t == key => Some(""),
        _ => None,
    })
}

/// Flush disks and restart the machine.
#[cfg(not(target_os = "linux"))]
pub fn reboot() {
    eprintln!("reboot is not available on this OS (IYAKKI runs as a program here, not as the init system)");
}

#[cfg(target_os = "linux")]
pub fn reboot() {
    quiet_sound();
    clock_floor_save();
    crate::crashlog::finish_clean(Path::new(DATA_DIR));
    // SAFETY: as for `power_off`.
    unsafe {
        libc::sync();
        libc::reboot(libc::LINUX_REBOOT_CMD_RESTART);
    }
}

/// Flush disks and power the machine off (never returns on success).
#[cfg(not(target_os = "linux"))]
pub fn power_off() {
    eprintln!("power off is not available on this OS (IYAKKI runs as a program here, not as the init system)");
}

#[cfg(target_os = "linux")]
pub fn power_off() {
    quiet_sound();
    clock_floor_save();
    crate::crashlog::finish_clean(Path::new(DATA_DIR));
    // SAFETY: sync/reboot take no pointers; reboot(2) with POWER_OFF is the documented shutdown path.
    unsafe {
        libc::sync();
        libc::reboot(libc::LINUX_REBOOT_CMD_POWER_OFF);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_scan_reaches_a_t2_macs_keyboard_ten_levels_down() {
        let root = std::env::temp_dir().join(format!("depth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dev = root.join("pci0000:00/0000:00:1c.4/0000:e6:00.1/apple-bce/apple-bce/bce-vhci/usb5/5-5/5-5:1.1/0003:05AC:027E.0007");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("modalias"), "hid:b0003g0001v000005ACp0000027E\n").unwrap();
        std::fs::write(dev.parent().unwrap().join("modalias"), "usb:v05ACp027Ed0933dc00dsc00dp00ic03isc01ip01in01\n").unwrap();
        let found = crate::boot::collect_modaliases(&root);
        assert!(found.iter().any(|a| a.starts_with("hid:b0003g0001v000005ACp0000027E")), "{found:?}");
        assert!(found.iter().any(|a| a.starts_with("usb:v05ACp027E")), "{found:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_mode_is_read_from_the_command_line() {
        assert!(super::install_mode("console=tty0 quiet iyakki.install=1"));
        assert!(!super::install_mode("console=tty0 quiet"));
        assert!(!super::install_mode("iyakki.install=0"));
        assert!(!super::install_mode("iyakki.installer=1"));
    }

    #[test]
    fn ssh_login_shells_get_the_tools_folder_and_the_cluster_kubeconfig() {
        let p = super::tools_profile("/var/lib/iyakki");
        assert!(p.contains("export PATH=\"/var/lib/iyakki/cli/bin:$PATH\""));
        assert!(p.contains("export KUBECONFIG=/var/lib/iyakki/k3s-dev/kubeconfig") && p.contains("alias kubectl='k3s kubectl'"));
    }

    #[test]
    fn ssh_is_keys_only_and_finds_the_address() {
        let c = super::ssh_server_command(None);
        assert_eq!(c[0], "dropbear");
        assert!(c.contains(&"-s".to_string()), "passwords are disabled");
        assert!(c.contains(&"-R".to_string()), "in RAM, host keys are made on demand");
        let k = super::ssh_server_command(Some(super::SSH_HOST_KEY));
        assert!(k.contains(&"-s".to_string()) && !k.contains(&"-R".to_string()));
        assert_eq!(&k[k.len() - 2..], &["-r".to_string(), super::SSH_HOST_KEY.to_string()], "the kept key is used");
        let out = "2: eth0    inet 192.168.1.42/24 brd 192.168.1.255 scope global eth0\n";
        assert_eq!(super::first_ipv4(out).as_deref(), Some("192.168.1.42"));
        assert_eq!(super::first_ipv4(""), None);
    }

    #[test]
    fn the_ssh_host_key_is_kept_on_the_data_volume_and_made_only_once() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("iyakki-hostkey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let key = d.join("ssh/dropbear_ed25519_host_key");
        let mut made = 0;
        let mut make = |p: &Path| {
            made += 1;
            std::fs::write(p, b"KEY").is_ok()
        };
        assert_eq!(super::persistent_host_key(&key, false, &mut make), None, "data in RAM: nothing is kept");
        assert_eq!(super::persistent_host_key(&key, true, &mut make).as_deref(), Some(key.as_path()), "first boot on a disk: made");
        assert_eq!(super::persistent_host_key(&key, true, &mut make).as_deref(), Some(key.as_path()), "next boot: the same file");
        assert_eq!(made, 1, "made once, then reused");
        assert_eq!(std::fs::metadata(&key).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(key.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        // an empty file from an interrupted first try is made again; a tool that fails gives no key (a fresh one per boot)
        std::fs::write(&key, b"").unwrap();
        assert!(super::persistent_host_key(&key, true, &mut |p: &Path| std::fs::write(p, b"K2").is_ok()).is_some());
        std::fs::remove_file(&key).unwrap();
        assert_eq!(super::persistent_host_key(&key, true, &mut |_: &Path| false), None);
        assert_eq!(super::ssh_host_key_command("/k"), vec!["dropbearkey", "-t", "ed25519", "-f", "/k"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_boot_screen_minimum_time_is_read_and_capped() {
        use std::time::Duration;
        assert_eq!(super::splash_minimum("quiet iyakki.splash=3"), Duration::from_secs(3));
        assert_eq!(super::splash_minimum("quiet"), Duration::ZERO);
        assert_eq!(super::splash_minimum("iyakki.splash=abc"), Duration::ZERO);
        assert_eq!(super::splash_minimum("iyakki.splash=9999"), Duration::from_secs(30), "never holds boot for long");
    }

    #[test]
    fn boot_waits_for_a_usb_disk_that_has_not_appeared_yet_but_not_for_nothing() {
        use std::time::Duration as D;
        let w = super::keep_waiting_for_storage;
        assert!(!w(D::ZERO, true, true, true), "found: never wait");
        assert!(w(D::from_secs(5), false, false, true), "usb storage plugged in, disk not listed yet: keep waiting");
        assert!(!w(D::from_secs(16), false, false, true), "but not for ever");
        assert!(w(D::from_secs(2), false, true, true), "disk listed, partitions still coming");
        assert!(!w(D::from_secs(5), false, true, true), "disk listed for a while and no data volume: it is just not an IYAKKI disk");
        assert!(!w(D::ZERO, false, false, false), "no disk and no usb storage: nothing to wait for");
    }

    #[test]
    fn usb_mass_storage_is_recognised_from_sysfs() {
        let d = std::env::temp_dir().join(format!("iyakki-usbs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let i = d.join("bus/usb/devices/1-1:1.0");
        std::fs::create_dir_all(&i).unwrap();
        std::fs::write(i.join("bInterfaceClass"), "03\n").unwrap();
        assert!(!super::usb_storage_present(&d), "a keyboard is not storage");
        std::fs::write(i.join("bInterfaceClass"), "08\n").unwrap();
        assert!(super::usb_storage_present(&d));
        assert!(!super::usb_storage_present(&d.join("nowhere")));
    }

    #[test]
    fn a_t2_macs_wifi_and_bluetooth_drivers_are_held_before_the_first_coldplug_and_loaded_by_the_packs_later() {
        let src = include_str!("boot.rs");
        let early = src.split("pub fn early_boot_critical").nth(1).unwrap();
        assert!(early.find("t2hold::apply").unwrap() < early.find("load_aliases(&critical").unwrap(), "held before the hardware drivers load");
        let late = src.split("pub fn late_boot").nth(1).unwrap();
        assert!(late.find("apply_boot").unwrap() < late.find("load_aliases(&plan.deferred").unwrap(), "the packs load their drivers before the deferred coldplug");
    }

    #[test]
    fn the_cs8409_check_runs_after_the_deferred_coldplug_loaded_the_sound_drivers() {
        let src = include_str!("boot.rs");
        let late = src.split("pub fn late_boot").nth(1).unwrap();
        assert!(late.find("load_aliases(&plan.deferred").unwrap() < late.find("cs8409_sound()").unwrap(), "after the hdaudio aliases had their chance");
        assert!(late.find("cs8409_sound()").unwrap() < late.find("net::bring_up").unwrap());
    }

    #[test]
    fn the_coldplug_scans_again_after_each_batch_and_the_apple_spi_modules_follow_the_first_one() {
        let src = include_str!("boot.rs");
        let early = src.split("pub fn early_boot_critical").nth(1).unwrap().split("pub fn late_boot").next().unwrap();
        let first = early.find("load_aliases(&critical").unwrap();
        let again = early.find("coldplug_settle(&mut seen, &mut deferred)").expect("second pass in the critical path");
        let spi = early.find("apple_spi_keyboard()").unwrap();
        assert!(first < again && again < spi, "scan again after the first batch, then the by-name list");
        assert!(early.find("seen.add_all(&deferred)").unwrap() < again, "the deferred aliases count as seen: no double load");
        let late = src.split("pub fn late_boot").nth(1).unwrap();
        assert!(late.find("load_aliases(&plan.deferred").unwrap() < late.find("coldplug_settle(&mut plan.seen").unwrap());
        assert!(late.find("coldplug_settle(&mut plan.seen").unwrap() < late.find("cs8409_sound()").unwrap());
    }

    #[test]
    fn the_firewall_is_applied_first_in_late_boot_and_again_once_the_network_is_up() {
        let src = include_str!("boot.rs");
        let late = src.split("pub fn late_boot").nth(1).unwrap().split("pub const SSH_KEYS_SRC").next().unwrap();
        let fw = late.find("firewall_boot()").expect("late boot starts the firewall");
        let net = late.find("net::bring_up").expect("and brings the network up");
        let ssh = late.find("start_remote_diagnostics()").unwrap();
        assert!(fw < net && fw < ssh, "before the network is configured and before SSH listens");
        assert!(late.find("iyakki_fw::service::network_changed()").unwrap() > net, "applied again after the network is up");
        assert!(late.find("mount_data_volume").is_none(), "the settings come from the data volume, which early boot has mounted");
        let early = src.split("pub fn early_boot_critical").nth(1).unwrap().split("static KERNEL_SECS").next().unwrap();
        assert!(early.find("mount_data_volume()").is_some(), "the data volume (firewall.json) is there before late boot starts");
    }

    #[test]
    fn the_boot_timeline_reads_as_one_line() {
        let mut t = super::Timeline::new();
        t.marks.push(("mounts", 20));
        t.marks.push(("drivers", 800));
        assert_eq!(t.summary(Some(2.14)), "boot: kernel 2.1 s, then iyakki took 820 ms (mounts 20 ms, drivers 800 ms)");
        assert_eq!(t.summary(None), "boot: iyakki took 820 ms (mounts 20 ms, drivers 800 ms)");
    }

    #[test]
    fn only_network_sound_wifi_and_bluetooth_wait_until_after_login() {
        use super::is_deferred as d;
        // deferred: Ethernet and WiFi PCI cards, audio, a Bluetooth dongle, USB audio, SDIO radios
        assert!(d("pci:v00008086d000015A3sv00001043sd00008672bc02sc00i00"), "Ethernet controller");
        assert!(d("pci:v000014E4d000043A0sv0000106Bsd0000009Ebc02sc80i00"), "WiFi (network controller, other)");
        assert!(d("pci:v00008086d00009C20sv00001043sd00001B2Dbc04sc03i00"), "HD audio");
        assert!(d("usb:v0A5Cp21E6d0112dcE0dsc01dp01icE0isc01ip01in00"), "Bluetooth dongle");
        assert!(d("usb:v0D8Cp0014d0100dc00dsc00dp00ic01isc01ip00in00"), "USB audio");
        assert!(d("hdaudio:v10EC0892r00100302a01"));
        // critical: display, SATA/NVMe storage, USB hub, keyboard/mouse, USB storage, virtio disk, platform devices
        assert!(!d("pci:v00008086d00000A2Esv0000106Bsd00000130bc03sc00i00"), "display controller");
        assert!(!d("pci:v0000144Dd0000A808sv0000144Dsd0000A801bc01sc08i02"), "NVMe");
        assert!(!d("pci:v00008086d00009C03sv00001043sd00001B2Dbc01sc06i01"), "SATA");
        assert!(!d("usb:v0000p0000d0000dc09dsc00dp01ic09isc00ip00in00"), "USB hub");
        assert!(!d("usb:v046Dp C31Cd0110dc00dsc00dp00ic03isc01ip01in00".replace(' ', "").as_str()), "USB keyboard");
        assert!(!d("usb:v058Fp6387d0103dc00dsc00dp00ic08isc06ip50in00"), "USB storage");
        assert!(!d("virtio:d00000002v00001AF4"), "virtio disk");
        assert!(!d("acpi:PNP0303:"), "PS/2 keyboard");
        assert!(!d("platform:simple-framebuffer"));
        assert!(!d("something:we-have-never-seen"), "unknown classes stay on the critical path");
    }

    #[test]
    fn memory_is_set_up_after_the_data_volume_and_before_the_network() {
        let src = include_str!("boot.rs");
        let body = src.split("pub fn early_boot_critical").nth(1).unwrap().split("static KERNEL_SECS").next().unwrap();
        let (data, mem) = (body.find("mount_data_volume();").unwrap(), body.find("memory_setup();").unwrap());
        assert!(data < mem, "system.conf lives on the data volume");
        let late = src.split("pub fn late_boot").nth(1).unwrap();
        assert!(late.find("swapfile_setup();").unwrap() < late.find("net::bring_up").unwrap());
    }

    #[test]
    fn the_slow_steps_have_their_own_plain_boot_screen_text_in_rising_order() {
        let src = include_str!("boot.rs");
        let body = src.split("pub fn early_boot_critical").nth(1).unwrap().split("static KERNEL_SECS").next().unwrap();
        assert!(body.find("progress(86, HOME_STAGE)").unwrap() < body.find("bind_home_on_data();").unwrap(), "the home folder text comes before it is set up");
        assert!(HOME_STAGE.contains("home folder") && DESKTOP_STAGE.contains("desktop") && FIRST_START_FORMAT_STAGE.contains("Formatting"));
        // every progress number in the early part only goes up: 8, 68, 72, 78, 80, 86, 94, then 98 in the shell thread
        let all = src.split("#[cfg(test)]").next().unwrap();
        let nums: Vec<u32> = all.split("progress(").skip(1).filter_map(|r| r.split(',').next()?.trim().parse().ok()).collect();
        let mut order: Vec<u32> = nums.iter().copied().filter(|n| matches!(n, 8 | 68 | 72 | 86 | 94)).collect();
        let sorted = { let mut o = order.clone(); o.sort(); o };
        order.dedup();
        assert_eq!(order, sorted, "stage numbers rise: {nums:?}");
        assert!(include_str!("main.rs").contains("progress(98, iyakki_core::boot::DESKTOP_STAGE)"));
    }

    #[test]
    fn home_is_bound_after_the_data_volume_and_before_anything_else_uses_it() {
        let src = include_str!("boot.rs");
        let body = src.split("pub fn early_boot_critical").nth(1).unwrap().split("static KERNEL_SECS").next().unwrap();
        let (data, home, mem) = (body.find("mount_data_volume();").unwrap(), body.find("bind_home_on_data();").unwrap(), body.find("memory_setup();").unwrap());
        assert!(data < home && home < mem, "/home is bound once the volume is mounted, before the rest of boot and the shell");
        assert!(!src.split("pub fn late_boot").nth(1).unwrap().split("pub const SSH_KEYS_SRC").next().unwrap().contains("bind_home_on_data"), "not in the background part");
        assert!(!body.contains("\"/root\""), "/root (the image's SSH keys) is never bound over");
    }

    #[test]
    fn the_boot_screen_says_when_the_first_start_formats_the_data_partition() {
        let src = include_str!("boot.rs");
        let body = src.split("pub fn mount_data_volume").nth(1).unwrap().split("pub fn usb_storage_present").next().unwrap();
        let (stage, format) = (body.find("progress(80, FIRST_START_FORMAT_STAGE)").unwrap(), body.find("format_pending(").unwrap());
        assert!(stage < format, "the stage text is shown before mke2fs runs");
        assert!(FIRST_START_FORMAT_STAGE.contains("first start"));
        // and the splash is up by then: it starts right after the device nodes, before drivers and storage
        let early = src.split("pub fn early_boot_critical").nth(1).unwrap();
        assert!(early.find("start_boot_screen();").unwrap() < early.find("mount_data_volume();").unwrap());
    }

    #[test]
    fn a_data_volume_mounted_by_the_first_stage_is_recognised() {
        let m = "rootfs / rootfs rw 0 0\n/dev/sda2 /var/lib/iyakki ext4 rw,nosuid 0 0\n";
        assert!(dir_is_mount_point(m, "/var/lib/iyakki"));
        assert!(!dir_is_mount_point(m, "/var/lib"));
        assert!(!dir_is_mount_point("", DATA_DIR));
    }

    #[test]
    fn the_kernels_log_rate_limit_is_off_before_the_boot_lines_are_written() {
        // `printk.devkmsg=ratelimit` (the default) drops all but about 10 program-written log lines per 5 s: the boot timeline lost its lines
        let src = include_str!("boot.rs");
        let body = src.split("pub fn early_boot_critical").nth(1).unwrap();
        let off = body.find("/proc/sys/kernel/printk_devkmsg").expect("the limit is switched off");
        assert!(off < body.find("clock_floor_early();").unwrap() && body[..off].contains("mount_pseudo_filesystems();"), "right after /proc is mounted, before the first line");
        // and a data volume mounted by the first stage is kept private like one this code mounts
        assert!(src.split("pub fn mount_data_volume").nth(1).unwrap().split("let roots = iyakki_storage::Roots::real();").next().unwrap().contains("from_mode(0o700)"));
    }

    #[test]
    fn crash_forensics_start_after_the_data_mount_and_the_power_paths_mark_the_end() {
        let boot = include_str!("boot.rs");
        let early = boot.split("pub fn early_boot_critical").nth(1).unwrap().split("static KERNEL_SECS").next().unwrap();
        assert!(early.find("mount_data_volume();").unwrap() < early.find("crate::crashlog::start(").unwrap(), "after the data mount");
        assert!(early.find("crate::crashlog::start(").unwrap() < early.find("crate::storageloss::start();").unwrap());
        for f in ["pub fn reboot", "pub fn power_off"] {
            let body = boot.split(f).nth(1).unwrap().split("\n}\n").next().unwrap();
            assert!(body.contains("crashlog::finish_clean(") || body.contains("eprintln"), "{f} marks the session clean before it syncs");
        }
    }

    #[test]
    fn cmdline_lookup() {
        let c = "console=ttyS0 iyakki.selftest=1 quiet iyakki.env=microvm";
        assert_eq!(super::cmdline_param(c, "iyakki.selftest"), Some("1"));
        assert_eq!(super::cmdline_param(c, "quiet"), Some(""));
        assert_eq!(super::cmdline_param(c, "iyakki.env"), Some("microvm"));
        assert_eq!(super::cmdline_param(c, "nope"), None);
    }

    use super::*;

    #[test]
    fn finds_modaliases_and_skips_symlinks() {
        let d = std::env::temp_dir().join(format!("iyakki-boot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("pci0000:00/0000:00:14.3")).unwrap();
        std::fs::write(d.join("pci0000:00/0000:00:14.3/modalias"), "pci:v00008086d00007AF0\n").unwrap();
        std::fs::write(d.join("pci0000:00/modalias"), "pci:v00008086d00007AF0\n").unwrap(); // dup
        std::fs::create_dir_all(d.join("loop")).unwrap();
        std::os::unix::fs::symlink(&d, d.join("loop/self")).unwrap();
        assert_eq!(collect_modaliases(&d), vec!["pci:v00008086d00007AF0"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn mount_table_sane() {
        assert!(MOUNTS.iter().any(|m| m.1 == "/proc") && MOUNTS.iter().any(|m| m.1 == "/dev"));
        // /dev must be mounted before /dev/pts
        let pos = |t: &str| MOUNTS.iter().position(|m| m.1 == t).unwrap();
        assert!(pos("/dev") < pos("/dev/pts") && pos("/sys") < pos("/sys/fs/cgroup"));
    }
}
