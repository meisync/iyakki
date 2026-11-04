//! Monitor detection at boot. Some monitors are not seen when the GPU driver first looks: on a Mac mini 2018 the HDMI port
//! (`DP-3` behind an on-board DP->HDMI converter) was reported "disconnected" after `i915` loaded, every pipe was off and the
//! screen stayed black until the cable was pulled and plugged in again. A monitor that wakes up slowly does the same.
//!
//! Writing `detect` to `/sys/class/drm/cardN-<connector>/status` makes the kernel probe that connector again; when it is
//! found the kernel's framebuffer emulation sets a mode on it (a DRM hotplug event) and the boot screen redraws on it.
//! Only connectors that are NOT connected are probed: probing a working one could, with a flaky converter, turn it off.
//!
//! Everything runs in a background thread and never holds up boot. The rules are plain functions over a sysfs folder, so
//! the tests use a fake one.

use std::path::Path;
use std::time::Duration;

/// Drivers that only wrap the firmware's screen: their connector is always "connected" and says nothing about a monitor.
const FIRMWARE_DRIVERS: [&str; 7] = ["simple-framebuffer", "simpledrm", "efi-framebuffer", "efidrm", "vesa-framebuffer", "vesadrm", "ofdrm"];

/// The driver bound to a DRM card (`cardN/device/driver` link, else `DRIVER=` in `cardN/device/uevent`).
fn card_driver(sys_drm: &Path, card: &str) -> String {
    let dev = sys_drm.join(card).join("device");
    if let Ok(t) = std::fs::read_link(dev.join("driver")) {
        if let Some(n) = t.file_name() {
            return n.to_string_lossy().to_string();
        }
    }
    std::fs::read_to_string(dev.join("uevent")).unwrap_or_default().lines().find_map(|l| l.strip_prefix("DRIVER=")).unwrap_or("").trim().to_string()
}

/// One connector of a real GPU driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connector {
    /// The sysfs name, `card0-DP-3`.
    pub sysfs: String,
    /// `connected`, `disconnected` or `unknown`.
    pub status: String,
}

impl Connector {
    /// The name people know, `DP-3`.
    pub fn short(&self) -> &str {
        self.sysfs.split_once('-').map_or(self.sysfs.as_str(), |(_, n)| n)
    }
    pub fn connected(&self) -> bool {
        self.status == "connected"
    }
}

/// The connectors of GPU drivers (firmware screens such as simpledrm left out), sorted by name.
pub fn connectors(sys_drm: &Path) -> Vec<Connector> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(sys_drm) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some((card, _)) = name.split_once('-') else { continue };
        if !card.strip_prefix("card").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
            continue;
        }
        let Ok(status) = std::fs::read_to_string(e.path().join("status")) else { continue };
        if FIRMWARE_DRIVERS.contains(&card_driver(sys_drm, card).as_str()) {
            continue;
        }
        out.push(Connector { sysfs: name, status: status.trim().to_string() });
    }
    out.sort_by(|a, b| a.sysfs.cmp(&b.sysfs));
    out
}

/// Ask the kernel to look again at every connector that is not connected. Returns how many were asked.
pub fn probe_disconnected(sys_drm: &Path) -> usize {
    connectors(sys_drm).iter().filter(|c| !c.connected()).filter(|c| std::fs::write(sys_drm.join(&c.sysfs).join("status"), "detect").is_ok()).count()
}

/// How the boot-time search ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No GPU driver with connectors appeared (a VM with a firmware screen only, `nomodeset`, safe graphics).
    NoGpu,
    /// A monitor was there without help.
    Connected(Vec<String>),
    /// Found after this many probe rounds.
    ConnectedAfter(Vec<String>, u32),
    /// Still nothing after this many rounds.
    GaveUp(u32),
}

impl Outcome {
    /// The line for the system log.
    pub fn describe(&self) -> String {
        match self {
            Outcome::NoGpu => "display: no GPU driver with connectors appeared; nothing to probe".into(),
            Outcome::Connected(n) => format!("display: {} connected", n.join(", ")),
            Outcome::ConnectedAfter(n, k) => format!("display: {} connected after {k} probe{}", n.join(", "), if *k == 1 { "" } else { "s" }),
            Outcome::GaveUp(k) => format!("display: no monitor detected after {k} probes; it is picked up when it is plugged in or wakes up"),
        }
    }
}

/// The timing of the boot-time search.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    /// How long to wait for a GPU driver to bind at all.
    pub wait_for_gpu: Duration,
    /// After it binds: how long the kernel gets to find the monitor itself.
    pub grace: Duration,
    /// Between probe rounds.
    pub every: Duration,
    /// Probing stops after this long.
    pub limit: Duration,
}

pub const BOOT_PLAN: Plan = Plan { wait_for_gpu: Duration::from_secs(30), grace: Duration::from_secs(1), every: Duration::from_millis(500), limit: Duration::from_secs(20) };

fn connected_names(sys_drm: &Path) -> Vec<String> {
    connectors(sys_drm).iter().filter(|c| c.connected()).map(|c| c.short().to_string()).collect()
}

/// Wait for a GPU driver, give the kernel `grace` to find a monitor, then probe the connectors that are not connected every
/// `every` until one connects or `limit` has passed. `wait` sleeps (the tests pass one that only counts) and `stop` ends the
/// search early (boot is shutting down).
pub fn search(sys_drm: &Path, plan: Plan, wait: &mut dyn FnMut(Duration), stop: &dyn Fn() -> bool) -> Outcome {
    let step = Duration::from_millis(100);
    let mut waited = Duration::ZERO;
    while connectors(sys_drm).is_empty() {
        if waited >= plan.wait_for_gpu || stop() {
            return Outcome::NoGpu;
        }
        wait(step);
        waited += step;
    }
    let mut waited = Duration::ZERO;
    while waited < plan.grace {
        let n = connected_names(sys_drm);
        if !n.is_empty() {
            return Outcome::Connected(n);
        }
        wait(step);
        waited += step;
    }
    let mut probes = 0u32;
    let mut spent = Duration::ZERO;
    loop {
        let n = connected_names(sys_drm);
        if !n.is_empty() {
            return if probes == 0 { Outcome::Connected(n) } else { Outcome::ConnectedAfter(n, probes) };
        }
        if spent >= plan.limit || stop() {
            return Outcome::GaveUp(probes);
        }
        probe_disconnected(sys_drm);
        probes += 1;
        wait(plan.every);
        spent += plan.every;
    }
}

/// Is this kernel event (a `NETLINK_KOBJECT_UEVENT` message: `change@/devices/...\0ACTION=change\0SUBSYSTEM=drm\0...`) a
/// DRM change, which is what a monitor being plugged in or waking up produces?
pub fn is_drm_change(msg: &[u8]) -> bool {
    let fields: Vec<&[u8]> = msg.split(|b| *b == 0).collect();
    fields.contains(&b"ACTION=change".as_slice()) && fields.contains(&b"SUBSYSTEM=drm".as_slice())
}

/// Rate limit for probes caused by events: our own probe produces an event when it finds something, which must not loop.
pub fn may_probe_again(since_last: Option<Duration>) -> bool {
    since_last.is_none_or(|d| d >= Duration::from_secs(2))
}

/// One probe round now (the shell starting, a DRM event), in its own thread so the caller never waits for a slow
/// connector. `found` runs when a connector that was not connected before is connected afterwards.
pub fn probe_once_in_background(why: &'static str, found: fn()) {
    let _ = std::thread::Builder::new().name("iyakki-display-probe".into()).spawn(move || probe_once(Path::new(SYS_DRM), why, found));
}

/// Whether a round of probes also runs while a monitor is already connected (`iyakki.touchscreen=` on the kernel command line).
///
/// The probe makes the kernel send a hotplug event and the console set the mode a second time, which on some machines is a full mode
/// set with the screen dark for a second or more. It is also what appears to wake a USB-C touch monitor (a Mac mini, the panel
/// 27c0:0859 behind a hub), whose touch stays dead after a boot until the cable is pulled and put back (KNOWN-ISSUES.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeMode {
    /// Probe with a monitor connected when a touch screen is present (the default).
    Auto,
    /// `iyakki.touchscreen=probe`: always probe, touch screen or not.
    Always,
    /// `iyakki.touchscreen=off`: never probe while a monitor is connected.
    Never,
}

impl ProbeMode {
    /// The mode named by the kernel command line.
    pub fn from_cmdline(cmdline: &str) -> ProbeMode {
        match crate::boot::cmdline_param(cmdline, "iyakki.touchscreen") {
            Some("probe") => ProbeMode::Always,
            Some("off") => ProbeMode::Never,
            _ => ProbeMode::Auto,
        }
    }

    fn wanted(self, touch_screen_present: bool) -> bool {
        match self {
            ProbeMode::Always => true,
            ProbeMode::Never => false,
            ProbeMode::Auto => touch_screen_present,
        }
    }
}

static PROBE_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn set_probe_mode(m: ProbeMode) {
    PROBE_MODE.store(m as u8, std::sync::atomic::Ordering::Relaxed);
}

fn probe_mode() -> ProbeMode {
    match PROBE_MODE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => ProbeMode::Always,
        2 => ProbeMode::Never,
        _ => ProbeMode::Auto,
    }
}

/// Is a touch screen among the input devices (a name with "touchscreen" in it, any case)? Reads `/proc/bus/input/devices`.
fn touch_screen_present() -> bool {
    std::fs::read_to_string("/proc/bus/input/devices").is_ok_and(|t| input_list_has_touch_screen(&t))
}

fn input_list_has_touch_screen(list: &str) -> bool {
    list.lines().any(|l| l.starts_with("N: Name=") && l.to_ascii_lowercase().contains("touchscreen"))
}

/// See [`probe_once_in_background`]. Returns the connectors that became connected.
pub fn probe_once(sys_drm: &Path, why: &str, found: fn()) -> Vec<String> {
    probe_once_with(sys_drm, why, found, probe_mode(), touch_screen_present)
}

fn probe_once_with(sys_drm: &Path, why: &str, found: fn(), mode: ProbeMode, touch_screen_present: impl Fn() -> bool) -> Vec<String> {
    let before = connected_names(sys_drm);
    if !before.is_empty() && !mode.wanted(touch_screen_present()) {
        return Vec::new();
    }
    if probe_disconnected(sys_drm) == 0 {
        return Vec::new();
    }
    std::thread::sleep(Duration::from_millis(300));
    let new: Vec<String> = connected_names(sys_drm).into_iter().filter(|n| !before.contains(n)).collect();
    if !new.is_empty() {
        log::info!("display: {} connected after a probe ({why})", new.join(", "));
        found();
    }
    new
}

pub const SYS_DRM: &str = "/sys/class/drm";

/// The boot-time search in a background thread, then a watch on DRM events for the rest of the run. `found` runs whenever a
/// monitor turns up through our probing (the boot screen and the shell redraw everything on it).
pub fn start(found: fn()) {
    let spawned = std::thread::Builder::new().name("iyakki-display".into()).spawn(move || {
        let sys = Path::new(SYS_DRM);
        let mode = ProbeMode::from_cmdline(&std::fs::read_to_string("/proc/cmdline").unwrap_or_default());
        set_probe_mode(mode);
        if mode != ProbeMode::Auto {
            log::info!("display: iyakki.touchscreen sets the probe with a monitor connected to {mode:?}");
        }
        let r = search(sys, BOOT_PLAN, &mut std::thread::sleep, &|| false);
        log::info!("{}", r.describe());
        if matches!(r, Outcome::ConnectedAfter(..)) {
            found();
        }
        if r != Outcome::NoGpu {
            watch_events(found);
        }
    });
    if let Err(e) = spawned {
        log::warn!("display: could not start the monitor search ({e})");
    }
}

/// Listen for kernel DRM events and probe once (at most every 2 s) when one arrives, so a monitor that wakes late is seen.
#[cfg(target_os = "linux")]
fn watch_events(found: fn()) {
    // SAFETY: plain socket calls with constant arguments; the descriptor is checked and closed on every exit path.
    let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, libc::NETLINK_KOBJECT_UEVENT) };
    if fd < 0 {
        log::warn!("display: no kernel event socket ({}); late monitors are found only by the probes at boot", std::io::Error::last_os_error());
        return;
    }
    // SAFETY: an all-zero sockaddr_nl is valid; the fields set are plain integers.
    let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    addr.nl_family = libc::AF_NETLINK as u16;
    addr.nl_groups = 1; // the kernel's uevent group
    // SAFETY: `addr` is a valid sockaddr_nl for the duration of the call and its size is passed.
    if unsafe { libc::bind(fd, (&addr as *const libc::sockaddr_nl).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32) } < 0 {
        log::warn!("display: kernel event socket: {}", std::io::Error::last_os_error());
        // SAFETY: closing the descriptor opened above.
        unsafe { libc::close(fd) };
        return;
    }
    let mut last: Option<std::time::Instant> = None;
    let mut buf = vec![0u8; 8192];
    loop {
        let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd.
        if unsafe { libc::poll(&mut p, 1, 60_000) } <= 0 {
            continue;
        }
        // SAFETY: `buf` is writable for its length.
        let n = unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n <= 0 {
            continue;
        }
        if is_drm_change(&buf[..n as usize]) && may_probe_again(last.map(|t| t.elapsed())) {
            last = Some(std::time::Instant::now());
            probe_once(Path::new(SYS_DRM), "display event", found);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn watch_events(_found: fn()) {}

/// The connectors as one line for logs ("DP-3 disconnected, HDMI-A-1 connected").
pub fn summary(sys_drm: &Path) -> String {
    let c = connectors(sys_drm);
    if c.is_empty() {
        return "no GPU connectors".into();
    }
    c.iter().map(|c| format!("{} {}", c.short(), c.status)).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A fake /sys/class/drm: card0 driven by `driver`, with these connectors and statuses.
    fn fake(tag: &str, driver: &str, conns: &[(&str, &str)]) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-drm-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("card0/device")).unwrap();
        std::fs::write(d.join("card0/device/uevent"), format!("DRIVER={driver}\nPCI_ID=8086:3E9B\n")).unwrap();
        for (n, s) in conns {
            std::fs::create_dir_all(d.join(format!("card0-{n}"))).unwrap();
            std::fs::write(d.join(format!("card0-{n}/status")), format!("{s}\n")).unwrap();
        }
        d
    }

    fn status(d: &Path, n: &str) -> String {
        std::fs::read_to_string(d.join(format!("card0-{n}/status"))).unwrap().trim().to_string()
    }

    #[test]
    fn connectors_of_firmware_screens_are_ignored_and_names_are_short() {
        let d = fake("fw", "simple-framebuffer", &[("Unknown-1", "connected")]);
        assert!(connectors(&d).is_empty(), "simpledrm's connector is always 'connected' and says nothing");
        std::fs::write(d.join("card0/device/uevent"), "DRIVER=i915\n").unwrap();
        let c = connectors(&d);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].short(), "Unknown-1");
        assert_eq!(summary(&d), "Unknown-1 connected");
        assert_eq!(summary(&d.join("none")), "no GPU connectors");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn only_connectors_that_are_not_connected_are_probed() {
        let d = fake("only", "i915", &[("DP-1", "connected"), ("DP-3", "disconnected"), ("HDMI-A-1", "unknown")]);
        assert_eq!(probe_disconnected(&d), 2);
        assert_eq!(status(&d, "DP-1"), "connected", "a working screen is never touched");
        assert_eq!(status(&d, "DP-3"), "detect");
        assert_eq!(status(&d, "HDMI-A-1"), "detect");
        assert_eq!(connectors(&d).len(), 3);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The fake kernel: counts our `detect` writes on DP-3 and reports the monitor after `wake_after` of them.
    fn kernel(d: &Path, wake_after: Option<u32>, probes: &mut u32) {
        if status(d, "DP-3") == "detect" {
            *probes += 1;
            let s = if wake_after.is_some_and(|w| *probes >= w) { "connected" } else { "disconnected" };
            std::fs::write(d.join("card0-DP-3/status"), s).unwrap();
            if status(d, "HDMI-A-1") == "detect" {
                std::fs::write(d.join("card0-HDMI-A-1/status"), "disconnected").unwrap();
            }
        }
    }

    #[test]
    fn the_search_probes_every_half_second_and_stops_when_the_monitor_connects() {
        let d = fake("wake", "i915", &[("DP-3", "disconnected"), ("HDMI-A-1", "disconnected")]);
        let (mut probes, mut slept) = (0u32, Duration::ZERO);
        let r = search(&d, BOOT_PLAN, &mut |t| {
            slept += t;
            kernel(&d, Some(4), &mut probes);
        }, &|| false);
        assert_eq!(r, Outcome::ConnectedAfter(vec!["DP-3".into()], 4));
        assert_eq!(probes, 4, "the fourth probe found it and probing stopped");
        assert_eq!(r.describe(), "display: DP-3 connected after 4 probes");
        // one second of grace, then four rounds 500 ms apart
        assert_eq!(slept, Duration::from_millis(1000 + 4 * 500));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_search_gives_up_after_twenty_seconds_and_leaves_a_connected_monitor_alone() {
        let d = fake("never", "i915", &[("DP-3", "disconnected"), ("HDMI-A-1", "disconnected")]);
        let (mut probes, mut slept) = (0u32, Duration::ZERO);
        let r = search(&d, BOOT_PLAN, &mut |t| {
            slept += t;
            kernel(&d, None, &mut probes);
        }, &|| false);
        assert_eq!(r, Outcome::GaveUp(40));
        assert_eq!(probes, 40, "every 500 ms for 20 s");
        assert!(slept <= Duration::from_secs(22), "never longer than the limit plus the grace");
        assert!(r.describe().contains("no monitor detected after 40 probes"));
        // a monitor that is there from the start: no probe at all
        let d2 = fake("there", "i915", &[("DP-3", "connected"), ("HDMI-A-1", "disconnected")]);
        let mut probes2 = 0;
        let r = search(&d2, BOOT_PLAN, &mut |_| kernel(&d2, None, &mut probes2), &|| false);
        assert_eq!(r, Outcome::Connected(vec!["DP-3".into()]));
        assert_eq!(status(&d2, "HDMI-A-1"), "disconnected", "nothing was written");
        // found by the kernel during the grace second: also no probe
        let d3 = fake("grace", "i915", &[("DP-3", "disconnected")]);
        let mut n = 0;
        let r = search(&d3, BOOT_PLAN, &mut |_| {
            n += 1;
            if n == 3 {
                std::fs::write(d3.join("card0-DP-3/status"), "connected").unwrap();
            }
        }, &|| false);
        assert_eq!(r, Outcome::Connected(vec!["DP-3".into()]));
        for p in [d, d2, d3] {
            let _ = std::fs::remove_dir_all(p);
        }
    }

    #[test]
    fn without_a_gpu_driver_the_search_ends_and_stop_is_honoured() {
        let d = fake("nogpu", "simple-framebuffer", &[("Unknown-1", "connected")]);
        let mut slept = Duration::ZERO;
        assert_eq!(search(&d, BOOT_PLAN, &mut |t| slept += t, &|| false), Outcome::NoGpu);
        assert_eq!(slept, BOOT_PLAN.wait_for_gpu);
        assert_eq!(search(&d, BOOT_PLAN, &mut |_| {}, &|| true), Outcome::NoGpu, "stopping ends the wait at once");
        // the GPU driver binds while we wait (simpledrm replaced by i915)
        let mut n = 0;
        let r = search(&d, BOOT_PLAN, &mut |_| {
            n += 1;
            if n == 5 {
                std::fs::write(d.join("card0/device/uevent"), "DRIVER=i915\n").unwrap();
            }
        }, &|| false);
        assert_eq!(r, Outcome::Connected(vec!["Unknown-1".into()]));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn drm_change_events_are_recognised_and_rate_limited() {
        assert!(is_drm_change(b"change@/devices/pci0000:00/0000:00:02.0/drm/card0\0ACTION=change\0DEVPATH=/devices/pci0000:00/0000:00:02.0/drm/card0\0SUBSYSTEM=drm\0HOTPLUG=1\0SEQNUM=2042\0"));
        assert!(!is_drm_change(b"add@/devices/x\0ACTION=add\0SUBSYSTEM=drm\0"), "only changes");
        assert!(!is_drm_change(b"change@/devices/x\0ACTION=change\0SUBSYSTEM=power_supply\0"), "only DRM");
        assert!(!is_drm_change(b"libudev\0garbage"));
        assert!(may_probe_again(None));
        assert!(!may_probe_again(Some(Duration::from_millis(500))), "our own probe's event does not probe again");
        assert!(may_probe_again(Some(Duration::from_secs(3))));
    }

    #[test]
    fn a_single_probe_reports_only_newly_connected_monitors() {
        static FOUND: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        fn found() {
            FOUND.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        let d = fake("once", "i915", &[("DP-1", "connected")]);
        assert!(probe_once(&d, "test", found).is_empty(), "nothing to probe: returns at once");
        assert_eq!(FOUND.load(std::sync::atomic::Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_probe_with_a_monitor_connected_follows_the_touch_screen_option() {
        fn found() {}
        let lit = |tag: &str| fake(tag, "i915", &[("DP-2", "connected"), ("HDMI-A-1", "disconnected"), ("DP-3", "disconnected")]);
        // iyakki.touchscreen=probe: always, even with no touch screen
        let d = lit("probe-always");
        probe_once_with(&d, "display event", found, ProbeMode::Always, || false);
        assert_eq!((status(&d, "HDMI-A-1"), status(&d, "DP-3")), ("detect".to_string(), "detect".to_string()));
        // off: never
        let d = lit("probe-never");
        assert!(probe_once_with(&d, "display event", found, ProbeMode::Never, || true).is_empty());
        assert_eq!(status(&d, "HDMI-A-1"), "disconnected");
        // auto: only with a touch screen
        let d = lit("probe-auto-none");
        probe_once_with(&d, "display event", found, ProbeMode::Auto, || false);
        assert_eq!(status(&d, "HDMI-A-1"), "disconnected", "no touch screen: left alone");
        let d = lit("probe-auto-touch");
        probe_once_with(&d, "display event", found, ProbeMode::Auto, || true);
        assert_eq!(status(&d, "HDMI-A-1"), "detect");
        // with nothing connected the round always looks at the others
        let d = fake("probe-dark", "i915", &[("DP-2", "disconnected"), ("HDMI-A-1", "disconnected")]);
        probe_once_with(&d, "display event", found, ProbeMode::Never, || false);
        assert_eq!(status(&d, "HDMI-A-1"), "detect");
    }

    #[test]
    fn the_option_is_read_from_the_kernel_command_line_and_the_touch_screen_from_the_input_list() {
        assert_eq!(ProbeMode::from_cmdline("quiet iyakki.touchscreen=probe"), ProbeMode::Always);
        assert_eq!(ProbeMode::from_cmdline("iyakki.touchscreen=off quiet"), ProbeMode::Never);
        assert_eq!(ProbeMode::from_cmdline("iyakki.touchscreen=maybe"), ProbeMode::Auto);
        assert_eq!(ProbeMode::from_cmdline("quiet"), ProbeMode::Auto);
        assert!(input_list_has_touch_screen("I: Bus=0003\nN: Name=\"wch.cn TouchScreen\"\nH: Handlers=event4\n"));
        assert!(!input_list_has_touch_screen("N: Name=\"Apple Internal Keyboard / Trackpad\"\nN: Name=\"USB Optical Mouse\"\n"));
    }
}
