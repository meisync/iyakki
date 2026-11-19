//! The instances this computer has enrolled a device on (`iyakki-connect window`, `devices`): a name, the address, the device id the
//! instance gave, and one Ed25519 device key per instance. Keys live in `<config>/device-keys/` (folder 700, files 600, like `key new`);
//! `<config>/instances.json` (600) holds the rest and never a secret. An operating-system key store (Keychain, DPAPI, Secret Service) is
//! a later step: until then the key is a file only this user can read, and the program, not the web page, is what uses it.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use iyakki_link::sigscheme::DeviceKey;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::devkey::KeyFile;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// What the person calls it.
    pub name: String,
    /// `host:port` as the pin is stored.
    pub addr: String,
    /// The id the instance gave this device; none until the enrolment finished.
    pub device: Option<String>,
    /// Public key, 64 hex digits (for the listing).
    pub public: String,
}

impl Record {
    fn to_json(&self) -> Value {
        json!({"name": self.name, "addr": self.addr, "device": self.device, "public": self.public})
    }

    fn from_json(v: &Value) -> Option<Record> {
        Some(Record { name: v["name"].as_str()?.into(), addr: v["addr"].as_str()?.into(), device: v["device"].as_str().map(str::to_string), public: v["public"].as_str()?.into() })
    }
}

pub struct Instances {
    dir: PathBuf,
}

/// A name for a list: 1 to 60 characters, no control characters, no slash.
pub fn clean_name(n: &str) -> Result<String, String> {
    let n = n.trim();
    if n.is_empty() || n.chars().count() > 60 || n.chars().any(|c| c.is_control() || c == '/' || c == '\\') {
        return Err("a name is 1 to 60 characters, without slashes or control characters".into());
    }
    Ok(n.to_string())
}

/// The name an instance gets when the person gives none: its host.
pub fn default_name(addr: &str) -> String {
    addr.rsplit_once(':').map_or(addr, |(h, _)| h).trim_matches(|c| c == '[' || c == ']').to_string()
}

impl Instances {
    pub fn new(config_dir: &Path) -> Instances {
        Instances { dir: config_dir.to_path_buf() }
    }

    fn file(&self) -> PathBuf {
        self.dir.join("instances.json")
    }

    fn key_file(&self, addr: &str) -> KeyFile {
        let h = Sha256::digest(addr.as_bytes());
        let name = iyakki_link::sigscheme::hex(&h[..12]);
        let kf = KeyFile::at(self.dir.join("device-keys").join(format!("{name}.key")));
        match crate::keystore::OsStore::from_env() {
            Ok(Some(store)) => kf.with_os_store(std::sync::Arc::new(store), &name),
            _ => kf,
        }
    }

    pub fn list(&self) -> Result<Vec<Record>, String> {
        match std::fs::read(self.file()) {
            Ok(b) => {
                let v: Value = serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", self.file().display()))?;
                v.as_array().and_then(|a| a.iter().map(Record::from_json).collect()).ok_or_else(|| format!("{} is not understood", self.file().display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(e) => Err(format!("{}: {e}", self.file().display())),
        }
    }

    /// By name (any case) or by address.
    pub fn find(&self, name_or_addr: &str) -> Result<Option<Record>, String> {
        let want = name_or_addr.trim();
        let key = crate::pins::key(want).ok();
        Ok(self.list()?.into_iter().find(|r| r.name.eq_ignore_ascii_case(want) || key.as_deref() == Some(r.addr.as_str())))
    }

    /// The key of the instance at `addr`, if one was saved.
    pub fn key(&self, addr: &str) -> Result<Option<DeviceKey>, String> {
        self.key_file(addr).load()
    }

    /// Save (or replace) the record of `rec.addr` with its key. A name another instance uses is refused.
    pub fn save(&self, rec: &Record, key: &DeviceKey) -> Result<(), String> {
        let name = clean_name(&rec.name)?;
        let mut all = self.list()?;
        if all.iter().any(|r| r.name.eq_ignore_ascii_case(&name) && r.addr != rec.addr) {
            return Err(format!("the name {name:?} is used for another instance: choose another with --name"));
        }
        self.key_file(&rec.addr).store(key, true)?;
        all.retain(|r| r.addr != rec.addr);
        all.push(Record { name, ..rec.clone() });
        self.write(&all)
    }

    /// Forget an instance: the record and its key file. True when there was one.
    pub fn forget(&self, name_or_addr: &str) -> Result<bool, String> {
        let Some(rec) = self.find(name_or_addr)? else { return Ok(false) };
        let all: Vec<Record> = self.list()?.into_iter().filter(|r| r.addr != rec.addr).collect();
        self.write(&all)?;
        if let Ok(Some(store)) = crate::keystore::OsStore::from_env() {
            let _ = store.delete(&iyakki_link::sigscheme::hex(&Sha256::digest(rec.addr.as_bytes())[..12]));
        }
        match std::fs::remove_file(self.dir.join("device-keys").join(format!("{}.key", iyakki_link::sigscheme::hex(&Sha256::digest(rec.addr.as_bytes())[..12])))) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
            Err(e) => Err(e.to_string()),
        }
    }

    fn write(&self, all: &[Record]) -> Result<(), String> {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let tmp = self.file().with_extension("new");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(serde_json::to_string_pretty(&all.iter().map(Record::to_json).collect::<Vec<_>>()).map_err(|e| e.to_string())?.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, self.file()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-instances-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn rec(name: &str, addr: &str) -> Record {
        Record { name: name.into(), addr: addr.into(), device: Some("dabc".into()), public: "00".repeat(32) }
    }

    #[test]
    fn each_instance_has_its_own_key_file_with_private_modes_and_forgetting_removes_both() {
        let dir = temp("keys");
        let s = Instances::new(&dir);
        let (k1, k2) = (DeviceKey::generate().unwrap(), DeviceKey::generate().unwrap());
        s.save(&rec("home", "mac.local:8443"), &k1).unwrap();
        s.save(&rec("work", "work.example:443"), &k2).unwrap();
        assert_eq!(s.key("mac.local:8443").unwrap().unwrap().public_hex(), k1.public_hex());
        assert_eq!(s.key("work.example:443").unwrap().unwrap().public_hex(), k2.public_hex());
        assert_eq!(s.key("other:443").unwrap().is_none(), true);
        for p in [dir.join("instances.json")].into_iter().chain(std::fs::read_dir(dir.join("device-keys")).unwrap().map(|e| e.unwrap().path())) {
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o077, 0, "{}", p.display());
        }
        assert_eq!(std::fs::metadata(dir.join("device-keys")).unwrap().permissions().mode() & 0o077, 0);
        // found by name in any case, or by address
        assert_eq!(s.find("HOME").unwrap().unwrap().addr, "mac.local:8443");
        assert_eq!(s.find("work.example").unwrap().unwrap().name, "work");
        assert!(s.forget("home").unwrap());
        assert!(!s.forget("home").unwrap());
        assert!(s.key("mac.local:8443").unwrap().is_none());
        assert_eq!(s.list().unwrap().len(), 1);
        assert_eq!(std::fs::read_dir(dir.join("device-keys")).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_cannot_belong_to_two_instances_and_saving_again_replaces() {
        let dir = temp("names");
        let s = Instances::new(&dir);
        let k = DeviceKey::generate().unwrap();
        s.save(&rec("home", "a:443"), &k).unwrap();
        assert!(s.save(&rec("Home", "b:443"), &k).is_err());
        let k2 = DeviceKey::generate().unwrap();
        s.save(&rec("home", "a:443"), &k2).unwrap();
        assert_eq!(s.list().unwrap().len(), 1);
        assert_eq!(s.key("a:443").unwrap().unwrap().public_hex(), k2.public_hex());
        assert!(clean_name("a/b").is_err() && clean_name("").is_err() && clean_name(&"x".repeat(61)).is_err());
        assert_eq!(default_name("[::1]:8443"), "::1");
        assert_eq!(default_name("box.local:443"), "box.local");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
