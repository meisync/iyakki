//! Small readings the app-event watchers take of the machine (F-ARCH-07): the memory-pressure file, the processor's heat slow-down count,
//! the names of the cameras and the pending shutdown notice. This is the one reader of those files; every function has an `_in` form that
//! takes the roots so a test uses a made-up tree.

use std::collections::BTreeSet;
use std::path::Path;

/// Is this an Apple computer with a T2 security chip (the model name is a T2 model and the vendor is Apple)?
pub fn is_apple_t2(product_name: &str, sys_vendor: &str) -> bool {
    iyakki_drivers::macboard::is_t2(product_name) && sys_vendor.starts_with("Apple")
}

/// The text of `<proc_>/pressure/memory`, empty when the kernel has no pressure file.
pub fn memory_pressure_in(proc_: &Path) -> String {
    std::fs::read_to_string(proc_.join("pressure/memory")).unwrap_or_default()
}

/// [`memory_pressure_in`] on this machine.
pub fn memory_pressure() -> String {
    memory_pressure_in(Path::new("/proc"))
}

/// The processor's heat slow-down count, summed over the cores that report one (`devices/system/cpu/*/thermal_throttle/core_throttle_count`).
pub fn throttle_total_in(sys: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(sys.join("devices/system/cpu")) else { return 0 };
    rd.flatten().filter_map(|e| std::fs::read_to_string(e.path().join("thermal_throttle/core_throttle_count")).ok()).filter_map(|t| t.trim().parse::<u64>().ok()).sum()
}

/// The trimmed names of the cameras (`<sys>/class/video4linux/*/name`), without duplicates.
pub fn camera_names_in(sys: &Path) -> BTreeSet<String> {
    std::fs::read_dir(sys.join("class/video4linux")).into_iter().flatten().flatten().filter_map(|e| std::fs::read_to_string(e.path().join("name")).ok()).map(|n| n.trim().to_string()).collect()
}

/// [`camera_names_in`] on this machine.
pub fn camera_names() -> BTreeSet<String> {
    camera_names_in(Path::new("/sys"))
}

/// The text of the pending-shutdown notice systemd leaves (`/run/systemd/shutdown/scheduled`), `None` when none is pending.
pub fn shutdown_scheduled() -> Option<String> {
    std::fs::read_to_string("/run/systemd/shutdown/scheduled").ok()
}

/// Whether a PCI device of the wireless network class (`0x0280`, "network controller") exists under `<sys>/bus/pci/devices`: a Wi-Fi card
/// the kernel has seen even when no driver made an interface for it yet.
pub fn pci_wifi_present_in(sys: &Path) -> bool {
    std::fs::read_dir(sys.join("bus/pci/devices")).map(|d| d.flatten().any(|e| std::fs::read_to_string(e.path().join("class")).is_ok_and(|c| c.trim().starts_with("0x0280")))).unwrap_or(false)
}

/// [`pci_wifi_present_in`] on this machine.
pub fn pci_wifi_present() -> bool {
    pci_wifi_present_in(Path::new("/sys"))
}

/// The machine's product name (`<sys>/class/dmi/id/product_name`, trimmed), empty when it has none.
pub fn product_name_in(sys: &Path) -> String {
    crate::debugfacts::dmi_in(sys, "product_name")
}

/// The machine's board name (`<sys>/class/dmi/id/board_name`, trimmed), empty when it has none.
pub fn board_name_in(sys: &Path) -> String {
    crate::debugfacts::dmi_in(sys, "board_name")
}

/// [`product_name_in`] on this machine.
pub fn product_name() -> String {
    product_name_in(Path::new("/sys"))
}

/// [`board_name_in`] on this machine.
pub fn board_name() -> String {
    board_name_in(Path::new("/sys"))
}

/// What processor 0 runs at now and which governor chose it, as the files hold them (`?` for a file that is not there), for the shell's
/// ten-second frame line.
pub fn cpu0_speed_in(sys: &Path) -> (String, String) {
    let f = |n: &str| std::fs::read_to_string(sys.join("devices/system/cpu/cpu0/cpufreq").join(n)).map(|s| s.trim().to_string()).unwrap_or_else(|_| "?".into());
    (f("scaling_cur_freq"), f("scaling_governor"))
}

/// [`cpu0_speed_in`] on this machine.
pub fn cpu0_speed() -> (String, String) {
    cpu0_speed_in(Path::new("/sys"))
}

/// The host name written in `file` (trimmed), `fallback` when the file is missing.
pub fn hostname_in(file: &Path, fallback: &str) -> String {
    std::fs::read_to_string(file).map(|s| s.trim().to_string()).unwrap_or_else(|_| fallback.to_string())
}

/// The host name of this machine (`/etc/hostname`), `fallback` when it is not readable.
pub fn hostname(fallback: &str) -> String {
    hostname_in(Path::new("/etc/hostname"), fallback)
}

/// The remote-diagnostics note the boot writes (`/run/iyakki-ssh`, how to reach this machine), trimmed; None when it is not there.
pub fn ssh_note_in(file: &Path) -> Option<String> {
    std::fs::read_to_string(file).ok().map(|s| s.trim().to_string())
}

pub fn ssh_note() -> Option<String> {
    ssh_note_in(Path::new("/run/iyakki-ssh"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_t2_model_counts_only_with_an_apple_vendor() {
        assert!(is_apple_t2("MacBookPro15,2", "Apple Inc."));
        assert!(!is_apple_t2("MacBookPro15,2", "Some Maker"));
        assert!(!is_apple_t2("ThinkPad X1", "Apple Inc."));
        assert!(!is_apple_t2("", ""));
    }

    fn tree(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-machfacts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_pressure_text_is_empty_when_the_file_is_missing() {
        let d = tree("psi");
        assert_eq!(memory_pressure_in(&d), "");
        std::fs::create_dir_all(d.join("pressure")).unwrap();
        std::fs::write(d.join("pressure/memory"), "some avg10=1.00\n").unwrap();
        assert_eq!(memory_pressure_in(&d), "some avg10=1.00\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn throttle_counts_add_up_over_the_cores_that_report_and_cameras_are_trimmed_names() {
        let d = tree("cpu");
        assert_eq!(throttle_total_in(&d), 0);
        for (i, n) in [(0, Some("3\n")), (1, Some("4\n")), (2, None), (3, Some("x"))] {
            let c = d.join(format!("devices/system/cpu/cpu{i}/thermal_throttle"));
            std::fs::create_dir_all(&c).unwrap();
            if let Some(n) = n {
                std::fs::write(c.join("core_throttle_count"), n).unwrap();
            }
        }
        assert_eq!(throttle_total_in(&d), 7);
        assert!(camera_names_in(&d).is_empty());
        for (i, n) in [(0, "Front cam\n"), (1, "Front cam\n"), (2, "IR cam\n")] {
            let c = d.join(format!("class/video4linux/video{i}"));
            std::fs::create_dir_all(&c).unwrap();
            std::fs::write(c.join("name"), n).unwrap();
        }
        assert_eq!(camera_names_in(&d).into_iter().collect::<Vec<_>>(), ["Front cam", "IR cam"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_wifi_card_is_a_pci_device_of_the_network_controller_class() {
        let d = tree("pciwifi");
        assert!(!pci_wifi_present_in(&d), "no pci folder: no card");
        std::fs::create_dir_all(d.join("bus/pci/devices/0000:00:1c.0")).unwrap();
        std::fs::write(d.join("bus/pci/devices/0000:00:1c.0/class"), "0x060400\n").unwrap();
        std::fs::create_dir_all(d.join("bus/pci/devices/0000:02:00.0")).unwrap();
        std::fs::write(d.join("bus/pci/devices/0000:02:00.0/class"), "0x028000\n").unwrap();
        assert!(pci_wifi_present_in(&d));
        std::fs::write(d.join("bus/pci/devices/0000:02:00.0/class"), "0x020000\n").unwrap();
        assert!(!pci_wifi_present_in(&d), "an Ethernet card is not a Wi-Fi card");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_machine_names_and_the_processor_speed_come_from_the_given_root() {
        let d = tree("names");
        assert_eq!((product_name_in(&d), board_name_in(&d)), (String::new(), String::new()));
        assert_eq!(cpu0_speed_in(&d), ("?".to_string(), "?".to_string()));
        std::fs::create_dir_all(d.join("class/dmi/id")).unwrap();
        std::fs::write(d.join("class/dmi/id/product_name"), "MacBookPro14,1\n").unwrap();
        std::fs::write(d.join("class/dmi/id/board_name"), "Mac-B4831CEBD52A0C4C\n").unwrap();
        assert_eq!((product_name_in(&d).as_str(), board_name_in(&d).as_str()), ("MacBookPro14,1", "Mac-B4831CEBD52A0C4C"));
        std::fs::create_dir_all(d.join("devices/system/cpu/cpu0/cpufreq")).unwrap();
        std::fs::write(d.join("devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"), "2300000\n").unwrap();
        assert_eq!(cpu0_speed_in(&d), ("2300000".to_string(), "?".to_string()));
        std::fs::write(d.join("devices/system/cpu/cpu0/cpufreq/scaling_governor"), "powersave\n").unwrap();
        assert_eq!(cpu0_speed_in(&d).1, "powersave");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_host_name_and_the_remote_diagnostics_note_are_trimmed_and_a_missing_file_gives_the_fallback() {
        let d = tree("hostname");
        assert_eq!(hostname_in(&d.join("hostname"), "iyakki"), "iyakki");
        assert_eq!(ssh_note_in(&d.join("note")), None);
        std::fs::write(d.join("hostname"), "iyakki-30ed15\n").unwrap();
        std::fs::write(d.join("note"), "ssh admin@10.0.0.2\n").unwrap();
        assert_eq!(hostname_in(&d.join("hostname"), "iyakki"), "iyakki-30ed15");
        assert_eq!(ssh_note_in(&d.join("note")).as_deref(), Some("ssh admin@10.0.0.2"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
