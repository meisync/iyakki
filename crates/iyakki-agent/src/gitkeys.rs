//! The person's git keys, tokens and host table (Settings → Git and SSH keys), and what git is allowed to use for a given address.
//!
//! * **Keys** are ed25519 OpenSSH keys kept in the person's vault under the same names the SSH client of the CLI tile uses
//!   (`ssh.<id>`, passphrase `ssh.<id>.passphrase`), so one key serves both. They are never files in a workspace or in a temporary
//!   folder: [`GitKeys::pair`] returns the key in memory for [`crate::gitagent`] to sign with. A key that is protected by a
//!   passphrase is opened with the remembered one (the vault), or with the one the person typed this session ([`Session`]).
//! * **HTTPS tokens** are vault entries `git.token.<host>` (user name in `git.user.<host>`), handed to git through the askpass helper.
//! * **The host table** (`git.json` next to the SSH folder, no secrets): which key (or token) each git host uses, public halves and
//!   fingerprints of the keys (so the list shows without unlocking), and a workspace may override a host's key in its project.json.
//! * **Known hosts** are the SSH client's per-person `known_hosts` (shared with the CLI tile). A host seen for the first time is
//!   shown with its `SHA256:` fingerprint and trusted only when the person confirms; a changed key is refused with an explanation.
//! * **Every use is audited** (host, key id; never key material), and an assistant's first use of a key in a session needs the
//!   person's approval ([`Session`]).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::gitsite::RepoUrl;
use crate::ssh::{HostKey, KnownHosts};
use crate::sshkey::{KeyError, KeyPair};

/// What the person's vault offers; the real one is [`ServicesStore`], tests use [`MemStore`].
pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Zeroizing<String>, String>;
    fn set(&self, name: &str, value: &str) -> Result<(), String>;
    fn delete(&self, name: &str) -> Result<(), String>;
    fn names(&self) -> Result<Vec<String>, String>;
}

/// The signed-in person's vault, through the services.
pub struct ServicesStore {
    svc: Arc<crate::services::Services>,
    user: String,
}

impl ServicesStore {
    pub fn new(svc: &Arc<crate::services::Services>, user: &str) -> ServicesStore {
        ServicesStore { svc: svc.clone(), user: user.into() }
    }
}

impl SecretStore for ServicesStore {
    fn get(&self, name: &str) -> Result<Zeroizing<String>, String> {
        self.svc.user_get(&self.user, name).map_err(|e| format!("could not read `{name}` from your vault: {e}"))
    }
    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.svc.user_set(&self.user, name, value).map_err(|e| format!("could not write `{name}` to your vault: {e}"))
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.svc.user_delete(&self.user, name).map_err(|e| format!("could not delete `{name}` from your vault: {e}"))
    }
    fn names(&self) -> Result<Vec<String>, String> {
        self.svc.user_names(&self.user).map_err(|e| format!("could not list your vault: {e}"))
    }
}

/// An in-memory vault (tests, and the shell's tests).
#[derive(Default)]
pub struct MemStore {
    pub map: Mutex<BTreeMap<String, String>>,
    pub locked: Mutex<bool>,
}

impl SecretStore for MemStore {
    fn get(&self, name: &str) -> Result<Zeroizing<String>, String> {
        if *self.locked.lock().unwrap() {
            return Err("your vault is locked".into());
        }
        self.map.lock().unwrap().get(name).map(|v| Zeroizing::new(v.clone())).ok_or_else(|| format!("`{name}` is not in your vault"))
    }
    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        if *self.locked.lock().unwrap() {
            return Err("your vault is locked".into());
        }
        if !crate::vault::valid_secret_name(name) {
            return Err(format!("`{name}` is not a valid vault name"));
        }
        self.map.lock().unwrap().insert(name.into(), value.into());
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.map.lock().unwrap().remove(name);
        Ok(())
    }
    fn names(&self) -> Result<Vec<String>, String> {
        Ok(self.map.lock().unwrap().keys().cloned().collect())
    }
}

// ------------------------------------------------------------------ names

pub fn valid_key_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && !id.ends_with(".passphrase") && crate::vault::valid_secret_name(&format!("ssh.{id}"))
}
pub fn vault_key(id: &str) -> String {
    format!("ssh.{id}")
}
pub fn vault_pass(id: &str) -> String {
    format!("ssh.{id}.passphrase")
}
/// `github.com` -> `github.com`, `gitea.lan:3000` -> `gitea.lan_3000`.
pub fn host_slug(host: &str) -> String {
    host.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-') { c } else { '_' }).collect()
}
pub fn vault_token(host: &str) -> String {
    format!("git.token.{}", host_slug(host))
}
pub fn vault_token_user(host: &str) -> String {
    format!("git.user.{}", host_slug(host))
}
pub fn valid_host_key(host: &str) -> bool {
    !host.is_empty() && host.len() <= 48 && !host.starts_with(['-', '.']) && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) && crate::vault::valid_secret_name(&vault_token_user(host))
}

// ------------------------------------------------------------------ the table (no secrets)

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyInfo {
    pub id: String,
    pub public: String,
    pub fingerprint: String,
    pub comment: String,
    pub passphrase: bool,
    /// The passphrase is kept in the vault (otherwise it is asked once per session).
    pub remembered: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostRule {
    /// The key id used for SSH addresses of this host (empty: none).
    pub key: String,
    /// A token is stored in the vault for HTTPS addresses of this host.
    pub token: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitTable {
    pub keys: BTreeMap<String, KeyInfo>,
    pub hosts: BTreeMap<String, HostRule>,
}

/// What an address will use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Auth {
    /// Nothing: a public repository over HTTPS, or an SSH address with no key chosen (git will say it cannot log in).
    None,
    Key { id: String, from_workspace: bool },
    Token { host: String },
}

impl Auth {
    /// The word written in project.json and the audit.
    pub fn label(&self) -> String {
        match self {
            Auth::None => String::new(),
            Auth::Key { id, .. } => id.clone(),
            Auth::Token { .. } => "token".into(),
        }
    }
}

impl GitTable {
    /// Which key or token an address uses. A workspace's own table wins over the person's for SSH addresses.
    pub fn resolve(&self, url: &RepoUrl, workspace: &BTreeMap<String, String>) -> Auth {
        let site = url.site();
        if url.is_ssh() {
            if let Some(id) = workspace.get(&site).or_else(|| workspace.get(&url.host)).filter(|id| !id.is_empty()) {
                return Auth::Key { id: id.clone(), from_workspace: true };
            }
            return match self.hosts.get(&site).or_else(|| self.hosts.get(&url.host)) {
                Some(r) if !r.key.is_empty() => Auth::Key { id: r.key.clone(), from_workspace: false },
                _ => Auth::None,
            };
        }
        match self.hosts.get(&site).or_else(|| self.hosts.get(&url.host)) {
            Some(r) if r.token => Auth::Token { host: if self.hosts.contains_key(&site) { site } else { url.host.clone() } },
            _ => Auth::None,
        }
    }
}

// ------------------------------------------------------------------ the session

/// Things kept for this session only, in memory: passphrases typed once, and which keys an assistant may use.
#[derive(Default)]
pub struct Session {
    pass: Mutex<HashMap<String, Zeroizing<String>>>,
    approved: Mutex<HashSet<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    Person,
    Assistant,
}

impl Session {
    pub fn remember_passphrase(&self, id: &str, pass: &str) {
        self.pass.lock().unwrap().insert(id.into(), Zeroizing::new(pass.into()));
    }
    pub fn passphrase(&self, id: &str) -> Option<Zeroizing<String>> {
        self.pass.lock().unwrap().get(id).cloned()
    }
    /// Whether an assistant must ask the person before using this key now. The person's own actions never ask again.
    pub fn needs_approval(&self, who: Who, key_id: &str) -> bool {
        who == Who::Assistant && !self.approved.lock().unwrap().contains(key_id)
    }
    pub fn approve(&self, key_id: &str) {
        self.approved.lock().unwrap().insert(key_id.into());
    }
    pub fn forget(&self) {
        self.pass.lock().unwrap().clear();
        self.approved.lock().unwrap().clear();
    }
}

/// The session of this run of the program: passphrases typed once and keys an assistant may use. It ends when the person signs out
/// (`Services::on_logout`) or the program stops; nothing in it is ever written to a disk.
pub fn shared_session() -> Arc<Session> {
    static S: std::sync::OnceLock<Arc<Session>> = std::sync::OnceLock::new();
    S.get_or_init(|| Arc::new(Session::default())).clone()
}

// ------------------------------------------------------------------ the store

pub type AuditFn = Arc<dyn Fn(&str, &str) + Send + Sync>;

pub struct GitKeys {
    /// `<services>/ssh/<user>`: `git.json` and `home/.ssh/known_hosts` live here.
    dir: PathBuf,
    store: Arc<dyn SecretStore>,
    audit: AuditFn,
    pub session: Arc<Session>,
}

impl GitKeys {
    pub fn new(dir: &Path, store: Arc<dyn SecretStore>, audit: AuditFn, session: Arc<Session>) -> GitKeys {
        GitKeys { dir: dir.into(), store, audit, session }
    }

    /// For the signed-in person: the same folder as the CLI tile's SSH settings.
    pub fn for_services(svc: &Arc<crate::services::Services>, user: &str, audit: AuditFn, session: Arc<Session>) -> GitKeys {
        let env = crate::ssh::Env::for_services(svc, user);
        GitKeys::new(&env.dir, Arc::new(ServicesStore::new(svc, user)), audit, session)
    }

    fn table_path(&self) -> PathBuf {
        self.dir.join("git.json")
    }

    pub fn known_hosts(&self) -> KnownHosts {
        KnownHosts::new(&self.dir.join("home").join(".ssh").join("known_hosts"))
    }
    pub fn known_hosts_path(&self) -> PathBuf {
        self.dir.join("home").join(".ssh").join("known_hosts")
    }

    pub fn table(&self) -> GitTable {
        std::fs::read(self.table_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self, t: &GitTable) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let tmp = self.table_path().with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(t).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, self.table_path()).map_err(|e| e.to_string())
    }

    pub fn audit(&self, action: &str, detail: &str) {
        (self.audit)(&format!("git.{action}"), detail);
    }

    /// The key ids in the vault (including keys the CLI tile made), with what is known about them.
    pub fn keys(&self) -> Vec<KeyInfo> {
        let mut t = self.table();
        let mut out: Vec<KeyInfo> = t.keys.values().cloned().collect();
        // keys that are in the vault but not in the table yet (made by the CLI tile or another window): read once
        if let Ok(names) = self.store.names() {
            let mut changed = false;
            for n in names {
                let Some(id) = n.strip_prefix("ssh.") else { continue };
                if id.ends_with(".passphrase") || t.keys.contains_key(id) || !valid_key_id(id) {
                    continue;
                }
                if let Ok(text) = self.store.get(&n) {
                    if let Ok(public) = crate::sshkey::public_line_of(&text) {
                        let blob = crate::sshkey::unb64(public.split(' ').nth(1).unwrap_or("")).unwrap_or_default();
                        let info = KeyInfo { id: id.into(), public, fingerprint: crate::sshkey::fingerprint_of_blob(&blob), comment: String::new(), passphrase: crate::sshkey::is_encrypted(&text), remembered: self.store.get(&vault_pass(id)).is_ok() };
                        t.keys.insert(id.into(), info.clone());
                        out.push(info);
                        changed = true;
                    }
                }
            }
            if changed {
                let _ = self.save(&t);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    pub fn key(&self, id: &str) -> Option<KeyInfo> {
        self.keys().into_iter().find(|k| k.id == id)
    }

    /// Makes a new ed25519 key and keeps it in the vault.
    pub fn generate(&self, id: &str, comment: &str) -> Result<KeyInfo, String> {
        if !valid_key_id(id) {
            return Err(format!("`{id}` is not a key name: lower-case letters, digits, `.`, `_` or `-`, up to 40"));
        }
        if self.store.get(&vault_key(id)).is_ok() {
            return Err(format!("a key called `{id}` already exists; choose another name"));
        }
        let pair = KeyPair::generate(comment)?;
        self.store.set(&vault_key(id), &pair.to_openssh())?;
        let info = KeyInfo { id: id.into(), public: pair.public_line(), fingerprint: pair.fingerprint(), comment: pair.comment.clone(), passphrase: false, remembered: false };
        let mut t = self.table();
        t.keys.insert(id.into(), info.clone());
        self.save(&t)?;
        self.audit("key.generate", &format!("{id} {}", info.fingerprint));
        Ok(info)
    }

    /// Imports an OpenSSH private key (text pasted or read from a file the person chose). A key with a passphrase must be opened
    /// with it now (so a wrong passphrase is found at once); `remember` keeps the passphrase in the vault, otherwise it is asked
    /// once per session.
    pub fn import(&self, id: &str, text: &str, passphrase: Option<&str>, remember: bool) -> Result<KeyInfo, String> {
        if !valid_key_id(id) {
            return Err(format!("`{id}` is not a key name: lower-case letters, digits, `.`, `_` or `-`, up to 40"));
        }
        if self.store.get(&vault_key(id)).is_ok() {
            return Err(format!("a key called `{id}` already exists; choose another name"));
        }
        let encrypted = crate::sshkey::is_encrypted(text);
        let pair = KeyPair::from_openssh(text, passphrase).map_err(|e| e.to_string())?;
        self.store.set(&vault_key(id), text.trim_end())?;
        if encrypted {
            if remember {
                self.store.set(&vault_pass(id), passphrase.unwrap_or(""))?;
            } else if let Some(p) = passphrase {
                self.session.remember_passphrase(id, p);
            }
        }
        let info = KeyInfo { id: id.into(), public: pair.public_line(), fingerprint: pair.fingerprint(), comment: pair.comment.clone(), passphrase: encrypted, remembered: encrypted && remember };
        let mut t = self.table();
        t.keys.insert(id.into(), info.clone());
        self.save(&t)?;
        self.audit("key.import", &format!("{id} {}", info.fingerprint));
        Ok(info)
    }

    pub fn delete_key(&self, id: &str) -> Result<(), String> {
        if !valid_key_id(id) {
            return Err("not a key name".into());
        }
        self.store.delete(&vault_key(id))?;
        let _ = self.store.delete(&vault_pass(id));
        let mut t = self.table();
        t.keys.remove(id);
        for r in t.hosts.values_mut() {
            if r.key == id {
                r.key.clear();
            }
        }
        self.save(&t)?;
        self.audit("key.delete", id);
        Ok(())
    }

    /// The key in memory, ready for the agent. Fails with a plain message when it needs a passphrase nobody gave.
    pub fn pair(&self, id: &str) -> Result<KeyPair, KeyError> {
        let text = self.store.get(&vault_key(id)).map_err(KeyError::Damaged)?;
        if !crate::sshkey::is_encrypted(&text) {
            return KeyPair::from_openssh(&text, None);
        }
        let pass = self.store.get(&vault_pass(id)).ok().or_else(|| self.session.passphrase(id));
        match pass {
            Some(p) => KeyPair::from_openssh(&text, Some(&p)),
            None => Err(KeyError::NeedPassphrase),
        }
    }

    /// Gives the passphrase of a key once for this session (`remember` also keeps it in the vault).
    pub fn unlock_key(&self, id: &str, passphrase: &str, remember: bool) -> Result<(), String> {
        let text = self.store.get(&vault_key(id))?;
        KeyPair::from_openssh(&text, Some(passphrase)).map_err(|e| e.to_string())?;
        if remember {
            self.store.set(&vault_pass(id), passphrase)?;
            let mut t = self.table();
            if let Some(k) = t.keys.get_mut(id) {
                k.remembered = true;
            }
            self.save(&t)?;
        } else {
            self.session.remember_passphrase(id, passphrase);
        }
        Ok(())
    }

    pub fn set_host_key(&self, host: &str, key: &str) -> Result<(), String> {
        let host = host.trim().to_ascii_lowercase();
        if !valid_host_key(&host) {
            return Err(format!("`{host}` is not a host name (for example github.com or git.example.org:3000)"));
        }
        if !key.is_empty() && self.key(key).is_none() {
            return Err(format!("there is no key called `{key}`"));
        }
        let mut t = self.table();
        t.hosts.entry(host.clone()).or_default().key = key.into();
        if t.hosts[&host] == HostRule::default() {
            t.hosts.remove(&host);
        }
        self.save(&t)?;
        self.audit("host.key", &format!("{host} -> {}", if key.is_empty() { "(none)" } else { key }));
        Ok(())
    }

    pub fn set_token(&self, host: &str, user: &str, token: &str) -> Result<(), String> {
        let host = host.trim().to_ascii_lowercase();
        if !valid_host_key(&host) {
            return Err(format!("`{host}` is not a host name (for example github.com)"));
        }
        if token.trim().is_empty() || token.chars().any(|c| c.is_control()) {
            return Err("the token is empty or has control characters".into());
        }
        if user.chars().any(|c| c.is_control() || c.is_whitespace()) || user.len() > 80 {
            return Err("the user name must not contain spaces".into());
        }
        self.store.set(&vault_token(&host), token.trim())?;
        self.store.set(&vault_token_user(&host), if user.trim().is_empty() { "git" } else { user.trim() })?;
        let mut t = self.table();
        t.hosts.entry(host.clone()).or_default().token = true;
        self.save(&t)?;
        self.audit("host.token", &format!("{host} set"));
        Ok(())
    }

    /// Uses IYAKKI's own token of a platform service (a vault entry) for the service's host, when the host has no token of its own yet.
    /// The value is copied vault to vault; nothing is shown. Returns whether a token was added.
    pub fn adopt_token(&self, host: &str, user: &str, vault_name: &str) -> Result<bool, String> {
        if self.table().hosts.get(&host.to_ascii_lowercase()).is_some_and(|r| r.token) {
            return Ok(false);
        }
        let t = self.store.get(vault_name)?;
        self.set_token(host, user, &t)?;
        Ok(true)
    }

    pub fn clear_token(&self, host: &str) -> Result<(), String> {
        let host = host.trim().to_ascii_lowercase();
        let _ = self.store.delete(&vault_token(&host));
        let _ = self.store.delete(&vault_token_user(&host));
        let mut t = self.table();
        if let Some(r) = t.hosts.get_mut(&host) {
            r.token = false;
            if *r == HostRule::default() {
                t.hosts.remove(&host);
            }
        }
        self.save(&t)?;
        self.audit("host.token", &format!("{host} removed"));
        Ok(())
    }

    /// (user, token) for a host, from the vault.
    pub fn token(&self, host: &str) -> Result<(String, Zeroizing<String>), String> {
        let t = match self.store.get(&vault_token(host)) {
            Ok(t) => t,
            Err(e) => {
                // no token of the host table: the token of a connection for this host (read from where it is kept; never copied)
                let c = self.conns().for_host(host).filter(|c| c.has_token()).ok_or(e)?;
                let t = self.store.get(&c.token_vault)?;
                let u = if c.account.is_empty() { "git".to_string() } else { c.account.clone() };
                return Ok((u, t));
            }
        };
        let u = self.store.get(&vault_token_user(host)).map(|u| u.to_string()).unwrap_or_else(|_| "git".into());
        Ok((u, t))
    }

    pub fn resolve(&self, url: &RepoUrl, workspace: &BTreeMap<String, String>) -> Auth {
        let a = self.table().resolve(url, workspace);
        if a != Auth::None {
            return a;
        }
        // a connection of "Connect Git or Gitea" for this host supplies the sign-in when the host table has none
        match self.conns().for_host(&url.site()) {
            Some(c) if url.is_ssh() && c.method == crate::gitconn::Method::Ssh && !c.key.is_empty() => Auth::Key { id: c.key, from_workspace: false },
            Some(c) if !url.is_ssh() && c.has_token() => Auth::Token { host: url.site() },
            _ => Auth::None,
        }
    }

    /// The git connections of this person (`gitconn.rs`): the same folder, the same vault.
    pub fn conns(&self) -> crate::gitconn::Conns {
        crate::gitconn::Conns::new(&self.dir, self.store.clone(), self.audit.clone())
    }

    /// A line for the audit log when a job starts with this auth (host, key id; never a key).
    pub fn audit_use(&self, what: &str, url: &RepoUrl, auth: &Auth) {
        self.audit("use", &format!("{what} {} via {}", url.site(), match auth {
            Auth::None => "no credentials".to_string(),
            Auth::Key { id, .. } => format!("key {id}"),
            Auth::Token { .. } => "token".to_string(),
        }));
    }
}

// ------------------------------------------------------------------ host keys

/// What the known-hosts file says about an SSH host and what it presented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostCheck {
    /// Recorded, and the server presented a recorded key.
    Trusted,
    /// Never seen: show these fingerprints and ask.
    New(Vec<HostKey>),
    /// Recorded keys exist and none of them was presented: refuse.
    Changed { recorded: Vec<HostKey>, presented: Vec<HostKey> },
}

pub fn host_check(kh: &KnownHosts, host: &str, port: u16, presented: Vec<HostKey>) -> HostCheck {
    let recorded = kh.for_host(host, port);
    if recorded.is_empty() {
        HostCheck::New(presented)
    } else if presented.iter().any(|k| recorded.iter().any(|r| r.kind == k.kind && r.b64 == k.b64)) {
        HostCheck::Trusted
    } else {
        HostCheck::Changed { recorded, presented }
    }
}

pub fn changed_text(host: &str, port: u16, recorded: &[HostKey], presented: &[HostKey]) -> String {
    let fp = |v: &[HostKey]| v.iter().map(|k| format!("{} {}", k.kind, k.fingerprint())).collect::<Vec<_>>().join(", ");
    format!(
        "The host key of {} has CHANGED. Recorded: {}. Presented now: {}. Nothing was sent. This happens when the server was reinstalled, but also when someone is intercepting the connection. Ask the server's owner; if you know the change is genuine, remove the old entry in Settings → Git and SSH keys → Known hosts and connect again.",
        crate::ssh::pattern(host, port),
        fp(recorded),
        fp(presented)
    )
}

/// Does the first connection to an SSH host need the person's confirmation? Fetches what the server presents with `probe`.
pub fn check_ssh_host(kh: &KnownHosts, host: &str, port: u16, probe: &dyn Fn() -> Result<Vec<HostKey>, String>) -> Result<HostCheck, String> {
    // no connection at all when the host is recorded and we can just let ssh verify it (StrictHostKeyChecking=yes); a changed key
    // is still caught by ssh, but a probe first gives the person the reason in words, so it is done here too
    let presented = probe()?;
    Ok(host_check(kh, host, port, presented))
}

#[cfg(test)]
#[path = "gitkeys_tests.rs"]
mod tests;
