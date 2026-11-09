//! Which driver packs fit the machine that is starting: a data volume that travels between machines (one T7 install booting a Mac
//! mini, a MacBook Pro, a Lenovo and an HP) carries every pack ever installed, and at every start each machine takes what fits it and
//! leaves the rest aside, saying so in the log. The facts come from the hardware itself, read before the drivers in question load.
//!
//! The rule that exists today, from the MacBookPro14,1 (2026-11-08): on a Mac without a T2 chip whose Bluetooth chip hangs off the
//! serial link (ACPI node `BCM2E7C`, the 2016-2017 MacBook Pros, BCM4350C0), Apple's firmware leaves the chip running its own patch and
//! the chip refuses a second one ("command disallowed"); the half-written attempt wedges it and the pack's driver reload repeats that at
//! every start. Without the patch file the chip has its address, scans and pairs. So the Mac Bluetooth pack (`mac-bt-broadcom`, the
//! `.hcd` patch) is left out on such a Mac, while the same pack serves a T2 Mac (BCM4364 on its serial link takes the patch) and a Mac
//! with USB Bluetooth (BCM20702, no `BCM2E7C` node). Everything else fits everywhere: files a machine does not ask for just sit there.

use std::path::Path;

use crate::macboard::is_t2;

/// The ACPI hardware id of the Broadcom serial-link Bluetooth controller of the Intel Macs (`\_SB_.PCI0.URT0.BLTH`).
pub const SERIAL_BT_ACPI_HID: &str = "BCM2E7C";

/// What the start-up rule looks at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Machine {
    /// `sys_vendor` says Apple, or the model name has Apple's shape.
    pub apple: bool,
    pub dmi_product: String,
    /// A T2 Mac (`macboard::is_t2`).
    pub t2: bool,
    /// ACPI lists the Broadcom serial-link Bluetooth node.
    pub serial_bt: bool,
}

impl Machine {
    /// Read from a sysfs root (`/sys`): `class/dmi/id/{sys_vendor,product_name}` and `bus/acpi/devices/BCM2E7C*`.
    pub fn read(sys: &Path) -> Machine {
        let r = |p: &str| std::fs::read_to_string(sys.join(p)).map(|s| s.trim().to_string()).unwrap_or_default();
        let vendor = r("class/dmi/id/sys_vendor");
        let product = r("class/dmi/id/product_name");
        let serial_bt = std::fs::read_dir(sys.join("bus/acpi/devices")).into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().starts_with(SERIAL_BT_ACPI_HID));
        Machine::from_facts(&vendor, &product, serial_bt)
    }

    pub fn from_facts(vendor: &str, product: &str, serial_bt: bool) -> Machine {
        let p = product.trim();
        let apple = vendor.trim().starts_with("Apple") || p.starts_with("MacBook") || p.starts_with("Macmini") || p.starts_with("iMac") || p.starts_with("MacPro");
        Machine { apple, dmi_product: p.to_string(), t2: apple && is_t2(p), serial_bt }
    }
}

/// Why `pack_id` is left out on this machine, or `None` when it fits. A machine with no facts (the default) takes every pack.
pub fn skip_reason(pack_id: &str, m: &Machine) -> Option<String> {
    match pack_id {
        "mac-bt-broadcom" if m.apple && !m.t2 && m.serial_bt => Some(format!(
            "left out on this machine ({}): its Bluetooth chip on the serial link runs Apple's own firmware and refuses a patch (a patch attempt wedges it); it works without one",
            if m.dmi_product.is_empty() { "a Mac without a T2 chip" } else { m.dmi_product.as_str() }
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mac_bluetooth_pack_is_left_out_on_a_non_t2_mac_with_a_serial_link_chip_and_fits_everywhere_else() {
        let mbp2017 = Machine::from_facts("Apple Inc.", "MacBookPro14,1", true);
        assert!(mbp2017.apple && !mbp2017.t2 && mbp2017.serial_bt);
        let why = skip_reason("mac-bt-broadcom", &mbp2017).expect("left out");
        assert!(why.contains("MacBookPro14,1") && why.contains("refuses a patch"), "{why}");
        // the T2 Mac mini has the same kind of node and TAKES the patch
        let mini = Machine::from_facts("Apple Inc.", "Macmini8,1", true);
        assert!(mini.t2 && skip_reason("mac-bt-broadcom", &mini).is_none());
        // a Mac with USB Bluetooth (no serial node), a PC, and a machine without facts take it
        assert!(skip_reason("mac-bt-broadcom", &Machine::from_facts("Apple Inc.", "Macmini7,1", false)).is_none());
        assert!(skip_reason("mac-bt-broadcom", &Machine::from_facts("LENOVO", "82WM", false)).is_none());
        assert!(skip_reason("mac-bt-broadcom", &Machine::default()).is_none());
        // the other packs fit everywhere: their files sit unused where no driver asks for them
        for id in ["mac-wifi-brcmfmac", "mac-facetimehd", "fw-hardware", "fw-mediatek"] {
            assert!(skip_reason(id, &mbp2017).is_none(), "{id}");
        }
    }

    #[test]
    fn the_facts_are_read_from_sysfs() {
        let d = std::env::temp_dir().join(format!("iyakki-packfit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("class/dmi/id")).unwrap();
        std::fs::create_dir_all(d.join("bus/acpi/devices/BCM2E7C:00")).unwrap();
        std::fs::write(d.join("class/dmi/id/sys_vendor"), "Apple Inc.\n").unwrap();
        std::fs::write(d.join("class/dmi/id/product_name"), "MacBookPro14,1\n").unwrap();
        let m = Machine::read(&d);
        assert_eq!(m, Machine { apple: true, dmi_product: "MacBookPro14,1".into(), t2: false, serial_bt: true });
        std::fs::remove_dir_all(d.join("bus/acpi/devices/BCM2E7C:00")).unwrap();
        assert!(!Machine::read(&d).serial_bt);
        assert_eq!(Machine::read(&d.join("nowhere")), Machine::default(), "no sysfs: no facts, every pack fits");
        let _ = std::fs::remove_dir_all(&d);
    }
}
