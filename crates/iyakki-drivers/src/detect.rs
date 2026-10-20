//! What this machine is and has, read from `/sys` and `/proc` only (so tests use fake trees), and which known driver
//! packs apply to it: by DMI product and vendor, PCI ids and USB ids.

use std::collections::BTreeSet;
use std::path::Path;

pub use iyakki_hwinfo::Roots;

use crate::pack::{glob, Match};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PciDev {
    pub addr: String,
    pub vendor: String,
    pub device: String,
    pub class: String,
    pub driver: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsbDev {
    pub vendor: String,
    pub product: String,
    pub name: String,
    pub drivers: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Machine {
    pub vendor: String,
    pub product: String,
    pub pci: Vec<PciDev>,
    pub usb: Vec<UsbDev>,
    pub modules: BTreeSet<String>,
    pub release: String,
    pub cmdline: String,
    pub bluetooth: Vec<String>,
    pub sound_cards: usize,
    pub applesmc: bool,
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn names(p: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

fn link_name(p: &Path) -> Option<String> {
    std::fs::read_link(p).ok().and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned()))
}

fn hex4(s: &str) -> String {
    s.trim().trim_start_matches("0x").to_ascii_lowercase()
}

impl Machine {
    pub fn collect(r: &Roots) -> Machine {
        let dmi = |f: &str| read(&r.sys.join("class/dmi/id").join(f)).unwrap_or_default();
        let mut m = Machine { vendor: dmi("sys_vendor"), product: dmi("product_name"), ..Default::default() };
        for a in names(&r.sys.join("bus/pci/devices")) {
            let d = r.sys.join("bus/pci/devices").join(&a);
            let (Some(v), Some(dv)) = (read(&d.join("vendor")), read(&d.join("device"))) else { continue };
            m.pci.push(PciDev { addr: a, vendor: hex4(&v), device: hex4(&dv), class: read(&d.join("class")).map(|c| hex4(&c)).unwrap_or_default(), driver: link_name(&d.join("driver")) });
        }
        let usb_dir = r.sys.join("bus/usb/devices");
        let all = names(&usb_dir);
        for n in all.iter().filter(|n| !n.contains(':')) {
            let d = usb_dir.join(n);
            let (Some(v), Some(p)) = (read(&d.join("idVendor")), read(&d.join("idProduct"))) else { continue };
            let prefix = format!("{n}:");
            let mut drivers: Vec<String> = all.iter().filter(|i| i.starts_with(&prefix)).filter_map(|i| link_name(&usb_dir.join(i).join("driver"))).collect();
            drivers.sort();
            drivers.dedup();
            m.usb.push(UsbDev { vendor: hex4(&v), product: hex4(&p), name: read(&d.join("product")).unwrap_or_default(), drivers });
        }
        m.modules = read(&r.proc.join("modules")).unwrap_or_default().lines().filter_map(|l| l.split_whitespace().next()).map(str::to_string).collect();
        m.release = read(&r.proc.join("sys/kernel/osrelease")).unwrap_or_default();
        m.cmdline = read(&r.proc.join("cmdline")).unwrap_or_default();
        m.bluetooth = names(&r.sys.join("class/bluetooth")).into_iter().filter(|n| n.starts_with("hci") && !n.contains(':')).collect();
        m.sound_cards = read(&r.proc.join("asound/cards")).unwrap_or_default().lines().filter(|l| !l.starts_with(' ') && !l.trim().is_empty()).count();
        m.applesmc = r.sys.join("devices/platform/applesmc.768").exists() || m.modules.contains("applesmc");
        m
    }

    pub fn is_apple(&self) -> bool {
        glob("Apple*", &self.vendor)
    }

    /// Descriptions of the devices a match picks out (empty when it is a DMI-only match that holds).
    /// `None` when the match does not apply to this machine.
    pub fn matched(&self, m: &Match) -> Option<Vec<String>> {
        if m.is_empty() {
            return None;
        }
        if (!m.dmi_product.is_empty() && !glob(&m.dmi_product, &self.product)) || (!m.dmi_vendor.is_empty() && !glob(&m.dmi_vendor, &self.vendor)) {
            return None;
        }
        let mut found = Vec::new();
        if !m.pci.is_empty() {
            let (v, d) = m.pci.split_once(':')?;
            let hits: Vec<String> = self.pci.iter().filter(|p| glob(v, &p.vendor) && glob(d, &p.device)).map(|p| format!("PCI {}:{} at {} ({})", p.vendor, p.device, p.addr, p.driver.as_deref().map_or("no driver".to_string(), |d| format!("driver {d}")))).collect();
            if hits.is_empty() {
                return None;
            }
            found.extend(hits);
        }
        if !m.usb.is_empty() {
            let (v, p) = m.usb.split_once(':')?;
            let hits: Vec<String> = self.usb.iter().filter(|u| glob(v, &u.vendor) && glob(p, &u.product)).map(|u| format!("USB {}:{} {} ({})", u.vendor, u.product, u.name, if u.drivers.is_empty() { "no driver".to_string() } else { format!("driver {}", u.drivers.join(", ")) })).collect();
            if hits.is_empty() {
                return None;
            }
            found.extend(hits);
        }
        Some(found)
    }

    /// The first of `matches` that applies, with its devices.
    pub fn first_match(&self, matches: &[Match]) -> Option<Vec<String>> {
        matches.iter().find_map(|m| self.matched(m))
    }

    /// Drivers bound to the devices a match picks out (PCI and USB).
    pub fn bound_drivers(&self, matches: &[Match]) -> Vec<String> {
        let mut out = BTreeSet::new();
        for m in matches.iter().filter(|m| self.matched(m).is_some()) {
            if let Some((v, d)) = m.pci.split_once(':') {
                self.pci.iter().filter(|p| glob(v, &p.vendor) && glob(d, &p.device)).filter_map(|p| p.driver.clone()).for_each(|d| {
                    out.insert(d);
                });
            }
            if let Some((v, p)) = m.usb.split_once(':') {
                self.usb.iter().filter(|u| glob(v, &u.vendor) && glob(p, &u.product)).flat_map(|u| u.drivers.clone()).for_each(|d| {
                    out.insert(d);
                });
            }
        }
        out.into_iter().collect()
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::path::{Path, PathBuf};

    use super::Roots;

    pub fn put(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    pub fn tree(tag: &str) -> (Roots, PathBuf) {
        let root = std::env::temp_dir().join(format!("iyakki-drivers-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sys")).unwrap();
        std::fs::create_dir_all(root.join("proc")).unwrap();
        (Roots { sys: root.join("sys"), proc: root.join("proc") }, root)
    }

    pub fn pci(root: &Path, addr: &str, vendor: &str, device: &str, driver: Option<&str>) {
        put(root, &format!("sys/bus/pci/devices/{addr}/vendor"), &format!("0x{vendor}\n"));
        put(root, &format!("sys/bus/pci/devices/{addr}/device"), &format!("0x{device}\n"));
        put(root, &format!("sys/bus/pci/devices/{addr}/class"), "0x028000\n");
        if let Some(d) = driver {
            std::fs::create_dir_all(root.join("sys/bus/pci/drivers").join(d)).unwrap();
            std::os::unix::fs::symlink(root.join("sys/bus/pci/drivers").join(d), root.join(format!("sys/bus/pci/devices/{addr}/driver"))).unwrap();
        }
    }

    pub fn usb(root: &Path, id: &str, vendor: &str, product: &str, name: &str, driver: Option<&str>) {
        put(root, &format!("sys/bus/usb/devices/{id}/idVendor"), &format!("{vendor}\n"));
        put(root, &format!("sys/bus/usb/devices/{id}/idProduct"), &format!("{product}\n"));
        put(root, &format!("sys/bus/usb/devices/{id}/product"), &format!("{name}\n"));
        if let Some(d) = driver {
            let intf = root.join(format!("sys/bus/usb/devices/{id}:1.0"));
            std::fs::create_dir_all(&intf).unwrap();
            std::fs::create_dir_all(root.join("sys/bus/usb/drivers").join(d)).unwrap();
            std::os::unix::fs::symlink(root.join("sys/bus/usb/drivers").join(d), intf.join("driver")).unwrap();
        }
    }

    pub fn dmi(root: &Path, vendor: &str, product: &str) {
        put(root, "sys/class/dmi/id/sys_vendor", &format!("{vendor}\n"));
        put(root, "sys/class/dmi/id/product_name", &format!("{product}\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    #[test]
    fn collects_dmi_pci_usb_modules_and_bound_drivers() {
        let (r, root) = tree("collect");
        dmi(&root, "Apple Inc.", "Macmini7,1");
        pci(&root, "0000:03:00.0", "14e4", "43a0", None);
        pci(&root, "0000:00:02.0", "8086", "0a26", Some("i915"));
        usb(&root, "1-4", "05ac", "8289", "Bluetooth USB Host Controller", Some("btusb"));
        put(&root, "proc/modules", "i915 1 0 - Live 0x0\nbtusb 1 0 - Live 0x0\n");
        put(&root, "proc/sys/kernel/osrelease", "6.12.112\n");
        let m = Machine::collect(&r);
        assert_eq!((m.vendor.as_str(), m.product.as_str(), m.release.as_str()), ("Apple Inc.", "Macmini7,1", "6.12.112"));
        assert!(m.is_apple());
        assert_eq!(m.pci.len(), 2);
        assert_eq!(m.pci.iter().find(|p| p.device == "0a26").unwrap().driver.as_deref(), Some("i915"));
        assert_eq!(m.pci.iter().find(|p| p.device == "43a0").unwrap().driver, None);
        assert_eq!(m.usb[0].drivers, vec!["btusb".to_string()]);
        assert!(m.modules.contains("btusb"));
    }

    #[test]
    fn matches_are_and_within_one_and_or_across_the_list() {
        let (r, root) = tree("match");
        dmi(&root, "Apple Inc.", "Macmini7,1");
        pci(&root, "0000:03:00.0", "14e4", "43a0", None);
        let m = Machine::collect(&r);
        let by_pci = Match { pci: "14e4:43a0".into(), ..Default::default() };
        assert_eq!(m.matched(&by_pci).unwrap().len(), 1);
        assert!(m.matched(&Match { pci: "14e4:*".into(), dmi_vendor: "Apple*".into(), ..Default::default() }).is_some());
        assert!(m.matched(&Match { pci: "14e4:43a0".into(), dmi_vendor: "Dell*".into(), ..Default::default() }).is_none(), "AND");
        assert!(m.matched(&Match::product("Macmini7,1")).unwrap().is_empty(), "a DMI-only match has no devices but holds");
        assert!(m.matched(&Match::default()).is_none(), "an empty match matches nothing");
        assert!(m.first_match(&[Match::product("Nope"), by_pci]).is_some(), "OR");
        assert!(m.matched(&Match { usb: "05ac:*".into(), ..Default::default() }).is_none());
    }
}
