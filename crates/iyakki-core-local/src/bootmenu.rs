//! The boot loader's menu file on a boot volume as typed local calls (F-ARCH-07): the developer update (`devmode::update`) repairs and
//! rewrites `EFI/iyakki-boot/boot.cfg` through this module so a menu made for a full image never lands unchanged on a lean stick
//! (KI-200), and never names the storage crate.

use std::path::Path;

pub use iyakki_storage::bootcfg::Change;

/// Where the loader's menu file lives on the boot volume (any letter case on the FAT volume).
pub const BOOT_CFG: &str = iyakki_storage::bootcfg::BOOT_CFG;

/// Is this volume-relative path the loader's menu file?
pub fn is_boot_cfg(rel: &str) -> bool {
    rel.eq_ignore_ascii_case(BOOT_CFG)
}

/// The text of a menu that is about to be written to the volume under `root`, with every row that names a missing RAM disk (or the
/// full one on a lean volume) pointed at the one that exists, and one [`Change`] per row changed.
pub fn fixed_for(cfg: &str, root: &Path) -> (String, Vec<Change>) {
    iyakki_storage::bootcfg::fix_for_volume(cfg, root)
}

/// Repair the menu file of a mounted boot volume in place. One log line per change; empty when the file is right or there is none.
pub fn repair(root: &Path) -> Result<Vec<String>, String> {
    iyakki_storage::bootcfg::fix_volume(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(tag: &str, files: &[&str], menu: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-bootmenu-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for f in files {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let m = d.join(BOOT_CFG);
        std::fs::create_dir_all(m.parent().unwrap()).unwrap();
        std::fs::write(m, menu).unwrap();
        d
    }

    const MENU: &str = "entry iyakki\n  title IYAKKI\n  kind linux\n  kernel /boot/bzImage\n  initrd /boot/initrd\n  cmdline quiet\n  default\n";

    #[test]
    fn the_menu_file_is_told_by_its_path_in_any_letter_case() {
        assert!(is_boot_cfg("EFI/iyakki-boot/boot.cfg"));
        assert!(is_boot_cfg("efi/IYAKKI-BOOT/BOOT.CFG"));
        assert!(!is_boot_cfg("EFI/iyakki-boot/boot.cfg.fix"));
    }

    #[test]
    fn a_lean_stick_gets_the_small_ram_disk_in_the_text_and_in_the_file() {
        let d = volume("lean", &["boot/bzImage", "boot/initrd-lean", "boot/root.sfs"], MENU);
        let (text, changes) = fixed_for(MENU, &d);
        assert!(text.contains("initrd /boot/initrd-lean") && !text.contains("initrd /boot/initrd\n"), "{text}");
        assert_eq!(changes.len(), 1);
        let lines = repair(&d).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(std::fs::read_to_string(d.join(BOOT_CFG)).unwrap().contains("initrd /boot/initrd-lean"));
        assert!(repair(&d).unwrap().is_empty(), "a second repair finds nothing to do");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_full_volume_and_a_volume_without_a_menu_are_left_alone() {
        let d = volume("full", &["boot/bzImage", "boot/initrd"], MENU);
        assert!(repair(&d).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(d.join(BOOT_CFG)).unwrap(), MENU);
        let empty = std::env::temp_dir().join(format!("iyakki-bootmenu-none-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        assert!(repair(&empty).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&empty);
    }
}
