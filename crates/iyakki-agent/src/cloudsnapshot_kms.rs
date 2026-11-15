//! Envelope mode for the snapshot keyring (F-CLOUD-15): the snapshot keys are not kept in the clear in the tenant's vault; each is wrapped
//! by a key that never leaves a key-management service (a KMS), and only the wrapped form is stored.
//!
//! * [`Kms`] is what a KMS must do: wrap and unwrap a small secret under a named key, with a context that is bound to the result (so a wrap
//!   made for one snapshot key cannot be passed off as another). A real service (a cloud KMS, an HSM, a vault transit engine) is a
//!   caller-side implementation of this trait; none is written here. [`LocalKms`] is the one that needs no service: a master key held by the
//!   caller (a file the operator keeps off the machine, for example).
//! * [`Blobs`] is where wrapped keys are kept (a directory, [`DirBlobs`], or the vault).
//! * [`KmsKeyring`] is a [`KeySource`], so the snapshot code and the vault CLI use it exactly like the file and vault keyrings.
//! * [`migrate`] copies every key of one keyring into another (plain vault to envelope, envelope to plain, one KMS key to another) and
//!   checks each copy by its key id before it reports success; nothing is deleted from the source.
//! * [`ProviderSetting`] is the value of the launch-configuration setting (`file`, `vault`, `kms:<key reference>`) with its checks.

use std::path::PathBuf;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use crate::cloudkeys::{hex, random, unhex};
use crate::cloudsnapshot::{key_id_ok, KeySource, SnapKey};

/// A key-management service.
pub trait Kms {
    /// Wraps `secret` under the key named `key_ref`; `context` is bound to the result.
    fn wrap(&self, key_ref: &str, secret: &[u8], context: &str) -> Result<Vec<u8>, String>;
    /// Opens what `wrap` made, for the same key reference and context only.
    fn unwrap(&self, key_ref: &str, wrapped: &[u8], context: &str) -> Result<Vec<u8>, String>;
}

/// A KMS that is a master key held by the caller. One master key serves one key reference name (the name is bound into every wrap).
pub struct LocalKms {
    master: Zeroizing<[u8; 32]>,
}

impl LocalKms {
    pub fn new(master: [u8; 32]) -> LocalKms {
        LocalKms { master: Zeroizing::new(master) }
    }

    pub fn from_hex(h: &str) -> Result<LocalKms, String> {
        let b = unhex(h.trim()).and_then(|b| <[u8; 32]>::try_from(b).ok()).ok_or("a master key is 64 hex digits")?;
        Ok(LocalKms::new(b))
    }

    fn aad(key_ref: &str, context: &str) -> Vec<u8> {
        format!("iyakki-snapshot-kms-v1\n{key_ref}\n{context}").into_bytes()
    }
}

impl Kms for LocalKms {
    fn wrap(&self, key_ref: &str, secret: &[u8], context: &str) -> Result<Vec<u8>, String> {
        let nonce = random::<24>()?;
        let c = XChaCha20Poly1305::new(Key::from_slice(&*self.master));
        let ct = c.encrypt(XNonce::from_slice(&nonce), Payload { msg: secret, aad: &Self::aad(key_ref, context) }).map_err(|_| "the key service could not wrap the key".to_string())?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        Ok(out)
    }

    fn unwrap(&self, key_ref: &str, wrapped: &[u8], context: &str) -> Result<Vec<u8>, String> {
        if wrapped.len() < 24 + 16 {
            return Err("the wrapped key is damaged".into());
        }
        let c = XChaCha20Poly1305::new(Key::from_slice(&*self.master));
        c.decrypt(XNonce::from_slice(&wrapped[..24]), Payload { msg: &wrapped[24..], aad: &Self::aad(key_ref, context) }).map_err(|_| "the key service does not open this key (wrong master key or key reference, or the stored key was changed)".to_string())
    }
}

/// A place for named byte strings.
pub trait Blobs {
    fn get(&self, name: &str) -> Result<Vec<u8>, String>;
    fn put(&self, name: &str, data: &[u8]) -> Result<(), String>;
    fn names(&self) -> Result<Vec<String>, String>;
}

/// Blobs as files of one folder (names are limited to letters, digits, dot, dash and underscore).
pub struct DirBlobs(pub PathBuf);

fn blob_name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 80 && !n.starts_with('.') && n.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
}

impl Blobs for DirBlobs {
    fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        if !blob_name_ok(name) {
            return Err("not a valid name".into());
        }
        std::fs::read(self.0.join(name)).map_err(|_| format!("{name} is not there"))
    }

    fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        if !blob_name_ok(name) {
            return Err("not a valid name".into());
        }
        std::fs::create_dir_all(&self.0).map_err(|e| e.to_string())?;
        iyakki_schema::write_atomic(&self.0.join(name), data, 0o600).map_err(|e| e.to_string())
    }

    fn names(&self) -> Result<Vec<String>, String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.0).map_err(|e| e.to_string())?.filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| blob_name_ok(n)).collect();
        v.sort();
        Ok(v)
    }
}

const PREFIX: &str = "snapshot-key.";
const SUFFIX: &str = ".wrapped";
const CURRENT: &str = "snapshot-key.current";

/// The snapshot keyring in envelope mode.
pub struct KmsKeyring<'a> {
    pub kms: &'a dyn Kms,
    pub blobs: &'a dyn Blobs,
    /// The KMS key that wraps the snapshot keys.
    pub key_ref: &'a str,
}

fn context(id: &str) -> String {
    format!("snapshot-key:{id}")
}

impl KmsKeyring<'_> {
    /// Stores `k` wrapped. When `current`, new snapshots use it from now on.
    pub fn store(&self, k: &SnapKey, current: bool) -> Result<(), String> {
        let raw = Zeroizing::new(unhex(&k.to_hex()).ok_or("the key is damaged")?);
        let w = self.kms.wrap(self.key_ref, &raw, &context(k.id()))?;
        self.blobs.put(&format!("{PREFIX}{}{SUFFIX}", k.id()), &w)?;
        if current {
            self.blobs.put(CURRENT, k.id().as_bytes())?;
        }
        Ok(())
    }

    /// Makes a new key, stores it wrapped and makes it current. Old keys stay.
    pub fn rotate(&self) -> Result<SnapKey, String> {
        let k = SnapKey::generate()?;
        self.store(&k, true)?;
        Ok(k)
    }

    /// The ids of the keys held, in name order.
    pub fn ids(&self) -> Result<Vec<String>, String> {
        Ok(self.blobs.names()?.into_iter().filter_map(|n| n.strip_prefix(PREFIX)?.strip_suffix(SUFFIX).map(String::from)).filter(|i| key_id_ok(i)).collect())
    }
}

impl KeySource for KmsKeyring<'_> {
    fn current(&self) -> Result<SnapKey, String> {
        let id = self.blobs.get(CURRENT).map_err(|e| format!("no current snapshot key ({e}); make one with: iyakki cloud snapshot key-rotate"))?;
        self.get(std::str::from_utf8(&id).map_err(|_| "the current key's id is damaged")?)
    }

    fn get(&self, id: &str) -> Result<SnapKey, String> {
        if !key_id_ok(id) {
            return Err("that is not a key id".into());
        }
        let w = self.blobs.get(&format!("{PREFIX}{id}{SUFFIX}")).map_err(|e| format!("key {id} is not stored ({e})"))?;
        let raw = Zeroizing::new(self.kms.unwrap(self.key_ref, &w, &context(id))?);
        let k = SnapKey::from_bytes(&raw)?;
        if k.id() != id {
            return Err(format!("the stored key {id} does not match its id"));
        }
        Ok(k)
    }
}

/// What a migration did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migrated {
    pub copied: Vec<String>,
    pub current: Option<String>,
}

/// Copies the keys named by `ids` from `from` into `to`, making `current_id` current in `to`. Each copy is read back from `to` and
/// compared by key id before the next one; the first failure stops the run and says which key (keys already copied stay, and running the
/// migration again is safe). Nothing is removed from `from`.
pub fn migrate(from: &dyn KeySource, ids: &[String], current_id: Option<&str>, to: &KmsKeyring) -> Result<Migrated, String> {
    let mut copied = Vec::new();
    for id in ids {
        let k = from.get(id).map_err(|e| format!("key {id} could not be read from the old keyring: {e}"))?;
        to.store(&k, current_id == Some(id.as_str())).map_err(|e| format!("key {id} could not be stored: {e}"))?;
        let back = to.get(id).map_err(|e| format!("key {id} did not read back from the new keyring: {e}"))?;
        if back.id() != k.id() || back.to_hex() != k.to_hex() {
            return Err(format!("key {id} came back different from the new keyring"));
        }
        copied.push(id.clone());
    }
    if let Some(c) = current_id {
        if !ids.iter().any(|i| i == c) {
            return Err("the current key is not among the keys to copy".into());
        }
    }
    Ok(Migrated { copied, current: current_id.map(String::from) })
}

/// The launch-configuration setting that says where the snapshot keys live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderSetting {
    /// Key files given on the command line.
    File,
    /// The tenant's vault, keys in the clear inside the vault.
    Vault,
    /// Envelope mode: the keys are wrapped by this KMS key reference.
    Kms(String),
}

impl ProviderSetting {
    pub fn parse(s: &str) -> Result<ProviderSetting, String> {
        let s = s.trim();
        match s {
            "file" => Ok(ProviderSetting::File),
            "vault" => Ok(ProviderSetting::Vault),
            _ => {
                let r = s.strip_prefix("kms:").ok_or("use file, vault or kms:<key reference>")?;
                if r.is_empty() || r.len() > 200 || !r.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'/' | b':' | b'.' | b'-' | b'_' | b'@')) || r.contains("..") {
                    return Err("the key reference may hold letters, digits and / : . - _ @ only (200 at most)".into());
                }
                Ok(ProviderSetting::Kms(r.to_string()))
            }
        }
    }

    pub fn text(&self) -> String {
        match self {
            ProviderSetting::File => "file".into(),
            ProviderSetting::Vault => "vault".into(),
            ProviderSetting::Kms(r) => format!("kms:{r}"),
        }
    }
}

/// The key id shown to a person (never the key): the first and last four digits.
pub fn short_id(id: &str) -> String {
    if id.len() >= 8 {
        format!("{}..{}", &id[..4], &id[id.len() - 4..])
    } else {
        hex(id.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloudsnapshot::KeyRing;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Mem(RefCell<BTreeMap<String, Vec<u8>>>);

    impl Blobs for Mem {
        fn get(&self, n: &str) -> Result<Vec<u8>, String> {
            self.0.borrow().get(n).cloned().ok_or_else(|| format!("{n} is not there"))
        }
        fn put(&self, n: &str, d: &[u8]) -> Result<(), String> {
            self.0.borrow_mut().insert(n.into(), d.to_vec());
            Ok(())
        }
        fn names(&self) -> Result<Vec<String>, String> {
            Ok(self.0.borrow().keys().cloned().collect())
        }
    }

    fn kms(b: u8) -> LocalKms {
        LocalKms::new([b; 32])
    }

    #[test]
    fn a_key_is_stored_only_wrapped_and_comes_back_by_id_and_as_current() {
        let (m, blobs) = (kms(1), Mem::default());
        let ring = KmsKeyring { kms: &m, blobs: &blobs, key_ref: "alias/snap" };
        let k = ring.rotate().unwrap();
        assert_eq!(ring.current().unwrap().to_hex(), k.to_hex());
        assert_eq!(ring.get(k.id()).unwrap().id(), k.id());
        assert_eq!(ring.ids().unwrap(), vec![k.id().to_string()]);
        let stored = blobs.0.borrow().get(&format!("snapshot-key.{}.wrapped", k.id())).unwrap().clone();
        let raw = unhex(&k.to_hex()).unwrap();
        assert!(!stored.windows(32).any(|w| w == raw.as_slice()) && stored.len() == 24 + 32 + 16, "the clear key is not in what is stored");
        let k2 = ring.rotate().unwrap();
        assert_eq!(ring.current().unwrap().id(), k2.id());
        assert_eq!(ring.get(k.id()).unwrap().id(), k.id(), "old keys still open old snapshots");
        assert_eq!(ring.ids().unwrap().len(), 2);
        assert!(ring.get("not-an-id").is_err() && ring.get("0123456789abcdef").is_err());
    }

    #[test]
    fn a_wrap_opens_only_under_its_master_key_reference_and_key_id() {
        let (m, blobs) = (kms(1), Mem::default());
        let ring = KmsKeyring { kms: &m, blobs: &blobs, key_ref: "alias/snap" };
        let k = ring.rotate().unwrap();
        let other = kms(2);
        assert!(KmsKeyring { kms: &other, blobs: &blobs, key_ref: "alias/snap" }.get(k.id()).unwrap_err().contains("does not open"));
        assert!(KmsKeyring { kms: &m, blobs: &blobs, key_ref: "alias/other" }.get(k.id()).is_err());
        // a wrap made for one key id does not pass as another
        let k2 = ring.rotate().unwrap();
        let w1 = blobs.get(&format!("snapshot-key.{}.wrapped", k.id())).unwrap();
        blobs.put(&format!("snapshot-key.{}.wrapped", k2.id()), &w1).unwrap();
        assert!(ring.get(k2.id()).is_err());
        // a changed or short blob is refused
        let mut bad = w1.clone();
        bad[30] ^= 1;
        blobs.put(&format!("snapshot-key.{}.wrapped", k.id()), &bad).unwrap();
        assert!(ring.get(k.id()).is_err());
        blobs.put(&format!("snapshot-key.{}.wrapped", k.id()), &w1[..20]).unwrap();
        assert!(ring.get(k.id()).unwrap_err().contains("damaged"));
        assert!(LocalKms::from_hex("zz").is_err() && LocalKms::from_hex(&"ab".repeat(32)).is_ok());
    }

    #[test]
    fn the_keys_of_a_plain_keyring_migrate_to_envelope_mode_and_between_kms_keys_and_back() {
        let a = SnapKey::generate().unwrap();
        let b = SnapKey::generate().unwrap();
        let (ida, idb) = (a.id().to_string(), b.id().to_string());
        let plain = KeyRing(vec![SnapKey::from_hex(&a.to_hex()).unwrap(), SnapKey::from_hex(&b.to_hex()).unwrap()]);
        let (m1, b1) = (kms(1), Mem::default());
        let env1 = KmsKeyring { kms: &m1, blobs: &b1, key_ref: "k1" };
        let done = migrate(&plain, &[ida.clone(), idb.clone()], Some(&ida), &env1).unwrap();
        assert_eq!((done.copied.len(), done.current.as_deref()), (2, Some(ida.as_str())));
        assert_eq!(env1.current().unwrap().to_hex(), a.to_hex());
        assert_eq!(env1.get(&idb).unwrap().to_hex(), b.to_hex());
        // from one KMS key to another (a new master key and reference), then the old one can go
        let (m2, b2) = (kms(9), Mem::default());
        let env2 = KmsKeyring { kms: &m2, blobs: &b2, key_ref: "k2" };
        let ids = env1.ids().unwrap();
        migrate(&env1, &ids, Some(&idb), &env2).unwrap();
        assert_eq!(env2.current().unwrap().id(), idb);
        assert!(env1.get(&ida).is_ok(), "nothing is removed from the source");
        // a key the source does not have stops the run with the key's id; keys already copied stay, and a second run is safe
        let missing = "0123456789abcdef".to_string();
        let (m3, b3) = (kms(3), Mem::default());
        let env3 = KmsKeyring { kms: &m3, blobs: &b3, key_ref: "k3" };
        let e = migrate(&plain, &[ida.clone(), missing.clone()], Some(&ida), &env3).unwrap_err();
        assert!(e.contains(&missing), "{e}");
        assert!(env3.get(&ida).is_ok());
        assert!(migrate(&plain, &[ida.clone(), idb.clone()], Some(&idb), &env3).is_ok());
        assert!(migrate(&plain, std::slice::from_ref(&ida), Some(&idb), &env3).unwrap_err().contains("not among"));
    }

    #[test]
    fn the_keyring_lives_in_a_folder_and_a_snapshot_key_round_trips_through_it() {
        let dir = std::env::temp_dir().join(format!("iyk-kms-{}-{}", std::process::id(), hex(&random::<4>().unwrap()))); // home-ok: a unique folder under the OS temp dir, removed below
        let (m, blobs) = (kms(5), DirBlobs(dir.clone()));
        let ring = KmsKeyring { kms: &m, blobs: &blobs, key_ref: "kms/alias" };
        let k = ring.rotate().unwrap();
        assert_eq!(ring.current().unwrap().to_hex(), k.to_hex());
        assert!(blobs.get("../x").is_err() && blobs.put(".hidden", b"x").is_err());
        assert_eq!(ring.ids().unwrap(), vec![k.id().to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_setting_names_file_vault_or_a_checked_kms_reference() {
        assert_eq!(ProviderSetting::parse("file"), Ok(ProviderSetting::File));
        assert_eq!(ProviderSetting::parse(" vault "), Ok(ProviderSetting::Vault));
        let k = ProviderSetting::parse("kms:arn:aws:kms:eu-west-1:1:key/abc-1").unwrap();
        assert_eq!(k.text(), "kms:arn:aws:kms:eu-west-1:1:key/abc-1");
        for bad in ["", "kms:", "kms:a b", "kms:../x", "s3", "kms:x\ny", &format!("kms:{}", "a".repeat(201))] {
            assert!(ProviderSetting::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(short_id("0123456789abcdef"), "0123..cdef");
    }
}
