//! Early userspace for running as PID 1 on bare metal: mount the pseudo filesystems,
//! populate /dev, load drivers for the hardware that is present (coldplug), set up
//! hotplug, and bring up networking. Everything is best-effort and logged — a failed
//! step must never panic PID 1 (a kernel panic would follow).

use std::ffi::CString;
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

fn mount(src: &str, target: &str, fstype: &str, flags: u64, data: &str) -> Result<(), String> {
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
        // a GPU driver that is a module only creates the screen after it loads: wait for it (boot may finish first)
        while !Path::new("/dev/fb0").exists() && !iyakki_shell::splash::is_done() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if iyakki_shell::splash::is_done() {
            iyakki_shell::splash::mark_stopped();
            return;
        }
        let r = std::panic::catch_unwind(|| iyakki_shell::backend::run_splash("/dev/fb0"));
        if !matches!(r, Ok(Ok(()))) {
            log::warn!("splash: stopped early: {r:?}");
            iyakki_shell::splash::mark_stopped();
        }
    });
    if spawn.is_err() {
        iyakki_shell::splash::mark_stopped();
    }
}
#[cfg(not(feature = "shell"))]
fn start_boot_screen() {}

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
    // Hotplug: let the kernel call mdev for new devices (creates nodes, loads modules).
    let _ = std::fs::write("/proc/sys/kernel/hotplug", "/sbin/mdev\n");
    let _ = Command::new("mdev").arg("-s").status();
    tl.mark("device nodes");
    start_boot_screen();
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
    tl.mark("storage");
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            // SAFETY: pointer/len describe a valid byte slice.
            unsafe { libc::sethostname(h.as_ptr().cast(), h.len()) };
        }
    }
    progress(94, "Almost ready");
    log::info!("{}", tl.summary(kernel_secs).replace("boot:", "boot (to login):"));
    LatePlan { deferred, tl: Timeline::new() }
}

static KERNEL_SECS: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();

/// The rest of boot: drivers for network, sound and Bluetooth, the network itself, remote diagnostics.
pub fn late_boot(mut plan: LatePlan) {
    for m in ALWAYS_LOAD.iter().filter(|m| !LOGIN_LOAD.contains(m)) {
        let _ = Command::new("modprobe").arg("-b").arg(m).stderr(std::process::Stdio::null()).status();
    }
    load_aliases(&plan.deferred, "Starting network and sound", 70, 90);
    let _ = Command::new("mdev").arg("-s").status();
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

/// The SSH server command for remote diagnostics: keys only (no passwords), host keys made on demand.
pub fn ssh_server_command() -> Vec<String> {
    ["dropbear", "-R", "-s", "-E", "-p", "22"].iter().map(|s| s.to_string()).collect()
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
    let cmd = ssh_server_command();
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

pub const DATA_DIR: &str = "/var/lib/iyakki";

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
        if !keep_waiting_for_storage(start.elapsed(), found, has_disk, usb) {
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
    // first boot after an install: the installer left a marked, unformatted data partition
    if iyakki_storage::find_data(&roots).is_none() {
        let mke2fs = ["/sbin/mke2fs", "/usr/sbin/mke2fs"].iter().map(std::path::PathBuf::from).find(|p| p.exists());
        match mke2fs.map(|m| iyakki_storage::format_pending(&roots, &m)) {
            Some(Ok(Some(dev))) => log::info!("data: first boot, formatted {dev} as the IYAKKI data volume"),
            Some(Ok(None)) | None => {}
            Some(Err(e)) => log::error!("data: could not format the new data partition: {e}"),
        }
    }
    match iyakki_storage::mount_data(&roots, &iyakki_storage::SysMounter, Path::new(DATA_DIR)) {
        Ok(Some(dev)) => {
            log::info!("data: persistent volume {dev} mounted at {DATA_DIR}");
            crate::console::out_write(&format!("IYAKKI-BOOT data: persistent volume {dev} mounted at {DATA_DIR}\n"));
        }
        Ok(None) => log::info!("data: no IYAKKI-DATA volume; this session is kept in RAM"),
        Err(e) => log::error!("data: {e}"),
    }
}

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
pub fn reboot() {
    // SAFETY: as for `power_off`.
    unsafe {
        libc::sync();
        libc::reboot(libc::LINUX_REBOOT_CMD_RESTART);
    }
}

/// Flush disks and power the machine off (never returns on success).
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
    fn ssh_is_keys_only_and_finds_the_address() {
        let c = super::ssh_server_command();
        assert_eq!(c[0], "dropbear");
        assert!(c.contains(&"-s".to_string()), "passwords are disabled");
        assert!(c.contains(&"-R".to_string()), "host keys are made on demand");
        let out = "2: eth0    inet 192.168.1.42/24 brd 192.168.1.255 scope global eth0\n";
        assert_eq!(super::first_ipv4(out).as_deref(), Some("192.168.1.42"));
        assert_eq!(super::first_ipv4(""), None);
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
