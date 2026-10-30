//! Secret storage.
//!
//! **Local vault** — an encrypted file holding named secrets. Secrets are encrypted with a random
//! 256-bit data key (XChaCha20-Poly1305); that key is stored wrapped once per *key slot* (LUKS
//! style), each slot protected by a key derived from that slot's password with Argon2id. So each
//! user can unlock their own vault with their login password, several admins can share the system
//! vault, and changing one password never re-encrypts the data.
//!
//! **Secret references** — settings never hold secrets, only references:
//! `vault:NAME` (system vault), `uvault:NAME` (the signed-in user's vault), `env:NAME`,
//! `file:/abs/path`, and `ext:ALIAS/path#field` (an external provider: HashiCorp Vault / OpenBao,
//! 1Password Connect).
//!
//! Secret values are zeroized on drop and are never logged or shown by the UI after being saved.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_proxy::Upstream;
use argon2::Argon2;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VaultError {
    #[error("the vault is locked")]
    Locked,
    #[error("wrong password")]
    BadPassword,
    #[error("no such key slot")]
    NoSuchSlot,
    #[error("key slot already exists")]
    SlotExists,
    #[error("cannot remove the last key slot")]
    LastSlot,
    #[error("invalid secret name (lowercase letters, digits, . _ -; max 64)")]
    BadName,
    #[error("secret value too large (limit 64 KiB)")]
    TooLarge,
    #[error("vault is full (limit {0} secrets)")]
    Full(usize),
    #[error("no such secret: {0}")]
    NotFound(String),
    #[error("vault file is corrupt or from a newer version")]
    Corrupt,
    #[error("storage error: {0}")]
    Storage(String),
}

pub const MAX_SECRETS: usize = 500;
pub const MAX_SECRET_BYTES: usize = 64 * 1024;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn rand_bytes<const N: usize>() -> Result<[u8; N], VaultError> {
    let mut b = [0u8; N];
    iyakki_sec::random_fill(&mut b).map_err(|e| VaultError::Storage(format!("no entropy: {e}")))?;
    Ok(b)
}

pub fn valid_secret_name(n: &str) -> bool {
    let mut c = n.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase() || f.is_ascii_digit()) && n.len() <= 64 && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || matches!(x, '.' | '_' | '-'))
}

#[derive(Serialize, Deserialize, Clone)]
struct Slot {
    id: String,
    salt: String,
    wrapped: String, // nonce(24) || ciphertext(32 + 16 tag)
}

#[derive(Serialize, Deserialize)]
struct File {
    v: u32,
    slots: Vec<Slot>,
    nonce: String,
    data: String,
}

fn kek(password: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>, VaultError> {
    let mut k = Zeroizing::new([0u8; 32]);
    Argon2::default().hash_password_into(password.as_bytes(), salt, &mut *k).map_err(|e| VaultError::Storage(e.to_string()))?;
    Ok(k)
}

fn slot_aad(id: &str) -> Vec<u8> {
    format!("iyakki-vault-slot-v1:{id}").into_bytes()
}

const DATA_AAD: &[u8] = b"iyakki-vault-data-v1";

fn seal(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Result<([u8; 24], Vec<u8>), VaultError> {
    let nonce = rand_bytes::<24>()?;
    let ct = XChaCha20Poly1305::new(Key::from_slice(key)).encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad }).map_err(|_| VaultError::Storage("encryption failed".into()))?;
    Ok((nonce, ct))
}

fn open_box(key: &[u8; 32], aad: &[u8], nonce: &[u8], ct: &[u8]) -> Option<Vec<u8>> {
    if nonce.len() != 24 {
        return None;
    }
    XChaCha20Poly1305::new(Key::from_slice(key)).decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad }).ok()
}

pub struct LocalVault {
    path: PathBuf,
    slots: Vec<Slot>,
    dek: Option<Zeroizing<[u8; 32]>>,
    secrets: BTreeMap<String, Zeroizing<String>>,
}

impl LocalVault {
    /// Create a new vault with one key slot, unlocked.
    pub fn create(path: &Path, slot_id: &str, password: &str) -> Result<LocalVault, VaultError> {
        let mut v = LocalVault { path: path.into(), slots: vec![], dek: Some(Zeroizing::new(rand_bytes::<32>()?)), secrets: BTreeMap::new() };
        v.push_slot(slot_id, password)?;
        v.save()?;
        Ok(v)
    }

    /// Open an existing vault file, locked.
    pub fn open(path: &Path) -> Result<LocalVault, VaultError> {
        let text = std::fs::read_to_string(path).map_err(|e| VaultError::Storage(e.to_string()))?;
        let f: File = serde_json::from_str(&text).map_err(|_| VaultError::Corrupt)?;
        if f.v != 1 || f.slots.is_empty() {
            return Err(VaultError::Corrupt);
        }
        Ok(LocalVault { path: path.into(), slots: f.slots, dek: None, secrets: BTreeMap::new() })
    }

    pub fn exists(path: &Path) -> bool {
        path.is_file()
    }

    pub fn is_unlocked(&self) -> bool {
        self.dek.is_some()
    }

    pub fn slot_ids(&self) -> Vec<String> {
        self.slots.iter().map(|s| s.id.clone()).collect()
    }

    pub fn has_slot(&self, id: &str) -> bool {
        self.slots.iter().any(|s| s.id == id)
    }

    fn push_slot(&mut self, id: &str, password: &str) -> Result<(), VaultError> {
        if self.has_slot(id) {
            return Err(VaultError::SlotExists);
        }
        let dek = self.dek.as_ref().ok_or(VaultError::Locked)?;
        let salt = rand_bytes::<16>()?;
        let k = kek(password, &salt)?;
        let (nonce, ct) = seal(&k, &slot_aad(id), &dek[..])?;
        let mut wrapped = nonce.to_vec();
        wrapped.extend(ct);
        self.slots.push(Slot { id: id.into(), salt: hex(&salt), wrapped: hex(&wrapped) });
        Ok(())
    }

    /// Unlock with a slot's password. Wrong password and unknown slot are distinguishable only to
    /// callers that already know the slot exists.
    pub fn unlock(&mut self, slot_id: &str, password: &str) -> Result<(), VaultError> {
        let slot = self.slots.iter().find(|s| s.id == slot_id).ok_or(VaultError::NoSuchSlot)?.clone();
        let salt = unhex(&slot.salt).ok_or(VaultError::Corrupt)?;
        let wrapped = unhex(&slot.wrapped).ok_or(VaultError::Corrupt)?;
        if wrapped.len() < 24 {
            return Err(VaultError::Corrupt);
        }
        let k = kek(password, &salt)?;
        let dek_bytes = open_box(&k, &slot_aad(slot_id), &wrapped[..24], &wrapped[24..]).ok_or(VaultError::BadPassword)?;
        let mut dek = Zeroizing::new([0u8; 32]);
        if dek_bytes.len() != 32 {
            return Err(VaultError::Corrupt);
        }
        dek.copy_from_slice(&dek_bytes);
        // decrypt the data
        let text = std::fs::read_to_string(&self.path).map_err(|e| VaultError::Storage(e.to_string()))?;
        let f: File = serde_json::from_str(&text).map_err(|_| VaultError::Corrupt)?;
        let (nonce, data) = (unhex(&f.nonce).ok_or(VaultError::Corrupt)?, unhex(&f.data).ok_or(VaultError::Corrupt)?);
        let plain = Zeroizing::new(open_box(&dek, DATA_AAD, &nonce, &data).ok_or(VaultError::Corrupt)?);
        let map: BTreeMap<String, String> = serde_json::from_slice(&plain).map_err(|_| VaultError::Corrupt)?;
        self.secrets = map.into_iter().map(|(k, v)| (k, Zeroizing::new(v))).collect();
        self.dek = Some(dek);
        Ok(())
    }

    /// Forget the data key and all decrypted secrets.
    pub fn lock(&mut self) {
        self.dek = None;
        self.secrets.clear();
    }

    fn save(&self) -> Result<(), VaultError> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let dek = self.dek.as_ref().ok_or(VaultError::Locked)?;
        let plain_map: BTreeMap<&String, &str> = self.secrets.iter().map(|(k, v)| (k, v.as_str())).collect();
        let mut plain = Zeroizing::new(serde_json::to_vec(&plain_map).map_err(|e| VaultError::Storage(e.to_string()))?);
        let (nonce, ct) = seal(dek, DATA_AAD, &plain)?;
        plain.zeroize();
        let f = File { v: 1, slots: self.slots.clone(), nonce: hex(&nonce), data: hex(&ct) };
        let json = serde_json::to_vec(&f).map_err(|e| VaultError::Storage(e.to_string()))?;
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d).map_err(|e| VaultError::Storage(e.to_string()))?;
        }
        let tmp = self.path.with_extension("tmp");
        let io = |e: std::io::Error| VaultError::Storage(e.to_string());
        let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(io)?;
        file.write_all(&json).and_then(|_| file.sync_all()).map_err(io)?;
        std::fs::rename(&tmp, &self.path).map_err(io)
    }

    pub fn names(&self) -> Result<Vec<String>, VaultError> {
        if !self.is_unlocked() {
            return Err(VaultError::Locked);
        }
        Ok(self.secrets.keys().cloned().collect())
    }

    pub fn get(&self, name: &str) -> Result<Zeroizing<String>, VaultError> {
        if !self.is_unlocked() {
            return Err(VaultError::Locked);
        }
        self.secrets.get(name).cloned().ok_or_else(|| VaultError::NotFound(name.into()))
    }

    pub fn set(&mut self, name: &str, value: &str) -> Result<(), VaultError> {
        if !self.is_unlocked() {
            return Err(VaultError::Locked);
        }
        if !valid_secret_name(name) {
            return Err(VaultError::BadName);
        }
        if value.len() > MAX_SECRET_BYTES {
            return Err(VaultError::TooLarge);
        }
        if !self.secrets.contains_key(name) && self.secrets.len() >= MAX_SECRETS {
            return Err(VaultError::Full(MAX_SECRETS));
        }
        self.secrets.insert(name.into(), Zeroizing::new(value.into()));
        self.save()
    }

    pub fn delete(&mut self, name: &str) -> Result<(), VaultError> {
        if !self.is_unlocked() {
            return Err(VaultError::Locked);
        }
        self.secrets.remove(name).ok_or_else(|| VaultError::NotFound(name.into()))?;
        self.save()
    }

    /// Give another person/credential access (vault must be unlocked).
    pub fn add_slot(&mut self, id: &str, password: &str) -> Result<(), VaultError> {
        self.push_slot(id, password)?;
        self.save()
    }

    pub fn remove_slot(&mut self, id: &str) -> Result<(), VaultError> {
        if !self.is_unlocked() {
            return Err(VaultError::Locked);
        }
        if !self.has_slot(id) {
            return Err(VaultError::NoSuchSlot);
        }
        if self.slots.len() == 1 {
            return Err(VaultError::LastSlot);
        }
        self.slots.retain(|s| s.id != id);
        self.save()
    }

    /// Change one slot's password, proving knowledge of the old one.
    pub fn change_password(&mut self, id: &str, old: &str, new: &str) -> Result<(), VaultError> {
        let was_locked = !self.is_unlocked();
        self.unlock(id, old)?;
        self.slots.retain(|s| s.id != id);
        self.push_slot(id, new)?;
        self.save()?;
        if was_locked {
            self.lock();
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ external providers

#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum SecretError {
    #[error("vault is locked")]
    Locked,
    #[error("secret not found: {0}")]
    NotFound(String),
    #[error("bad secret reference: {0}")]
    BadRef(String),
    #[error("secret provider error: {0}")]
    Backend(String),
}

impl From<VaultError> for SecretError {
    fn from(e: VaultError) -> Self {
        match e {
            VaultError::Locked => SecretError::Locked,
            VaultError::NotFound(n) => SecretError::NotFound(n),
            other => SecretError::Backend(other.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ProviderConfig {
    /// HashiCorp Vault or OpenBao, KV version 2.
    Hashicorp { addr: String, #[serde(default = "default_mount")] mount: String, #[serde(default)] namespace: Option<String>, token_ref: String },
    /// 1Password Connect server. Paths are `<vault-id>/<item-id>`; field is a field label.
    Onepassword { addr: String, token_ref: String },
}

fn default_mount() -> String {
    "secret".into()
}

fn url_safe_segment(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn path_encode(p: &str) -> Result<String, SecretError> {
    let segs: Vec<&str> = p.split('/').collect();
    if segs.iter().any(|s| !url_safe_segment(s) || *s == "." || *s == "..") {
        return Err(SecretError::BadRef(format!("unsupported characters in path `{p}`")));
    }
    Ok(segs.join("/"))
}

fn field_of(v: &Value, field: Option<&str>) -> Result<String, SecretError> {
    let obj = v.as_object().ok_or_else(|| SecretError::Backend("unexpected response shape".into()))?;
    let text = |x: &Value| x.as_str().map(str::to_string).or_else(|| if x.is_null() { None } else { Some(x.to_string()) });
    match field {
        Some(f) => obj.get(f).and_then(text).ok_or_else(|| SecretError::NotFound(format!("field `{f}`"))),
        None if obj.len() == 1 => obj.values().next().and_then(text).ok_or_else(|| SecretError::NotFound("value".into())),
        None => Err(SecretError::BadRef("secret has several fields; add `#field` to the reference".into())),
    }
}

fn http_get_json(up: &dyn Upstream, url: &str, headers: &[(String, String)]) -> Result<Value, SecretError> {
    let r = up.request("GET", url, headers, b"").map_err(|e| SecretError::Backend(e))?;
    match r.status {
        200 => serde_json::from_slice(&r.body).map_err(|_| SecretError::Backend("provider returned non-JSON".into())),
        403 | 401 => Err(SecretError::Backend("provider rejected the credentials".into())),
        404 => Err(SecretError::NotFound(url.rsplit('/').next().unwrap_or("").into())),
        s => Err(SecretError::Backend(format!("provider returned HTTP {s}"))),
    }
}

fn fetch_external(cfg: &ProviderConfig, token: &str, path: &str, field: Option<&str>, up: &dyn Upstream) -> Result<String, SecretError> {
    match cfg {
        ProviderConfig::Hashicorp { addr, mount, namespace, .. } => {
            let url = format!("{}/v1/{}/data/{}", addr.trim_end_matches('/'), path_encode(mount)?, path_encode(path)?);
            let mut h = vec![("X-Vault-Token".to_string(), token.to_string())];
            if let Some(ns) = namespace {
                h.push(("X-Vault-Namespace".into(), ns.clone()));
            }
            let v = http_get_json(up, &url, &h)?;
            field_of(v.pointer("/data/data").ok_or_else(|| SecretError::Backend("not a KV v2 response (is the mount KV v2?)".into()))?, field)
        }
        ProviderConfig::Onepassword { addr, .. } => {
            let (vault, item) = path.split_once('/').ok_or_else(|| SecretError::BadRef("1Password path is `<vault-id>/<item-id>`".into()))?;
            let url = format!("{}/v1/vaults/{}/items/{}", addr.trim_end_matches('/'), path_encode(vault)?, path_encode(item)?);
            let v = http_get_json(up, &url, &[("Authorization".to_string(), format!("Bearer {token}"))])?;
            let fields = v.get("fields").and_then(Value::as_array).ok_or_else(|| SecretError::Backend("item has no fields".into()))?;
            let want = field.unwrap_or("password");
            fields
                .iter()
                .find(|f| f.get("label").and_then(Value::as_str).is_some_and(|l| l.eq_ignore_ascii_case(want)) || (field.is_none() && f.get("purpose").and_then(Value::as_str) == Some("PASSWORD")))
                .and_then(|f| f.get("value").and_then(Value::as_str))
                .map(str::to_string)
                .ok_or_else(|| SecretError::NotFound(format!("field `{want}`")))
        }
    }
}

// ------------------------------------------------------------------ reference resolution

/// Where references resolve from. Cheap to clone; share one per process.
#[derive(Clone)]
pub struct Secrets {
    pub system: Arc<Mutex<Option<LocalVault>>>,
    providers: Arc<Mutex<HashMap<String, ProviderConfig>>>,
    upstream: Arc<dyn Upstream>,
    cache: Arc<Mutex<HashMap<String, (Instant, Zeroizing<String>)>>>,
    pub cache_ttl: Duration,
}

impl Secrets {
    pub fn new(system: Option<LocalVault>, upstream: Arc<dyn Upstream>) -> Secrets {
        Secrets { system: Arc::new(Mutex::new(system)), providers: Default::default(), upstream, cache: Default::default(), cache_ttl: Duration::from_secs(60) }
    }

    pub fn set_providers(&self, p: HashMap<String, ProviderConfig>) {
        *self.providers.lock().unwrap() = p;
        self.cache.lock().unwrap().clear();
    }

    pub fn system_unlocked(&self) -> bool {
        self.system.lock().unwrap().as_ref().is_some_and(|v| v.is_unlocked())
    }

    /// Resolve a reference. `user_vault` is consulted for `uvault:` references.
    pub fn resolve(&self, reference: &str, user_vault: Option<&LocalVault>) -> Result<Zeroizing<String>, SecretError> {
        let (scheme, rest) = reference.split_once(':').ok_or_else(|| SecretError::BadRef("expected scheme:name".into()))?;
        match scheme {
            "vault" => {
                let g = self.system.lock().unwrap();
                Ok(g.as_ref().ok_or(SecretError::Locked)?.get(rest)?)
            }
            "uvault" => Ok(user_vault.ok_or(SecretError::Locked)?.get(rest)?),
            "env" => std::env::var(rest).map(Zeroizing::new).map_err(|_| SecretError::NotFound(format!("environment variable {rest}"))),
            "file" => {
                if !rest.starts_with('/') {
                    return Err(SecretError::BadRef("file: needs an absolute path".into()));
                }
                std::fs::read_to_string(rest).map(|s| Zeroizing::new(s.trim_end_matches(['\n', '\r']).to_string())).map_err(|e| SecretError::Backend(format!("{rest}: {e}")))
            }
            "ext" => self.resolve_ext(rest),
            other => Err(SecretError::BadRef(format!("unknown scheme `{other}`"))),
        }
    }

    fn resolve_ext(&self, rest: &str) -> Result<Zeroizing<String>, SecretError> {
        let (alias, tail) = rest.split_once('/').ok_or_else(|| SecretError::BadRef("expected ext:alias/path[#field]".into()))?;
        let (path, field) = match tail.split_once('#') {
            Some((p, f)) => (p, Some(f)),
            None => (tail, None),
        };
        let key = format!("{alias}/{tail}");
        if let Some((t, v)) = self.cache.lock().unwrap().get(&key) {
            if t.elapsed() < self.cache_ttl {
                return Ok(v.clone());
            }
        }
        let cfg = self.providers.lock().unwrap().get(alias).cloned().ok_or_else(|| SecretError::BadRef(format!("unknown provider `{alias}`")))?;
        let token_ref = match &cfg {
            ProviderConfig::Hashicorp { token_ref, .. } | ProviderConfig::Onepassword { token_ref, .. } => token_ref.clone(),
        };
        if token_ref.starts_with("ext:") {
            return Err(SecretError::BadRef("a provider's own token cannot come from another provider".into()));
        }
        let token = self.resolve(&token_ref, None)?;
        let v = Zeroizing::new(fetch_external(&cfg, &token, path, field, self.upstream.as_ref())?);
        self.cache.lock().unwrap().insert(key, (Instant::now(), v.clone()));
        Ok(v)
    }

    /// Check a provider is reachable and the token works, without reading a user secret.
    pub fn test_provider(&self, alias: &str) -> Result<(), SecretError> {
        let cfg = self.providers.lock().unwrap().get(alias).cloned().ok_or_else(|| SecretError::BadRef(format!("unknown provider `{alias}`")))?;
        let (addr, token_ref, url, header) = match &cfg {
            ProviderConfig::Hashicorp { addr, token_ref, namespace, .. } => (addr, token_ref, format!("{}/v1/auth/token/lookup-self", addr.trim_end_matches('/')), {
                let mut h = vec![];
                if let Some(ns) = namespace {
                    h.push(("X-Vault-Namespace".to_string(), ns.clone()));
                }
                h
            }),
            ProviderConfig::Onepassword { addr, token_ref } => (addr, token_ref, format!("{}/heartbeat", addr.trim_end_matches('/')), vec![]),
        };
        let _ = addr;
        let token = self.resolve(token_ref, None)?;
        let mut headers = header;
        match cfg {
            ProviderConfig::Hashicorp { .. } => headers.push(("X-Vault-Token".into(), token.to_string())),
            ProviderConfig::Onepassword { .. } => headers.push(("Authorization".into(), format!("Bearer {}", &*token))),
        }
        let r = self.upstream.request("GET", &url, &headers, b"").map_err(SecretError::Backend)?;
        match r.status {
            200 | 204 => Ok(()),
            401 | 403 => Err(SecretError::Backend("provider rejected the credentials".into())),
            s => Err(SecretError::Backend(format!("provider returned HTTP {s}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_proxy::UpstreamResponse;

    fn tmp(tag: &str) -> PathBuf {
        // one folder per call: tests run in parallel and several of them ask for the same tag
        crate::testdir::unique("iyakki-vault", tag)
    }

    #[test]
    fn create_store_lock_unlock_roundtrip() {
        let d = tmp("rt");
        let p = d.join("v.afv");
        let mut v = LocalVault::create(&p, "user:root", "correct horse").unwrap();
        v.set("openai-key", "sk-secret-123").unwrap();
        v.set("note", "multi\nline ✓").unwrap();
        assert_eq!(&*v.get("openai-key").unwrap(), "sk-secret-123");
        v.lock();
        assert_eq!(v.get("openai-key"), Err(VaultError::Locked));
        assert_eq!(v.names(), Err(VaultError::Locked));
        // reopen from disk
        let mut v2 = LocalVault::open(&p).unwrap();
        assert!(!v2.is_unlocked());
        assert_eq!(v2.unlock("user:root", "wrong"), Err(VaultError::BadPassword));
        assert_eq!(v2.unlock("user:nobody", "x"), Err(VaultError::NoSuchSlot));
        v2.unlock("user:root", "correct horse").unwrap();
        assert_eq!(v2.names().unwrap(), vec!["note", "openai-key"]);
        assert_eq!(&*v2.get("note").unwrap(), "multi\nline ✓");
        assert_eq!(v2.get("missing"), Err(VaultError::NotFound("missing".into())));
    }

    #[test]
    fn nothing_sensitive_is_in_the_file() {
        let d = tmp("plain");
        let p = d.join("v.afv");
        let mut v = LocalVault::create(&p, "user:root", "pass-phrase-1").unwrap();
        v.set("k", "TOP-SECRET-VALUE").unwrap();
        let raw = std::fs::read(&p).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(!text.contains("TOP-SECRET-VALUE") && !text.contains("pass-phrase-1"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        // each save uses a fresh nonce: same content, different ciphertext
        let before = std::fs::read_to_string(&p).unwrap();
        v.set("k", "TOP-SECRET-VALUE").unwrap();
        assert_ne!(before, std::fs::read_to_string(&p).unwrap());
    }

    #[test]
    fn tampering_and_corruption_are_detected() {
        let d = tmp("tamper");
        let p = d.join("v.afv");
        let mut v = LocalVault::create(&p, "user:root", "pass-phrase-1").unwrap();
        v.set("k", "v").unwrap();
        drop(v);
        let mut f: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        // flip a byte of the ciphertext
        let data = f["data"].as_str().unwrap().to_string();
        let flipped = format!("{}{}", if &data[..1] == "0" { "1" } else { "0" }, &data[1..]);
        f["data"] = Value::String(flipped);
        std::fs::write(&p, serde_json::to_vec(&f).unwrap()).unwrap();
        let mut v = LocalVault::open(&p).unwrap();
        assert_eq!(v.unlock("user:root", "pass-phrase-1"), Err(VaultError::Corrupt));
        std::fs::write(&p, b"not json").unwrap();
        assert!(matches!(LocalVault::open(&p), Err(VaultError::Corrupt)));
        std::fs::write(&p, br#"{"v":2,"slots":[],"nonce":"","data":""}"#).unwrap();
        assert!(matches!(LocalVault::open(&p), Err(VaultError::Corrupt)));
    }

    #[test]
    fn a_wrapped_key_is_bound_to_its_slot_name() {
        let d = tmp("rename");
        let p = d.join("v.afv");
        LocalVault::create(&p, "user:root", "pass-phrase-1").unwrap();
        // rename the slot in the file: same salt, same wrapped key, same password
        let text = std::fs::read_to_string(&p).unwrap().replace("user:root", "user:eve");
        std::fs::write(&p, text).unwrap();
        let mut v = LocalVault::open(&p).unwrap();
        assert_eq!(v.unlock("user:eve", "pass-phrase-1"), Err(VaultError::BadPassword), "authenticated slot name stops slot-swapping");
    }

    #[test]
    fn key_slots_share_one_vault() {
        let d = tmp("slots");
        let p = d.join("v.afv");
        let mut v = LocalVault::create(&p, "user:root", "root-pass-1").unwrap();
        v.set("shared", "s3cret").unwrap();
        v.add_slot("user:bob", "bob-pass-12").unwrap();
        assert_eq!(v.add_slot("user:bob", "x"), Err(VaultError::SlotExists));
        assert_eq!(v.slot_ids(), vec!["user:root", "user:bob"]);
        let mut b = LocalVault::open(&p).unwrap();
        b.unlock("user:bob", "bob-pass-12").unwrap();
        assert_eq!(&*b.get("shared").unwrap(), "s3cret");
        // bob's password cannot open root's slot
        let mut b2 = LocalVault::open(&p).unwrap();
        assert_eq!(b2.unlock("user:root", "bob-pass-12"), Err(VaultError::BadPassword));
        // revoke bob, but never the last slot
        v.remove_slot("user:bob").unwrap();
        assert_eq!(LocalVault::open(&p).unwrap().unlock("user:bob", "bob-pass-12"), Err(VaultError::NoSuchSlot));
        assert_eq!(v.remove_slot("user:root"), Err(VaultError::LastSlot));
    }

    #[test]
    fn password_change_keeps_data() {
        let d = tmp("chpw");
        let p = d.join("v.afv");
        let mut v = LocalVault::create(&p, "user:amy", "old-password-1").unwrap();
        v.set("k", "v").unwrap();
        v.lock();
        assert_eq!(v.change_password("user:amy", "wrong-old", "new-password-1"), Err(VaultError::BadPassword));
        v.change_password("user:amy", "old-password-1", "new-password-1").unwrap();
        assert!(!v.is_unlocked(), "stays locked if it was locked");
        let mut v2 = LocalVault::open(&p).unwrap();
        assert_eq!(v2.unlock("user:amy", "old-password-1"), Err(VaultError::BadPassword));
        v2.unlock("user:amy", "new-password-1").unwrap();
        assert_eq!(&*v2.get("k").unwrap(), "v");
    }

    #[test]
    fn limits_and_names() {
        let d = tmp("limits");
        let mut v = LocalVault::create(&d.join("v.afv"), "s", "pass-phrase-1").unwrap();
        assert_eq!(v.set("Bad Name", "x"), Err(VaultError::BadName));
        assert_eq!(v.set("../x", "x"), Err(VaultError::BadName));
        assert_eq!(v.set("ok", &"x".repeat(MAX_SECRET_BYTES + 1)), Err(VaultError::TooLarge));
        v.delete("nothing").unwrap_err();
        v.set("ok", "x").unwrap();
        v.delete("ok").unwrap();
        assert!(v.names().unwrap().is_empty());
    }

    // ---- references and external providers

    struct Mock {
        calls: Mutex<Vec<(String, Vec<(String, String)>)>>,
        reply: Mutex<Box<dyn Fn(&str) -> (u16, String) + Send>>,
    }
    impl Upstream for Mock {
        fn request(&self, _m: &str, url: &str, h: &[(String, String)], _b: &[u8]) -> Result<UpstreamResponse, String> {
            self.calls.lock().unwrap().push((url.into(), h.to_vec()));
            let (status, body) = (self.reply.lock().unwrap())(url);
            Ok(UpstreamResponse { status, body: body.into_bytes() })
        }
    }
    fn mock(f: impl Fn(&str) -> (u16, String) + Send + 'static) -> Arc<Mock> {
        Arc::new(Mock { calls: Default::default(), reply: Mutex::new(Box::new(f)) })
    }

    #[test]
    fn simple_reference_schemes() {
        let d = tmp("refs");
        let mut sys = LocalVault::create(&d.join("s.afv"), "user:root", "pass-phrase-1").unwrap();
        sys.set("openai", "sk-live").unwrap();
        let s = Secrets::new(Some(sys), mock(|_| (500, String::new())));
        assert_eq!(&*s.resolve("vault:openai", None).unwrap(), "sk-live");
        assert_eq!(s.resolve("vault:nope", None), Err(SecretError::NotFound("nope".into())));
        std::env::set_var("IYAKKI_TEST_SECRET_X", "from-env");
        assert_eq!(&*s.resolve("env:IYAKKI_TEST_SECRET_X", None).unwrap(), "from-env");
        assert!(s.resolve("env:IYAKKI_NO_SUCH_VAR", None).is_err());
        std::fs::write(d.join("k"), "from-file\n").unwrap();
        assert_eq!(&*s.resolve(&format!("file:{}", d.join("k").display()), None).unwrap(), "from-file");
        assert!(matches!(s.resolve("file:relative", None), Err(SecretError::BadRef(_))));
        assert!(matches!(s.resolve("nonsense", None), Err(SecretError::BadRef(_))));
        assert!(matches!(s.resolve("ftp:x", None), Err(SecretError::BadRef(_))));
        // user vault only when provided
        let mut uv = LocalVault::create(&d.join("u.afv"), "user:amy", "amy-pass-123").unwrap();
        uv.set("mine", "personal").unwrap();
        assert_eq!(&*s.resolve("uvault:mine", Some(&uv)).unwrap(), "personal");
        assert_eq!(s.resolve("uvault:mine", None), Err(SecretError::Locked));
        // locking the system vault makes vault: references fail closed
        s.system.lock().unwrap().as_mut().unwrap().lock();
        assert_eq!(s.resolve("vault:openai", None), Err(SecretError::Locked));
        assert!(!s.system_unlocked());
    }

    fn hashi_secrets(m: Arc<Mock>) -> Secrets {
        let d = tmp("hashi");
        let mut sys = LocalVault::create(&d.join("s.afv"), "user:root", "pass-phrase-1").unwrap();
        sys.set("hashi-token", "hvs.TOKEN").unwrap();
        let s = Secrets::new(Some(sys), m);
        s.set_providers(
            [("corp".to_string(), ProviderConfig::Hashicorp { addr: "https://vault.example:8200/".into(), mount: "secret".into(), namespace: Some("team-a".into()), token_ref: "vault:hashi-token".into() })].into(),
        );
        s
    }

    #[test]
    fn hashicorp_kv2_fetch_with_field_cache_and_headers() {
        let m = mock(|url| {
            assert_eq!(url, "https://vault.example:8200/v1/secret/data/iyakki/openai");
            (200, r#"{"data":{"data":{"api_key":"sk-from-vault","org":"acme"},"metadata":{"version":3}}}"#.into())
        });
        let s = hashi_secrets(m.clone());
        assert_eq!(&*s.resolve("ext:corp/iyakki/openai#api_key", None).unwrap(), "sk-from-vault");
        assert_eq!(&*s.resolve("ext:corp/iyakki/openai#api_key", None).unwrap(), "sk-from-vault");
        let calls = m.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "second lookup is served from the cache");
        assert!(calls[0].1.contains(&("X-Vault-Token".into(), "hvs.TOKEN".into())), "token came from the local vault");
        assert!(calls[0].1.contains(&("X-Vault-Namespace".into(), "team-a".into())));
        drop(calls);
        // several fields and no #field is ambiguous; a missing field is NotFound
        assert!(matches!(s.resolve("ext:corp/iyakki/openai", None), Err(SecretError::BadRef(_))));
        assert!(matches!(s.resolve("ext:corp/iyakki/openai#nope", None), Err(SecretError::NotFound(_))));
    }

    #[test]
    fn external_failures_are_clear_and_safe() {
        let s = hashi_secrets(mock(|_| (403, "permission denied".into())));
        assert!(s.resolve("ext:corp/a/b#c", None).unwrap_err().to_string().contains("rejected the credentials"));
        let s = hashi_secrets(mock(|_| (404, "{}".into())));
        assert!(matches!(s.resolve("ext:corp/a/b#c", None), Err(SecretError::NotFound(_))));
        let s = hashi_secrets(mock(|_| (200, "<html>".into())));
        assert!(s.resolve("ext:corp/a/b#c", None).is_err());
        // path traversal / URL injection in references is refused before any request
        let m = mock(|_| (200, "{}".into()));
        let s = hashi_secrets(m.clone());
        for bad in ["ext:corp/../sys/seal-status#x", "ext:corp/a b/c#x", "ext:corp/a?x=1/c#x", "ext:corp/a//c#x", "ext:nope/a/b#x", "ext:corp"] {
            assert!(s.resolve(bad, None).is_err(), "{bad}");
        }
        assert!(m.calls.lock().unwrap().is_empty(), "no request was made for malformed references");
        // a locked system vault means the provider token is unavailable: fail closed
        let s = hashi_secrets(mock(|_| (200, "{}".into())));
        s.system.lock().unwrap().as_mut().unwrap().lock();
        assert_eq!(s.resolve("ext:corp/a/b#c", None), Err(SecretError::Locked));
    }

    #[test]
    fn provider_token_cannot_chain_to_another_provider() {
        let s = Secrets::new(None, mock(|_| (200, "{}".into())));
        s.set_providers([("a".to_string(), ProviderConfig::Hashicorp { addr: "http://x".into(), mount: "secret".into(), namespace: None, token_ref: "ext:b/x/y".into() })].into());
        assert!(matches!(s.resolve("ext:a/p/q#f", None), Err(SecretError::BadRef(_))));
    }

    #[test]
    fn onepassword_connect_fetch() {
        let m = mock(|url| {
            assert_eq!(url, "https://op.example/v1/vaults/vault1/items/item1");
            (200, r#"{"fields":[{"label":"username","value":"bob"},{"label":"credential","purpose":"PASSWORD","value":"p@ss"}]}"#.into())
        });
        let d = tmp("op");
        let mut sys = LocalVault::create(&d.join("s.afv"), "user:root", "pass-phrase-1").unwrap();
        sys.set("op-token", "optoken").unwrap();
        let s = Secrets::new(Some(sys), m.clone());
        s.set_providers([("op".to_string(), ProviderConfig::Onepassword { addr: "https://op.example".into(), token_ref: "vault:op-token".into() })].into());
        assert_eq!(&*s.resolve("ext:op/vault1/item1#username", None).unwrap(), "bob");
        assert_eq!(&*s.resolve("ext:op/vault1/item1", None).unwrap(), "p@ss", "default is the PASSWORD field");
        assert!(m.calls.lock().unwrap()[0].1.contains(&("Authorization".into(), "Bearer optoken".into())));
        assert!(s.resolve("ext:op/justone", None).is_err());
    }

    #[test]
    fn provider_connection_test() {
        let s = hashi_secrets(mock(|url| if url.ends_with("/v1/auth/token/lookup-self") { (200, "{}".into()) } else { (500, String::new()) }));
        assert!(s.test_provider("corp").is_ok());
        assert!(s.test_provider("nope").is_err());
        let s = hashi_secrets(mock(|_| (403, String::new())));
        assert!(s.test_provider("corp").is_err());
    }

    /// Live check against a real OpenBao/Vault dev server (set IYAKKI_LIVE_BAO=http://127.0.0.1:8200,
    /// root token `root`).
    #[test]
    fn live_openbao_kv2() {
        let Ok(addr) = std::env::var("IYAKKI_LIVE_BAO") else { return };
        let up: Arc<dyn Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(10)));
        let h = vec![("X-Vault-Token".to_string(), "root".to_string()), ("Content-Type".to_string(), "application/json".to_string())];
        let r = up.request("POST", &format!("{addr}/v1/secret/data/iyakki/openai"), &h, br#"{"data":{"api_key":"sk-from-openbao","org":"acme"}}"#).unwrap();
        assert!((200..300).contains(&r.status), "{} {}", r.status, String::from_utf8_lossy(&r.body));
        let d = tmp("live");
        let mut sys = LocalVault::create(&d.join("s.afv"), "user:root", "pass-phrase-1").unwrap();
        sys.set("bao-token", "root").unwrap();
        let s = Secrets::new(Some(sys), up);
        s.set_providers([("bao".to_string(), ProviderConfig::Hashicorp { addr: addr.clone(), mount: "secret".into(), namespace: None, token_ref: "vault:bao-token".into() })].into());
        s.test_provider("bao").unwrap();
        assert_eq!(&*s.resolve("ext:bao/iyakki/openai#api_key", None).unwrap(), "sk-from-openbao");
        assert!(matches!(s.resolve("ext:bao/iyakki/missing#x", None), Err(SecretError::NotFound(_))));
        // wrong token is rejected with a clear message
        s.system.lock().unwrap().as_mut().unwrap().set("bao-token", "not-the-token").unwrap();
        s.set_providers([("bao".to_string(), ProviderConfig::Hashicorp { addr, mount: "secret".into(), namespace: None, token_ref: "vault:bao-token".into() })].into());
        assert!(s.resolve("ext:bao/iyakki/openai#api_key", None).unwrap_err().to_string().contains("rejected the credentials"));
        println!("live OpenBao: read, not-found and bad-token paths all behaved");
    }
}
