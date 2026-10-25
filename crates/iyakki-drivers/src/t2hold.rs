//! Keeping the Broadcom Wi-Fi and Bluetooth drivers of a T2 Mac from loading before their firmware exists.
//!
//! On a T2 Mac the kernel has the drivers (brcmfmac, hci_uart with its BCM part, hci_bcm4377) but not Apple's firmware.
//! Loaded without it they fail, one line after another (`Direct firmware load ... failed with error -2`, a `__flush_work`
//! warning from brcmfmac's failed setup, `failed to write update baudrate (-16)` from the Bluetooth chip), and a Bluetooth
//! chip that was started in that state keeps no address. So at boot, on a T2 Mac with no Apple firmware visible, IYAKKI
//! writes one `blacklist` file for those three modules: the automatic loading by device alias skips them, and the driver
//! pack that brings the firmware loads them by name (`load` / `reload` of the pack, which a blacklist does not stop) once its
//! files are linked in. Nothing else is touched, and on any other machine nothing is written.
//!
//! The file is in `/etc/modprobe.d`, which is in RAM on the image: it is written again at every boot, never kept.
//!
//! A blacklist alone did not hold on the Mac mini 2018 (BusyBox `modprobe` honours it only for aliases, and only with `-b` for
//! names; something still loaded both drivers within 4 s), and BusyBox has no `install` lines. So the hold is also done on the module
//! files themselves: `brcmfmac.ko`, `hci_uart.ko` and `hci_bcm4377.ko` are renamed to `<name>.ko.iyakki-held` in the RAM root
//! (`/lib/modules` is rewritten at every boot, so nothing is kept), and no program or kernel request can load what is not there.
//! A driver pack that brings the firmware gives them back ([`release`], called from `apply::activate`) just before it loads them.

use std::path::Path;

use crate::macboard::is_t2;

/// The modules held back: the Wi-Fi driver, the UART Bluetooth driver (the BCM4364 of the Mac mini 2018 and the 2018-2019
/// MacBook Pros) and the PCIe one (BCM4377).
pub const HELD: &[&str] = &["brcmfmac", "hci_uart", "hci_bcm4377"];
pub const CONF_NAME: &str = "iyakki-t2-holdback.conf";

/// The firmware path prefixes whose presence means the driver can start: (module, prefixes under the firmware folder).
const NEEDS: &[(&str, &[&str])] = &[("brcmfmac", &["brcm/brcmfmac4364", "brcm/brcmfmac4377", "brcm/brcmfmac4355"]), ("hci_uart", &["brcm/BCM.hcd"]), ("hci_bcm4377", &["brcm/brcmbt4377"])];

fn has_prefix(fw_root: &Path, rel_prefix: &str) -> bool {
    let (dir, stem) = match rel_prefix.rsplit_once('/') {
        Some((d, s)) => (fw_root.join(d), s),
        None => (fw_root.to_path_buf(), rel_prefix),
    };
    std::fs::read_dir(dir).into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().starts_with(stem))
}

/// The modules to hold back on this machine: none unless it is a T2 Mac; a module is left out when firmware for it is in the
/// firmware folder already.
pub fn held_modules(dmi_product: &str, fw_root: &Path) -> Vec<&'static str> {
    if !is_t2(dmi_product) {
        return vec![];
    }
    NEEDS.iter().filter(|(_, prefixes)| !prefixes.iter().any(|p| has_prefix(fw_root, p))).map(|(m, _)| *m).collect()
}

/// The text of the file (`blacklist` lines only: a driver pack's own rules and `modprobe name` are not affected).
pub fn conf_text(mods: &[&str]) -> String {
    let mut t = String::from("# written at every boot by IYAKKI on a T2 Mac: these drivers wait for Apple's firmware (Mac drivers page)\n");
    for m in mods {
        t.push_str(&format!("blacklist {m}\n"));
    }
    t
}

/// Suffix of a held module file.
pub const HELD_SUFFIX: &str = ".iyakki-held";

fn find_module_files(dir: &Path, file: &str, depth: u32, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() && depth > 0 {
            find_module_files(&p, file, depth - 1, out);
        } else if t.is_file() && e.file_name().to_string_lossy() == file {
            out.push(p);
        }
    }
}

/// Rename the module files of `mods` under `modules_root` (`/lib/modules`) out of the way. Returns how many were moved.
pub fn hold_files(modules_root: &Path, mods: &[&str]) -> usize {
    let mut n = 0;
    for m in mods {
        let mut found = Vec::new();
        find_module_files(modules_root, &format!("{m}.ko"), 10, &mut found);
        for f in found {
            let mut held = f.clone().into_os_string();
            held.push(HELD_SUFFIX);
            if std::fs::rename(&f, held).is_ok() {
                n += 1;
            }
        }
    }
    n
}

/// Give the held module files of `mods` back (a name with `-` or `_` is the same module). Returns the modules that were restored
/// (each name once, with `_`).
pub fn release_names(modules_root: &Path, mods: &[String]) -> Vec<String> {
    let mut back = Vec::new();
    for m in mods {
        let m = m.replace('-', "_");
        let mut found = Vec::new();
        find_module_files(modules_root, &format!("{m}.ko{HELD_SUFFIX}"), 10, &mut found);
        let mut any = false;
        for f in found {
            let s = f.to_string_lossy().into_owned();
            if std::fs::rename(&f, s.trim_end_matches(HELD_SUFFIX)).is_ok() {
                any = true;
            }
        }
        if any && !back.contains(&m) {
            back.push(m);
        }
    }
    back
}

/// How many files [`release_names`] gave back (kept for callers that only count).
pub fn release(modules_root: &Path, mods: &[String]) -> usize {
    let mut n = 0;
    for m in mods {
        let m = m.replace('-', "_");
        let mut found = Vec::new();
        find_module_files(modules_root, &format!("{m}.ko{HELD_SUFFIX}"), 10, &mut found);
        for f in found {
            let s = f.to_string_lossy().into_owned();
            if std::fs::rename(&f, s.trim_end_matches(HELD_SUFFIX)).is_ok() {
                n += 1;
            }
        }
    }
    n
}

/// The Bluetooth drivers in the order they load: `btbcm` (the Broadcom part `hci_uart` and `hci_bcm4377` depend on; it is not held
/// itself, but a file renamed by hand or by an older IYAKKI is given back too), the UART driver, the PCIe one.
pub const BT_GROUP: &[&str] = &["btbcm", "hci_uart", "hci_bcm4377"];

/// The Wi-Fi drivers.
pub const WIFI_GROUP: &[&str] = &["brcmfmac"];

/// The marker the person's "Load the Bluetooth driver anyway" leaves on the data volume (the RAM root forgets the files at every
/// boot, so the choice is applied again after the data volume is mounted).
pub const LOAD_ANYWAY_MARK: &str = "firmware/bt-load-anyway";

/// The modules (of the held set and `btbcm`) whose files are renamed right now.
pub fn held_now(modules_root: &Path) -> Vec<String> {
    let mut v = Vec::new();
    for m in HELD.iter().chain(["btbcm"].iter()) {
        let mut found = Vec::new();
        find_module_files(modules_root, &format!("{m}.ko{HELD_SUFFIX}"), 10, &mut found);
        if !found.is_empty() {
            v.push(m.to_string());
        }
    }
    v
}

/// A Bluetooth driver is held back right now (the Mac drivers page offers "Load the Bluetooth driver anyway").
pub fn bluetooth_held(modules_root: &Path) -> bool {
    held_now(modules_root).iter().any(|m| BT_GROUP.contains(&m.as_str()))
}

/// Write (or remove) the hold-back file for this machine and move the module files aside. Returns the modules held back.
/// `dmi_product_file` is `/sys/class/dmi/id/product_name`; `modules_root` is `/lib/modules`.
pub fn apply(dmi_product_file: &Path, fw_root: &Path, modprobe_d: &Path, modules_root: &Path) -> Vec<&'static str> {
    let product = std::fs::read_to_string(dmi_product_file).unwrap_or_default();
    let mods = held_modules(product.trim(), fw_root);
    let conf = modprobe_d.join(CONF_NAME);
    if mods.is_empty() {
        let _ = std::fs::remove_file(&conf);
    } else {
        let _ = std::fs::create_dir_all(modprobe_d);
        let _ = std::fs::write(&conf, conf_text(&mods));
        hold_files(modules_root, &mods);
    }
    mods
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let p = std::path::PathBuf::from(std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())).join(format!("iyakki-t2hold-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_t2_mac_without_firmware_holds_back_wifi_and_both_bluetooth_drivers() {
        let r = tmp("t2");
        std::fs::write(r.join("product_name"), "Macmini8,1\n").unwrap();
        let held = apply(&r.join("product_name"), &r.join("fw"), &r.join("modprobe.d"), &r.join("mods"));
        assert_eq!(held, vec!["brcmfmac", "hci_uart", "hci_bcm4377"]);
        let t = std::fs::read_to_string(r.join("modprobe.d").join(CONF_NAME)).unwrap();
        assert!(t.contains("blacklist brcmfmac\n") && t.contains("blacklist hci_uart\n") && t.contains("blacklist hci_bcm4377\n"));
        assert!(t.lines().filter(|l| !l.starts_with('#')).all(|l| crate::pack::valid_modprobe_line(l)), "only lines the pack validator accepts");
        std::fs::remove_dir_all(&r).unwrap();
    }

    #[test]
    fn firmware_in_the_folder_lifts_the_hold_for_that_driver_only() {
        let r = tmp("fw");
        std::fs::write(r.join("product_name"), "MacBookPro15,2").unwrap();
        std::fs::create_dir_all(r.join("fw/brcm")).unwrap();
        std::fs::write(r.join("fw/brcm/brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin"), b"x").unwrap();
        assert_eq!(held_modules("MacBookPro15,2", &r.join("fw")), vec!["hci_uart", "hci_bcm4377"]);
        std::fs::write(r.join("fw/brcm/BCM.hcd"), b"x").unwrap();
        assert_eq!(held_modules("MacBookPro15,2", &r.join("fw")), vec!["hci_bcm4377"]);
        std::fs::write(r.join("fw/brcm/brcmbt4377b3-apple,formosa-m.bin"), b"x").unwrap();
        assert!(held_modules("MacBookPro15,2", &r.join("fw")).is_empty());
        // with everything there the file from an earlier boot is removed
        std::fs::create_dir_all(r.join("modprobe.d")).unwrap();
        std::fs::write(r.join("modprobe.d").join(CONF_NAME), "blacklist brcmfmac\n").unwrap();
        assert!(apply(&r.join("product_name"), &r.join("fw"), &r.join("modprobe.d"), &r.join("mods")).is_empty());
        assert!(!r.join("modprobe.d").join(CONF_NAME).exists());
        std::fs::remove_dir_all(&r).unwrap();
    }

    fn fake_modules(r: &std::path::Path) {
        for f in ["kernel/drivers/net/wireless/broadcom/brcm80211/brcmfmac/brcmfmac.ko", "kernel/drivers/net/wireless/broadcom/brcm80211/brcmfmac/wcc/brcmfmac-wcc.ko", "kernel/drivers/bluetooth/hci_uart.ko", "kernel/drivers/bluetooth/btbcm.ko", "kernel/drivers/bluetooth/hci_bcm4377.ko"] {
            let p = r.join("mods/6.12.112").join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"ko").unwrap();
        }
    }

    #[test]
    fn the_module_files_are_moved_aside_on_a_t2_mac_and_given_back_by_name() {
        let r = tmp("files");
        fake_modules(&r);
        std::fs::write(r.join("product_name"), "Macmini8,1").unwrap();
        apply(&r.join("product_name"), &r.join("fw"), &r.join("modprobe.d"), &r.join("mods"));
        let m = r.join("mods/6.12.112/kernel/drivers");
        assert!(!m.join("bluetooth/hci_uart.ko").exists() && m.join("bluetooth/hci_uart.ko.iyakki-held").exists());
        assert!(!m.join("net/wireless/broadcom/brcm80211/brcmfmac/brcmfmac.ko").exists() && !m.join("bluetooth/hci_bcm4377.ko").exists());
        assert!(m.join("bluetooth/btbcm.ko").exists() && m.join("net/wireless/broadcom/brcm80211/brcmfmac/wcc/brcmfmac-wcc.ko").exists(), "nothing else is touched");
        // a second run changes nothing; releasing one driver gives back only that one
        apply(&r.join("product_name"), &r.join("fw"), &r.join("modprobe.d"), &r.join("mods"));
        assert_eq!(release(&r.join("mods"), &["hci_uart".to_string()]), 1);
        assert!(m.join("bluetooth/hci_uart.ko").exists() && !m.join("bluetooth/hci_bcm4377.ko").exists());
        assert_eq!(release(&r.join("mods"), &["hci-bcm4377".to_string(), "brcmfmac".to_string()]), 2, "dashes count as underscores");
        assert_eq!(release(&r.join("mods"), &["hci_uart".to_string()]), 0);
        // a Mac that is not a T2 keeps every file
        let r2 = tmp("files2");
        fake_modules(&r2);
        std::fs::write(r2.join("product_name"), "Macmini7,1").unwrap();
        apply(&r2.join("product_name"), &r2.join("fw"), &r2.join("modprobe.d"), &r2.join("mods"));
        assert!(r2.join("mods/6.12.112/kernel/drivers/bluetooth/hci_uart.ko").exists());
        std::fs::remove_dir_all(&r).unwrap();
        std::fs::remove_dir_all(&r2).unwrap();
    }

    #[test]
    fn release_names_says_which_modules_came_back_and_the_group_is_in_load_order() {
        let r = tmp("names");
        fake_modules(&r);
        std::fs::write(r.join("product_name"), "Macmini8,1").unwrap();
        apply(&r.join("product_name"), &r.join("fw"), &r.join("modprobe.d"), &r.join("mods"));
        // a btbcm held by hand is given back with the group
        let bt = r.join("mods/6.12.112/kernel/drivers/bluetooth");
        std::fs::rename(bt.join("btbcm.ko"), bt.join("btbcm.ko.iyakki-held")).unwrap();
        assert_eq!(held_now(&r.join("mods")), vec!["brcmfmac", "hci_uart", "hci_bcm4377", "btbcm"]);
        assert!(bluetooth_held(&r.join("mods")));
        let g: Vec<String> = BT_GROUP.iter().map(|s| s.to_string()).collect();
        assert_eq!(release_names(&r.join("mods"), &g), vec!["btbcm", "hci_uart", "hci_bcm4377"]);
        assert!(!bluetooth_held(&r.join("mods")) && held_now(&r.join("mods")) == vec!["brcmfmac"], "Wi-Fi is not in the group");
        assert!(release_names(&r.join("mods"), &g).is_empty(), "nothing left to give back");
        std::fs::remove_dir_all(&r).unwrap();
    }

    #[test]
    fn other_machines_are_never_touched() {
        let r = tmp("other");
        for product in ["Macmini7,1", "ThinkPad X1", "", "MacBookPro14,1", "iMac19,1"] {
            std::fs::write(r.join("product_name"), product).unwrap();
            assert!(apply(&r.join("product_name"), &r.join("fw"), &r.join("modprobe.d"), &r.join("mods")).is_empty(), "{product:?}");
        }
        assert!(!r.join("modprobe.d").exists(), "no folder, no file");
        assert!(apply(&r.join("missing"), &r.join("fw"), &r.join("modprobe.d"), &r.join("mods")).is_empty(), "no DMI at all");
        std::fs::remove_dir_all(&r).unwrap();
    }
}
