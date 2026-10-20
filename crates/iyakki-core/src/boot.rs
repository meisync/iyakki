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
pub fn collect_modaliases(sys_devices: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>, depth: u32) {
        if depth > 8 {
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

/// Show the boot screen on the framebuffer the kernel started with, until [`finish_boot_screen`].
#[cfg(feature = "shell")]
fn start_boot_screen() {
    if cmdline_param(&std::fs::read_to_string("/proc/cmdline").unwrap_or_default(), "iyakki.shell") == Some("0") {
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

/// Load drivers for all present hardware, in one go.
pub fn coldplug() -> usize {
    let aliases = collect_modaliases(Path::new("/sys/devices"));
    load_aliases(&aliases, "Starting hardware drivers", 25, 70);
    aliases.len()
}

/// Mandatory "always load" modules that have no modalias (crypto for WiFi/WPA, RNG, etc).
pub const ALWAYS_LOAD: &[&str] = &["af_packet", "cfg80211", "mac80211", "ccm", "gcm", "cmac", "arc4", "ctr", "sha256_generic", "tun", "vfat", "nls_cp437", "nls_iso8859_1"];
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
    tl.mark("mounts");
    // Before anything can load the camera or USB sound driver by alias (mdev below, the coldplug): IYAKKI loads those
    // itself, so a camera that needs a uvcvideo quirk gets it on its first probe (see camera.rs).
    crate::camera::block_autoload();
    // Hotplug: let the kernel call mdev for new devices (creates nodes, loads modules).
    let _ = std::fs::write("/proc/sys/kernel/hotplug", "/sbin/mdev\n");
    let _ = Command::new("mdev").arg("-s").status();
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
    tl.mark("drivers");
    progress(68, "Setting up devices");
    let _ = Command::new("mdev").arg("-s").status(); // nodes for devices whose drivers just loaded
    progress(72, "Opening your storage");
    mount_data_volume();
    progress(86, HOME_STAGE);
    bind_home_on_data();
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
    LatePlan { deferred, tl: Timeline::new() }
}

static KERNEL_SECS: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();

/// The rest of boot: drivers for network, sound and Bluetooth, the network itself, remote diagnostics.
pub fn late_boot(mut plan: LatePlan) {
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
    let _ = Command::new("mdev").arg("-s").status();
    crate::camera::start();
    plan.tl.mark("late drivers");
    match crate::net::bring_up(Path::new("/etc/iyakki/net.conf"), Path::new("/sys/class/net")) {
        Ok(errs) => errs.iter().for_each(|e| log::warn!("net: {e}")),
        Err(e) => log::error!("net config: {e}"),
    }
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    if cmdline_param(&cmdline, "iyakki.ssh") == Some("1") {
        start_remote_diagnostics();
    }
    plan.tl.mark("network");
    log::info!("{}", plan.tl.summary(None).replace("boot: iyakki took", "boot (after login): iyakki took"));
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
    // SAFETY: sync/reboot take no pointers; reboot(2) with POWER_OFF is the documented shutdown path.
    unsafe {
        libc::sync();
        libc::reboot(libc::LINUX_REBOOT_CMD_POWER_OFF);
    }
}

#[cfg(test)]
mod tests {
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
