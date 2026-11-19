//! Hotplug: drivers for devices that appear after the coldplug, and the network setup of interfaces that appear later.
//!
//! BusyBox `mdev` is the kernel's hotplug helper, but without an `/etc/mdev.conf` rule it creates device nodes only: it never loads a
//! driver. A USB Ethernet adapter plugged in after boot therefore got no driver (`lsmod` unchanged, no `ethN`), and even after the
//! driver was loaded by hand nothing brought the new interface up (no link, no DHCP). Measured on a MacBookPro16,2.
//!
//! This listens on the kernel's uevent socket (read only, like `display.rs` and `camera.rs`) and does two things:
//! * an `add` event with a `MODALIAS` goes to `modprobe -b -a` (blacklists, like the T2 hold-back of Wi-Fi/Bluetooth drivers and the
//!   camera drivers IYAKKI loads itself, are honoured); events that arrive together are loaded in one call;
//! * a new Ethernet or Wi-Fi interface is set up like at boot ([`crate::net::bring_up_new`]); a vanished one is forgotten
//!   ([`crate::net::interface_gone`]) so the same name comes up again when it returns.
//!
//! The decisions are pure ([`Hotplug::feed`]) and the effects sit behind [`Effects`], so the tests use fake uevents.

use std::collections::BTreeSet;
use std::process::Command;

/// One kernel event, reduced to what is used here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Uevent {
    pub action: String,
    pub subsystem: String,
    pub devpath: String,
    pub modalias: Option<String>,
    pub interface: Option<String>,
}

/// Parse a kernel uevent message (`add@/devices/...\0ACTION=add\0DEVPATH=...\0...`). Messages from user space (`libudev`) and
/// anything without an action are `None`.
pub fn parse(msg: &[u8]) -> Option<Uevent> {
    if msg.starts_with(b"libudev") {
        return None;
    }
    let mut ev = Uevent::default();
    for f in msg.split(|b| *b == 0) {
        let Ok(s) = std::str::from_utf8(f) else { continue };
        let Some((k, v)) = s.split_once('=') else { continue };
        match k {
            "ACTION" => ev.action = v.to_string(),
            "SUBSYSTEM" => ev.subsystem = v.to_string(),
            "DEVPATH" => ev.devpath = v.to_string(),
            "MODALIAS" if !v.is_empty() => ev.modalias = Some(v.to_string()),
            "INTERFACE" if !v.is_empty() => ev.interface = Some(v.to_string()),
            _ => {}
        }
    }
    (!ev.action.is_empty()).then_some(ev)
}

/// What the events cause. Implemented for the real machine in [`System`], by a recorder in the tests.
pub trait Effects {
    /// `modprobe -b -a` for these aliases.
    fn load(&mut self, aliases: &[String]);
    /// A network interface appeared (a physical one; virtual devices are filtered out before).
    fn net_added(&mut self, name: &str);
    /// A network interface is gone.
    fn net_removed(&mut self, name: &str);
}

/// Collects events and applies them in batches.
#[derive(Default)]
pub struct Hotplug {
    aliases: BTreeSet<String>,
    added: Vec<String>,
    removed: Vec<String>,
}

impl Hotplug {
    pub fn new() -> Hotplug {
        Hotplug::default()
    }

    /// Take one kernel message into the batch.
    pub fn feed(&mut self, msg: &[u8]) {
        let Some(ev) = parse(msg) else { return };
        match ev.action.as_str() {
            "add" => {
                if let Some(a) = ev.modalias {
                    self.aliases.insert(a);
                }
                if ev.subsystem == "net" && !ev.devpath.starts_with("/devices/virtual/") {
                    if let Some(i) = ev.interface.or_else(|| ev.devpath.rsplit('/').next().map(String::from)) {
                        self.added.push(i);
                    }
                }
            }
            "remove" if ev.subsystem == "net" && !ev.devpath.starts_with("/devices/virtual/") => {
                if let Some(i) = ev.interface.or_else(|| ev.devpath.rsplit('/').next().map(String::from)) {
                    self.removed.push(i);
                }
            }
            _ => {}
        }
    }

    pub fn pending(&self) -> bool {
        !(self.aliases.is_empty() && self.added.is_empty() && self.removed.is_empty())
    }

    /// Apply the batch: gone interfaces first, then the drivers (their children may bring new interfaces, which arrive as new events),
    /// then the interfaces that appeared.
    pub fn flush(&mut self, fx: &mut dyn Effects) {
        for n in std::mem::take(&mut self.removed) {
            fx.net_removed(&n);
        }
        if !self.aliases.is_empty() {
            let a: Vec<String> = std::mem::take(&mut self.aliases).into_iter().collect();
            for chunk in a.chunks(64) {
                fx.load(chunk);
            }
        }
        for n in std::mem::take(&mut self.added) {
            fx.net_added(&n);
        }
    }
}

/// The real effects.
pub struct System;

impl Effects for System {
    fn load(&mut self, aliases: &[String]) {
        // Until boot has finished its own order (keyboard and storage first, network and sound after the sign-in screen), the
        // aliases of network, sound and the like wait; the rest (a keyboard that shows up late) load at once.
        let (now, later): (Vec<String>, Vec<String>) = if LATE_READY.load(std::sync::atomic::Ordering::SeqCst) {
            (aliases.to_vec(), Vec::new())
        } else {
            aliases.iter().cloned().partition(|a| !crate::boot::is_deferred(a))
        };
        if !later.is_empty() {
            HELD.lock().unwrap_or_else(|e| e.into_inner()).extend(later);
        }
        if !now.is_empty() {
            modprobe(&now);
        }
    }
    fn net_added(&mut self, name: &str) {
        if !crate::net::NETWORK_READY.load(std::sync::atomic::Ordering::SeqCst) {
            return; // boot sets the network up itself, once, for everything that exists then
        }
        set_up_interfaces(Some(name));
    }
    fn net_removed(&mut self, name: &str) {
        crate::net::interface_gone(name);
    }
}

static LATE_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static HELD: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn modprobe(aliases: &[String]) {
    let _ = Command::new("modprobe").arg("-b").arg("-a").args(aliases).stderr(std::process::Stdio::null()).status();
}

fn set_up_interfaces(only: Option<&str>) {
    match crate::net::bring_up_new(std::path::Path::new("/etc/iyakki/net.conf"), std::path::Path::new(crate::net::SYS_CLASS_NET), std::path::Path::new(crate::net::KNOWN_WIFI), only) {
        Ok(done) if !done.started.is_empty() => {
            log::info!("hotplug: network interface set up: {}", done.started.join(", "));
            iyakki_fw::service::network_changed();
        }
        Ok(_) => {}
        Err(e) => log::warn!("hotplug: network config: {e}"),
    }
}

/// Boot has loaded its drivers and set the network up: load what the listener held back, and set up interfaces that appeared
/// while boot was busy and were not part of its lists. From here on every event is handled at once.
pub fn boot_done() {
    LATE_READY.store(true, std::sync::atomic::Ordering::SeqCst);
    let held: Vec<String> = std::mem::take(&mut *HELD.lock().unwrap_or_else(|e| e.into_inner()));
    for chunk in held.chunks(64) {
        modprobe(chunk);
    }
    crate::net::NETWORK_READY.store(true, std::sync::atomic::Ordering::SeqCst);
    set_up_interfaces(None);
}

/// Start the listener in a background thread. Called early (the coldplug scans cover devices that exist, this covers every later one;
/// the drivers the camera code holds back stay blacklisted).
pub fn start() {
    let r = std::thread::Builder::new().name("iyakki-hotplug".into()).spawn(|| {
        let Some(fd) = open_events() else { return };
        run(fd, &mut System);
    });
    if let Err(e) = r {
        log::warn!("hotplug: could not start the listener ({e}); devices plugged in later get no driver");
    }
}

#[cfg(target_os = "linux")]
fn open_events() -> Option<i32> {
    // SAFETY: plain socket calls with constant arguments; the descriptor is checked and closed on failure.
    let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, libc::NETLINK_KOBJECT_UEVENT) };
    if fd < 0 {
        log::warn!("hotplug: no kernel event socket ({})", std::io::Error::last_os_error());
        return None;
    }
    // SAFETY: an all-zero sockaddr_nl is valid; the fields set are plain integers.
    let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    addr.nl_family = libc::AF_NETLINK as u16;
    addr.nl_groups = 1; // the kernel's uevent group
    // SAFETY: `addr` is valid for the call; its size is passed.
    let bound = unsafe { libc::bind(fd, (&addr as *const libc::sockaddr_nl).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32) };
    // a short receive timeout ends a batch: events that arrive together (all interfaces of one USB device) are handled together
    let tv = libc::timeval { tv_sec: 0, tv_usec: 250_000 };
    // SAFETY: `tv` is a valid timeval for the call.
    let opt = unsafe { libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_RCVTIMEO, (&tv as *const libc::timeval).cast(), std::mem::size_of::<libc::timeval>() as u32) };
    if bound < 0 || opt < 0 {
        log::warn!("hotplug: kernel event socket: {}", std::io::Error::last_os_error());
        // SAFETY: closing the descriptor opened above.
        unsafe { libc::close(fd) };
        return None;
    }
    Some(fd)
}

#[cfg(not(target_os = "linux"))]
fn open_events() -> Option<i32> {
    None
}

#[cfg(target_os = "linux")]
fn run(fd: i32, fx: &mut dyn Effects) {
    let mut buf = vec![0u8; 16384];
    let mut hp = Hotplug::new();
    loop {
        // SAFETY: `buf` is writable for its length.
        let n = unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n > 0 {
            hp.feed(&buf[..n as usize]);
            continue;
        }
        // timeout (or an error): the batch is complete
        if hp.pending() {
            hp.flush(fx);
        } else if n < 0 && !matches!(std::io::Error::last_os_error().kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn run(_fd: i32, _fx: &mut dyn Effects) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Rec {
        log: Vec<String>,
    }
    impl Effects for Rec {
        fn load(&mut self, a: &[String]) {
            self.log.push(format!("load {}", a.join(" ")));
        }
        fn net_added(&mut self, n: &str) {
            self.log.push(format!("added {n}"));
        }
        fn net_removed(&mut self, n: &str) {
            self.log.push(format!("removed {n}"));
        }
    }

    fn ev(action: &str, subsystem: &str, devpath: &str, extra: &[&str]) -> Vec<u8> {
        let mut v = format!("{action}@{devpath}\0ACTION={action}\0DEVPATH={devpath}\0SUBSYSTEM={subsystem}\0").into_bytes();
        for e in extra {
            v.extend_from_slice(e.as_bytes());
            v.push(0);
        }
        v
    }

    #[test]
    fn a_usb_network_adapter_gets_its_driver_and_then_its_interface() {
        let mut hp = Hotplug::new();
        hp.feed(&ev("add", "usb", "/devices/pci0000:00/0000:00:14.0/usb3/3-1/3-1.4", &["DEVTYPE=usb_device", "PRODUCT=bda/8153/3000"]));
        hp.feed(&ev("add", "usb", "/devices/pci0000:00/0000:00:14.0/usb3/3-1/3-1.4/3-1.4:1.0", &["DEVTYPE=usb_interface", "MODALIAS=usb:v0BDAp8153d3000dc00dsc00dp00ic02isc06ip00in00"]));
        hp.feed(&ev("add", "net", "/devices/pci0000:00/0000:00:14.0/usb3/3-1/3-1.4/3-1.4:1.0/net/eth0", &["INTERFACE=eth0"]));
        let mut fx = Rec::default();
        assert!(hp.pending());
        hp.flush(&mut fx);
        assert_eq!(fx.log, vec!["load usb:v0BDAp8153d3000dc00dsc00dp00ic02isc06ip00in00", "added eth0"]);
        assert!(!hp.pending());
        hp.flush(&mut fx);
        assert_eq!(fx.log.len(), 2, "nothing is repeated");
    }

    #[test]
    fn events_that_arrive_together_load_in_one_call_and_each_alias_once() {
        let mut hp = Hotplug::new();
        for a in ["hid:b0003g0001v000005ACp0000027E", "hid:b0003g0001v000005ACp00008102", "hid:b0003g0001v000005ACp0000027E"] {
            hp.feed(&ev("add", "hid", "/devices/x/0003:05AC:027E.0007", &[&format!("MODALIAS={a}")]));
        }
        let mut fx = Rec::default();
        hp.flush(&mut fx);
        assert_eq!(fx.log, vec!["load hid:b0003g0001v000005ACp0000027E hid:b0003g0001v000005ACp00008102"]);
    }

    #[test]
    fn removals_virtual_devices_and_user_space_messages_do_not_load_anything() {
        let mut hp = Hotplug::new();
        hp.feed(&ev("remove", "usb", "/devices/a/1-1:1.0", &["MODALIAS=usb:v1p2"]));
        hp.feed(&ev("change", "drm", "/devices/a/card0", &["MODALIAS=pci:x"]));
        hp.feed(&ev("add", "net", "/devices/virtual/net/veth1234", &["INTERFACE=veth1234"]));
        hp.feed(&ev("remove", "net", "/devices/virtual/net/veth1234", &["INTERFACE=veth1234"]));
        hp.feed(b"libudev\0\xfe\xed\xca\xfe\0ACTION=add\0MODALIAS=evil\0");
        hp.feed(b"garbage");
        assert!(!hp.pending());
    }

    #[test]
    fn a_vanished_interface_is_reported_before_the_same_name_comes_back() {
        let mut hp = Hotplug::new();
        hp.feed(&ev("remove", "net", "/devices/pci0000:00/usb3/3-1.4/3-1.4:1.0/net/eth0", &["INTERFACE=eth0"]));
        hp.feed(&ev("add", "net", "/devices/pci0000:00/usb3/3-1.4/3-1.4:1.0/net/eth0", &["INTERFACE=eth0"]));
        let mut fx = Rec::default();
        hp.flush(&mut fx);
        assert_eq!(fx.log, vec!["removed eth0", "added eth0"]);
    }

    #[test]
    fn the_listener_starts_before_the_coldplug_so_late_children_are_not_missed() {
        let boot = include_str!("boot.rs");
        let at = |s: &str| boot.find(s).unwrap_or_else(|| panic!("{s} not in boot.rs"));
        assert!(at("crate::hotplug::start()") < at("load_aliases(&critical"));
        assert!(at("crate::camera::block_autoload()") < at("crate::hotplug::start()"), "the camera drivers are blacklisted before anything can load by alias");
        assert!(at("t2hold::apply") < at("crate::hotplug::start()"), "the T2 hold-back is in place first");
    }
}
