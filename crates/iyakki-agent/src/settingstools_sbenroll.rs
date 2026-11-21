//! Chat tools of Secure Boot route 3 (F-BOOT-12; the command line is `iyakki-usb --sb-status` and `--sb-enrol`, docs/SECURE-BOOT.md):
//!
//! * `secureboot_enroll_status` (free, administrators): what the firmware allows, in words (setup mode is the only open state), and
//!   which of the keys PK, KEK and db it holds. Reads variables, writes nothing.
//! * `secureboot_enroll` (administrators; asks EVERY TIME, a hard ask that no standing allow and no Auto mode covers): writes the
//!   three signed `.auth` files that the key owner prepared in a folder of the data folder (`secureboot-enrol/NAME`) into the
//!   firmware: db, then KEK, then the platform key last, only in setup mode. The call must repeat `confirm` = "enrol keys". The signing
//!   key is never read here: the files are signed by the owner's own step (`dist/buildroot/secureboot.sh enrol-files`).
//!
//! Both go through `iyakki_core_local::sbwrite`, the installer's own code; the chat has no other way to touch the firmware's keys.

use std::sync::Arc;

use iyakki_core_local::sbwrite::{self, VarWriter};
use serde_json::json;

use crate::settingstools::{arg_str, Entry, Env};

pub const TOOLS: [&str; 2] = ["secureboot_enroll_status", "secureboot_enroll"];

/// The firmware's variables and whether the computer was started through UEFI (the real one, or a fake in tests).
pub type Open = Arc<dyn Fn() -> (Box<dyn VarWriter>, bool) + Send + Sync>;

pub fn real_open() -> Open {
    Arc::new(sbwrite::real_firmware)
}

/// The folder of the prepared files: a plain name under `secureboot-enrol` of the data folder, nothing else.
fn folder(env: &Env, a: &serde_json::Value) -> Result<std::path::PathBuf, String> {
    let name = arg_str(a, "folder")?;
    if name.is_empty() || name.starts_with('.') || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("`folder` is a plain name (letters, digits, - and _) of a folder under secureboot-enrol in the data folder".into());
    }
    Ok(env.dir.join("secureboot-enrol").join(name))
}

pub fn entries(env: &Env, open: Open) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let open = open.clone();
        v.push(
            Entry::read("Install", "secureboot_enroll_status", "Say what this computer's firmware allows for putting IYAKKI's own Secure Boot key in (only setup mode is open) and which of the keys PK, KEK and db it holds. Changes nothing.", json!({"type": "object", "properties": {}}), move |_| {
                let (vars, efi) = open();
                Ok(sbwrite::status_words(&*vars, efi))
            })
            .admin(),
        );
    }
    {
        let (open1, open2, e, e2, e3) = (open.clone(), open, env.clone(), env.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"folder": {"type": "string", "description": "the folder under secureboot-enrol in the data folder that holds PK.auth, KEK.auth and db.auth"}, "confirm": {"type": "string", "description": "the words: enrol keys"}}, "required": ["folder", "confirm"]});
        v.push(
            Entry::change("Install", "secureboot_enroll", "Write IYAKKI's own Secure Boot keys (three signed .auth files prepared by the key owner) into this computer's firmware, db then KEK then the platform key, only in firmware setup mode. The person approves a card every time; `confirm` must be \"enrol keys\". Cannot be undone from the running system.", schema, |a| format!("Write Secure Boot keys from {} into the firmware", a["folder"].as_str().unwrap_or("a folder")), move |a| {
                let dir = folder(&e, a)?;
                if arg_str(a, "confirm")? != "enrol keys" {
                    return Err("confirm must be the words \"enrol keys\"; nothing was written".into());
                }
                let signed = sbwrite::load_signed(&dir)?;
                let (mut vars, efi) = open1();
                e.audit("secureboot.enrol", "started");
                let r = sbwrite::enrol(&mut *vars, efi, &signed);
                e.audit("secureboot.enrol", if r.is_ok() { "done" } else { "failed" });
                let r = r?;
                Ok(format!("Written: {}. The firmware is in user mode with IYAKKI's key now.", r.written.join(", ")))
            })
            .with_preview(move |a| {
                let (vars, efi) = open2();
                let state = sbwrite::status_words(&*vars, efi);
                let files = folder(&e2, a).map(|d| d.display().to_string()).unwrap_or_else(|e| e);
                format!("{state}\nFiles: {files}\nThis writes Secure Boot keys into the firmware and cannot be undone from the running system; a wrong key can leave a computer that starts nothing signed by anyone else.")
            })
            .with_refusal(move |a| folder(&e3, a).err())
            .admin(),
        );
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Risk;
    use iyakki_core_local::sbwrite::{auth_file, efi_time, VarSource};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        vars: BTreeMap<String, Vec<u8>>,
        log: Arc<Mutex<Vec<String>>>,
    }
    impl VarSource for Fake {
        fn read(&self, name: &str, _: &str) -> Option<Vec<u8>> {
            self.vars.get(name).cloned()
        }
    }
    impl VarWriter for Fake {
        fn write(&mut self, name: &str, _: &str, _: u32, bytes: &[u8]) -> Result<(), String> {
            self.log.lock().unwrap().push(name.to_string());
            self.vars.insert(name.to_string(), bytes.to_vec());
            if name == "PK" {
                self.vars.insert("SetupMode".into(), vec![0]);
            }
            Ok(())
        }
    }

    fn open(setup: bool, log: Arc<Mutex<Vec<String>>>) -> Open {
        Arc::new(move || {
            let mut f = Fake { log: log.clone(), ..Fake::default() };
            f.vars.insert("SetupMode".into(), vec![u8::from(setup)]);
            f.vars.insert("SecureBoot".into(), vec![u8::from(!setup)]);
            (Box::new(f), true)
        })
    }

    fn env(dir: &std::path::Path, audit: Arc<Mutex<Vec<String>>>) -> Env {
        Env { dir: dir.to_path_buf(), user: "root".into(), admin: true, settings: iyakki_shared::settings_ref::SettingsRef::of_store(&Arc::new(Mutex::new(crate::settings::SettingsStore::open(&dir.join("settings.json")).unwrap()))), audit: Arc::new(move |a, d| audit.lock().unwrap().push(format!("{a}: {d}"))) }
    }

    fn call(es: &[Entry], name: &str, a: serde_json::Value) -> Result<String, String> {
        (es.iter().find(|e| e.name == name).unwrap().call)(&a)
    }

    #[test]
    fn status_is_free_and_the_enrol_tool_asks_every_time_needs_the_words_and_writes_in_the_safe_order() {
        let d = crate::testdir::unique("iyakki-sbtool", "a");
        let auth = auth_file(&efi_time(2026, 11, 18, 10, 0, 0), &[0x30, 0x03, 1, 2, 3], b"LIST").unwrap();
        let f = d.join("secureboot-enrol").join("mine");
        std::fs::create_dir_all(&f).unwrap();
        for n in ["PK", "KEK", "db"] {
            std::fs::write(f.join(format!("{n}.auth")), &auth).unwrap();
        }
        let (log, audit) = (Arc::new(Mutex::new(vec![])), Arc::new(Mutex::new(vec![])));
        let es = entries(&env(&d, audit.clone()), open(true, log.clone()));
        assert!(es.iter().all(|e| e.admin_only));
        assert_eq!(es.iter().find(|e| e.name == "secureboot_enroll").unwrap().risk, Risk::Ask);
        assert_eq!(es.iter().find(|e| e.name == "secureboot_enroll_status").unwrap().risk, Risk::Safe);
        assert!(call(&es, "secureboot_enroll_status", json!({})).unwrap().contains("PK absent"));
        // wrong words, bad folder names and a missing folder write nothing
        assert!(call(&es, "secureboot_enroll", json!({"folder": "mine", "confirm": "yes"})).is_err());
        assert!(call(&es, "secureboot_enroll", json!({"folder": "../mine", "confirm": "enrol keys"})).is_err());
        assert!(call(&es, "secureboot_enroll", json!({"folder": "none", "confirm": "enrol keys"})).is_err());
        assert!(log.lock().unwrap().is_empty());
        let out = call(&es, "secureboot_enroll", json!({"folder": "mine", "confirm": "enrol keys"})).unwrap();
        assert!(out.contains("db, KEK, PK"), "{out}");
        assert_eq!(*log.lock().unwrap(), ["db", "KEK", "PK"]);
        assert_eq!(*audit.lock().unwrap(), ["secureboot.enrol: started", "secureboot.enrol: done"]);
    }

    #[test]
    fn user_mode_writes_nothing_and_the_card_says_what_the_firmware_allows() {
        let d = crate::testdir::unique("iyakki-sbtool", "b");
        let auth = auth_file(&efi_time(2026, 11, 18, 10, 0, 0), &[0x30, 0x03, 1, 2, 3], b"LIST").unwrap();
        let f = d.join("secureboot-enrol").join("mine");
        std::fs::create_dir_all(&f).unwrap();
        for n in ["PK", "KEK", "db"] {
            std::fs::write(f.join(format!("{n}.auth")), &auth).unwrap();
        }
        let log = Arc::new(Mutex::new(vec![]));
        let es = entries(&env(&d, Arc::new(Mutex::new(vec![]))), open(false, log.clone()));
        let e = call(&es, "secureboot_enroll", json!({"folder": "mine", "confirm": "enrol keys"})).unwrap_err();
        assert!(e.starts_with("nothing was written"), "{e}");
        assert!(log.lock().unwrap().is_empty());
        let card = (es.iter().find(|e| e.name == "secureboot_enroll").unwrap().preview.as_ref().unwrap())(&json!({"folder": "mine"}));
        assert!(card.contains("cannot be undone"), "{card}");
    }
}
