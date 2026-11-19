//! What the machine says about the disk it started from (F-ARCH-06, first Disks slice): where the firmware's startup variables and the
//! block devices are, and whether a partition is on a built-in disk. The startup-disk step of the agent (`startupdisk.rs`) decides what to
//! do with it; reading the kernel's files is done here, in the one place that may.

use std::path::Path;

/// Where the kernel shows the firmware's variables (efivarfs).
pub const EFIVARS: &str = "/sys/firmware/efi/efivars";
/// Where the kernel lists the block devices.
pub const SYS_BLOCK: &str = "/sys/block";

/// Is partition `dev` (`nvme0n1p3`, `sda3`) on a disk that is built in: not flagged removable and not behind USB? `sys_block` is
/// [`SYS_BLOCK`] on the machine and a made-up tree in a test.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partition_on_a_fixed_disk_not_behind_usb_is_internal_and_removable_or_unknown_ones_are_not() {
        let d = std::env::temp_dir().join(format!("iyakki-bootdisk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (disk, part, removable) in [("nvme0n1", "nvme0n1p3", "0"), ("sda", "sda3", "1")] {
            std::fs::create_dir_all(d.join(disk).join(part)).unwrap();
            std::fs::write(d.join(disk).join("removable"), format!("{removable}\n")).unwrap();
        }
        std::fs::create_dir_all(d.join("sdb/sdb1")).unwrap();
        assert!(on_internal_disk(&d, "nvme0n1p3"));
        assert!(!on_internal_disk(&d, "sda3"), "removable");
        assert!(!on_internal_disk(&d, "sdb1"), "no removable flag: unknown counts as not internal");
        assert!(!on_internal_disk(&d, "vda1"), "no such disk");
        assert!(!on_internal_disk(&d.join("missing"), "nvme0n1p3"));
        assert_eq!((EFIVARS, SYS_BLOCK), ("/sys/firmware/efi/efivars", "/sys/block"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
