//! Keep the USB controllers behind Thunderbolt ports awake. (F-ARCH-07: the sysfs reads and writes moved here from the agent; the agent
//! keeps `step`, the timing and the log line.)
//!
//! Observed on the Mac mini 2018 (2026-11-05): the USB-A ports sit on the PCH controller (`0000:00:14.0`, power control `on`), the USB-C
//! ports on the Thunderbolt-side controller (`0000:08:00.0`), which the kernel had runtime-suspended (`control=auto`, `runtime_status=suspended`). A
//! stick or a phone plugged into USB-C after the start was never seen (no line in the kernel log at all), a camera on USB-A was. A suspended
//! controller that does not wake when something is plugged in loses the plug-in; one that is kept on cannot. So on a machine with a Thunderbolt
//! bus every PCI USB host controller that is left to runtime power management is switched to `on` (the same write `touchprobe` and the
//! USB-disk keep-awake already make for USB devices). It costs a little power, which is what a desktop with USB-C ports should spend.
//!
//! `iyakki.usbawake=off` on the kernel command line leaves the controllers alone. Whether this is the cause is tested on the Mac itself: the
//! log line says what was changed, and a USB-C plug-in afterwards either shows up in the kernel log or does not.

use std::path::{Path, PathBuf};

/// PCI class of a USB host controller: base 0x0c, sub 0x03 (any programming interface).
const USB_CLASS: &str = "0x0c03";

/// The PCI USB host controllers that runtime power management may still put to sleep, on a machine that has a Thunderbolt bus.
pub fn sleeping_controllers(root: &Path) -> Vec<PathBuf> {
    if !root.join("sys/bus/thunderbolt").is_dir() {
        return Vec::new();
    }
    let Ok(rd) = std::fs::read_dir(root.join("sys/bus/pci/devices")) else { return Vec::new() };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|d| std::fs::read_to_string(d.join("class")).is_ok_and(|c| c.trim().starts_with(USB_CLASS)))
        .filter(|d| std::fs::read_to_string(d.join("power/control")).is_ok_and(|c| c.trim() == "auto"))
        .collect();
    out.sort();
    out
}

/// Write `on` to each; the names of the controllers that took it.
pub fn keep_awake(controllers: &[PathBuf]) -> Vec<String> {
    controllers
        .iter()
        .filter(|d| std::fs::write(d.join("power/control"), "on").is_ok())
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect()
}

/// The report of the boot stage `usbawake` (dist/qemu/usb-awake-test.sh) on the machine under `root`: whether it has a Thunderbolt bus, every PCI USB controller with its power
/// setting, the controllers put in `auto` first (the emulated ones start as `on`, the Mac mini's Thunderbolt-side controller was in `auto`), the ones [`sleeping_controllers`] finds, how many
/// [`keep_awake`] switched, and each one's setting afterwards. The lines are plain text for the host script.
pub fn selftest_lines(root: &Path) -> Vec<String> {
    let mut out = vec![format!("thunderbolt-bus {}", root.join("sys/bus/thunderbolt").is_dir())];
    let ctl = |d: &Path| std::fs::read_to_string(d.join("power/control")).unwrap_or_default().trim().to_string();
    let name = |d: &Path| d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut all: Vec<PathBuf> = std::fs::read_dir(root.join("sys/bus/pci/devices")).into_iter().flatten().flatten().map(|e| e.path()).filter(|d| std::fs::read_to_string(d.join("class")).is_ok_and(|c| c.trim().starts_with(USB_CLASS))).collect();
    all.sort();
    for d in &all {
        out.push(format!("usb-controller {} control={}", name(d), ctl(d)));
    }
    for d in &all {
        let _ = std::fs::write(d.join("power/control"), "auto");
        out.push(format!("set-auto {} control={}", name(d), ctl(d)));
    }
    let sleeping = sleeping_controllers(root);
    out.push(format!("sleeping {}", sleeping.len()));
    out.push(format!("switched {}", keep_awake(&sleeping).len()));
    for d in &sleeping {
        out.push(format!("after {} control={}", name(d), ctl(d)));
    }
    out
}

/// Is the switch-off on the command line?
pub fn switched_off(cmdline: &str) -> bool {
    cmdline.split_whitespace().any(|w| w == "iyakki.usbawake=off")
}

/// [`switched_off`] for this machine's command line.
pub fn switched_off_here() -> bool {
    switched_off(&iyakki_storage::cmdline_text())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str, thunderbolt: bool) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-usbawake-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let dev = |id: &str, class: &str, ctl: &str| {
            let p = d.join("sys/bus/pci/devices").join(id);
            std::fs::create_dir_all(p.join("power")).unwrap();
            std::fs::write(p.join("class"), format!("{class}\n")).unwrap();
            std::fs::write(p.join("power/control"), format!("{ctl}\n")).unwrap();
        };
        dev("0000:08:00.0", "0x0c0330", "auto"); // the Thunderbolt-side xHCI, left to runtime PM
        dev("0000:00:14.0", "0x0c0330", "on"); // the PCH xHCI, already on
        dev("0000:00:1c.0", "0x060400", "auto"); // a PCI bridge: not a USB controller
        dev("0000:02:00.0", "0x0c0310", "auto"); // an OHCI-class USB controller (prog-if 0x10): also a USB host controller
        if thunderbolt {
            std::fs::create_dir_all(d.join("sys/bus/thunderbolt")).unwrap();
        }
        d
    }

    #[test]
    fn on_a_thunderbolt_machine_the_sleeping_usb_controllers_are_kept_awake_and_nothing_else_is_touched() {
        let d = tree("tb", true);
        let sleeping = sleeping_controllers(&d);
        let names: Vec<String> = sleeping.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["0000:02:00.0", "0000:08:00.0"], "USB controllers left to runtime PM; not the bridge, not the one that is already on");
        assert_eq!(keep_awake(&sleeping), vec!["0000:02:00.0".to_string(), "0000:08:00.0".to_string()]);
        let ctl = |id: &str| std::fs::read_to_string(d.join("sys/bus/pci/devices").join(id).join("power/control")).unwrap();
        assert_eq!(ctl("0000:08:00.0").trim(), "on");
        assert_eq!(ctl("0000:00:1c.0").trim(), "auto", "a bridge is left alone");
        assert!(sleeping_controllers(&d).is_empty(), "a second look finds nothing to do");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn without_a_thunderbolt_bus_nothing_is_changed() {
        let d = tree("plain", false);
        assert!(sleeping_controllers(&d).is_empty(), "a plain PC keeps its normal USB power management");
        let _ = std::fs::remove_dir_all(&d);
        assert!(sleeping_controllers(Path::new("/nonexistent-root")).is_empty());
    }

    #[test]
    fn the_command_line_can_switch_it_off() {
        assert!(switched_off("quiet iyakki.usbawake=off loglevel=3"));
        assert!(!switched_off("quiet iyakki.usbawake=on"));
        assert!(!switched_off("quiet"));
    }
}
