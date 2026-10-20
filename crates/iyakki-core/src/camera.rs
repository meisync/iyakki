//! USB cameras that need a `uvcvideo` quirk, applied at boot (coldplug) and when one is plugged in (hotplug). The rules
//! (the table of known cameras, the order of the sysfs writes, the fallback) are in `iyakki_drivers::camquirk`; this file
//! only runs them: [`block_autoload`] before anything can load the camera driver, [`prepare_boot`] before the deferred
//! drivers, then a background thread for the cameras already there and for kernel `add` events after.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use iyakki_drivers::camquirk::{self, RealSysfs};

/// Very early in boot, before the kernel's hotplug helper is set and before any coldplug: keep `uvcvideo` and
/// `snd-usb-audio` from being loaded by alias, so IYAKKI loads them itself after holding back cameras that need a quirk.
pub fn block_autoload() {
    if let Err(e) = camquirk::block_autoload(Path::new("/etc/modprobe.d")) {
        log::warn!("camera: could not write the modprobe file ({e}); a camera that needs a quirk may be probed without it");
    }
}

/// Before the deferred drivers load at boot: known cameras lose their microphone and are held back, then `uvcvideo` and
/// `snd-usb-audio` are loaded by name (only when such devices are present).
pub fn prepare_boot() {
    for l in camquirk::prepare_boot(&RealSysfs::system(), &iyakki_drivers::run::System) {
        log::info!("{l}");
    }
}

/// Is this kernel event (`add@/devices/...\0ACTION=add\0SUBSYSTEM=usb\0DEVTYPE=usb_device\0...`) a new USB device?
/// Returns its sysfs name (`1-8`).
pub fn usb_device_added(msg: &[u8]) -> Option<String> {
    let fields: Vec<&[u8]> = msg.split(|b| *b == 0).collect();
    let has = |f: &[u8]| fields.contains(&f);
    if !(has(b"ACTION=add") && has(b"SUBSYSTEM=usb") && has(b"DEVTYPE=usb_device")) {
        return None;
    }
    let path = fields.iter().find_map(|f| f.strip_prefix(b"DEVPATH=".as_slice()))?;
    let name = String::from_utf8_lossy(path).rsplit('/').next()?.to_string();
    (!name.is_empty() && !name.contains(':')).then_some(name)
}

/// Devices being handled right now (a camera plugged in while the boot scan runs is not handled twice at once).
static BUSY: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Claim a device for handling; false when it is already being handled.
fn claim(dev: &str) -> bool {
    let mut g = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashSet::new).insert(dev.to_string())
}

fn release(dev: &str) {
    if let Some(s) = BUSY.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        s.remove(dev);
    }
}

fn handle(dev: &str) {
    if !claim(dev) {
        return;
    }
    let lines = camquirk::handle_device(&RealSysfs::system(), &iyakki_drivers::run::System, dev, &kernel_log, &mut std::thread::sleep);
    release(dev);
    for l in lines {
        log::info!("{l}");
    }
}

/// The kernel log ring buffer as text (what `dmesg` shows).
#[cfg(target_os = "linux")]
fn kernel_log() -> String {
    const SYSLOG_ACTION_READ_ALL: libc::c_int = 3;
    const SYSLOG_ACTION_SIZE_BUFFER: libc::c_int = 10;
    // SAFETY: SIZE_BUFFER takes no buffer.
    let size = unsafe { libc::klogctl(SYSLOG_ACTION_SIZE_BUFFER, std::ptr::null_mut(), 0) };
    let mut buf = vec![0u8; size.clamp(16 * 1024, 4 * 1024 * 1024) as usize];
    // SAFETY: `buf` is writable for its length, which is passed.
    let n = unsafe { libc::klogctl(SYSLOG_ACTION_READ_ALL, buf.as_mut_ptr().cast(), buf.len() as libc::c_int) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buf[..n as usize]).into_owned()
}

#[cfg(not(target_os = "linux"))]
fn kernel_log() -> String {
    String::new()
}

/// After the camera drivers loaded at boot: listen for new USB devices, look at the ones already there, then handle
/// every camera plugged in later. All in background threads; never holds up boot.
pub fn start() {
    let spawned = std::thread::Builder::new().name("iyakki-camera".into()).spawn(|| {
        // the event socket first, so a camera plugged in during the scan is not missed
        let fd = open_events();
        // known cameras first, each device in its own thread: one slow camera never delays another
        let sys = RealSysfs::system();
        let mut devs = camquirk::usb_devices(&sys);
        devs.sort_by_key(|d| camquirk::ids(&sys, d).and_then(|id| camquirk::known(id.0, id.1)).is_none());
        for dev in devs {
            let _ = std::thread::Builder::new().name("iyakki-camera-dev".into()).spawn(move || handle(&dev));
        }
        if let Some(fd) = fd {
            watch(fd);
        }
    });
    if let Err(e) = spawned {
        log::warn!("camera: could not start the camera quirk watch ({e})");
    }
}

#[cfg(target_os = "linux")]
fn open_events() -> Option<i32> {
    // SAFETY: plain socket calls with constant arguments; the descriptor is checked and closed on failure.
    let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, libc::NETLINK_KOBJECT_UEVENT) };
    if fd < 0 {
        log::warn!("camera: no kernel event socket ({}); cameras plugged in later get no quirks", std::io::Error::last_os_error());
        return None;
    }
    // SAFETY: an all-zero sockaddr_nl is valid; the fields set are plain integers.
    let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    addr.nl_family = libc::AF_NETLINK as u16;
    addr.nl_groups = 1; // the kernel's uevent group
    // SAFETY: `addr` is a valid sockaddr_nl for the duration of the call and its size is passed.
    if unsafe { libc::bind(fd, (&addr as *const libc::sockaddr_nl).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32) } < 0 {
        log::warn!("camera: kernel event socket: {}", std::io::Error::last_os_error());
        // SAFETY: closing the descriptor opened above.
        unsafe { libc::close(fd) };
        return None;
    }
    Some(fd)
}

#[cfg(target_os = "linux")]
fn watch(fd: i32) {
    let mut buf = vec![0u8; 8192];
    loop {
        // SAFETY: `buf` is writable for its length.
        let n = unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n <= 0 {
            continue;
        }
        if let Some(dev) = usb_device_added(&buf[..n as usize]) {
            let _ = std::thread::Builder::new().name("iyakki-camera-dev".into()).spawn(move || handle(&dev));
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn open_events() -> Option<i32> {
    None
}

#[cfg(not(target_os = "linux"))]
fn watch(_fd: i32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_new_usb_devices_are_picked_from_kernel_events() {
        let add = b"add@/devices/pci0000:00/0000:00:14.0/usb1/1-8\0ACTION=add\0DEVPATH=/devices/pci0000:00/0000:00:14.0/usb1/1-8\0SUBSYSTEM=usb\0DEVTYPE=usb_device\0PRODUCT=1d6c/103/1\0SEQNUM=4100\0";
        assert_eq!(usb_device_added(add).as_deref(), Some("1-8"));
        let iface = b"add@/devices/pci0000:00/0000:00:14.0/usb1/1-8/1-8:1.0\0ACTION=add\0DEVPATH=/devices/pci0000:00/0000:00:14.0/usb1/1-8/1-8:1.0\0SUBSYSTEM=usb\0DEVTYPE=usb_interface\0";
        assert_eq!(usb_device_added(iface), None, "interfaces are found from the device");
        let bind = b"bind@/devices/pci0000:00/0000:00:14.0/usb1/1-8\0ACTION=bind\0DEVPATH=/devices/pci0000:00/0000:00:14.0/usb1/1-8\0SUBSYSTEM=usb\0DEVTYPE=usb_device\0";
        assert_eq!(usb_device_added(bind), None, "our own (re)binding does not start another round");
        assert_eq!(usb_device_added(b"add@/x\0ACTION=add\0SUBSYSTEM=drm\0DEVTYPE=usb_device\0DEVPATH=/x/1-1\0"), None);
        assert_eq!(usb_device_added(b"libudev\0garbage"), None);
    }

    #[test]
    fn boot_blocks_autoload_before_the_hotplug_helper_and_holds_cameras_back_before_the_deferred_drivers() {
        let boot = include_str!("boot.rs");
        let at = |s: &str| boot.find(s).unwrap_or_else(|| panic!("{s} not in boot.rs"));
        assert!(at("crate::camera::block_autoload()") < at("/proc/sys/kernel/hotplug"), "before mdev can load uvcvideo");
        assert!(at("crate::camera::block_autoload()") < at("load_aliases(&critical"), "before the coldplug");
        assert!(at("crate::camera::prepare_boot()") < at("load_aliases(&plan.deferred"));
        assert!(at("load_aliases(&plan.deferred") < at("crate::camera::start()"));
    }

    #[test]
    fn a_device_is_handled_by_one_thread_at_a_time() {
        assert!(claim("9-9.9"));
        assert!(!claim("9-9.9"), "the boot scan and a hotplug event do not both handle it");
        release("9-9.9");
        assert!(claim("9-9.9"));
        release("9-9.9");
    }
}
