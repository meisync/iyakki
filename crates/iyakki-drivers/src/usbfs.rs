//! The real device behind the [`touchbar`](crate::touchbar) seams: usbfs ioctls for the display interface and sysfs for the device's
//! configuration, plus the one device-level reset. Raw `libc::ioctl` on purpose (no USB library): four requests, listed in
//! `BOUNDARY.md`. Nothing here ever runs on its own: the Touch Bar code calls it from a background thread and only for the device whose
//! id was read from sysfs.
//!
//! Safety rules kept here: the node opened is the one sysfs named, and its own device descriptor is read back and must be `05ac:8302`
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
