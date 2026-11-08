//! Software recovery for a touch screen that is there but silent (KI-077; tablets cannot be unplugged).
//!
//! Detection: a touch screen (evdev with ABS_MT_POSITION_X and INPUT_PROP_DIRECT, or at least not a pad) that the kernel lists, and that has
//! produced no event at all since it appeared, for [`GRACE_SECS`]. A person who simply has not touched the screen looks the same, so the
//! automatic run only uses steps that cost at most a second of re-enumeration of that one device and is rate limited ([`Limiter`]).
//!
//! The ladder, least disruptive first, each step logged and followed by a wait for an event ([`Env::events`]):
//! 0. `ReportProtocol` (USB panels with a boot-protocol mouse interface, class 03 subclass 01): the HID request SET_PROTOCOL(report) to
//!    that interface, because firmware that binds boot mice for its own menus (EDK2) can leave such a controller in boot protocol, which
//!    Linux never switches back. Also run once at the first sight of the panel ([`prime`]). A panel without a boot interface is skipped:
//!    the Mac mini's wch.cn 27c0:0859 is one (all its interfaces are subclass 00; measured 2026-11-07 over SSH), so this step never
//!    applied there. What that panel really does after a cold boot: its controller is in MOUSE MODE, the multi-touch interface is listed
//!    and silent while the mouse-like interface (`:1.2`, report id 7, absolute X/Y and a button) reports every finger; the Device Mode
//!    feature (report 33) set to 2 by the kernel driver and by hand is ignored by it, only a power loss (the replug) puts it back. The
//!    shell therefore hears the mouse-like interface whenever the multi-touch one has not spoken (`iyakki-shell::input::mouse_like_muted`)
//!    and counts those events here through [`note_event`], so this ladder does not reset a panel that is working through that interface.
//! 1. `Reopen`: the shell opens the evdev node again ([`reopen_generation`]).
//! 2. `HidRebind`: unbind and bind the HID device from `hid-multitouch` (USB and I2C).
//! 3. `UsbReset` (USB): `authorized` 0 then 1 of the one USB device; unbind/bind of the `usb` driver if that file cannot be written.
//!    `I2cRebind` (I2C): unbind and bind the I2C client from its `i2c_hid*` driver.
//! 4. `ModuleReload` (manual only): `modprobe -r hid_multitouch`, `modprobe hid_multitouch`, only when every device bound to the driver is
//!    a touch screen.
//!
//! Safety: only a leaf USB device whose interfaces are all HID and none a keyboard, never a hub, a root hub, a device with a keyboard
//! input or a disk; only an I2C client driven by `i2c_hid*`; only a HID device driven by `hid-multitouch`; every path must lie below
//! `<sys>/devices` and every name must have the exact shape of its kind. Anything else is refused with a plain sentence.
//!
//! All sysfs access goes through [`Env`] and a `sys` root, so tests run on a temp tree.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::Duration;

/// Seconds a present touch screen may be silent before the automatic run.
pub const GRACE_SECS: u64 = 90;
/// Seconds to wait for an event after each step.
pub const WAIT_SECS: u64 = 6;
/// Automatic tries per boot.
pub const AUTO_MAX: usize = 3;
/// Seconds that must pass before automatic try number n (index 0 is the first): backoff.
pub const AUTO_GAPS: [u64; AUTO_MAX] = [0, 180, 900];
/// Manual: seconds between two presses, and presses per hour.
pub const MANUAL_GAP: u64 = 20;
pub const MANUAL_PER_HOUR: usize = 8;

const ABS_MT_POSITION_X: u32 = 0x35;
const INPUT_PROP_DIRECT: u32 = 1;
const KEY_A: u32 = 30;
const KEY_ENTER: u32 = 28;

// ---------------------------------------------------------------- events and reopen requests

static EVENTS: AtomicU64 = AtomicU64::new(0);
static REOPEN: AtomicU64 = AtomicU64::new(0);

/// The shell calls this for every raw event of a touch screen (one atomic add).
pub fn note_event() {
    EVENTS.fetch_add(1, Relaxed);
}

pub fn event_count() -> u64 {
    EVENTS.load(Relaxed)
}

/// The shell opens every touch screen node again when this number changes.
pub fn reopen_generation() -> u64 {
    REOPEN.load(Relaxed)
}

// ---------------------------------------------------------------- the machine behind a trait

pub trait Env {
    fn read(&self, path: &Path) -> Option<String>;
    fn write(&self, path: &Path, value: &str) -> io::Result<()>;
    /// File and directory names in a directory.
    fn list(&self, dir: &Path) -> Vec<String>;
    fn resolve(&self, path: &Path) -> Option<PathBuf>;
    fn sleep(&self, d: Duration);
    /// Events counted so far.
    fn events(&self) -> u64;
    /// Waits up to `d` for the count to exceed `since`; true when it did.
    fn wait_events(&self, since: u64, d: Duration) -> bool;
    /// Asks the shell to open the touch nodes again.
    fn request_reopen(&self);
    /// Runs a program (modprobe); true on exit 0.
    fn run(&self, prog: &str, args: &[&str]) -> bool;
    fn log(&self, line: &str);
    /// SET_PROTOCOL(report) to the HID interfaces `ifaces` of the USB device at `node`, which must read back as `vendor:product`
    /// (`iyakki_drivers::usbfs::hid_set_report_protocol`). One sentence per interface.
    fn set_report_protocol(&self, node: &Path, vendor: u16, product: u16, ifaces: &[u8]) -> Result<Vec<String>, String>;
}

pub struct RealEnv;

impl Env for RealEnv {
    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
    fn write(&self, path: &Path, value: &str) -> io::Result<()> {
        std::fs::write(path, value)
    }
    fn list(&self, dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        v.sort();
        v
    }
    fn resolve(&self, path: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(path).ok()
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
    fn events(&self) -> u64 {
        event_count()
    }
    fn wait_events(&self, since: u64, d: Duration) -> bool {
        let end = std::time::Instant::now() + d;
        loop {
            if event_count() > since {
                return true;
            }
            if std::time::Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    fn request_reopen(&self) {
        REOPEN.fetch_add(1, Relaxed);
    }
    fn run(&self, prog: &str, args: &[&str]) -> bool {
        std::process::Command::new(prog).args(args).status().map(|s| s.success()).unwrap_or(false)
    }
    fn log(&self, line: &str) {
        log::info!("{line}");
        *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = line.trim_start_matches("touch-recover: ").to_string();
    }
    fn set_report_protocol(&self, node: &Path, vendor: u16, product: u16, ifaces: &[u8]) -> Result<Vec<String>, String> {
        iyakki_drivers::usbfs::hid_set_report_protocol(node, vendor, product, ifaces).map_err(|e| format!("{e:?}"))
    }
}

static STATUS: Mutex<String> = Mutex::new(String::new());

/// The plain status line for Settings: the last thing the recovery said, or what it sees.
pub fn status() -> String {
    let s = STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !s.is_empty() {
        return s;
    }
    let found = discover(&RealEnv, Path::new("/sys"));
    match found.len() {
        0 => "no touch screen is listed by the kernel".into(),
        n => format!("{n} touch screen(s) listed ({}); nothing has been reset", found.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")),
    }
}

// ---------------------------------------------------------------- discovery

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    Usb,
    I2c,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Touch {
    pub name: String,
    pub bus: Bus,
    /// `0003:27C0:0859.0001`: the HID device.
    pub hid: String,
    /// `27C0:0859` for USB; the client name (`i2c-ELAN0001:00`) for I2C: stays the same when the node comes back.
    pub ident: String,
    /// The leaf USB device directory (USB only).
    pub usb: Option<PathBuf>,
    /// The I2C client name (I2C only).
    pub i2c: Option<String>,
}

/// Bit `n` of a `capabilities/*` file (hex words, most significant first, as the kernel prints them).
pub fn cap_bit(caps: &str, n: u32) -> bool {
    let words: Vec<&str> = caps.split_whitespace().collect();
    let bits = std::mem::size_of::<usize>() as u32 * 8;
    let idx = (n / bits) as usize;
    if idx >= words.len() {
        return false;
    }
    u64::from_str_radix(words[words.len() - 1 - idx], 16).is_ok_and(|w| w & (1u64 << (n % bits)) != 0)
}

/// `0003:27C0:0859.0001`
fn hid_bus(name: &str) -> Option<Bus> {
    let b = name.as_bytes();
    let ok = b.len() == 19 && b[4] == b':' && b[9] == b':' && b[14] == b'.' && b.iter().enumerate().all(|(i, c)| [4, 9, 14].contains(&i) || c.is_ascii_hexdigit());
    if !ok {
        return None;
    }
    match &name[..4] {
        "0003" => Some(Bus::Usb),
        "0018" => Some(Bus::I2c),
        _ => None,
    }
}

/// A USB device name like `1-5` or `1-5.2.1` (not a root hub `usb1`, not an interface `1-5:1.0`).
fn is_usb_device_name(s: &str) -> bool {
    let Some((bus, rest)) = s.split_once('-') else { return false };
    !bus.is_empty() && bus.bytes().all(|c| c.is_ascii_digit()) && !rest.is_empty() && rest.split('.').all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
}

/// An I2C client name like `i2c-ELAN0001:00` or `0-0038` (not an adapter `i2c-0`).
fn is_i2c_client_name(s: &str) -> bool {
    let plain = |t: &str| !t.is_empty() && t.bytes().all(|c| c.is_ascii_alphanumeric() || c == b':' || c == b'_' || c == b'.' || c == b'-');
    if let Some(r) = s.strip_prefix("i2c-") {
        return r.contains(':') && plain(r);
    }
    match s.split_once('-') {
        Some((a, b)) => !a.is_empty() && a.bytes().all(|c| c.is_ascii_digit()) && b.len() == 4 && b.bytes().all(|c| c.is_ascii_hexdigit()),
        None => false,
    }
}

fn driver_name(env: &dyn Env, link: &Path) -> Option<String> {
    env.resolve(link).and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
}

/// Every touch screen the kernel lists that sits on USB or I2C HID. Touch pads, keyboards and everything else are not returned.
pub fn discover(env: &dyn Env, sys: &Path) -> Vec<Touch> {
    let mut out: Vec<Touch> = Vec::new();
    let devices = sys.join("devices");
    for n in env.list(&sys.join("class/input")) {
        if !n.starts_with("input") || !n[5..].bytes().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let base = sys.join("class/input").join(&n);
        let name = env.read(&base.join("name")).unwrap_or_default().trim().to_string();
        let lower = name.to_ascii_lowercase();
        let abs = env.read(&base.join("capabilities/abs")).unwrap_or_default();
        let props = env.read(&base.join("properties")).unwrap_or_default();
        let key = env.read(&base.join("capabilities/key")).unwrap_or_default();
        let screen = cap_bit(&abs, ABS_MT_POSITION_X) && (cap_bit(&props, INPUT_PROP_DIRECT) || !(lower.contains("pad") || lower.contains("touch bar")));
        if !screen || (cap_bit(&key, KEY_A) && cap_bit(&key, KEY_ENTER)) {
            continue;
        }
        let Some(real) = env.resolve(&base) else { continue };
        if !real.starts_with(&devices) {
            continue;
        }
        let comps: Vec<String> = real.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let Some((hid, bus)) = comps.iter().find_map(|c| hid_bus(c).map(|b| (c.clone(), b))) else { continue };
        let t = match bus {
            Bus::Usb => {
                let Some(i) = comps.iter().rposition(|c| is_usb_device_name(c)) else { continue };
                let mut dir = PathBuf::from("/");
                for c in &comps[1..=i] {
                    dir.push(c);
                }
                let ident = format!("{}:{}", env.read(&dir.join("idVendor")).unwrap_or_default().trim(), env.read(&dir.join("idProduct")).unwrap_or_default().trim());
                Touch { name, bus, hid, ident, usb: Some(dir), i2c: None }
            }
            Bus::I2c => {
                let Some(c) = comps.iter().rev().find(|c| is_i2c_client_name(c)).cloned() else { continue };
                Touch { name, bus, hid, ident: c.clone(), usb: None, i2c: Some(c) }
            }
        };
        if !out.iter().any(|o| o.hid == t.hid) {
            out.push(t);
        }
    }
    out
}

// ---------------------------------------------------------------- safety

/// Why the device may not be reset, or `Ok`. Checked again before every step that writes.
pub fn check_safe(env: &dyn Env, sys: &Path, t: &Touch) -> Result<(), String> {
    let devices = sys.join("devices");
    let drv = driver_name(env, &sys.join("bus/hid/devices").join(&t.hid).join("driver"));
    if drv.as_deref() != Some("hid-multitouch") {
        return Err(format!("{} is driven by {}, not hid-multitouch", t.name, drv.unwrap_or_else(|| "no driver".into())));
    }
    match t.bus {
        Bus::Usb => {
            let usb = t.usb.as_ref().ok_or("no USB device known")?;
            let leaf = usb.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            if !usb.starts_with(&devices) || !is_usb_device_name(&leaf) {
                return Err(format!("{} is not a plain USB device ({})", t.name, usb.display()));
            }
            if env.read(&usb.join("bDeviceClass")).map(|s| s.trim() == "09").unwrap_or(false) {
                return Err(format!("{} is a USB hub: refused", t.name));
            }
            let mut ifaces = 0;
            for e in env.list(usb) {
                if !e.starts_with(&format!("{leaf}:")) {
                    continue;
                }
                ifaces += 1;
                let d = usb.join(&e);
                let class = env.read(&d.join("bInterfaceClass")).unwrap_or_default();
                let proto = env.read(&d.join("bInterfaceProtocol")).unwrap_or_default();
                if class.trim() != "03" {
                    return Err(format!("{} also has a non-HID interface (class {}): refused, it may carry a disk or other service", t.name, class.trim()));
                }
                if proto.trim() == "01" {
                    return Err(format!("{} has a keyboard interface: refused", t.name));
                }
            }
            if ifaces == 0 {
                return Err(format!("{}: the USB interfaces cannot be read: refused", t.name));
            }
            // another input device below the same USB device that types: refused
            for n in env.list(&sys.join("class/input")) {
                if !n.starts_with("input") {
                    continue;
                }
                let p = sys.join("class/input").join(&n);
                if env.resolve(&p).is_some_and(|r| r.starts_with(usb)) {
                    let key = env.read(&p.join("capabilities/key")).unwrap_or_default();
                    if cap_bit(&key, KEY_A) && cap_bit(&key, KEY_ENTER) {
                        return Err(format!("{} also provides a keyboard: refused", t.name));
                    }
                }
            }
        }
        Bus::I2c => {
            let c = t.i2c.as_ref().ok_or("no I2C client known")?;
            if !is_i2c_client_name(c) {
                return Err(format!("{c} is not an I2C client"));
            }
            let d = driver_name(env, &sys.join("bus/i2c/devices").join(c).join("driver"));
            if !d.as_deref().is_some_and(|d| d.starts_with("i2c_hid") || d.starts_with("i2c-hid")) {
                return Err(format!("{c} is driven by {}, not i2c_hid: refused", d.unwrap_or_else(|| "no driver".into())));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- the ladder

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rung {
    /// USB panels with a boot-protocol mouse interface only: SET_PROTOCOL(report) to that interface (KI-077, see the module doc).
    ReportProtocol,
    Reopen,
    HidRebind,
    UsbReset,
    I2cRebind,
    ModuleReload,
    /// Manual only, USB panels only: the connected DisplayPort connectors are switched off and on (the screen blinks for 2 to 4 s),
    /// which makes the kernel retrain the link, the one thing a cable replug does that the steps above do not (KI-077).
    LinkRetrain,
}

impl Rung {
    pub fn label(self) -> &'static str {
        match self {
            Rung::ReportProtocol => "switch the panel's boot-mouse interface to report protocol",
            Rung::Reopen => "re-open the input node",
            Rung::HidRebind => "unbind and bind the HID driver",
            Rung::UsbReset => "reset the USB device",
            Rung::I2cRebind => "unbind and bind the I2C driver",
            Rung::ModuleReload => "reload hid_multitouch",
            Rung::LinkRetrain => "retrain the display link (the screen blinks)",
        }
    }
}

/// The HID interfaces of a USB touch screen that speak the boot protocol (class 03, subclass 01): their `bInterfaceNumber`s. Only
/// those take SET_PROTOCOL; a panel without one has nothing to switch.
pub fn boot_interfaces(env: &dyn Env, t: &Touch) -> Vec<u8> {
    let Some(usb) = t.usb.as_ref() else { return Vec::new() };
    let leaf = usb.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let mut v = Vec::new();
    for e in env.list(usb) {
        if !e.starts_with(&format!("{leaf}:")) {
            continue;
        }
        let d = usb.join(&e);
        let hex = |f: &str| env.read(&d.join(f)).and_then(|s| u8::from_str_radix(s.trim(), 16).ok());
        if hex("bInterfaceClass") == Some(3) && hex("bInterfaceSubClass") == Some(1) {
            if let Some(n) = hex("bInterfaceNumber") {
                v.push(n);
            }
        }
    }
    v.sort_unstable();
    v
}

/// The usbfs node of a USB device directory: `/dev/bus/usb/BBB/DDD` from its `busnum` and `devnum`.
fn usb_node(env: &dyn Env, usb: &Path) -> Option<PathBuf> {
    let num = |f: &str| env.read(&usb.join(f)).and_then(|s| s.trim().parse::<u32>().ok());
    Some(PathBuf::from(format!("/dev/bus/usb/{:03}/{:03}", num("busnum")?, num("devnum")?)))
}

/// The one step every USB panel with a boot interface gets at first sight, before any silence is counted: SET_PROTOCOL(report).
/// Touches nothing else; one sentence per panel.
pub fn prime(env: &dyn Env, sys: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for t in discover(env, sys) {
        if t.bus != Bus::Usb || boot_interfaces(env, &t).is_empty() {
            continue;
        }
        if let Err(why) = check_safe(env, sys, &t) {
            out.push(format!("{}: not primed: {why}", t.name));
            continue;
        }
        match do_rung(env, sys, &t, Rung::ReportProtocol) {
            Ok(()) => out.push(format!("{}: boot-mouse interface switched to report protocol at first sight", t.name)),
            Err(why) => out.push(format!("{}: report protocol not set: {why}", t.name)),
        }
    }
    for l in &out {
        env.log(&format!("touch-recover: prime: {l}"));
    }
    out
}

pub fn ladder(bus: Bus, module: bool) -> Vec<Rung> {
    let mut v = vec![Rung::Reopen, Rung::HidRebind, if bus == Bus::Usb { Rung::UsbReset } else { Rung::I2cRebind }];
    if module {
        v.push(Rung::ModuleReload);
        if bus == Bus::Usb {
            v.push(Rung::LinkRetrain);
        }
    }
    v
}

/// Link off, 1.5 s, on, then detect again, for every DP connector that reads `connected`. `detect` is written in every case, so a
/// failed step never leaves a connector forced.
fn link_retrain(env: &dyn Env, sys: &Path) -> Result<(), String> {
    let dir = sys.join("class/drm");
    let status: Vec<PathBuf> = env
        .list(&dir)
        .into_iter()
        .filter(|n| crate::touchfix::is_dp_connector(n))
        .map(|n| dir.join(n).join("status"))
        .filter(|p| env.read(p).is_some_and(|v| v.trim() == "connected"))
        .collect();
    if status.is_empty() {
        return Err("no connected DisplayPort connector".into());
    }
    env.log(&format!("touch-recover: {} DisplayPort connector(s) switched off and on; the screen blinks for about 2 to 4 seconds", status.len()));
    let mut failed = Vec::new();
    for p in &status {
        if let Err(e) = env.write(p, "off") {
            failed.push(format!("off: {e}"));
        }
    }
    env.sleep(Duration::from_millis(1500));
    for p in &status {
        if let Err(e) = env.write(p, "on") {
            failed.push(format!("on: {e}"));
        }
    }
    for p in &status {
        if let Err(e) = env.write(p, "detect") {
            failed.push(format!("detect: {e}"));
        }
    }
    env.sleep(Duration::from_millis(2500));
    if failed.is_empty() { Ok(()) } else { Err(failed.join("; ")) }
}

fn bind_cycle(env: &dyn Env, driver_dir: &Path, name: &str) -> io::Result<()> {
    env.write(&driver_dir.join("unbind"), name)?;
    env.sleep(Duration::from_millis(700));
    env.write(&driver_dir.join("bind"), name)?;
    env.sleep(Duration::from_millis(700));
    Ok(())
}

fn do_rung(env: &dyn Env, sys: &Path, t: &Touch, r: Rung) -> Result<(), String> {
    let e = |x: io::Error| x.to_string();
    match r {
        Rung::ReportProtocol => {
            let usb = t.usb.as_ref().ok_or("no USB device")?;
            let ifaces = boot_interfaces(env, t);
            if ifaces.is_empty() {
                return Err("no boot-protocol HID interface: nothing to switch".into());
            }
            let node = usb_node(env, usb).ok_or("busnum/devnum cannot be read")?;
            let id = |f: &str| env.read(&usb.join(f)).and_then(|s| u16::from_str_radix(s.trim(), 16).ok());
            let (vendor, product) = (id("idVendor").ok_or("idVendor cannot be read")?, id("idProduct").ok_or("idProduct cannot be read")?);
            let lines = env.set_report_protocol(&node, vendor, product, &ifaces)?;
            for l in &lines {
                env.log(&format!("touch-recover: {}: {l}", t.name));
            }
            if lines.iter().all(|l| l.contains("refused") || l.contains("cannot")) {
                return Err(lines.join("; "));
            }
            env.sleep(Duration::from_millis(300));
            Ok(())
        }
        Rung::Reopen => {
            env.request_reopen();
            env.sleep(Duration::from_millis(500));
            Ok(())
        }
        Rung::HidRebind => bind_cycle(env, &sys.join("bus/hid/drivers/hid-multitouch"), &t.hid).map_err(e),
        Rung::UsbReset => {
            let usb = t.usb.as_ref().ok_or("no USB device")?;
            let auth = usb.join("authorized");
            // keep it awake while it comes back
            let _ = env.write(&usb.join("power/control"), "on");
            let r = env.write(&auth, "0").and_then(|_| {
                env.sleep(Duration::from_millis(1000));
                env.write(&auth, "1")
            });
            if let Err(first) = r {
                let name = usb.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                // make sure it is not left unauthorized
                let _ = env.write(&auth, "1");
                return bind_cycle(env, &sys.join("bus/usb/drivers/usb"), &name).map_err(|s| format!("authorized: {first}; unbind/bind: {s}"));
            }
            env.sleep(Duration::from_millis(1500));
            Ok(())
        }
        Rung::I2cRebind => {
            let c = t.i2c.as_ref().ok_or("no I2C client")?;
            let drv = env.resolve(&sys.join("bus/i2c/devices").join(c).join("driver")).ok_or("no I2C driver")?;
            bind_cycle(env, &drv, c).map_err(e)
        }
        Rung::LinkRetrain => link_retrain(env, sys),
        Rung::ModuleReload => {
            if env.read(&sys.join("module/hid_multitouch/initstate")).is_none() {
                return Err("hid_multitouch is built into the kernel (no module to reload)".into());
            }
            let bound: Vec<String> = env.list(&sys.join("bus/hid/drivers/hid-multitouch")).into_iter().filter(|n| hid_bus(n).is_some()).collect();
            let screens = discover(env, sys);
            if let Some(other) = bound.iter().find(|b| !screens.iter().any(|s| &s.hid == *b)) {
                return Err(format!("{other} (not a touch screen) is bound to hid-multitouch too: not reloading it"));
            }
            if !env.run("modprobe", &["-r", "hid_multitouch"]) {
                return Err("modprobe -r hid_multitouch failed".into());
            }
            let ok = env.run("modprobe", &["hid_multitouch"]);
            env.sleep(Duration::from_millis(1000));
            if ok { Ok(()) } else { Err("modprobe hid_multitouch failed after the unload".into()) }
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub tried: Vec<Rung>,
    /// Plain sentence per refused or failed thing.
    pub problems: Vec<String>,
    /// The rung after which an event arrived.
    pub fixed_by: Option<Rung>,
    pub found: usize,
}

/// Runs the ladder for every touch screen found. `module`: allow the module reload (manual runs only). `wait`: wait after each step.
pub fn run_ladder(env: &dyn Env, sys: &Path, module: bool, wait: Duration) -> Outcome {
    let mut out = Outcome::default();
    let found = discover(env, sys);
    out.found = found.len();
    for t in found {
        if let Err(why) = check_safe(env, sys, &t) {
            env.log(&format!("touch-recover: refused: {why}"));
            out.problems.push(why);
            continue;
        }
        let mut rungs = ladder(t.bus, module);
        if t.bus == Bus::Usb && !boot_interfaces(env, &t).is_empty() {
            rungs.insert(0, Rung::ReportProtocol);
        }
        for r in rungs {
            // the device may have come back under another HID name: find it again by its ident
            let cur = match discover(env, sys).into_iter().find(|x| x.ident == t.ident) {
                Some(c) => c,
                None => {
                    env.sleep(Duration::from_secs(2));
                    match discover(env, sys).into_iter().find(|x| x.ident == t.ident) {
                        Some(c) => c,
                        None => {
                            out.problems.push(format!("{} did not come back", t.name));
                            break;
                        }
                    }
                }
            };
            if let Err(why) = check_safe(env, sys, &cur) {
                env.log(&format!("touch-recover: refused: {why}"));
                out.problems.push(why);
                break;
            }
            let since = env.events();
            env.log(&format!("touch-recover: {}: step {:?}: {}; touch the screen now", cur.name, r, r.label()));
            out.tried.push(r);
            if let Err(why) = do_rung(env, sys, &cur, r) {
                env.log(&format!("touch-recover: {:?} failed: {why}", r));
                out.problems.push(format!("{}: {why}", r.label()));
                continue;
            }
            if env.wait_events(since, wait) {
                env.log(&format!("touch-recover: {}: an event arrived after {:?}: touch works", cur.name, r));
                out.fixed_by = Some(r);
                return out;
            }
            env.log(&format!("touch-recover: {}: no event after {:?}", cur.name, r));
        }
    }
    out
}

/// The plain status sentence for an outcome.
pub fn describe(o: &Outcome) -> String {
    if o.found == 0 {
        return "No touch screen is listed by the kernel (USB or I2C HID). Nothing was reset.".into();
    }
    if let Some(r) = o.fixed_by {
        return format!("Touch events arrived after: {}. Touch works.", r.label());
    }
    let tried: Vec<&str> = o.tried.iter().map(|r| r.label()).collect();
    let mut s = if tried.is_empty() { "Nothing was reset.".to_string() } else { format!("Tried: {}. No touch event was seen afterwards (touch the screen while it runs; an untouched screen cannot be told from a silent one).", tried.join(", ")) };
    if !o.problems.is_empty() {
        s.push_str(&format!(" Problems: {}.", o.problems.join("; ")));
    }
    s
}

// ---------------------------------------------------------------- detection

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// No touch screen listed.
    Absent,
    /// An event came since it appeared.
    Working,
    /// Present and silent for less than the grace period.
    Waiting(u64),
    /// Present and silent for the grace period or longer.
    Silent,
}

#[derive(Default, Debug)]
pub struct Watch {
    since: Option<u64>,
    base: u64,
    seen: bool,
}

impl Watch {
    /// `now`: seconds (uptime); `present`: a touch screen is listed; `events`: events counted so far.
    pub fn observe(&mut self, now: u64, present: bool, events: u64) -> Verdict {
        if !present {
            self.since = None;
            self.seen = false;
            return Verdict::Absent;
        }
        if self.since.is_none() {
            self.since = Some(now);
            self.base = events;
            self.seen = false;
        }
        if events > self.base {
            self.seen = true;
        }
        if self.seen {
            return Verdict::Working;
        }
        let silent = now.saturating_sub(self.since.unwrap_or(now));
        if silent >= GRACE_SECS { Verdict::Silent } else { Verdict::Waiting(GRACE_SECS - silent) }
    }

    /// A ladder ran: the silence is counted again from now.
    pub fn restart(&mut self, now: u64, events: u64) {
        self.since = Some(now);
        self.base = events;
    }
}

// ---------------------------------------------------------------- rate limits

#[derive(Default, Debug)]
pub struct Limiter {
    auto: Vec<u64>,
    manual: Vec<u64>,
}

impl Limiter {
    /// May a run start now? Records it when yes.
    pub fn allow(&mut self, manual: bool, now: u64) -> Result<(), String> {
        if manual {
            if let Some(l) = self.manual.last() {
                if now < l + MANUAL_GAP {
                    return Err(format!("Reset touch screen was run {} s ago; wait {} s before the next try.", now - l, l + MANUAL_GAP - now));
                }
            }
            let recent = self.manual.iter().filter(|t| now < **t + 3600).count();
            if recent >= MANUAL_PER_HOUR {
                return Err(format!("Reset touch screen was run {MANUAL_PER_HOUR} times in the last hour; wait before trying again."));
            }
            self.manual.push(now);
        } else {
            let n = self.auto.len();
            if n >= AUTO_MAX {
                return Err("the automatic tries of this boot are used up".into());
            }
            if let Some(l) = self.auto.last() {
                if now < l + AUTO_GAPS[n] {
                    return Err(format!("next automatic try in {} s", l + AUTO_GAPS[n] - now));
                }
            }
            self.auto.push(now);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- the running parts

struct Live {
    watch: Watch,
    limiter: Limiter,
    running: bool,
}

static LIVE: Mutex<Live> = Mutex::new(Live { watch: Watch { since: None, base: 0, seen: false }, limiter: Limiter { auto: Vec::new(), manual: Vec::new() }, running: false });
static LAST_TICK: AtomicU64 = AtomicU64::new(0);

/// `iyakki.touchrecover=off` turns the automatic run off.
pub fn auto_enabled(cmdline: &str) -> bool {
    !cmdline.split_whitespace().any(|w| w == "iyakki.touchrecover=off")
}

/// Called from the startup poll with the uptime. Never waits: a ladder runs in a thread.
pub fn tick(uptime_secs: u64) {
    if uptime_secs < LAST_TICK.load(Relaxed) + 5 && LAST_TICK.load(Relaxed) != 0 {
        return;
    }
    LAST_TICK.store(uptime_secs.max(1), Relaxed);
    let present = !discover(&RealEnv, Path::new("/sys")).is_empty();
    let mut l = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let first_sight = present && l.watch.since.is_none();
    let v = l.watch.observe(uptime_secs, present, event_count());
    if first_sight && !l.running && auto_enabled(&iyakki_storage::cmdline_text()) {
        // the protocol switch at first sight: no waiting, no re-enumeration, nothing to rate-limit
        std::thread::spawn(|| {
            for s in prime(&RealEnv, Path::new("/sys")) {
                log::info!("audit: touch.recover prime: {s}");
            }
        });
    }
    if v != Verdict::Silent || l.running || !auto_enabled(&iyakki_storage::cmdline_text()) {
        return;
    }
    if l.limiter.allow(false, uptime_secs).is_err() {
        return;
    }
    l.running = true;
    let n = l.limiter.auto.len();
    drop(l);
    std::thread::spawn(move || {
        log::info!("audit: touch.recover automatic try {n}: a touch screen is listed and sent no event for {GRACE_SECS} s");
        let o = run_ladder(&RealEnv, Path::new("/sys"), false, Duration::from_secs(WAIT_SECS));
        log::info!("audit: touch.recover automatic try {n} ended: {}", describe(&o));
        let mut l = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        l.running = false;
        l.watch.restart(LAST_TICK.load(Relaxed), event_count());
    });
}

/// The Reset touch screen button: blocks for a few seconds per step. Returns the status sentence.
pub fn reset_now(uptime_secs: u64) -> String {
    {
        let mut l = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        if l.running {
            return "A reset is already running.".into();
        }
        if let Err(why) = l.limiter.allow(true, uptime_secs) {
            return why;
        }
        l.running = true;
    }
    log::info!("audit: touch.recover manual run");
    let o = run_ladder(&RealEnv, Path::new("/sys"), true, Duration::from_secs(WAIT_SECS));
    let s = describe(&o);
    log::info!("audit: touch.recover manual run ended: {s}");
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = s.clone();
    let mut l = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    l.running = false;
    l.watch.restart(uptime_secs, event_count());
    s
}

#[cfg(test)]
#[path = "touchrecover_tests.rs"]
mod tests;
