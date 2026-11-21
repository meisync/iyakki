//! The Secure Boot key writer as typed local calls (F-ARCH-07): the chat tools `secureboot_enroll_status` and `secureboot_enroll` open
//! the firmware's variables, read the prepared `.auth` files and write them through this module, and never name the installer crate.
//! The writer is a trait ([`VarWriter`]) so the order, the refusals and the half-done report are tested with a fake; the real one is
//! `efivarfs`.

use std::path::Path;

pub use iyakki_usb::sbenroll::{auth_file, efi_time, VarSource};
pub use iyakki_usb::sbenroll_write::{Report, Signed, VarWriter};

/// The firmware's variables of this computer and whether it was started through UEFI.
pub fn real_firmware() -> (Box<dyn VarWriter>, bool) {
    (Box::new(iyakki_usb::sbenroll_write::EfiVarFs::real()), iyakki_usb::gpt_install::booted_uefi())
}

/// The three signed files (`PK.auth`, `KEK.auth`, `db.auth`) of a folder; the error says which one is missing.
pub fn load_signed(dir: &Path) -> Result<Signed, String> {
    iyakki_usb::sbenroll_cli::load_signed(dir)
}

/// What the firmware allows, in words (setup mode is the only open state), and which of PK, KEK and db it holds. Changes nothing.
pub fn status_words(vars: &dyn VarSource, efi_present: bool) -> String {
    iyakki_usb::sbenroll_write::status_words(vars, efi_present)
}

/// Write the keys: db, then KEK, then the platform key last, only in setup mode.
pub fn enrol(vars: &mut dyn VarWriter, efi_present: bool, files: &Signed) -> Result<Report, String> {
    iyakki_usb::sbenroll_write::enrol(vars, efi_present, files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake {
        vars: BTreeMap<String, Vec<u8>>,
        order: Vec<String>,
    }
    impl VarSource for Fake {
        fn read(&self, name: &str, _: &str) -> Option<Vec<u8>> {
            self.vars.get(name).cloned()
        }
    }
    impl VarWriter for Fake {
        fn write(&mut self, name: &str, _: &str, _: u32, bytes: &[u8]) -> Result<(), String> {
            self.order.push(name.to_string());
            self.vars.insert(name.to_string(), bytes.to_vec());
            if name == "PK" {
                self.vars.insert("SetupMode".into(), vec![0]);
            }
            Ok(())
        }
    }

    fn folder(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-sbwrite-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let auth = auth_file(&efi_time(2026, 11, 18, 10, 0, 0), &[0x30, 0x03, 1, 2, 3], b"LIST").unwrap();
        for n in ["PK", "KEK", "db"] {
            std::fs::write(d.join(format!("{n}.auth")), &auth).unwrap();
        }
        d
    }

    fn firmware(setup: bool) -> Fake {
        let mut f = Fake::default();
        f.vars.insert("SetupMode".into(), vec![u8::from(setup)]);
        f.vars.insert("SecureBoot".into(), vec![u8::from(!setup)]);
        f
    }

    #[test]
    fn in_setup_mode_the_keys_are_written_db_then_kek_then_the_platform_key() {
        let d = folder("order");
        let signed = load_signed(&d).unwrap();
        let mut fw = firmware(true);
        assert!(status_words(&fw, true).contains("PK absent"));
        let r = enrol(&mut fw, true, &signed).unwrap();
        assert_eq!(r.written, ["db", "KEK", "PK"]);
        assert_eq!(fw.order, ["db", "KEK", "PK"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn user_mode_a_missing_file_and_a_non_uefi_start_write_nothing() {
        let d = folder("refuse");
        let signed = load_signed(&d).unwrap();
        let mut user = firmware(false);
        assert!(enrol(&mut user, true, &signed).unwrap_err().starts_with("nothing was written"));
        let mut bios = firmware(true);
        assert!(enrol(&mut bios, false, &signed).is_err());
        assert!(user.order.is_empty() && bios.order.is_empty());
        std::fs::remove_file(d.join("KEK.auth")).unwrap();
        assert!(load_signed(&d).err().unwrap().contains("KEK.auth"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
