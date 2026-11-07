//! Logs what the firmware says about the startup disk once per boot and, on a built-in disk, puts this system first in the boot order.
//!
//! Why: a stick plugged in while the Mac starts takes over the boot, and the Apple firmware may use its own startup variables (`efi-boot-device`,
//! ...) rather than `BootOrder`. Before anything is written, the next image says what is there (`iyakki dev dmesg` shows these lines), so the choice of
//! what to change rests on the machine's own answer. Plan: docs/development/USB-DETECT-AND-STARTUP-DISK-PLAN.md, section 2.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

static DONE: AtomicBool = AtomicBool::new(false);

/// The lines for the variables in `dir` (efivarfs), empty on a machine without them.
pub fn lines(dir: &Path) -> Vec<String> {
    if !dir.is_dir() {
        return Vec::new();
    }
    iyakki_usb::efi::startup_info(dir).lines()
}

/// Called every poll; acts once, at 12 s of uptime.
pub fn step(uptime_secs: u64) {
    if cfg!(test) || uptime_secs < 12 || DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let efivars = Path::new("/sys/firmware/efi/efivars");
    for l in lines(efivars) {
        log::info!("{l}");
    }
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    if own_first_off(&cmdline) {
        return;
    }
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let Some(dev) = mount_source(&mounts, DATA_DIR) else { return };
    if !on_internal_disk(Path::new("/sys/block"), &dev) {
        log::info!("startup-disk: running from a removable disk; the boot order stays as it is");
        return;
    }
    match iyakki_usb::efi::own_disk_first(efivars, Path::new(DATA_DIR)) {
        Ok(Some(m)) => log::info!("startup-disk: {m} (a disk plugged in at power-on no longer takes over; `iyakki startup-disk revert` undoes it)"),
        Ok(None) => {}
        Err(e) => log::warn!("startup-disk: could not put this disk first: {e}"),
    }
}

const DATA_DIR: &str = "/var/lib/iyakki";

/// `iyakki.startupfirst=off` on the kernel command line leaves the firmware's boot order alone.
pub fn own_first_off(cmdline: &str) -> bool {
    cmdline.split_whitespace().any(|w| w == "iyakki.startupfirst=off")
}

/// The device mounted at `dir` in a `/proc/mounts` text (the last mount wins).
pub fn mount_source(mounts: &str, dir: &str) -> Option<String> {
    mounts.lines().filter_map(|l| {
        let mut f = l.split_whitespace();
        let (src, at) = (f.next()?, f.next()?);
        (at == dir).then(|| src.strip_prefix("/dev/").unwrap_or(src).to_string())
    }).last()
}

/// Is partition `dev` (`nvme0n1p3`, `sda3`) on a disk that is built in: not flagged removable and not behind USB?
pub fn on_internal_disk(sys_block: &Path, dev: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(sys_block) else { return false };
    for e in entries.flatten() {
        let disk = e.file_name().to_string_lossy().to_string();
        if !dev.starts_with(&disk) || !e.path().join(dev).exists() {
            continue;
        }
        let removable = std::fs::read_to_string(e.path().join("removable")).map(|v| v.trim() == "1").unwrap_or(true);
        let real = std::fs::canonicalize(e.path()).unwrap_or_else(|_| e.path());
        return !removable && !real.to_string_lossy().contains("/usb");
    }
    false
}

/// `iyakki startup-disk status | own-first | revert`: what the firmware starts first, and (with `own-first`) make this computer's own IYAKKI entry the first
/// one, which stops a stick that is plugged in at power-on from taking over. `revert` puts the earlier order back. Needs root (it writes a firmware variable).
pub fn cli(args: &[String]) -> i32 {
    let dir = Path::new("/sys/firmware/efi/efivars");
    let backup = Path::new(&std::env::var("IYAKKI_DATA").unwrap_or_else(|_| "/var/lib/iyakki".into())).to_path_buf();
    match args.first().map(String::as_str) {
        None | Some("status") => {
            let l = lines(dir);
            if l.is_empty() {
                crate::qprintln!("This computer was not started through UEFI (no firmware variables).");
                return 1;
            }
            l.iter().for_each(|x| crate::qprintln!("{x}"));
            0
        }
        Some("own-first") => match iyakki_usb::efi::own_disk_first(dir, &backup) {
            Ok(Some(m)) => {
                crate::qprintln!("{m}. `iyakki startup-disk revert` puts the old order back.");
                0
            }
            Ok(None) => {
                crate::qprintln!("Nothing to change: IYAKKI's own entry is already first, or this computer has no IYAKKI entry.");
                0
            }
            Err(e) => {
                crate::qeprintln!("{e}");
                1
            }
        },
        Some("revert") => match iyakki_usb::efi::revert_boot_order(dir, &backup) {
            Ok(true) => {
                crate::qprintln!("The earlier boot order is back.");
                0
            }
            Ok(false) => {
                crate::qprintln!("There is no saved order to put back.");
                0
            }
            Err(e) => {
                crate::qeprintln!("{e}");
                1
            }
        },
        _ => {
            crate::qeprintln!("usage: iyakki startup-disk status | own-first | revert");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_built_in_disk_is_put_first_and_it_can_be_switched_off() {
        let d = std::env::temp_dir().join(format!("iyakki-startdisk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (disk, rem, part) in [("nvme0n1", "0", "nvme0n1p3"), ("sda", "1", "sda3")] {
            std::fs::create_dir_all(d.join(disk).join(part)).unwrap();
            std::fs::write(d.join(disk).join("removable"), rem).unwrap();
        }
        assert!(on_internal_disk(&d, "nvme0n1p3"));
        assert!(!on_internal_disk(&d, "sda3"));
        assert!(!on_internal_disk(&d, "sdb1"));
        let m = "/dev/nvme0n1p3 /var/lib/iyakki ext4 rw 0 0\ntmpfs /tmp tmpfs rw 0 0\n";
        assert_eq!(mount_source(m, "/var/lib/iyakki").as_deref(), Some("nvme0n1p3"));
        assert_eq!(mount_source(m, "/nowhere"), None);
        assert!(own_first_off("quiet iyakki.startupfirst=off"));
        assert!(!own_first_off("quiet"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_machine_without_efi_variables_says_nothing() {
        assert!(lines(Path::new("/nonexistent-efivars")).is_empty());
    }
}
