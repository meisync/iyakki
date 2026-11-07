//! A read-only USB port monitor: what the kernel sees on the USB side, written to the log on every change, so a plug-in that "does nothing"
//! can be told apart from one that was never seen.
//!
//! The Mac mini 2018 showed no kernel line at all for a stick on USB-C (2026-11-05). The plain kernel log cannot say whether the port saw a
//! connection that never became a device, or saw nothing. This monitor writes, once a second and only when something changed:
//! * USB devices appearing and going (`1-8.2`, vendor:product, name, speed);
//! * each PCI USB host controller's runtime power state (`suspended` / `active`) and power control (`auto` / `on`);
//! * when debugfs is mounted: each xHCI port's `portsc` line (connected, enabled, link state such as U0, Polling, Disabled), so "the port saw
//!   a connection" shows up even when no device followed.
//!
//! Nothing is written to the machine; only files are read. The lines go to the log (`dev dmesg` shows them) and nothing here is a secret.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One look at the USB side.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// PCI USB host controller id -> `runtime_status/control` (for example `suspended/auto`).
    pub controllers: BTreeMap<String, String>,
    /// USB device name (`1-8.2`) -> what to print about it.
    pub devices: BTreeMap<String, String>,
    /// `controller/portNN` -> the portsc line.
    pub ports: BTreeMap<String, String>,
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// Look (all reads). `root` is `/` in the product and a fake tree in tests.
pub fn snapshot(root: &Path) -> Snapshot {
    let mut s = Snapshot::default();
    if let Ok(rd) = std::fs::read_dir(root.join("sys/bus/pci/devices")) {
        for e in rd.flatten() {
            let d = e.path();
            if read(&d.join("class")).is_some_and(|c| c.starts_with("0x0c03")) {
                let id = e.file_name().to_string_lossy().into_owned();
                let st = read(&d.join("power/runtime_status")).unwrap_or_else(|| "?".into());
                let ctl = read(&d.join("power/control")).unwrap_or_else(|| "?".into());
                s.controllers.insert(id, format!("{st}/{ctl}"));
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(root.join("sys/bus/usb/devices")) {
        for e in rd.flatten() {
            let d = e.path();
            let (Some(v), Some(p)) = (read(&d.join("idVendor")), read(&d.join("idProduct"))) else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            // the root hubs of the controllers are listed as `usbN`: their changes are the controller's, not a plug-in
            if name.starts_with("usb") {
                continue;
            }
            let product = read(&d.join("product")).unwrap_or_default();
            let speed = read(&d.join("speed")).unwrap_or_default();
            s.devices.insert(name, format!("{v}:{p} {product} {speed} Mbit/s").replace("  ", " "));
        }
    }
    if let Ok(rd) = std::fs::read_dir(root.join("sys/kernel/debug/usb/xhci")) {
        for c in rd.flatten() {
            let ctl = c.file_name().to_string_lossy().into_owned();
            if let Ok(ports) = std::fs::read_dir(c.path().join("ports")) {
                for p in ports.flatten() {
                    if let Some(t) = read(&p.path().join("portsc")) {
                        s.ports.insert(format!("{ctl}/{}", p.file_name().to_string_lossy()), t);
                    }
                }
            }
        }
    }
    s
}

/// The log lines that say what changed from `old` to `new` (none: nothing changed).
pub fn changes(old: &Snapshot, new: &Snapshot) -> Vec<String> {
    let mut out = Vec::new();
    for (k, v) in &new.controllers {
        match old.controllers.get(k) {
            None => out.push(format!("usb: controller {k} {v}")),
            Some(o) if o != v => out.push(format!("usb: controller {k} {o} -> {v}")),
            _ => {}
        }
    }
    for (k, v) in &new.devices {
        if !old.devices.contains_key(k) {
            out.push(format!("usb: device {k} appeared: {v}"));
        }
    }
    for k in old.devices.keys().filter(|k| !new.devices.contains_key(*k)) {
        out.push(format!("usb: device {k} went away"));
    }
    for (k, v) in &new.ports {
        match old.ports.get(k) {
            None => out.push(format!("usb: port {k}: {v}")),
            Some(o) if o != v => out.push(format!("usb: port {k}: {o} -> {v}")),
            _ => {}
        }
    }
    out
}

static LAST: Mutex<Option<Snapshot>> = Mutex::new(None);
static DEBUGFS_SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Called every poll: writes the lines for what changed since the last call (the first call writes the starting state).
pub fn step() {
    if cfg!(test) {
        return;
    }
    let root = PathBuf::from("/");
    let now = snapshot(&root);
    if now.ports.is_empty() && !DEBUGFS_SAID.swap(true, std::sync::atomic::Ordering::Relaxed) && !now.controllers.is_empty() {
        log::info!("usb: port states are not available (debugfs is not mounted or has no xHCI ports); devices and controller power are still logged");
    }
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let old = last.take().unwrap_or_default();
    for line in changes(&old, &now) {
        log::info!("{line}");
    }
    *last = Some(now);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(d: &Path, rel: &str, text: &str) {
        let p = d.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("{text}\n")).unwrap();
    }

    fn tree(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-usbports-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        put(&d, "sys/bus/pci/devices/0000:08:00.0/class", "0x0c0330");
        put(&d, "sys/bus/pci/devices/0000:08:00.0/power/runtime_status", "suspended");
        put(&d, "sys/bus/pci/devices/0000:08:00.0/power/control", "auto");
        put(&d, "sys/bus/pci/devices/0000:00:1c.0/class", "0x060400"); // a bridge: not listed
        put(&d, "sys/bus/usb/devices/1-14/idVendor", "328f");
        put(&d, "sys/bus/usb/devices/1-14/idProduct", "00ac");
        put(&d, "sys/bus/usb/devices/1-14/product", "EMEET SmartCam");
        put(&d, "sys/bus/usb/devices/1-14/speed", "480");
        put(&d, "sys/bus/usb/devices/usb1/idVendor", "1d6b"); // a root hub: not a plug-in
        put(&d, "sys/bus/usb/devices/usb1/idProduct", "0002");
        put(&d, "sys/kernel/debug/usb/xhci/0000:08:00.0/ports/port03/portsc", "Powered Not-connected Disabled Link:RxDetect");
        d
    }

    #[test]
    fn a_look_lists_usb_controllers_devices_and_port_states_but_not_bridges_or_root_hubs() {
        let d = tree("look");
        let s = snapshot(&d);
        assert_eq!(s.controllers.keys().collect::<Vec<_>>(), vec!["0000:08:00.0"]);
        assert_eq!(s.controllers["0000:08:00.0"], "suspended/auto");
        assert_eq!(s.devices.len(), 1);
        assert!(s.devices["1-14"].contains("328f:00ac") && s.devices["1-14"].contains("EMEET SmartCam") && s.devices["1-14"].contains("480 Mbit/s"), "{:?}", s.devices);
        assert!(s.ports["0000:08:00.0/port03"].contains("Link:RxDetect"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_lines_say_what_changed_a_connection_that_never_became_a_device_included() {
        let d = tree("diff");
        let before = snapshot(&d);
        assert!(changes(&before, &before).is_empty(), "nothing changed, nothing is written");
        // a stick is plugged into the sleeping Thunderbolt-side controller: the port sees it, no device follows
        put(&d, "sys/kernel/debug/usb/xhci/0000:08:00.0/ports/port03/portsc", "Powered Connected Disabled Link:Polling");
        put(&d, "sys/bus/pci/devices/0000:08:00.0/power/runtime_status", "active");
        let mid = snapshot(&d);
        let lines = changes(&before, &mid);
        assert!(lines.iter().any(|l| l.contains("port 0000:08:00.0/port03") && l.contains("Not-connected") && l.contains("Polling")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("controller 0000:08:00.0 suspended/auto -> active/auto")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("appeared")), "no device came: that is the point");
        // then the device does come, and later goes
        put(&d, "sys/bus/usb/devices/4-1/idVendor", "04e8");
        put(&d, "sys/bus/usb/devices/4-1/idProduct", "4001");
        put(&d, "sys/bus/usb/devices/4-1/product", "PSSD T7");
        put(&d, "sys/bus/usb/devices/4-1/speed", "10000");
        let after = snapshot(&d);
        assert!(changes(&mid, &after).iter().any(|l| l.contains("device 4-1 appeared") && l.contains("04e8:4001") && l.contains("10000 Mbit/s")));
        std::fs::remove_dir_all(d.join("sys/bus/usb/devices/4-1")).unwrap();
        assert_eq!(changes(&after, &snapshot(&d)), vec!["usb: device 4-1 went away".to_string()]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_machine_without_usb_or_debugfs_gives_an_empty_look_and_no_lines() {
        let s = snapshot(Path::new("/nonexistent-root"));
        assert_eq!(s, Snapshot::default());
        assert!(changes(&s, &s).is_empty());
    }
}
