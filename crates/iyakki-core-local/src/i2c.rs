//! The I2C buses of the screens (F-KBD-08): which `/dev/i2c-N` belongs to which display connector, and a bus opened for one monitor
//! message at a time. This is the one place that opens the device; the DDC/CI messages and the rules (read first, write only to the
//! monitor the person chose) are in `iyakki-agent::ddcci`. The roots are arguments so a test points them at a made-up tree.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// `ioctl` request that sets the address of the device the next read and write talk to.
const I2C_SLAVE: u32 = 0x0703;

/// A display connector whose DDC channel is an I2C bus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DdcBus {
    /// The connector, for example `card0-HDMI-A-1`.
    pub connector: String,
    /// The bus number: the `N` of `/dev/i2c-N`.
    pub bus: u32,
}

/// The connectors that have a DDC channel and a screen on them, from `<sys>/class/drm/*/ddc` (a link to `.../i2c-N`). A connector with
/// no screen is left out. Sorted by connector name.
pub fn ddc_buses(sys: &Path) -> Vec<DdcBus> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(sys.join("class").join("drm")).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let dir = e.path();
        let connected = std::fs::read_to_string(dir.join("status")).map(|s| s.trim() == "connected").unwrap_or(false);
        if !connected {
            continue;
        }
        let Ok(link) = std::fs::read_link(dir.join("ddc")) else { continue };
        let Some(n) = link.file_name().and_then(|f| f.to_str()).and_then(|f| f.strip_prefix("i2c-")).and_then(|n| n.parse::<u32>().ok()) else { continue };
        out.push(DdcBus { connector: name, bus: n });
    }
    out.sort_by(|a, b| a.connector.cmp(&b.connector));
    out
}

/// Where the device file of bus `n` is under `dev_root`.
pub fn bus_path(dev_root: &Path, n: u32) -> PathBuf {
    dev_root.join(format!("i2c-{n}"))
}

/// An opened bus. Each message first names the address it is for.
pub struct I2cDev {
    file: File,
}

impl I2cDev {
    /// Opens the bus. Fails plainly when there is no such file (the kernel lacks `i2c-dev`) or the caller may not use it.
    pub fn open(path: &Path) -> io::Result<I2cDev> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        Ok(I2cDev { file })
    }

    fn address(&self, addr: u16) -> io::Result<()> {
        // SAFETY: `ioctl` with a plain integer argument on a descriptor this struct owns.
        let r = unsafe { libc::ioctl(self.file.as_raw_fd(), I2C_SLAVE as _, libc::c_ulong::from(addr)) };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub fn write(&mut self, addr: u16, data: &[u8]) -> io::Result<()> {
        self.address(addr)?;
        self.file.write_all(data)
    }

    pub fn read(&mut self, addr: u16, len: usize) -> io::Result<Vec<u8>> {
        self.address(addr)?;
        let mut b = vec![0u8; len];
        let n = self.file.read(&mut b)?;
        b.truncate(n);
        Ok(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-i2c-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn connector(sys: &Path, name: &str, status: &str, ddc: Option<&str>) {
        let d = sys.join("class").join("drm").join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("status"), format!("{status}\n")).unwrap();
        if let Some(t) = ddc {
            std::os::unix::fs::symlink(t, d.join("ddc")).unwrap();
        }
    }

    #[test]
    fn only_connected_connectors_with_a_ddc_link_to_an_i2c_bus_are_listed_in_order() {
        let sys = tmp("list");
        connector(&sys, "card0-HDMI-A-1", "connected", Some("../../../devices/pci0000:00/i2c-7"));
        connector(&sys, "card0-DP-1", "connected", Some("../../../devices/pci0000:00/i2c-3"));
        connector(&sys, "card0-DP-2", "disconnected", Some("../../../devices/pci0000:00/i2c-4"));
        connector(&sys, "card0-eDP-1", "connected", None);
        connector(&sys, "card0-VGA-1", "connected", Some("../../../devices/pci0000:00/not-a-bus"));
        let got = ddc_buses(&sys);
        assert_eq!(got, vec![DdcBus { connector: "card0-DP-1".into(), bus: 3 }, DdcBus { connector: "card0-HDMI-A-1".into(), bus: 7 }]);
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn a_machine_without_drm_has_no_buses_and_the_device_path_names_the_bus() {
        let sys = tmp("none");
        assert!(ddc_buses(&sys).is_empty());
        assert_eq!(bus_path(Path::new("/x"), 5), PathBuf::from("/x/i2c-5"));
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn a_missing_bus_file_is_an_error_and_an_ordinary_file_refuses_the_address() {
        let d = tmp("open");
        assert!(I2cDev::open(&bus_path(&d, 9)).is_err(), "no i2c-dev: no file");
        std::fs::write(bus_path(&d, 1), b"x").unwrap();
        let mut dev = I2cDev::open(&bus_path(&d, 1)).unwrap();
        assert!(dev.write(0x37, &[1]).is_err(), "an ordinary file is not an I2C adapter");
        assert!(dev.read(0x50, 4).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
