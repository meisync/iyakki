//! A Broadcom Wi-Fi chip that does not answer when its driver first probes it (Macs: BCM4350 and others on the PCIe bus).
//!
//! Seen on a MacBookPro14,1 on the first start after an update: the driver loaded late and its probe read `0xffffffff` from the chip
//! (`brcmf_chip_recognition: MMIO read failed`, `brcmf_pcie_probe: failed 14e4:43a3`), so there was no `wlan0` until the next restart.
//! A PCI remove and rescan makes the kernel enumerate the chip afresh (a function reset of the device); the driver is then loaded again.
//!
//! [`needs_recovery`] looks (read only): a Broadcom network controller on the PCI bus with no driver bound and no wireless interface.
//! [`recover`] does it, at most a few times. Both go through a [`Roots`] so tests use a folder.

use std::path::{Path, PathBuf};

use crate::run::Runner;

/// Broadcom's PCI vendor id.
pub const VENDOR: u32 = 0x14e4;
/// PCI class 0x02xxxx: network controller (Wi-Fi chips report 0x028000).
const CLASS_NETWORK: u32 = 0x02;
/// The driver loaded again after the reset.
pub const MODULE: &str = "brcmfmac";

#[derive(Clone, Debug)]
pub struct Roots {
    pub sys: PathBuf,
}

impl Roots {
    pub fn real() -> Roots {
        Roots { sys: "/sys".into() }
    }
}

fn read_hex(p: &Path) -> Option<u32> {
    let t = std::fs::read_to_string(p).ok()?;
    u32::from_str_radix(t.trim().trim_start_matches("0x"), 16).ok()
}

/// Is there a wireless interface (`wlan0`, `wlp2s0`, `wl0`: anything with a `wireless` folder or a `phy80211` link)?
pub fn has_wireless_interface(r: &Roots) -> bool {
    std::fs::read_dir(r.sys.join("class/net")).into_iter().flatten().flatten().any(|e| e.path().join("wireless").exists() || e.path().join("phy80211").exists())
}

/// The PCI address of a Broadcom network controller that has no driver bound, when there is no wireless interface: the chip that
/// failed. `None` when there is nothing to do.
pub fn needs_recovery(r: &Roots) -> Option<String> {
    if has_wireless_interface(r) {
        return None;
    }
    let dir = r.sys.join("bus/pci/devices");
    let mut found: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|e| {
            let p = e.path();
            read_hex(&p.join("vendor")) == Some(VENDOR) && read_hex(&p.join("class")).is_some_and(|c| c >> 16 == CLASS_NETWORK) && !p.join("driver").exists()
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    found.into_iter().next()
}

/// Remove the device and scan the bus again, then load the driver. At most `attempts` times, stopping as soon as an interface is
/// there. `pause` waits (a test passes a no-op). Returns one log line when it tried anything.
pub fn recover(r: &Roots, runner: &dyn Runner, pause: &dyn Fn(u64), attempts: u32) -> Option<String> {
    let addr = needs_recovery(r)?;
    for n in 1..=attempts {
        let remove = r.sys.join("bus/pci/devices").join(&addr).join("remove");
        if let Err(e) = std::fs::write(&remove, "1") {
            return Some(format!("wifi: {addr} did not answer and could not be reset ({e})"));
        }
        pause(1000);
        let _ = std::fs::write(r.sys.join("bus/pci/rescan"), "1");
        pause(2000);
        let _ = runner.run("modprobe", &["-b", MODULE]);
        // the probe runs in the background: give it a few seconds
        for _ in 0..15 {
            if has_wireless_interface(r) {
                return Some(format!("wifi: {addr} did not answer at first; reset it (attempt {n}) and the interface is there"));
            }
            pause(200);
        }
        let _ = runner.run("modprobe", &["-r", MODULE]);
        pause(500);
    }
    Some(format!("wifi: {addr} still does not answer after {attempts} reset(s); a restart may bring it back"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake(RefCell<Vec<String>>, Option<PathBuf>);
    impl Runner for Fake {
        fn run(&self, program: &str, args: &[&str]) -> Result<(), String> {
            self.0.borrow_mut().push(format!("{program} {}", args.join(" ")));
            // a successful modprobe brings the interface up
            if let (Some(dir), "modprobe", Some(&"-b")) = (&self.1, program, args.first()) {
                let _ = std::fs::create_dir_all(dir.join("wlan0/wireless"));
            }
            Ok(())
        }
    }

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-wifirecover-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("class/net")).unwrap();
        d
    }

    fn pci(root: &Path, addr: &str, vendor: &str, class: &str, driver: bool) {
        let p = root.join("bus/pci/devices").join(addr);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("vendor"), format!("{vendor}\n")).unwrap();
        std::fs::write(p.join("class"), format!("{class}\n")).unwrap();
        if driver {
            std::fs::create_dir_all(p.join("driver")).unwrap();
        }
    }

    #[test]
    fn only_a_broadcom_network_chip_without_a_driver_and_without_an_interface_needs_a_reset() {
        let d = dir("needs");
        let r = Roots { sys: d.clone() };
        assert_eq!(needs_recovery(&r), None, "no devices");
        pci(&d, "0000:02:00.0", "0x14e4", "0x028000", false);
        assert_eq!(needs_recovery(&r).as_deref(), Some("0000:02:00.0"));
        pci(&d, "0000:03:00.0", "0x8086", "0x028000", false);
        pci(&d, "0000:04:00.0", "0x14e4", "0x040000", false); // the camera: not a network chip
        assert_eq!(needs_recovery(&r).as_deref(), Some("0000:02:00.0"));
        std::fs::create_dir_all(d.join("bus/pci/devices/0000:02:00.0/driver")).unwrap();
        assert_eq!(needs_recovery(&r), None, "bound: nothing to do");
        std::fs::remove_dir_all(d.join("bus/pci/devices/0000:02:00.0/driver")).unwrap();
        std::fs::create_dir_all(d.join("class/net/wlan0/wireless")).unwrap();
        assert_eq!(needs_recovery(&r), None, "an interface exists");
    }

    #[test]
    fn the_reset_removes_rescans_loads_the_driver_and_stops_when_the_interface_appears() {
        let d = dir("recover");
        pci(&d, "0000:02:00.0", "0x14e4", "0x028000", false);
        let r = Roots { sys: d.clone() };
        let fake = Fake(RefCell::new(Vec::new()), Some(d.join("class/net")));
        let line = recover(&r, &fake, &|_| {}, 3).expect("tried");
        assert!(line.contains("attempt 1") && line.contains("interface is there"), "{line}");
        assert_eq!(std::fs::read_to_string(d.join("bus/pci/devices/0000:02:00.0/remove")).unwrap(), "1");
        assert_eq!(std::fs::read_to_string(d.join("bus/pci/rescan")).unwrap(), "1");
        assert_eq!(fake.0.borrow().as_slice(), ["modprobe -b brcmfmac"]);
    }

    #[test]
    fn a_chip_that_stays_silent_is_tried_a_few_times_and_then_left_alone() {
        let d = dir("silent");
        pci(&d, "0000:02:00.0", "0x14e4", "0x028000", false);
        let r = Roots { sys: d };
        let fake = Fake(RefCell::new(Vec::new()), None);
        let line = recover(&r, &fake, &|_| {}, 2).expect("tried");
        assert!(line.contains("still does not answer after 2"), "{line}");
        assert_eq!(fake.0.borrow().iter().filter(|c| c.starts_with("modprobe -b")).count(), 2);
        assert_eq!(recover(&Roots { sys: std::env::temp_dir().join("iyakki-wifirecover-none") }, &fake, &|_| {}, 2), None, "nothing to do on another machine");
    }
}
