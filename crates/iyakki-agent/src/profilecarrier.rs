//! Moving the sealed browser data between this computer and a cloud tenant (F-VAULT-11, docs/development/BROWSER-PROFILE-PROTECTION.md 4.8).
//!
//! The file that travels is the sealed profile exactly as it is on disk: the tenant holds bytes it cannot read, the key never leaves the
//! person's own vault, and the claim of `ProfileStore` (one session per profile) is taken for the length of the move and given back.
//! [`send`] copies the sealed file to the tenant (only while it is sealed here); [`fetch`] takes the tenant's copy in after it has been read end
//! to end with THIS person's key (a wrong key, a changed byte or a plain file is refused and nothing changes here). The carrier
//! ([`iyakki_agent_api::ProfileCarrier`]) is given by the shell from the signed-in tenant connection; this file never sees a password.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iyakki_agent_api::ProfileCarrier;

use crate::profilestore::{holder_id, secure_remove, ProfileKey, ProfileStore};

fn registry() -> &'static Mutex<BTreeMap<String, Arc<dyn ProfileCarrier>>> {
    static R: Mutex<BTreeMap<String, Arc<dyn ProfileCarrier>>> = Mutex::new(BTreeMap::new());
    &R
}

pub fn set(tenant: &str, c: Option<Arc<dyn ProfileCarrier>>) {
    let mut r = registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match c {
        Some(c) => {
            r.insert(tenant.to_string(), c);
        }
        None => {
            r.remove(tenant);
        }
    }
}

pub fn names() -> Vec<String> {
    registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner).keys().cloned().collect()
}

fn carrier(tenant: &str) -> Result<Arc<dyn ProfileCarrier>, String> {
    registry().lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(tenant).cloned().ok_or_else(|| format!("{tenant} is not signed in here: sign in to the tenant first"))
}

/// A private scratch file for the move (a folder of the person's own data, mode 0700; the file 0600).
fn scratch(data: &Path, user: &str, tag: &str) -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = data.join("profile-carry");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| e.to_string())?;
    let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    Ok(dir.join(format!("{safe}-{tag}-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst))))
}

/// Send the sealed data to `tenant`. The number of bytes sent.
pub fn send_with(data: &Path, user: &str, carrier: &dyn ProfileCarrier, now: u64) -> Result<u64, String> {
    let st = ProfileStore::new(data, user);
    let tmp = scratch(data, user, "out")?;
    let r = (|| {
        st.export_sealed(&tmp, &holder_id(data), now).map_err(|e| e.to_string())?;
        let bytes = std::fs::read(&tmp).map_err(|e| e.to_string())?;
        carrier.put(&bytes)?;
        Ok(bytes.len() as u64)
    })();
    secure_remove(&tmp);
    r
}

/// Take in the sealed data the tenant holds; nothing changes here unless it opens end to end with `key`.
pub fn fetch_with(data: &Path, user: &str, carrier: &dyn ProfileCarrier, key: &ProfileKey, now: u64) -> Result<(), String> {
    let bytes = carrier.get()?.ok_or("the tenant holds no browser data of yours: send it from the computer that has it first")?;
    let st = ProfileStore::new(data, user);
    let tmp = scratch(data, user, "in")?;
    let r = (|| {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut f, &bytes).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
        drop(f);
        st.import_sealed(&tmp, key, &holder_id(data), now).map_err(|e| e.to_string())
    })();
    secure_remove(&tmp);
    r
}

pub fn send(data: &Path, user: &str, tenant: &str, now: u64) -> Result<u64, String> {
    send_with(data, user, carrier(tenant)?.as_ref(), now)
}

pub fn fetch(data: &Path, user: &str, tenant: &str, key_hex: &str, now: u64) -> Result<(), String> {
    let key = ProfileKey::from_hex(key_hex).ok_or("the browser key is not valid")?;
    fetch_with(data, user, carrier(tenant)?.as_ref(), &key, now)
}

#[cfg(test)]
#[path = "profilecarrier_tests.rs"]
mod tests;
