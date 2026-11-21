//! The default-entry switch of the signed unified kernel image (F-BOOT-02). When the boot partition carries `EFI/IYAKKI/iyakki-uki.efi`
//! (built by `secureboot.sh image` when `IYAKKI_UKI_STUB` is set), the boot menu shows a row "IYAKKI (signed image)" that starts it, and
//! the person may make that row the default. The switch is one small GRUB file, `boot/grub/iyakki-uki-default.cfg`, that the menu sources;
//! without the file (the normal state) every entry and the default are exactly as before, so the plain GRUB start is always one menu
//! press away and a broken image is never the only way in.

use std::path::Path;

/// The GRUB file the menu sources (next to `grub.cfg`).
pub const DEFAULT_FILE: &str = "iyakki-uki-default.cfg";
/// The unified image on the boot partition.
pub const UKI_PATH: &str = "EFI/IYAKKI/iyakki-uki.efi";
/// The id of the menu row; the file makes it the default by this id.
pub const ENTRY_ID: &str = "iyakki-uki";

/// The text of the GRUB file: only a `set default` of the entry's id, nothing a person typed.
pub fn file_text() -> String {
    format!("# written by IYAKKI (Settings -> Boot): the signed unified image is the default entry; delete this file to go back\nset default=\"{ENTRY_ID}\"\n")
}

/// Is the default switch on, on the boot partition under `boot_root`?
pub fn is_on(boot_root: &Path) -> bool {
    boot_root.join("boot/grub").join(DEFAULT_FILE).is_file()
}

/// Does this boot partition carry the unified image (so the row exists)?
pub fn has_image(boot_root: &Path) -> bool {
    std::fs::metadata(boot_root.join(UKI_PATH)).is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// Turn the switch on or off under `boot_root`. On is refused unless the image is there (a default that points at nothing would leave
/// the menu on a missing entry; GRUB would fall back to the first row, but the person should be told instead).
pub fn write_default_file(boot_root: &Path, on: bool) -> Result<String, String> {
    let file = boot_root.join("boot/grub").join(DEFAULT_FILE);
    if on {
        if !boot_root.join("boot/grub").is_dir() {
            return Err("the boot partition has no boot/grub folder (not an IYAKKI boot partition)".into());
        }
        if !has_image(boot_root) {
            return Err("this boot partition has no signed unified image (EFI/IYAKKI/iyakki-uki.efi): it was not built into this image, so the default stays as it is".into());
        }
        std::fs::write(&file, file_text()).map_err(|e| format!("could not write {}: {e}", file.display()))?;
        Ok("The signed unified image starts by default from the next start. Choose the plain \"IYAKKI\" row in the boot menu, or switch this off, to go back.".into())
    } else {
        match std::fs::remove_file(&file) {
            Ok(()) => Ok("The plain IYAKKI start is the default again from the next start.".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("The signed unified image was not the default.".into()),
            Err(e) => Err(format!("could not remove {}: {e}", file.display())),
        }
    }
}

/// Find the boot partition and turn the switch on or off there.
pub fn set_default_on_boot_partition(on: bool) -> Result<String, String> {
    crate::storageloss::with_boot_partition(|root| write_default_file(root, on))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boot_root(tag: &str, image: bool) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("iyakki-ukidefault-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("boot/grub")).unwrap();
        if image {
            std::fs::create_dir_all(root.join("EFI/IYAKKI")).unwrap();
            std::fs::write(root.join(UKI_PATH), b"MZ-signed-image").unwrap();
        }
        root
    }

    #[test]
    fn the_switch_is_a_file_that_only_sets_the_default_and_only_with_the_image_present() {
        let with = boot_root("with", true);
        assert!(has_image(&with) && !is_on(&with));
        assert!(write_default_file(&with, true).unwrap().contains("next start"));
        assert!(is_on(&with));
        let text = std::fs::read_to_string(with.join("boot/grub").join(DEFAULT_FILE)).unwrap();
        assert_eq!(text.lines().filter(|l| !l.starts_with('#')).collect::<Vec<_>>(), ["set default=\"iyakki-uki\""], "nothing but the default");
        assert!(write_default_file(&with, false).unwrap().contains("plain IYAKKI start"));
        assert!(!is_on(&with));
        assert!(write_default_file(&with, false).unwrap().contains("was not the default"));
        let _ = std::fs::remove_dir_all(&with);
        let without = boot_root("without", false);
        assert!(write_default_file(&without, true).unwrap_err().contains("no signed unified image"));
        assert!(!is_on(&without), "a refused switch writes nothing");
        let _ = std::fs::remove_dir_all(&without);
        let empty = std::env::temp_dir().join(format!("iyakki-ukidefault-{}-empty", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        assert!(write_default_file(&empty, true).unwrap_err().contains("not an IYAKKI boot partition"));
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn an_empty_image_file_does_not_count() {
        let root = boot_root("empty-image", true);
        std::fs::write(root.join(UKI_PATH), b"").unwrap();
        assert!(!has_image(&root));
        assert!(write_default_file(&root, true).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
