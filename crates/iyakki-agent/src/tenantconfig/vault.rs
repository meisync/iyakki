//! The tenant's common vault, as the configuration sees it. Secrets go in here and nowhere else; the config holds references.
//! The real binding is [`for_user`]: the vault service named by the pod's environment (`cloudvaultlink`) with the signed-in member's key
//! (`cloudrotate::actor_keys`), through [`LinkVault`]. With no link or no key for that person the vault reads as locked ([`NoVault`]) and
//! every item that needs a secret stays off. [`bind`] still wires a fixed store (tests, a deployment of its own).

use super::schema::SecretRef;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretState {
    Set,
    Missing,
    Locked,
}

pub trait SecretStore: Send + Sync {
    /// Whether the secret exists, without reading it.
    fn state(&self, r: &SecretRef) -> SecretState;
    /// Write-only: set or replace. No call returns the value to a caller outside an enforcement point.
    fn put(&self, r: &SecretRef, value: &str) -> Result<(), String>;
    fn remove(&self, r: &SecretRef) -> Result<(), String>;
    /// "Use a secret for a named action": `f` gets the value for the duration of the call and returns what the action produced.
    /// `action` is recorded by the vault (the audit line names the action and the reference, never the value).
    fn use_secret(&self, r: &SecretRef, action: &str, f: &mut dyn FnMut(&str) -> Result<String, String>) -> Result<String, String>;
}

/// No vault bound: locked.
pub struct NoVault;

impl SecretStore for NoVault {
    fn state(&self, _: &SecretRef) -> SecretState {
        SecretState::Locked
    }
    fn put(&self, _: &SecretRef, _: &str) -> Result<(), String> {
        Err("the tenant vault is locked".into())
    }
    fn remove(&self, _: &SecretRef) -> Result<(), String> {
        Err("the tenant vault is locked".into())
    }
    fn use_secret(&self, _: &SecretRef, _: &str, _: &mut dyn FnMut(&str) -> Result<String, String>) -> Result<String, String> {
        Err("the tenant vault is locked".into())
    }
}

/// An in-memory vault for tests and for a deployment that wires its own.
#[derive(Default)]
pub struct MemVault {
    locked: std::sync::atomic::AtomicBool,
    map: Mutex<BTreeMap<String, String>>,
    /// (action, reference) of every use.
    pub uses: Mutex<Vec<(String, String)>>,
}

impl MemVault {
    pub fn new() -> Arc<MemVault> {
        Arc::new(MemVault::default())
    }
    pub fn set_locked(&self, l: bool) {
        self.locked.store(l, std::sync::atomic::Ordering::SeqCst);
    }
    fn is_locked(&self) -> bool {
        self.locked.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl SecretStore for MemVault {
    fn state(&self, r: &SecretRef) -> SecretState {
        if self.is_locked() {
            SecretState::Locked
        } else if self.map.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&r.text()) {
            SecretState::Set
        } else {
            SecretState::Missing
        }
    }
    fn put(&self, r: &SecretRef, value: &str) -> Result<(), String> {
        if self.is_locked() {
            return Err("the tenant vault is locked".into());
        }
        self.map.lock().unwrap_or_else(|e| e.into_inner()).insert(r.text(), value.into());
        Ok(())
    }
    fn remove(&self, r: &SecretRef) -> Result<(), String> {
        if self.is_locked() {
            return Err("the tenant vault is locked".into());
        }
        self.map.lock().unwrap_or_else(|e| e.into_inner()).remove(&r.text());
        Ok(())
    }
    fn use_secret(&self, r: &SecretRef, action: &str, f: &mut dyn FnMut(&str) -> Result<String, String>) -> Result<String, String> {
        if self.is_locked() {
            return Err("the tenant vault is locked".into());
        }
        let v = self.map.lock().unwrap_or_else(|e| e.into_inner()).get(&r.text()).cloned().ok_or("secret not set")?;
        self.uses.lock().unwrap_or_else(|e| e.into_inner()).push((action.into(), r.text()));
        f(&v)
    }
}

type Registry = Mutex<BTreeMap<PathBuf, Arc<dyn SecretStore>>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Bind the vault of the tenant whose volume is `data_dir` (the supervisor does this when the vault link is up).
pub fn bind(data_dir: &Path, v: Arc<dyn SecretStore>) {
    registry().lock().unwrap_or_else(|e| e.into_inner()).insert(data_dir.to_path_buf(), v);
}

pub fn unbind(data_dir: &Path) {
    registry().lock().unwrap_or_else(|e| e.into_inner()).remove(data_dir);
}

/// The tenant's vault: the bound one, else a locked one.
pub fn for_dir(data_dir: &Path) -> Arc<dyn SecretStore> {
    registry().lock().unwrap_or_else(|e| e.into_inner()).get(data_dir).cloned().unwrap_or_else(|| Arc::new(NoVault))
}

// ------------------------------------------------------------------------------------------------------------------ the real binding

/// What the tenant vault service offers a member, as far as the configuration needs it. Implemented by [`crate::cloudvaultd::Client`]
/// (the member's side: signs requests, wraps and seals on this side) and by fakes in tests.
pub trait VaultOps: Send + Sync {
    fn list(&self, s: &crate::cloudvaultd::Scope) -> Result<Vec<String>, String>;
    fn put(&self, s: &crate::cloudvaultd::Scope, name: &str, value: &[u8]) -> Result<(), String>;
    fn get(&self, s: &crate::cloudvaultd::Scope, name: &str) -> Result<Vec<u8>, String>;
    fn delete(&self, s: &crate::cloudvaultd::Scope, name: &str) -> Result<(), String>;
    fn create_scope(&self, s: &crate::cloudvaultd::Scope) -> Result<(), String>;
}

impl VaultOps for crate::cloudvaultd::Client {
    fn list(&self, s: &crate::cloudvaultd::Scope) -> Result<Vec<String>, String> {
        crate::cloudvaultd::Client::list(self, s)
    }
    fn put(&self, s: &crate::cloudvaultd::Scope, name: &str, value: &[u8]) -> Result<(), String> {
        crate::cloudvaultd::Client::put(self, s, name, value)
    }
    fn get(&self, s: &crate::cloudvaultd::Scope, name: &str) -> Result<Vec<u8>, String> {
        crate::cloudvaultd::Client::get(self, s, name)
    }
    fn delete(&self, s: &crate::cloudvaultd::Scope, name: &str) -> Result<(), String> {
        crate::cloudvaultd::Client::delete(self, s, name)
    }
    fn create_scope(&self, s: &crate::cloudvaultd::Scope) -> Result<(), String> {
        crate::cloudvaultd::Client::create_scope(self, s)
    }
}

/// Where a reference lives in the vault service: `vault:team/<team>/<name>` is secret `<name>` in the team's namespace scope; any other
/// reference is secret `<ns>.<name>` in the tenant scope.
pub fn scope_of(r: &SecretRef) -> Result<(crate::cloudvaultd::Scope, String), String> {
    use crate::cloudvaultd::Scope;
    if r.ns == "team" {
        let (team, name) = r.name.split_once('/').ok_or("not a team reference")?;
        return Ok((Scope::Namespace(team.into()), name.into()));
    }
    let name = format!("{}.{}", r.ns, r.name);
    if name.len() > 128 {
        return Err("the secret's name is too long for the vault".into());
    }
    Ok((Scope::Tenant, name))
}

/// The tenant vault as seen by ONE member (the person whose request is being served), over the vault service. It reads as unlocked
/// exactly when this member holds a key on this volume and the service answers for them; otherwise (no key, no link, no answer, no
/// right to the scope) `Locked`. Existence is read from the secret LIST (no value is opened); values are opened only inside
/// [`SecretStore::use_secret`]. A scope that does not exist yet is created by the first write (the member must be allowed to).
pub struct LinkVault<C: VaultOps> {
    pub ops: C,
}

impl<C: VaultOps> SecretStore for LinkVault<C> {
    fn state(&self, r: &SecretRef) -> SecretState {
        let Ok((scope, name)) = scope_of(r) else { return SecretState::Missing };
        match self.ops.list(&scope) {
            Ok(names) => {
                if names.iter().any(|n| *n == name) {
                    SecretState::Set
                } else {
                    SecretState::Missing
                }
            }
            Err(e) if e.starts_with("404") => SecretState::Missing,
            Err(_) => SecretState::Locked,
        }
    }
    fn put(&self, r: &SecretRef, value: &str) -> Result<(), String> {
        let (scope, name) = scope_of(r)?;
        match self.ops.put(&scope, &name, value.as_bytes()) {
            Ok(()) => Ok(()),
            Err(e) if e.starts_with("404") => {
                self.ops.create_scope(&scope).map_err(|_| "the tenant vault is locked".to_string())?;
                self.ops.put(&scope, &name, value.as_bytes()).map_err(|_| "the tenant vault is locked".to_string())
            }
            Err(_) => Err("the tenant vault is locked".into()),
        }
    }
    fn remove(&self, r: &SecretRef) -> Result<(), String> {
        let (scope, name) = scope_of(r)?;
        self.ops.delete(&scope, &name).map_err(|_| "the tenant vault is locked or the secret is not there".to_string())
    }
    fn use_secret(&self, r: &SecretRef, _action: &str, f: &mut dyn FnMut(&str) -> Result<String, String>) -> Result<String, String> {
        let (scope, name) = scope_of(r)?;
        let bytes = self.ops.get(&scope, &name).map_err(|e| if e.starts_with("404") { "secret not set".to_string() } else { "the tenant vault is locked".to_string() })?;
        let v = String::from_utf8(bytes).map_err(|_| "the secret is not text".to_string())?;
        f(&v)
    }
}

/// The vault for the person `user` of the tenant whose volume is `data_dir`: the bound one (tests, a deployment that wires its own); else
/// the vault service named by the pod's environment (`IYAKKI_VAULT_URL`, `IYAKKI_TENANT`) with `user`'s key from
/// `<data_dir>/cloud/vault-keys/<user>.key`; else a locked vault. Where a member's key does not exist yet (nobody has enrolled the
/// person), the answer is "locked": the gap is the key enrolment, not this binding.
pub fn for_user(data_dir: &Path, user: &str) -> Arc<dyn SecretStore> {
    for_user_with(data_dir, user, &crate::cloudvaultlink::Link::from_env())
}

pub fn for_user_with(data_dir: &Path, user: &str, link: &crate::cloudvaultlink::Link) -> Arc<dyn SecretStore> {
    if let Some(v) = registry().lock().unwrap_or_else(|e| e.into_inner()).get(data_dir).cloned() {
        return v;
    }
    let crate::cloudvaultlink::Link::Target(t) = link else { return Arc::new(NoVault) };
    let Some(keys) = crate::cloudrotate::actor_keys(data_dir, user) else { return Arc::new(NoVault) };
    match t.client(user, keys) {
        Ok(c) => Arc::new(LinkVault { ops: c }),
        Err(_) => Arc::new(NoVault),
    }
}
