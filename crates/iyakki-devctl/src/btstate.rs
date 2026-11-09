//! What the Bluetooth page needs to know about the machine, and the one switch that turns Bluetooth on or off.
//!
//! The page (a tile) must not read the kernel or the hardware crates itself (`docs/development/UI-OS-SPLIT-PLAN.md`), so the reading
//! of the adapter, the radio switches, the controller list and the firmware state, and the radio plus controller switching, live here
//! behind small functions the shell's view model calls.

use crate::btview::{self, Controller};
use crate::{Control, Rfkill, Roots};

/// The Bluetooth hardware as the kernel shows it now.
#[derive(Clone, Debug)]
pub struct BtState {
    /// Controllers the kernel made (`hci0`).
    pub adapters: Vec<String>,
    /// A USB Bluetooth device is plugged in, whether or not a driver took it.
    pub usb_present: bool,
    /// The radio switches (rfkill) of type bluetooth.
    pub switches: Vec<Rfkill>,
    /// Controllers that exist but never answered (no address file).
    pub unstarted: Vec<String>,
    /// The controllers the kernel's management interface lists (`Err`: it could not be asked).
    pub controllers: Result<Vec<Controller>, String>,
    /// The machine model (DMI product name), for the chip lookup and the Mac checks.
    pub product: String,
    /// The patch file the kernel asks for exists.
    pub fw_file: bool,
    /// The Bluetooth drivers are set aside until firmware exists.
    pub held: bool,
    /// A Mac whose Broadcom controller is listed without a valid address and without its patch file.
    pub needs_firmware: bool,
}

impl BtState {
    /// The first controller that is set up (the one the page switches and scans with).
    pub fn controller(&self) -> Option<&Controller> {
        self.controllers.as_ref().ok().and_then(|c| c.iter().find(|c| c.configured).or(c.first()))
    }
    /// Radio switched off by software on any switch.
    pub fn soft_blocked(&self) -> bool {
        self.switches.iter().any(|k| k.soft)
    }
    /// Radio switched off by a key or switch on the machine.
    pub fn hard_blocked(&self) -> bool {
        self.switches.iter().any(|k| k.hard)
    }
}

/// Reads the state. The management interface is asked only when the kernel made a controller.
pub fn read(ctl: &dyn Control, r: &Roots) -> BtState {
    let bt = crate::bluetooth(r);
    let controllers = if bt.adapters.is_empty() { Ok(Vec::new()) } else { ctl.bt_controllers() };
    let product = std::fs::read_to_string(r.sys.join("class/dmi/id/product_name")).unwrap_or_default().trim().to_string();
    let fw_file = ctl.bt_firmware_file();
    let needs_firmware = needs_firmware(&product, &controllers, fw_file);
    BtState { adapters: bt.adapters, usb_present: bt.usb_present, switches: bt.switches, unstarted: bt.unstarted, controllers, product, fw_file, held: ctl.bt_held(), needs_firmware }
}

/// A Mac whose Broadcom Bluetooth controller is listed without a valid address and without its patch file. Only the kernel's management
/// interface can tell (a USB adapter that works looks the same in sysfs), so when it cannot be asked the answer is no.
pub fn needs_firmware(product: &str, controllers: &Result<Vec<Controller>, String>, fw_file: bool) -> bool {
    let mac = product.starts_with("Mac") || product.starts_with("iMac") || product.starts_with("Xserve");
    if !mac || fw_file {
        return false;
    }
    matches!(controllers, Ok(cs) if !cs.is_empty() && cs.iter().any(|c| btview::needs_firmware(c, fw_file)))
}

/// Why powering the controller failed, in words the person can act on. `blocked` is (soft, hard): the radio switches.
pub fn power_failure(error: &str, on: bool, blocked: (bool, bool)) -> String {
    if on && blocked.1 {
        return "Blocked by the hardware switch: switch Bluetooth on with the key or switch on the machine first.".into();
    }
    if on && blocked.0 {
        return "The radio is switched off: turn the radio on (Advanced) first.".into();
    }
    if error.ends_with("failed") || error.ends_with("unknown error") {
        return format!("{error}. The controller gave no reason: check that its firmware is there (Store, Setup) and choose Look again.");
    }
    error.to_string()
}

/// The one Bluetooth switch, on: frees the radio and powers the controller. A hardware block is reported, never worked around.
pub fn switch_on(ctl: &dyn Control, r: &Roots) -> Result<String, String> {
    let st = read(ctl, r);
    if st.hard_blocked() {
        return Err(power_failure("", true, (false, true)));
    }
    for k in st.switches.iter().filter(|k| k.soft) {
        crate::rfkill_set(r, &k.id, false)?;
    }
    match st.controller() {
        Some(c) if c.configured => {
            ctl.bt_power(c.index, true).map_err(|e| power_failure(&e, true, (false, false)))?;
            Ok("Bluetooth is on.".into())
        }
        Some(_) => Err("The controller is not set up: it has no address until its firmware is installed.".into()),
        None if !st.switches.is_empty() => Ok("The Bluetooth radio is on.".into()),
        None => Err("There is no Bluetooth controller to switch on.".into()),
    }
}

/// The one Bluetooth switch, off: powers the controller down and blocks the radio.
pub fn switch_off(ctl: &dyn Control, r: &Roots) -> Result<String, String> {
    let st = read(ctl, r);
    if let Some(c) = st.controller().filter(|c| c.configured && c.powered()) {
        ctl.bt_power(c.index, false).map_err(|e| power_failure(&e, false, (false, false)))?;
    }
    for k in st.switches.iter().filter(|k| !k.soft && !k.hard) {
        crate::rfkill_set(r, &k.id, true)?;
    }
    Ok("Bluetooth is off.".into())
}

/// Advanced: only the radio switch (rfkill).
pub fn set_radio(r: &Roots, on: bool) -> Result<String, String> {
    let sw = crate::bluetooth(r).switches;
    if sw.iter().any(|k| k.hard) && on {
        return Err(power_failure("", true, (false, true)));
    }
    if sw.is_empty() {
        return Err("This adapter has no radio switch.".into());
    }
    sw.iter().try_for_each(|k| crate::rfkill_set(r, &k.id, !on))?;
    Ok(format!("The Bluetooth radio is {}.", if on { "on" } else { "off" }))
}

/// The IPv4 address of the phone tether link, when it has one.
pub fn tether_address(ctl: &dyn Control) -> Option<String> {
    ctl.ipv4().into_iter().find(|(n, _)| n == crate::BT_PAN_IFACE).map(|(_, ip)| ip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake {
        powered: Mutex<bool>,
        calls: Mutex<Vec<String>>,
    }

    impl Control for Fake {
        fn set_link(&self, _: &str, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn ipv4(&self) -> Vec<(String, String)> {
            vec![(crate::BT_PAN_IFACE.into(), "192.168.44.2".into())]
        }
        fn volume(&self, _: u32) -> Option<crate::Volume> {
            None
        }
        fn change_volume(&self, _: u32, _: Option<i32>) -> Result<crate::Volume, String> {
            Err("no".into())
        }
        fn set_volume(&self, _: u32, _: u32) -> Result<crate::Volume, String> {
            Err("no".into())
        }
        fn dhcp(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn bt_controllers(&self) -> Result<Vec<Controller>, String> {
            let on = *self.powered.lock().unwrap();
            Ok(vec![Controller { index: 0, configured: true, address: "11:22:33:44:55:66".into(), version: Some(6), manufacturer: 2, supported: 0x281, current: u32::from(on), name: "x".into() }])
        }
        fn bt_power(&self, i: u16, on: bool) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("power {i} {on}"));
            *self.powered.lock().unwrap() = on;
            Ok(())
        }
    }

    fn machine(tag: &str, soft: bool, hard: bool) -> Roots {
        let d = std::env::temp_dir().join(format!("iyakki-btstate-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (f, v) in [("class/bluetooth/hci0/x", ""), ("class/rfkill/rfkill1/type", "bluetooth\n"), ("class/rfkill/rfkill1/soft", if soft { "1\n" } else { "0\n" }), ("class/rfkill/rfkill1/hard", if hard { "1\n" } else { "0\n" })] {
            let p = d.join("sys").join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, v).unwrap();
        }
        std::fs::create_dir_all(d.join("proc")).unwrap();
        Roots { sys: d.join("sys"), proc_: d.join("proc") }
    }

    #[test]
    fn the_switch_frees_the_radio_and_powers_the_controller_and_off_does_the_reverse() {
        let r = machine("sw", true, false);
        let f = Fake { powered: Mutex::new(false), calls: Mutex::new(vec![]) };
        assert_eq!(switch_on(&f, &r).unwrap(), "Bluetooth is on.");
        assert!(!crate::rfkill_list(&r)[0].soft);
        assert_eq!(f.calls.lock().unwrap().clone(), vec!["power 0 true"]);
        assert_eq!(switch_off(&f, &r).unwrap(), "Bluetooth is off.");
        assert!(crate::rfkill_list(&r)[0].soft);
        assert_eq!(f.calls.lock().unwrap().last().unwrap(), "power 0 false");
        assert_eq!(tether_address(&f).as_deref(), Some("192.168.44.2"));
    }

    #[test]
    fn a_hardware_block_is_reported_and_never_worked_around() {
        let r = machine("hard", false, true);
        let f = Fake { powered: Mutex::new(false), calls: Mutex::new(vec![]) };
        assert!(switch_on(&f, &r).unwrap_err().contains("hardware switch"));
        assert!(set_radio(&r, true).unwrap_err().contains("hardware switch"));
        assert!(f.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn power_failures_name_the_cause_and_firmware_is_a_broadcom_mac_without_its_file() {
        assert!(power_failure("x", true, (true, false)).contains("radio is switched off"));
        assert!(power_failure("could not switch the controller on: failed", true, (false, false)).contains("gave no reason"));
        assert_eq!(power_failure("the kernel will not power", true, (false, false)), "the kernel will not power");
        let c = |m| Ok(vec![Controller { index: 0, configured: false, address: "00:00:00:00:00:00".into(), version: None, manufacturer: m, supported: 0, current: 0, name: String::new() }]);
        assert!(needs_firmware("MacBookPro14,1", &c(15), false));
        assert!(!needs_firmware("MacBookPro14,1", &c(15), true));
        assert!(!needs_firmware("HP Laptop", &c(15), false));
        assert!(!needs_firmware("Macmini8,1", &c(2), false));
        assert!(!needs_firmware("Macmini8,1", &Err("no".into()), false));
    }
}
