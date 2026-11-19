//! The software "re-plug" for a USB-C touch monitor whose panel stays silent after a cold boot (KI-077), the later rungs of the touch ladder
//! in [`crate::touchprobe`].
//!
//! A physical USB-C replug retrains the DisplayPort link and re-enumerates USB together, and that is the only thing that woke the panel.
//! Rung 1 (`link`) does the first half in software: the connected DP connector's sysfs `status` is written `off`, then `on`, then `detect`,
//! so the kernel retrains the link (the screen blinks for about 2 to 4 seconds). Rung 2 (`xhci`) unbinds and binds the xHCI controller the
//! touch hub hangs off (every USB device, keyboard and mouse included, re-enumerates for a moment); it runs only when rung 1 did not help.
//!
//! Once per boot, and only when: a USB touch panel is there, no finger event was ever seen, the boot is 45 s old, nobody typed or moved the
//! mouse in the last few seconds, and `iyakki.touchfix` on the kernel command line is not `off` (`link` stops after rung 1; default `off`: it is opt-in).
//!
//! All sysfs access goes through [`Env`], so the order of the rungs, the restore of `detect` and the path checks are unit-tested with a fake.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

/// The boot has to be this old before the first rung (after the hub re-attach of the ladder, which is due at 30 s).
pub const FIX_AFTER_SECS: u64 = 45;
/// After each rung: this long to see a finger event before the next rung.
pub const RUNG_WAIT_SECS: u64 = 6;
/// The link stays off this long (milliseconds).
pub const LINK_OFF_MS: u64 = 1500;
/// A key or pointer event this recent means a person is working: the fix waits.
pub const BUSY_SECS: u64 = 5;

/// `iyakki.touchfix=`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixMode {
    Off,
    /// Rung 1 only.
    Link,
    /// Rungs 1 and 3 (link, then the root-hub port off and on).
    Port,
    /// Rung 1 (link), rung 3 (port), then rung 2 (xhci) when they did not help (opt-in: `iyakki.touchfix=all`).
    All,
}

/// The mode on a kernel command line. OFF unless asked for (owner, 2026-11-04: the late retrain blinks the screen and re-enumerates USB,
/// and none of its rungs woke the Mac mini's touch panel): `link`, `port` and `all` opt in; anything else is `off`.
pub fn fix_mode(cmdline: &str) -> FixMode {
    let v = cmdline.split_whitespace().find_map(|t| t.strip_prefix("iyakki.touchfix=")).unwrap_or("off");
    match v {
        "link" => FixMode::Link,
        "port" => FixMode::Port,
        "all" => FixMode::All,
        _ => FixMode::Off,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rung {
    Link,
    /// The root-hub port the monitor hangs on, disabled and enabled.
    Port,
    Xhci,
}

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// `iyakki.touchfix=off`: nothing was written.
    Disabled,
    /// A finger event arrived within the wait after this rung.
    Worked(Rung),
    /// All rungs that were allowed ran and no finger event came.
    NoEffect,
}

/// Whether the fix is due now. `idle_secs`: seconds since the last key or pointer event, `None` when none was ever noted (then the check is skipped).
pub fn due(mode: FixMode, uptime_secs: u64, panels: usize, panels_attached: bool, finger_seen: bool, done: bool, idle_secs: Option<u64>) -> bool {
    mode != FixMode::Off
        && panels > 0
        && panels_attached
        && !finger_seen
        && !done
        && uptime_secs >= FIX_AFTER_SECS
        && idle_secs.is_none_or(|s| s >= BUSY_SECS)
}

// ---- the last key or pointer event, noted by the shell ----

static LAST_INPUT_MS: AtomicU64 = AtomicU64::new(0);
static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn now_ms() -> u64 {
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + 1
}

/// The shell calls this for every input event (it is one atomic store).
pub fn note_input() {
    LAST_INPUT_MS.store(now_ms(), Relaxed);
}

/// Seconds since the last [`note_input`], `None` when there was none.
pub fn idle_secs() -> Option<u64> {
    match LAST_INPUT_MS.load(Relaxed) {
        0 => None,
        t => Some(now_ms().saturating_sub(t) / 1000),
    }
}

/// Set when the once-per-boot run has started.
pub static FIX_STARTED: AtomicBool = AtomicBool::new(false);

// ---- the machine, behind a trait so tests can run the ladder ----

pub trait Env {
    fn write(&self, path: &Path, value: &str) -> io::Result<()>;
    fn read(&self, path: &Path) -> Option<String>;
    /// File names in a directory.
    fn list(&self, dir: &Path) -> Vec<String>;
    fn sleep(&self, d: Duration);
    /// The real path of a link or path (`canonicalize`); `None` when it does not resolve.
    fn resolve(&self, path: &Path) -> Option<PathBuf>;
    /// Waits up to `d` for a finger event; true when one came.
    fn wait_finger(&self, d: Duration) -> bool;
    fn log(&self, line: &str);
}

/// The real thing.
pub struct RealEnv;

impl Env for RealEnv {
    fn write(&self, path: &Path, value: &str) -> io::Result<()> {
        std::fs::write(path, value)
    }
    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
    fn list(&self, dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
    fn resolve(&self, path: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(path).ok()
    }
    fn wait_finger(&self, d: Duration) -> bool {
        let end = std::time::Instant::now() + d;
        loop {
            if crate::touchprobe::finger_seen() {
                return true;
            }
            if std::time::Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    fn log(&self, line: &str) {
        log::info!("{line}");
    }
}

// ---- path checks ----

fn is_pci_addr(s: &str) -> bool {
    // dddd:bb:dd.f in lower case hex
    let b = s.as_bytes();
    b.len() == 12
        && b[4] == b':'
        && b[7] == b':'
        && b[10] == b'.'
        && [0, 1, 2, 3, 5, 6, 8, 9].iter().all(|&i| b[i].is_ascii_digit() || (b'a'..=b'f').contains(&b[i]))
        && (b'0'..=b'7').contains(&b[11])
}

fn is_usb_root(s: &str) -> bool {
    s.strip_prefix("usb").is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

/// The PCI address of the USB host controller a USB device hangs off: the path component just before `usbN` in
/// `<sys>/devices/pci0000:00/0000:00:14.0/usb1/1-5`. `None` for any path that is not under `<sys>/devices` or has no such shape.
pub fn xhci_addr(sys: &Path, usb: &Path) -> Option<String> {
    let rel = usb.strip_prefix(sys.join("devices")).ok()?;
    let parts: Vec<&str> = rel.components().map(|c| match c {
        std::path::Component::Normal(s) => s.to_str(),
        _ => None,
    }).collect::<Option<_>>()?;
    let i = parts.iter().position(|p| is_usb_root(p))?;
    let addr = *parts.get(i.checked_sub(1)?)?;
    (is_pci_addr(addr) && parts[0].starts_with("pci")).then(|| addr.to_string())
}

/// `cardN-DP-M` (not `eDP`, not HDMI).
pub fn is_dp_connector(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("card") else { return false };
    let Some((card, port)) = rest.split_once("-DP-") else { return false };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    digits(card) && digits(port)
}

/// The `status` files of the DP connectors that read `connected`.
pub fn connected_dp(env: &dyn Env, sys: &Path) -> Vec<PathBuf> {
    let drm = sys.join("class/drm");
    let mut names: Vec<String> = env.list(&drm).into_iter().filter(|n| is_dp_connector(n)).collect();
    names.sort();
    names.into_iter().map(|n| drm.join(n).join("status")).filter(|p| env.read(p).is_some_and(|s| s.trim() == "connected")).collect()
}

// ---- guards ----

/// Writes `detect` to a connector's status when dropped (or at [`DetectGuard::restore`]), so the screen is never left forced off.
struct DetectGuard<'a> {
    env: &'a dyn Env,
    status: PathBuf,
    done: bool,
}

impl DetectGuard<'_> {
    fn restore(&mut self) -> io::Result<()> {
        if self.done {
            return Ok(());
        }
        self.done = true;
        self.env.write(&self.status, "detect")
    }
}

impl Drop for DetectGuard<'_> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// Binds the controller again when dropped, if it was unbound and not yet bound.
struct BindGuard<'a> {
    env: &'a dyn Env,
    bind: PathBuf,
    addr: String,
    pending: bool,
}

impl BindGuard<'_> {
    fn rebind(&mut self) -> io::Result<()> {
        if !self.pending {
            return Ok(());
        }
        let r = self.env.write(&self.bind, &self.addr).or_else(|_| {
            self.env.sleep(Duration::from_millis(1000));
            self.env.write(&self.bind, &self.addr)
        });
        if r.is_ok() {
            self.pending = false;
        }
        r
    }
}

impl Drop for BindGuard<'_> {
    fn drop(&mut self) {
        let _ = self.rebind();
    }
}

// ---- the rungs ----

/// Rung 1: link off, 1.5 s, on, detect, for every connected DP connector. `Err` text when nothing could be done.
pub fn rung_link(env: &dyn Env, sys: &Path) -> Result<(), String> {
    let found = connected_dp(env, sys);
    if found.is_empty() {
        return Err("no connected DP connector".into());
    }
    env.log(&format!(
        "touch: fix rung 1 (link): {} switched off and on; the screen blinks for about 2 to 4 seconds",
        found.iter().filter_map(|p| p.parent().and_then(|d| d.file_name())).map(|n| n.to_string_lossy().into_owned()).collect::<Vec<_>>().join(", ")
    ));
    let mut guards: Vec<DetectGuard> = found.into_iter().map(|status| DetectGuard { env, status, done: false }).collect();
    let mut failed = Vec::new();
    for g in &guards {
        if let Err(e) = env.write(&g.status, "off") {
            failed.push(format!("off: {e}"));
        }
    }
    env.sleep(Duration::from_millis(LINK_OFF_MS));
    for g in &guards {
        if let Err(e) = env.write(&g.status, "on") {
            failed.push(format!("on: {e}"));
        }
    }
    for g in &mut guards {
        if let Err(e) = g.restore() {
            failed.push(format!("detect: {e}"));
        }
    }
    if failed.is_empty() { Ok(()) } else { Err(failed.join("; ")) }
}

/// The sysfs device paths of every block device that is mounted or used as swap, following device-mapper and similar stacks down to
/// their parts. `Err` when `/proc/mounts` or `/proc/swaps` cannot be read or a `/dev` source cannot be resolved (fail closed).
pub fn busy_block_paths(env: &dyn Env, sys: &Path) -> Result<Vec<PathBuf>, String> {
    let mounts = env.read(Path::new("/proc/mounts")).ok_or("/proc/mounts is unreadable")?;
    let swaps = env.read(Path::new("/proc/swaps")).ok_or("/proc/swaps is unreadable")?;
    let sources = mounts.lines().filter_map(|l| l.split_whitespace().next()).chain(swaps.lines().skip(1).filter_map(|l| l.split_whitespace().next()));
    let mut out = Vec::new();
    for src in sources.filter(|s| s.starts_with("/dev/")) {
        let real = env.resolve(Path::new(src)).ok_or_else(|| format!("{src} does not resolve"))?;
        let name = real.file_name().and_then(|n| n.to_str()).ok_or_else(|| format!("{src}: no device name"))?.to_string();
        let mut todo = vec![(name, 0u8)];
        while let Some((n, depth)) = todo.pop() {
            let p = env.resolve(&sys.join("class/block").join(&n)).ok_or_else(|| format!("{n} has no sysfs device"))?;
            out.push(p);
            if depth < 5 {
                todo.extend(env.list(&sys.join("class/block").join(&n).join("slaves")).into_iter().map(|s| (s, depth + 1)));
            }
        }
    }
    Ok(out)
}

/// The PCI device directory (`<sys>/devices/pci0000:00/0000:00:14.0`) of the controller a USB device hangs off.
fn controller_dir(sys: &Path, usb: &Path) -> Option<PathBuf> {
    let addr = xhci_addr(sys, usb)?;
    usb.ancestors().find(|a| a.file_name().is_some_and(|n| *n == *addr.as_str())).map(Path::to_path_buf)
}

/// The `disable` file of the root-hub port a USB device hangs on: for `<sys>/devices/pci.../usb1/1-5/1-5.3` it is
/// `<sys>/bus/usb/devices/usb1/1-0:1.0/usb1-port5/disable`. `None` unless the path has exactly that shape (first hub component `N-P`
/// with `N` equal to the bus number of `usbN`, plain digits only).
pub fn port_disable_path(sys: &Path, usb: &Path) -> Option<PathBuf> {
    xhci_addr(sys, usb)?;
    let rel = usb.strip_prefix(sys.join("devices")).ok()?;
    let parts: Vec<&str> = rel.components().map(|c| c.as_os_str().to_str()).collect::<Option<_>>()?;
    let i = parts.iter().position(|p| is_usb_root(p))?;
    let bus = parts[i].strip_prefix("usb")?;
    let (n, port) = parts.get(i + 1)?.split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.len() <= 3 && s.bytes().all(|c| c.is_ascii_digit());
    if n != bus || !digits(port) || !digits(n) {
        return None;
    }
    Some(sys.join("bus/usb/devices").join(parts[i]).join(format!("{n}-0:1.0")).join(format!("usb{n}-port{port}")).join("disable"))
}

/// Writes `0` to a port's `disable` when dropped, so the port is never left disabled.
struct PortGuard<'a> {
    env: &'a dyn Env,
    disable: PathBuf,
    done: bool,
}

impl PortGuard<'_> {
    fn restore(&mut self) -> io::Result<()> {
        if self.done {
            return Ok(());
        }
        self.done = true;
        self.env.write(&self.disable, "0")
    }
}

impl Drop for PortGuard<'_> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// Rung 3: the root-hub port of the monitor: `disable` 1, 3 s, 0. This also drops the port's power and its USB-C side.
pub fn rung_port(env: &dyn Env, sys: &Path, panel: &Path) -> Result<(), String> {
    let disable = port_disable_path(sys, panel).ok_or_else(|| format!("{} has no root-hub port path of the expected shape", panel.display()))?;
    let name = disable.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    env.log(&format!("touch: fix rung port ({name} disable 1/0)"));
    let mut g = PortGuard { env, disable, done: false };
    let first = env.write(&g.disable, "1");
    env.sleep(Duration::from_millis(3000));
    let last = g.restore();
    first.map_err(|e| format!("disable 1: {e}"))?;
    last.map_err(|e| format!("disable 0: {e}"))
}

// ---- a finger that comes after the waits ----

static WATCH: std::sync::Mutex<Option<(Rung, std::time::Instant)>> = std::sync::Mutex::new(None);

/// The text to log for a first finger event that came `secs` after `rung`.
pub fn late_finger_line(rung: Rung, secs: u64) -> String {
    format!("touch: first finger event arrived {secs} s after rung {rung:?}")
}

/// Called by [`crate::touchprobe::note_finger`] for the first finger event of the boot: logs once when a rung had re-enumerated the panel
/// and the finger came after its wait.
pub fn finger_arrived() {
    let w = WATCH.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some((r, t)) = w {
        log::info!("{}", late_finger_line(r, t.elapsed().as_secs()));
    }
}

/// Rung 2: unbind and bind the xHCI controller of the panel's USB tree. Only that controller, only when it is bound to `xhci_hcd`.
pub fn rung_xhci(env: &dyn Env, sys: &Path, panel: &Path) -> Result<(), String> {
    let addr = xhci_addr(sys, panel).ok_or_else(|| format!("{} is not below a PCI USB controller", panel.display()))?;
    let drv = sys.join("bus/pci/drivers/xhci_hcd");
    if !env.list(&drv).contains(&addr) {
        return Err(format!("{addr} is not bound to xhci_hcd"));
    }
    let ctrl = controller_dir(sys, panel).ok_or_else(|| "no controller directory".to_string())?;
    match busy_block_paths(env, sys) {
        Ok(v) if v.iter().any(|p| p.starts_with(&ctrl)) => {
            env.log("touch: fix rung 2 (xhci) skipped: a mounted disk is below this controller");
            return Err("a mounted disk is below this controller".into());
        }
        Ok(_) => {}
        Err(e) => {
            env.log(&format!("touch: fix rung 2 (xhci) skipped: cannot tell which disks are mounted ({e})"));
            return Err(format!("refused: {e}"));
        }
    }
    env.log(&format!("touch: fix rung 2 (xhci): {addr} unbound and bound; every USB device, keyboard and mouse included, re-enumerates for a moment"));
    env.write(&drv.join("unbind"), &addr).map_err(|e| format!("unbind: {e}"))?;
    let mut g = BindGuard { env, bind: drv.join("bind"), addr, pending: true };
    env.sleep(Duration::from_millis(1000));
    g.rebind().map_err(|e| format!("bind: {e}"))
}

/// Runs the ladder once: each rung, then [`RUNG_WAIT_SECS`] for a finger event; rung 2 only when rung 1 failed or did not help.
pub fn run(env: &dyn Env, sys: &Path, panel: &Path, mode: FixMode) -> Outcome {
    if mode == FixMode::Off {
        return Outcome::Disabled;
    }
    let devnum = |env: &dyn Env| env.read(&panel.join("devnum")).map(|s| s.trim().to_string());
    let mut rungs = vec![Rung::Link];
    if matches!(mode, FixMode::Port | FixMode::All) {
        rungs.push(Rung::Port);
    }
    if mode == FixMode::All {
        rungs.push(Rung::Xhci);
    }
    for r in rungs {
        let before = devnum(env);
        let done = match r {
            Rung::Link => rung_link(env, sys),
            Rung::Port => rung_port(env, sys, panel),
            Rung::Xhci => rung_xhci(env, sys, panel),
        };
        if let Err(e) = &done {
            env.log(&format!("touch: fix rung {r:?} did not run cleanly: {e}"));
        }
        let finger = env.wait_finger(Duration::from_secs(RUNG_WAIT_SECS));
        let after = devnum(env);
        env.log(&format!(
            "touch: fix after rung {r:?}: finger event {}; panel {}",
            if finger { "seen" } else { "not seen" },
            if before != after { "re-enumerated" } else { "not re-enumerated (same device number)" }
        ));
        if finger {
            return Outcome::Worked(r);
        }
        if before != after {
            *WATCH.lock().unwrap_or_else(|e| e.into_inner()) = Some((r, std::time::Instant::now()));
        }
    }
    Outcome::NoEffect
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    const SYS: &str = "/sys";
    const PANEL: &str = "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-5/1-5.3";

    /// A fake machine: files, a record of every write and sleep, and the wait in which a finger shows up.
    #[derive(Default)]
    struct Fake {
        files: RefCell<HashMap<String, String>>,
        dirs: HashMap<String, Vec<String>>,
        events: RefCell<Vec<String>>,
        logs: RefCell<Vec<String>>,
        fail: Vec<String>,
        links: HashMap<String, String>,
        /// Finger comes in this wait (1 = after rung 1); 0 = never.
        finger_in_wait: usize,
        waits: RefCell<usize>,
    }

    impl Fake {
        fn new() -> Self {
            let mut f = Fake::default();
            f.dirs.insert("/sys/class/drm".into(), vec!["card0".into(), "card0-DP-1".into(), "card0-DP-2".into(), "card0-eDP-1".into(), "card0-HDMI-A-1".into(), "version".into()]);
            f.dirs.insert("/sys/bus/pci/drivers/xhci_hcd".into(), vec!["0000:00:14.0".into(), "bind".into(), "unbind".into()]);
            f.files.borrow_mut().insert("/proc/mounts".into(), "/dev/nvme0n1p2 / ext4 rw 0 0\ntmpfs /run tmpfs rw 0 0\n".into());
            f.files.borrow_mut().insert("/proc/swaps".into(), "Filename Type Size Used Priority\n".into());
            f.links.insert("/dev/nvme0n1p2".into(), "/dev/nvme0n1p2".into());
            f.links.insert("/sys/class/block/nvme0n1p2".into(), "/sys/devices/pci0000:00/0000:00:1d.0/0000:7e:00.0/nvme/nvme0/nvme0n1/nvme0n1p2".into());
            for (n, s) in [("card0-DP-1", "connected\n"), ("card0-DP-2", "disconnected\n"), ("card0-eDP-1", "connected\n"), ("card0-HDMI-A-1", "connected\n")] {
                f.files.borrow_mut().insert(format!("/sys/class/drm/{n}/status"), s.into());
            }
            f
        }
        fn ev(&self) -> Vec<String> {
            self.events.borrow().clone()
        }
    }

    impl Env for Fake {
        fn write(&self, path: &Path, value: &str) -> io::Result<()> {
            let p = path.to_string_lossy().into_owned();
            self.events.borrow_mut().push(format!("{p}={value}"));
            if self.fail.iter().any(|f| p.ends_with(f.as_str())) {
                return Err(io::Error::other("denied"));
            }
            Ok(())
        }
        fn read(&self, path: &Path) -> Option<String> {
            self.files.borrow().get(path.to_str()?).cloned()
        }
        fn list(&self, dir: &Path) -> Vec<String> {
            self.dirs.get(dir.to_str().unwrap()).cloned().unwrap_or_default()
        }
        fn resolve(&self, path: &Path) -> Option<PathBuf> {
            self.links.get(path.to_str()?).map(PathBuf::from)
        }
        fn sleep(&self, d: Duration) {
            self.events.borrow_mut().push(format!("sleep {}", d.as_millis()));
        }
        fn wait_finger(&self, d: Duration) -> bool {
            *self.waits.borrow_mut() += 1;
            self.events.borrow_mut().push(format!("wait {}", d.as_secs()));
            self.finger_in_wait != 0 && *self.waits.borrow() == self.finger_in_wait
        }
        fn log(&self, line: &str) {
            self.logs.borrow_mut().push(line.into());
        }
    }

    #[test]
    fn the_mode_is_read_from_the_command_line_and_defaults_to_all() {
        assert_eq!(fix_mode("quiet iyakki.touchfix=off"), FixMode::Off);
        assert_eq!(fix_mode("iyakki.touchfix=link x"), FixMode::Link);
        assert_eq!(fix_mode("iyakki.touchfix=all"), FixMode::All);
        assert_eq!(fix_mode("a iyakki.touchfix=port"), FixMode::Port);
        assert_eq!(fix_mode("quiet splash"), FixMode::Off, "off unless asked for");
        assert_eq!(fix_mode("iyakki.touchfix=what"), FixMode::Off);
        assert_eq!(fix_mode("xiyakki.touchfix=all"), FixMode::Off, "whole words only");
    }

    #[test]
    fn it_is_due_once_for_a_silent_panel_after_45_seconds_when_nobody_is_typing() {
        let ok = |m, up, p, a, f, d, idle| due(m, up, p, a, f, d, idle);
        assert!(ok(FixMode::All, 45, 1, true, false, false, Some(9)));
        assert!(ok(FixMode::Link, 45, 1, true, false, false, None), "no input ever noted: the check is skipped");
        assert!(!ok(FixMode::All, 44, 1, true, false, false, None), "too early");
        assert!(!ok(FixMode::All, 99, 0, true, false, false, None), "no panel");
        assert!(!ok(FixMode::All, 99, 1, false, false, false, None), "the earlier rungs are not over");
        assert!(!ok(FixMode::All, 99, 1, true, true, false, None), "a finger was seen");
        assert!(!ok(FixMode::All, 99, 1, true, false, true, None), "already done");
        assert!(!ok(FixMode::All, 99, 1, true, false, false, Some(2)), "a person is typing");
        assert!(!ok(FixMode::Off, 99, 1, true, false, false, None), "off");
    }

    #[test]
    fn the_pci_address_comes_from_the_path_and_nothing_else_is_accepted() {
        let s = Path::new(SYS);
        assert_eq!(xhci_addr(s, Path::new(PANEL)).as_deref(), Some("0000:00:14.0"));
        assert_eq!(xhci_addr(s, Path::new("/sys/devices/pci0000:00/0000:00:14.0/usb1/1-5")).as_deref(), Some("0000:00:14.0"));
        assert_eq!(xhci_addr(s, Path::new("/sys/devices/pci0000:00/0000:00:1c.0/0000:03:00.0/usb2/2-1")).as_deref(), Some("0000:03:00.0"), "behind a bridge: the controller itself");
        for bad in [
            "/devices/pci0000:00/0000:00:14.0/usb1/1-5",
            "/sys/devices/pci0000:00/0000:00:14.0/usb1/../../../etc",
            "/sys/devices/pci0000:00/0000:00:14.0/usb1x/1-5",
            "/sys/devices/pci0000:00/0000:00:14.0",
            "/sys/devices/pci0000:00/not-a-pci/usb1/1-5",
            "/sys/devices/platform/0000:00:14.0/usb1/1-5",
            "/sys/devices/usb1/1-5",
            "/sys/class/input/input4",
            "/other/sys/devices/pci0000:00/0000:00:14.0/usb1/1-5",
            "/sys/devices/pci0000:00/0000:00:14.8/usb1/1-5",
            "/sys/devices/pci0000:00/0000:00:14.0/../0000:00:1f.0/usb1",
        ] {
            assert_eq!(xhci_addr(s, Path::new(bad)), None, "{bad}");
        }
    }

    #[test]
    fn only_connected_dp_connectors_are_chosen() {
        assert!(is_dp_connector("card0-DP-1") && is_dp_connector("card12-DP-3"));
        for n in ["card0-eDP-1", "card0-HDMI-A-1", "card0", "cardx-DP-1", "card0-DP-", "card0-DP-1a", "../card0-DP-1"] {
            assert!(!is_dp_connector(n), "{n}");
        }
        let f = Fake::new();
        assert_eq!(connected_dp(&f, Path::new(SYS)), vec![PathBuf::from("/sys/class/drm/card0-DP-1/status")]);
    }

    #[test]
    fn rung_one_writes_off_waits_on_then_detect() {
        let f = Fake::new();
        rung_link(&f, Path::new(SYS)).unwrap();
        assert_eq!(f.ev(), ["/sys/class/drm/card0-DP-1/status=off", "sleep 1500", "/sys/class/drm/card0-DP-1/status=on", "/sys/class/drm/card0-DP-1/status=detect"]);
        assert!(f.logs.borrow()[0].contains("blinks"), "the blink is logged");
    }

    #[test]
    fn detect_is_written_even_when_off_or_on_fails() {
        let mut f = Fake::new();
        f.fail = vec!["status".into()];
        // every write fails, detect included: the restore is still attempted last
        assert!(rung_link(&f, Path::new(SYS)).is_err());
        assert_eq!(f.ev().last().unwrap(), "/sys/class/drm/card0-DP-1/status=detect");
        let mut g = Fake::new();
        g.fail = vec![];
        let n_before = g.ev().len();
        {
            let _guard = DetectGuard { env: &g, status: PathBuf::from("/sys/class/drm/card0-DP-1/status"), done: false };
        }
        assert_eq!(g.ev()[n_before..], ["/sys/class/drm/card0-DP-1/status=detect"], "the guard restores when dropped");
    }

    #[test]
    fn the_guard_restores_detect_on_a_panic() {
        let f = Fake::new();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = DetectGuard { env: &f, status: PathBuf::from("/sys/class/drm/card0-DP-1/status"), done: false };
            panic!("boom");
        }));
        assert!(r.is_err());
        assert_eq!(f.ev(), ["/sys/class/drm/card0-DP-1/status=detect"]);
    }

    #[test]
    fn rung_two_unbinds_then_binds_only_the_panels_controller() {
        let f = Fake::new();
        rung_xhci(&f, Path::new(SYS), Path::new(PANEL)).unwrap();
        assert_eq!(f.ev(), ["/sys/bus/pci/drivers/xhci_hcd/unbind=0000:00:14.0", "sleep 1000", "/sys/bus/pci/drivers/xhci_hcd/bind=0000:00:14.0"]);
    }

    #[test]
    fn rung_two_refuses_a_foreign_path_or_a_controller_not_on_xhci_hcd() {
        let f = Fake::new();
        assert!(rung_xhci(&f, Path::new(SYS), Path::new("/sys/devices/pci0000:00/0000:00:1f.3/sound/card0")).is_err());
        assert!(rung_xhci(&f, Path::new(SYS), Path::new("/sys/devices/pci0000:00/0000:00:1f.3/usb1/1-1")).is_err(), "not in the driver's list");
        assert!(f.ev().is_empty(), "nothing was written: {:?}", f.ev());
    }

    fn disk(f: &mut Fake, dev: &str, sysdev: &str) {
        f.links.insert(format!("/dev/{dev}"), format!("/dev/{dev}"));
        f.links.insert(format!("/sys/class/block/{dev}"), sysdev.into());
    }

    const USB_DISK: &str = "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-6/1-6:1.0/host6/target6:0:0/6:0:0:0/block/sdb/sdb1";

    #[test]
    fn rung_two_is_refused_when_a_mounted_usb_disk_is_below_the_controller() {
        let mut f = Fake::new();
        disk(&mut f, "sdb1", USB_DISK);
        f.files.borrow_mut().insert("/proc/mounts".into(), "/dev/sdb1 /data ext4 rw 0 0\n".into());
        assert!(rung_xhci(&f, Path::new(SYS), Path::new(PANEL)).is_err());
        assert!(f.ev().is_empty(), "{:?}", f.ev());
        assert!(f.logs.borrow().iter().any(|l| l == "touch: fix rung 2 (xhci) skipped: a mounted disk is below this controller"));
    }

    #[test]
    fn rung_two_is_allowed_when_the_mounted_disk_is_on_another_controller() {
        let mut f = Fake::new();
        disk(&mut f, "sdc1", "/sys/devices/pci0000:00/0000:00:1d.0/0000:7e:00.0/usb6/6-1/6-1:1.0/host7/target7:0:0/7:0:0:0/block/sdc/sdc1");
        f.files.borrow_mut().insert("/proc/mounts".into(), "/dev/sdc1 /data ext4 rw 0 0\n/dev/nvme0n1p2 / ext4 rw 0 0\n".into());
        rung_xhci(&f, Path::new(SYS), Path::new(PANEL)).unwrap();
        assert!(f.ev().iter().any(|x| x.ends_with("unbind=0000:00:14.0")));
    }

    #[test]
    fn rung_two_is_refused_for_a_swap_only_device_below_the_controller() {
        let mut f = Fake::new();
        disk(&mut f, "sdb2", USB_DISK);
        f.files.borrow_mut().insert("/proc/swaps".into(), "Filename Type Size Used Priority\n/dev/sdb2 partition 1024 0 -2\n".into());
        assert!(rung_xhci(&f, Path::new(SYS), Path::new(PANEL)).is_err());
        assert!(f.ev().is_empty());
    }

    #[test]
    fn rung_two_is_refused_when_the_mounts_cannot_be_read() {
        let f = Fake::new();
        f.files.borrow_mut().remove("/proc/mounts");
        assert!(rung_xhci(&f, Path::new(SYS), Path::new(PANEL)).is_err());
        let g = Fake::new();
        g.files.borrow_mut().insert("/proc/mounts".into(), "/dev/unknown1 /x ext4 rw 0 0\n".into());
        assert!(rung_xhci(&g, Path::new(SYS), Path::new(PANEL)).is_err(), "an unresolved source fails closed");
        assert!(f.ev().is_empty() && g.ev().is_empty());
    }

    #[test]
    fn the_controller_is_bound_again_even_when_the_first_bind_fails_once() {
        let f = Fake::new();
        let mut g = BindGuard { env: &f, bind: PathBuf::from("/sys/bus/pci/drivers/xhci_hcd/bind"), addr: "0000:00:14.0".into(), pending: true };
        drop(g.rebind());
        assert!(!g.pending);
        let h = Fake::new();
        {
            let _g = BindGuard { env: &h, bind: PathBuf::from("/sys/bus/pci/drivers/xhci_hcd/bind"), addr: "0000:00:14.0".into(), pending: true };
        }
        assert_eq!(h.ev(), ["/sys/bus/pci/drivers/xhci_hcd/bind=0000:00:14.0"], "dropped while pending: bound");
    }

    #[test]
    fn the_ladder_runs_link_then_xhci_with_a_six_second_wait_after_each() {
        let f = Fake::new();
        assert_eq!(run(&f, Path::new(SYS), Path::new(PANEL), FixMode::All), Outcome::NoEffect);
        let e = f.ev();
        let pos = |s: &str| e.iter().position(|x| x == s).unwrap_or_else(|| panic!("{s} in {e:?}"));
        let (off, det, w1, unb, bind) = (pos("/sys/class/drm/card0-DP-1/status=off"), pos("/sys/class/drm/card0-DP-1/status=detect"), pos("wait 6"), pos("/sys/bus/pci/drivers/xhci_hcd/unbind=0000:00:14.0"), pos("/sys/bus/pci/drivers/xhci_hcd/bind=0000:00:14.0"));
        let (p1, p0) = (pos("/sys/bus/usb/devices/usb1/1-0:1.0/usb1-port5/disable=1"), pos("/sys/bus/usb/devices/usb1/1-0:1.0/usb1-port5/disable=0"));
        assert!(off < det && det < w1 && w1 < p1 && p1 < p0 && p0 < unb && unb < bind, "{e:?}");
        assert!(e[p1 + 1] == "sleep 3000" && e[p0 + 1] == "wait 6", "{e:?}");
        assert_eq!(e.iter().filter(|x| *x == "wait 6").count(), 3, "{e:?}");
        assert_eq!(e.last().unwrap(), "wait 6");
    }

    #[test]
    fn a_finger_after_rung_one_stops_the_ladder_before_the_xhci_rebind() {
        let mut f = Fake::new();
        f.finger_in_wait = 1;
        assert_eq!(run(&f, Path::new(SYS), Path::new(PANEL), FixMode::All), Outcome::Worked(Rung::Link));
        assert!(!f.ev().iter().any(|x| x.contains("xhci_hcd")), "{:?}", f.ev());
    }

    #[test]
    fn a_finger_after_rung_two_is_reported() {
        let mut f = Fake::new();
        f.finger_in_wait = 3;
        assert_eq!(run(&f, Path::new(SYS), Path::new(PANEL), FixMode::All), Outcome::Worked(Rung::Xhci));
    }

    #[test]
    fn a_finger_after_the_port_rung_skips_xhci_and_port_mode_stops_there() {
        let mut f = Fake::new();
        f.finger_in_wait = 2;
        assert_eq!(run(&f, Path::new(SYS), Path::new(PANEL), FixMode::All), Outcome::Worked(Rung::Port));
        assert!(!f.ev().iter().any(|x| x.contains("xhci_hcd")));
        let g = Fake::new();
        assert_eq!(run(&g, Path::new(SYS), Path::new(PANEL), FixMode::Port), Outcome::NoEffect);
        assert!(!g.ev().iter().any(|x| x.contains("xhci_hcd")) && g.ev().iter().any(|x| x.ends_with("usb1-port5/disable=0")));
        assert!(g.logs.borrow().iter().any(|l| l == "touch: fix rung port (usb1-port5 disable 1/0)"));
    }

    #[test]
    fn the_root_hub_port_comes_from_the_path_and_odd_shapes_are_refused() {
        let s = Path::new(SYS);
        assert_eq!(port_disable_path(s, Path::new(PANEL)), Some(PathBuf::from("/sys/bus/usb/devices/usb1/1-0:1.0/usb1-port5/disable")));
        assert_eq!(port_disable_path(s, Path::new("/sys/devices/pci0000:00/0000:00:14.0/usb2/2-12")), Some(PathBuf::from("/sys/bus/usb/devices/usb2/2-0:1.0/usb2-port12/disable")));
        for bad in [
            "/sys/devices/pci0000:00/0000:00:14.0/usb1",
            "/sys/devices/pci0000:00/0000:00:14.0/usb1/2-5",
            "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-5.3",
            "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-",
            "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-x",
            "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-5:1.0",
            "/sys/devices/pci0000:00/0000:00:14.0/usb1/../1-5",
            "/sys/devices/platform/usb1/1-5",
            "/other/devices/pci0000:00/0000:00:14.0/usb1/1-5",
        ] {
            assert_eq!(port_disable_path(s, Path::new(bad)), None, "{bad}");
        }
        let f = Fake::new();
        assert!(rung_port(&f, s, Path::new("/sys/devices/pci0000:00/0000:00:14.0/usb1")).is_err());
        assert!(f.ev().is_empty());
    }

    #[test]
    fn the_port_is_enabled_again_on_drop_and_when_disable_fails() {
        let f = Fake::new();
        rung_port(&f, Path::new(SYS), Path::new(PANEL)).unwrap();
        assert_eq!(f.ev(), ["/sys/bus/usb/devices/usb1/1-0:1.0/usb1-port5/disable=1", "sleep 3000", "/sys/bus/usb/devices/usb1/1-0:1.0/usb1-port5/disable=0"]);
        let g = Fake::new();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = PortGuard { env: &g, disable: PathBuf::from("/p/disable"), done: false };
            panic!("boom");
        }));
        assert!(r.is_err());
        assert_eq!(g.ev(), ["/p/disable=0"]);
        let mut h = Fake::new();
        h.fail = vec!["disable=1".into()];
        // the fake matches on the path, so make every write to the file fail: the restore is still attempted
        h.fail = vec!["disable".into()];
        assert!(rung_port(&h, Path::new(SYS), Path::new(PANEL)).is_err());
        assert_eq!(h.ev().last().unwrap(), "/sys/bus/usb/devices/usb1/1-0:1.0/usb1-port5/disable=0");
    }

    #[test]
    fn a_late_finger_is_reported_with_the_rung_and_seconds() {
        assert_eq!(late_finger_line(Rung::Port, 17), "touch: first finger event arrived 17 s after rung Port");
    }

    #[test]
    fn rung_two_runs_when_rung_one_cannot_because_no_dp_is_connected() {
        let f = Fake::new();
        f.files.borrow_mut().insert("/sys/class/drm/card0-DP-1/status".into(), "disconnected\n".into());
        run(&f, Path::new(SYS), Path::new(PANEL), FixMode::All);
        assert!(f.ev().iter().any(|x| x.ends_with("unbind=0000:00:14.0")));
        assert!(!f.ev().iter().any(|x| x.contains("class/drm")), "no connector was touched");
    }

    #[test]
    fn link_mode_stops_after_rung_one_and_off_writes_nothing() {
        let f = Fake::new();
        assert_eq!(run(&f, Path::new(SYS), Path::new(PANEL), FixMode::Link), Outcome::NoEffect);
        assert!(!f.ev().iter().any(|x| x.contains("xhci_hcd")));
        let g = Fake::new();
        assert_eq!(run(&g, Path::new(SYS), Path::new(PANEL), FixMode::Off), Outcome::Disabled);
        assert!(g.ev().is_empty() && g.logs.borrow().is_empty());
    }

    #[test]
    fn the_once_flag_allows_one_start() {
        let flag = AtomicBool::new(false);
        assert!(flag.compare_exchange(false, true, Relaxed, Relaxed).is_ok());
        assert!(flag.compare_exchange(false, true, Relaxed, Relaxed).is_err());
    }
}
