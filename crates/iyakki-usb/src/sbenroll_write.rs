//! The write side of Secure Boot route 3 (F-BOOT-12): put the three signed `.auth` files into the firmware through `efivarfs`, in the
//! only safe order, and only when [`crate::sbenroll::judge`] says setup mode. The variables come and go through a trait, so the order, the
//! refusals and the half-done report are tested with a fake; [`EfiVarFs`] is the real one (a folder of files named `NAME-GUID`, each
//! `4 attribute bytes + contents`, written in ONE `write` call as the kernel requires).
//!
//! Order: db, then KEK, then PK last. Writing the PK ends setup mode, so a failure before it leaves the firmware exactly as open as it
//! was (nothing is locked); a failure of the PK write itself leaves db and KEK added, which only trust the same certificate.

use crate::sbenroll::{judge, Var, VarSource, Verdict, AUTH_ATTRS, GLOBAL_GUID};
use std::path::PathBuf;

pub trait VarWriter: VarSource {
    /// Write one variable (the whole `.auth` blob for a time-based authenticated write).
    fn write(&mut self, name: &str, guid: &str, attrs: u32, bytes: &[u8]) -> Result<(), String>;
}

/// The signed files of one enrolment, in any order; [`enrol`] writes them in the safe one.
pub struct Signed {
    pub pk: Vec<u8>,
    pub kek: Vec<u8>,
    pub db: Vec<u8>,
}

/// What was written, or what stopped it and what is left.
#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub written: Vec<&'static str>,
}

/// Enrol. Refuses (nothing written) unless the firmware is in setup mode and every file looks like an `.auth` file.
pub fn enrol(vars: &mut dyn VarWriter, efi_present: bool, files: &Signed) -> Result<Report, String> {
    let v = judge(vars, efi_present);
    if v != Verdict::SetupMode {
        return Err(format!("nothing was written. {}", v.words()));
    }
    for (what, f) in [("PK", &files.pk), ("KEK", &files.kek), ("db", &files.db)] {
        // 16 bytes of time, then a WIN_CERTIFICATE_UEFI_GUID whose length field is at 16..20
        let ok = f.len() > 36 && u32::from_le_bytes([f[16], f[17], f[18], f[19]]) as usize >= 24 && f[20..22] == [0x00, 0x02] && f[22..24] == [0xF1, 0x0E];
        if !ok {
            return Err(format!("nothing was written. The {what} file is not a signed .auth file."));
        }
    }
    let mut written = Vec::new();
    for (var, blob) in [(Var::Db, &files.db), (Var::Kek, &files.kek), (Var::Pk, &files.pk)] {
        if let Err(e) = vars.write(var.name(), var.guid(), AUTH_ATTRS, blob) {
            let left = if written.is_empty() { "nothing was changed".to_string() } else { format!("{} already written", written.join(" and ")) };
            let state = if var == Var::Pk { "the platform key was refused, so the firmware is still in setup mode and nothing is locked" } else { "the firmware is still in setup mode and nothing is locked" };
            return Err(format!("the firmware refused {}: {e}; {left}; {state}.", var.name()));
        }
        written.push(var.name());
    }
    match judge(vars, efi_present) {
        Verdict::SetupMode => Err("the three keys were written but the firmware is still in setup mode: it did not take the platform key.".into()),
        _ => Ok(Report { written }),
    }
}

/// `efivarfs` as a folder of `NAME-GUID` files.
pub struct EfiVarFs {
    pub root: PathBuf,
}

impl EfiVarFs {
    pub fn real() -> EfiVarFs {
        EfiVarFs { root: PathBuf::from(crate::gpt_install::EFIVARS_DIR) }
    }
    fn path(&self, name: &str, guid: &str) -> PathBuf {
        self.root.join(format!("{name}-{guid}"))
    }
}

impl VarSource for EfiVarFs {
    fn read(&self, name: &str, guid: &str) -> Option<Vec<u8>> {
        let b = std::fs::read(self.path(name, guid)).ok()?;
        (b.len() >= 4).then(|| b[4..].to_vec())
    }
}

impl VarWriter for EfiVarFs {
    fn write(&mut self, name: &str, guid: &str, attrs: u32, bytes: &[u8]) -> Result<(), String> {
        use std::io::Write;
        if name.is_empty() || name.contains('/') || guid.contains('/') {
            return Err("not a variable name".into());
        }
        let mut one = attrs.to_le_bytes().to_vec();
        one.extend_from_slice(bytes);
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(self.path(name, guid)).map_err(|e| e.to_string())?;
        f.write_all(&one).map_err(|e| e.to_string())
    }
}

/// The free, read-only answer for the chat tool `secureboot_enroll_status`: what the firmware allows, in words.
pub fn status_words(vars: &dyn VarSource, efi_present: bool) -> String {
    let v = judge(vars, efi_present);
    let have = |n: &str| if vars.read(n, if n == "db" { crate::sbenroll::DB_GUID } else { GLOBAL_GUID }).is_some() { "present" } else { "absent" };
    format!("{} (PK {}, KEK {}, db {}.)", v.words(), have("PK"), have("KEK"), have("db"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake {
        vars: BTreeMap<String, Vec<u8>>,
        log: Vec<String>,
        refuse: Option<&'static str>,
        pk_takes_effect: bool,
    }
    impl VarSource for Fake {
        fn read(&self, name: &str, _: &str) -> Option<Vec<u8>> {
            self.vars.get(name).cloned()
        }
    }
    impl VarWriter for Fake {
        fn write(&mut self, name: &str, _: &str, attrs: u32, bytes: &[u8]) -> Result<(), String> {
            assert_eq!(attrs, AUTH_ATTRS);
            if self.refuse == Some(name) {
                return Err("security violation".into());
            }
            self.log.push(name.to_string());
            self.vars.insert(name.to_string(), bytes.to_vec());
            if name == "PK" && self.pk_takes_effect {
                self.vars.insert("SetupMode".into(), vec![0]);
            }
            Ok(())
        }
    }
    fn setup() -> Fake {
        let mut f = Fake { pk_takes_effect: true, ..Fake::default() };
        f.vars.insert("SetupMode".into(), vec![1]);
        f.vars.insert("SecureBoot".into(), vec![0]);
        f
    }
    fn auth() -> Vec<u8> {
        crate::sbenroll::auth_file(&crate::sbenroll::efi_time(2026, 11, 18, 10, 0, 0), &[0x30, 0x03, 1, 2, 3], b"LIST").unwrap()
    }
    fn files() -> Signed {
        Signed { pk: auth(), kek: auth(), db: auth() }
    }

    #[test]
    fn the_keys_go_in_as_db_then_kek_then_the_platform_key_last_and_setup_mode_ends() {
        let mut f = setup();
        let r = enrol(&mut f, true, &files()).unwrap();
        assert_eq!(f.log, ["db", "KEK", "PK"]);
        assert_eq!(r.written, ["db", "KEK", "PK"]);
    }

    #[test]
    fn user_mode_and_no_uefi_write_nothing_and_say_where_to_go() {
        let mut f = setup();
        f.vars.insert("SetupMode".into(), vec![0]);
        f.vars.insert("SecureBoot".into(), vec![1]);
        let e = enrol(&mut f, true, &files()).unwrap_err();
        assert!(e.starts_with("nothing was written") && e.contains("setup screen"), "{e}");
        assert!(f.log.is_empty());
        assert!(enrol(&mut setup(), false, &files()).unwrap_err().contains("UEFI"));
    }

    #[test]
    fn a_file_that_is_not_a_signed_auth_file_stops_everything_before_the_first_write() {
        let mut f = setup();
        let mut s = files();
        s.kek = vec![0; 100];
        assert!(enrol(&mut f, true, &s).unwrap_err().contains("KEK file is not a signed .auth file"));
        assert!(f.log.is_empty());
    }

    #[test]
    fn a_refusal_names_the_variable_what_was_written_and_that_nothing_is_locked() {
        let mut f = setup();
        f.refuse = Some("PK");
        let e = enrol(&mut f, true, &files()).unwrap_err();
        assert!(e.contains("refused PK") && e.contains("db and KEK already written") && e.contains("still in setup mode"), "{e}");
        let mut g = setup();
        g.refuse = Some("db");
        let e = enrol(&mut g, true, &files()).unwrap_err();
        assert!(e.contains("nothing was changed") && g.log.is_empty(), "{e}");
    }

    #[test]
    fn a_firmware_that_stays_in_setup_mode_after_all_three_writes_is_reported_not_called_done() {
        let mut f = setup();
        f.pk_takes_effect = false;
        assert!(enrol(&mut f, true, &files()).unwrap_err().contains("did not take the platform key"));
    }

    #[test]
    fn efivarfs_files_hold_four_attribute_bytes_then_the_contents_in_one_file_per_variable() {
        let d = std::env::temp_dir().join(format!("iyakki-usb-efivars-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let mut v = EfiVarFs { root: d.clone() };
        v.write("db", crate::sbenroll::DB_GUID, AUTH_ATTRS, b"xyz").unwrap();
        let raw = std::fs::read(d.join(format!("db-{}", crate::sbenroll::DB_GUID))).unwrap();
        assert_eq!(&raw[..4], &AUTH_ATTRS.to_le_bytes());
        assert_eq!(&raw[4..], b"xyz");
        assert_eq!(v.read("db", crate::sbenroll::DB_GUID).unwrap(), b"xyz");
        assert!(v.write("../x", GLOBAL_GUID, 7, b"").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_status_words_name_the_route_and_which_keys_exist() {
        let t = status_words(&setup(), true);
        assert!(t.contains("setup mode") && t.contains("PK absent"), "{t}");
    }
}
