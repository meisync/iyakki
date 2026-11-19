//! The real device behind the [`touchbar`](crate::touchbar) seams: usbfs ioctls for the display interface and sysfs for the device's
//! configuration, plus the one device-level reset. Raw `libc::ioctl` on purpose (no USB library): four requests, listed in
//! `BOUNDARY.md`. Nothing here ever runs on its own: the Touch Bar code calls it from a background thread and only for the device whose
//! id was read from sysfs.
//!
//! Safety rules kept here (the HID protocol switch at the end follows them for its own device): the node opened is the one sysfs named, and its own device descriptor is read back and must be `05ac:8302`
//! before any ioctl; the claim fails (EBUSY) rather than taking an interface from another driver (there is no "disconnect driver"
//! request in this file); a reset is only sent to a node that passed both checks.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use crate::touchbar::{Found, Port, Usb, UsbError, DISPLAY_PRODUCT, VENDOR};

const fn ioc(dir: u32, nr: u32, size: u32) -> u32 {
    (dir << 30) | (size << 16) | ((b'U' as u32) << 8) | nr
}
const READ: u32 = 2;
const RW: u32 = 3;
const NONE: u32 = 0;

/// `struct usbdevfs_bulktransfer { unsigned ep; unsigned len; unsigned timeout; void *data; }`
#[repr(C)]
struct Bulk {
    ep: u32,
    len: u32,
    timeout: u32,
    data: *mut libc::c_void,
}

pub const USBDEVFS_BULK: u32 = ioc(RW, 2, std::mem::size_of::<Bulk>() as u32);
pub const USBDEVFS_CLAIMINTERFACE: u32 = ioc(READ, 15, 4);
pub const USBDEVFS_RELEASEINTERFACE: u32 = ioc(READ, 16, 4);
pub const USBDEVFS_SETCONFIGURATION: u32 = ioc(READ, 5, 4);
pub const USBDEVFS_RESET: u32 = ioc(NONE, 20, 0);
pub const USBDEVFS_CLEAR_HALT: u32 = ioc(READ, 21, 4);

fn last_error() -> UsbError {
    UsbError::from_errno(std::io::Error::last_os_error().raw_os_error().unwrap_or(5))
}

/// An open usbfs node.
pub struct UsbFs {
    file: std::fs::File,
}

impl UsbFs {
    /// Open the node and check that it is a Touch Bar display (the device descriptor read back: vendor 05ac, product 8302).
    pub fn open(node: &Path) -> Result<UsbFs, UsbError> {
        let mut file = std::fs::OpenOptions::new().read(true).write(true).open(node).map_err(|e| UsbError::from_errno(e.raw_os_error().unwrap_or(5)))?;
        let mut d = [0u8; 18];
        file.read_exact(&mut d).map_err(|_| UsbError::Other(5))?;
        if u16::from_le_bytes([d[8], d[9]]) != VENDOR || u16::from_le_bytes([d[10], d[11]]) != DISPLAY_PRODUCT {
            return Err(UsbError::Other(19));
        }
        Ok(UsbFs { file })
    }

    fn ioctl_uint(&mut self, req: u32, v: u32) -> Result<(), UsbError> {
        let mut x = v;
        // SAFETY: the request takes a pointer to one unsigned int, which lives for the call.
        let r = unsafe { libc::ioctl(self.file.as_raw_fd(), req as _, &mut x as *mut u32) };
        if r < 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }

    /// Select a configuration (the kernel unbinds the old interfaces' drivers and binds the new ones).
    pub fn set_configuration(&mut self, value: u8) -> Result<(), UsbError> {
        self.ioctl_uint(USBDEVFS_SETCONFIGURATION, value as u32)
    }

    /// Reset this device (a port reset and re-enumeration).
    pub fn reset(&mut self) -> Result<(), UsbError> {
        // SAFETY: USBDEVFS_RESET takes no argument.
        let r = unsafe { libc::ioctl(self.file.as_raw_fd(), USBDEVFS_RESET as _, 0) };
        if r < 0 {
            Err(last_error())
        } else {
            Ok(())
        }
    }

    fn bulk(&mut self, ep: u8, data: *mut u8, len: usize, timeout_ms: u32) -> Result<usize, UsbError> {
        let mut b = Bulk { ep: ep as u32, len: len.min(1 << 20) as u32, timeout: timeout_ms, data: data as *mut libc::c_void };
        // SAFETY: `data` points at `len` valid bytes (the callers pass a slice) that stay alive for the call; the kernel reads or
        // writes at most `len` bytes of it and returns how many.
        let r = unsafe { libc::ioctl(self.file.as_raw_fd(), USBDEVFS_BULK as _, &mut b as *mut Bulk) };
        if r < 0 {
            Err(last_error())
        } else {
            Ok(r as usize)
        }
    }
}

impl Usb for UsbFs {
    fn claim(&mut self, interface: u8) -> Result<(), UsbError> {
        self.ioctl_uint(USBDEVFS_CLAIMINTERFACE, interface as u32)
    }
    fn release(&mut self, interface: u8) {
        let _ = self.ioctl_uint(USBDEVFS_RELEASEINTERFACE, interface as u32);
    }
    fn bulk_out(&mut self, ep: u8, data: &[u8], timeout_ms: u32) -> Result<usize, UsbError> {
        // the kernel only reads from an OUT transfer; the cast is for the shared struct
        self.bulk(ep, data.as_ptr() as *mut u8, data.len(), timeout_ms)
    }
    fn bulk_in(&mut self, ep: u8, buf: &mut [u8], timeout_ms: u32) -> Result<usize, UsbError> {
        self.bulk(ep, buf.as_mut_ptr(), buf.len(), timeout_ms)
    }
    fn clear_halt(&mut self, ep: u8) -> Result<(), UsbError> {
        self.ioctl_uint(USBDEVFS_CLEAR_HALT, ep as u32)
    }
}

/// The Touch Bar as a whole, through sysfs (configuration) and its own usbfs node (reset).
pub struct SysPort {
    pub found: Found,
    pub dev_root: PathBuf,
}

impl SysPort {
    pub fn new(found: Found, dev_root: PathBuf) -> SysPort {
        SysPort { found, dev_root }
    }

    fn read(&self, f: &str) -> String {
        std::fs::read_to_string(self.found.sysfs.join(f)).unwrap_or_default().trim().to_string()
    }

    /// Still the same device: the folder exists with the same ids and the same bus and device numbers (a replug changes the number).
    fn same_device(&self) -> bool {
        self.read("idVendor") == "05ac" && self.read("idProduct") == "8302" && self.read("busnum").parse() == Ok(self.found.bus) && self.read("devnum").parse() == Ok(self.found.dev)
    }
}

impl Port for SysPort {
    fn present(&mut self) -> bool {
        self.same_device()
    }
    fn configured(&mut self) -> Option<u8> {
        self.read("bConfigurationValue").parse().ok()
    }
    fn set_configuration(&mut self, value: u8) -> Result<(), UsbError> {
        if !self.same_device() {
            return Err(UsbError::NoDevice);
        }
        let mut f = std::fs::OpenOptions::new().write(true).open(self.found.sysfs.join("bConfigurationValue")).map_err(|e| UsbError::from_errno(e.raw_os_error().unwrap_or(5)))?;
        f.write_all(value.to_string().as_bytes()).map_err(|e| UsbError::from_errno(e.raw_os_error().unwrap_or(5)))
    }
    fn reset(&mut self) -> Result<(), UsbError> {
        if !self.same_device() {
            return Err(UsbError::NoDevice);
        }
        UsbFs::open(&self.found.usbfs_path(&self.dev_root))?.reset()
    }
    fn sleep_ms(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    fn switch_configuration(&mut self, value: u8) -> Result<(), UsbError> {
        if !self.same_device() {
            return Err(UsbError::NoDevice);
        }
        UsbFs::open(&self.found.usbfs_path(&self.dev_root))?.set_configuration(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_numbers_are_the_kernels() {
        // from <linux/usbdevice_fs.h> on a 64-bit machine: _IOWR('U',2,struct usbdevfs_bulktransfer) with a 24-byte struct, _IOR('U',15,uint), ...
        if std::mem::size_of::<usize>() == 8 {
            assert_eq!(USBDEVFS_BULK, 0xC018_5502);
        }
        assert_eq!(USBDEVFS_CLAIMINTERFACE, 0x8004_550F);
        assert_eq!(USBDEVFS_RELEASEINTERFACE, 0x8004_5510);
        assert_eq!(USBDEVFS_SETCONFIGURATION, 0x8004_5505);
        assert_eq!(USBDEVFS_RESET, 0x0000_5514);
        assert_eq!(USBDEVFS_CLEAR_HALT, 0x8004_5515);
    }

    fn fake(tag: &str, product: &str, dev: &str) -> (PathBuf, Found) {
        let d = std::env::temp_dir().join(format!("usbfs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("5-6")).unwrap();
        for (f, v) in [("idVendor", "05ac"), ("idProduct", product), ("busnum", "5"), ("devnum", dev), ("bConfigurationValue", "")] {
            std::fs::write(d.join("5-6").join(f), format!("{v}\n")).unwrap();
        }
        let found = Found { sysfs: d.join("5-6"), bus: 5, dev: 7, configured: None };
        (d, found)
    }

    #[test]
    fn the_port_only_writes_to_the_device_it_was_given_and_notices_a_replug() {
        let (d, found) = fake("port", "8302", "7");
        let mut p = SysPort::new(found, d.join("dev"));
        assert!(p.present());
        assert_eq!(p.configured(), None);
        assert!(p.set_configuration(1).is_ok());
        assert_eq!(std::fs::read_to_string(d.join("5-6/bConfigurationValue")).unwrap(), "1");
        assert_eq!(p.configured(), Some(1));
        // replugged: the device number changed, nothing may be written or reset
        std::fs::write(d.join("5-6/devnum"), "9\n").unwrap();
        assert!(!p.present());
        assert_eq!(p.set_configuration(2), Err(UsbError::NoDevice));
        assert_eq!(p.reset(), Err(UsbError::NoDevice));
        assert_eq!(p.switch_configuration(2), Err(UsbError::NoDevice));
        assert_eq!(std::fs::read_to_string(d.join("5-6/bConfigurationValue")).unwrap(), "1");
        // another product in that folder is not ours
        let (d2, f2) = fake("other", "8102", "7");
        let mut p2 = SysPort::new(f2, d2.join("dev"));
        assert!(!p2.present());
        assert_eq!(p2.set_configuration(1), Err(UsbError::NoDevice));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }

    #[test]
    fn a_node_that_is_not_a_touch_bar_is_refused_before_any_ioctl() {
        let d = std::env::temp_dir().join(format!("usbfs-node-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // a device descriptor of a Realtek ethernet adapter, and a Touch Bar's
        let desc = |v: u16, p: u16| {
            let mut x = vec![18u8, 1, 0, 2, 0, 0, 0, 64];
            x.extend_from_slice(&v.to_le_bytes());
            x.extend_from_slice(&p.to_le_bytes());
            x.extend_from_slice(&[0; 6]);
            x
        };
        std::fs::write(d.join("other"), desc(0x0bda, 0x8153)).unwrap();
        std::fs::write(d.join("short"), [1u8, 2, 3]).unwrap();
        assert_eq!(UsbFs::open(&d.join("other")).err(), Some(UsbError::Other(19)));
        assert_eq!(UsbFs::open(&d.join("short")).err(), Some(UsbError::Other(5)));
        assert!(UsbFs::open(&d.join("missing")).is_err());
        // a real-looking Touch Bar descriptor passes the check (the ioctls then fail on a plain file, which is fine here)
        std::fs::write(d.join("tb"), desc(0x05ac, 0x8302)).unwrap();
        let mut u = UsbFs::open(&d.join("tb")).unwrap();
        assert!(u.claim(1).is_err(), "a plain file is not a usbfs node");
        assert!(u.reset().is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}

// ---------------------------------------------------------------- HID boot protocol (KI-077)
//
// A USB HID touch controller with a boot-protocol mouse interface can be switched to BOOT protocol by the firmware (EDK2-style firmware,
// Apple's included, binds every boot mouse for its own menus and sends SET_PROTOCOL(boot)). Linux's usbhid never sends SET_PROTOCOL(report)
// (it only sets idle), and a cheap controller keeps the protocol across bus resets, re-enumeration and driver rebinds: only a power loss
// (the cable replug) puts it back. In boot protocol the multitouch interface stays silent. `hid_set_report_protocol` sends the one
// class request the HID specification defines for this (SET_PROTOCOL, bRequest 0x0b, wValue 1 = report protocol) to the named
// interfaces of the one device named by its node, after reading the device descriptor back and checking vendor and product.
// Three more usbfs requests: DISCONNECT_CLAIM (the kernel driver is detached from that interface and the interface claimed, the
// request needs the claim), CONTROL, and IOCTL(CONNECT) which hands the interface back to the kernel driver.

/// `struct usbdevfs_ctrltransfer`
#[repr(C)]
pub struct Ctrl {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
    pub timeout: u32,
    pub data: *mut libc::c_void,
}

/// `struct usbdevfs_disconnect_claim`
#[repr(C)]
pub struct DisconnectClaim {
    pub interface: u32,
    pub flags: u32,
    pub driver: [u8; 256],
}

/// `struct usbdevfs_ioctl`
#[repr(C)]
pub struct IoctlArg {
    pub ifno: i32,
    pub ioctl_code: i32,
    pub data: *mut libc::c_void,
}

pub const USBDEVFS_CONTROL: u32 = ioc(RW, 0, std::mem::size_of::<Ctrl>() as u32);
pub const USBDEVFS_IOCTL: u32 = ioc(RW, 18, std::mem::size_of::<IoctlArg>() as u32);
pub const USBDEVFS_CONNECT: u32 = ioc(NONE, 23, 0);
pub const USBDEVFS_DISCONNECT_CLAIM: u32 = ioc(READ, 27, std::mem::size_of::<DisconnectClaim>() as u32);
/// HID class request `SET_PROTOCOL` and the value for the report protocol (HID 1.11, 7.2.6).
pub const HID_SET_PROTOCOL: u8 = 0x0b;
pub const HID_REPORT_PROTOCOL: u16 = 1;
/// bmRequestType: host to device, class, interface.
pub const HID_CLASS_INTERFACE_OUT: u8 = 0x21;

/// The control request that puts interface `interface` into report protocol.
pub fn set_report_protocol_request(interface: u8) -> Ctrl {
    Ctrl { request_type: HID_CLASS_INTERFACE_OUT, request: HID_SET_PROTOCOL, value: HID_REPORT_PROTOCOL, index: interface as u16, length: 0, timeout: 1000, data: std::ptr::null_mut() }
}

/// Send SET_PROTOCOL(report) to each of `interfaces` of the device at `node` (`/dev/bus/usb/BBB/DDD`), whose descriptor must read back
/// as `vendor:product`. Returns one sentence per interface (what happened), or the error that stopped everything before the first request.
pub fn hid_set_report_protocol(node: &Path, vendor: u16, product: u16, interfaces: &[u8]) -> Result<Vec<String>, UsbError> {
    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(node).map_err(|e| UsbError::from_errno(e.raw_os_error().unwrap_or(5)))?;
    let mut d = [0u8; 18];
    file.read_exact(&mut d).map_err(|_| UsbError::Other(5))?;
    let (v, p) = (u16::from_le_bytes([d[8], d[9]]), u16::from_le_bytes([d[10], d[11]]));
    if v != vendor || p != product {
        return Err(UsbError::Other(19));
    }
    let fd = file.as_raw_fd();
    let mut out = Vec::new();
    for &i in interfaces {
        let mut dc = DisconnectClaim { interface: i as u32, flags: 0, driver: [0; 256] };
        // SAFETY: each request takes a pointer to one struct of the matching layout, alive for the call; `data` is null and `length` 0,
        // so the kernel reads and writes no buffer.
        let r = unsafe { libc::ioctl(fd, USBDEVFS_DISCONNECT_CLAIM as _, &mut dc as *mut DisconnectClaim) };
        if r < 0 {
            out.push(format!("interface {i}: cannot take it from its driver ({:?})", last_error()));
            continue;
        }
        let mut c = set_report_protocol_request(i);
        let r = unsafe { libc::ioctl(fd, USBDEVFS_CONTROL as _, &mut c as *mut Ctrl) };
        let what = if r < 0 { format!("interface {i}: SET_PROTOCOL(report) refused ({:?})", last_error()) } else { format!("interface {i}: report protocol set") };
        let mut rel = i as u32;
        unsafe { libc::ioctl(fd, USBDEVFS_RELEASEINTERFACE as _, &mut rel as *mut u32) };
        let mut back = IoctlArg { ifno: i as i32, ioctl_code: USBDEVFS_CONNECT as i32, data: std::ptr::null_mut() };
        let r = unsafe { libc::ioctl(fd, USBDEVFS_IOCTL as _, &mut back as *mut IoctlArg) };
        out.push(if r < 0 { format!("{what}; the kernel driver did not take the interface back ({:?})", last_error()) } else { what });
    }
    Ok(out)
}

#[cfg(test)]
mod protocol_tests {
    use super::*;

    #[test]
    fn the_usbfs_structures_have_the_kernels_layout_and_request_numbers() {
        assert_eq!(std::mem::size_of::<Ctrl>(), 24); // 8 bytes of fields, 4 of timeout, the pointer 8-aligned at 16
        assert_eq!(std::mem::size_of::<DisconnectClaim>(), 264);
        assert_eq!(std::mem::size_of::<IoctlArg>(), 16);
        assert_eq!(USBDEVFS_CONTROL, 0xc018_5500); // _IOWR(U, 0, 24 bytes), the value libusb uses on x86-64
        assert_eq!(USBDEVFS_IOCTL, 0xc010_5512);
        assert_eq!(USBDEVFS_CONNECT, 0x5517);
        assert_eq!(USBDEVFS_DISCONNECT_CLAIM, 0x8108_551b);
    }

    #[test]
    fn the_set_protocol_request_is_the_hid_class_request_for_report_protocol_on_that_interface() {
        let c = set_report_protocol_request(2);
        assert_eq!((c.request_type, c.request, c.value, c.index, c.length), (0x21, 0x0b, 1, 2, 0));
        assert!(c.data.is_null() && c.timeout > 0);
    }

    #[test]
    fn a_node_that_is_not_the_named_device_is_refused_before_any_request() {
        let d = std::env::temp_dir().join(format!("iyakki-usbfs-proto-{}", std::process::id()));
        let mut bytes = vec![0u8; 18];
        bytes[8..12].copy_from_slice(&[0x40, 0x1a, 0x01, 0x01]); // 1a40:0101, a hub, not the touch screen
        std::fs::write(&d, &bytes).unwrap();
        assert!(matches!(hid_set_report_protocol(&d, 0x27c0, 0x0859, &[2]), Err(UsbError::Other(19))));
        let _ = std::fs::remove_file(&d);
    }
}
