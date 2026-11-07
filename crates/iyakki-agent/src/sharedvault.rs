//! Which cryptography keeps the shared vault: the selection point (docs/development/SHARED-VAULT-CRYPTO-OPTIONS.md, F-CLOUD-15).
//!
//! The built-in shared vault (`cloudvaultd.rs`, `cloudkeys.rs`) is **not externally reviewed**, and the owner has decided not to wait for
//! a review. Whoever runs the system chooses: `builtin` (the default) or `external`, where the secret bytes live in the person's own
//! HashiCorp-style (KV version 2) system and IYAKKI keeps only the reference: the address, the mount, a path prefix and the NAME of a
//! vault secret that holds the access token. The setting is `cloud/shared-vault.json` on the data volume:
//! `{"v":1,"provider":"external","external":{"address":"https://vault.example","mount":"secret","path":"iyakki/acme","token_secret":"shared-vault-token"}}`.
//!
//! * With `external`, [`SharedVault`] never opens the built-in service, never writes a value to a local file and never falls back to the
//!   built-in store: when the external system cannot be reached, refuses the token or answers something else, the call fails with a
//!   plain message and nothing is read or written.
//! * A setting that cannot be read (damaged, or from a newer IYAKKI: a higher `v` or a provider this version does not know) is an error
//!   too. It is never taken for `builtin`: a person who chose `external` must not get the built-in store because a file was damaged.
//! * Every call writes one audit line (`cloud/roles-audit.log`) with the provider, the action, the scope and the secret's NAME, and the
//!   outcome in a few plain words. Never a value, a token or the transport's own error text.
//! * Values must be text for `external` (they are stored as the field `value`, so the person's own system shows them as they are).
//!
//! Not done: the snapshot keyring (`cloudsnapshot::VaultKeyring`) and the `iyakki vault` command still use the built-in client directly;
//! the built-in service does not itself refuse writes when the setting says `external`; the setting is not part of the launch
//! configuration of a tenant. See the options document.

use crate::cloudroles::{self, Change};
use crate::cloudvaultd::{Client, Scope};
use iyakki_proxy::Upstream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Shown wherever the shared vault is shown.
pub const LABEL: &str = "not externally reviewed";
pub const FILE_VERSION: u32 = 1;
const MAX_VALUE: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Builtin,
    External,
}

impl Provider {
    pub fn word(self) -> &'static str {
        match self {
            Provider::Builtin => "builtin",
            Provider::External => "external",
        }
    }
}

/// Where the secret bytes live with `external`. Only references: the token is the NAME of a vault secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalRef {
    /// `https://host[:port]`, no path.
    pub address: String,
    /// The KV version 2 mount (`secret`).
    #[serde(default = "default_mount")]
    pub mount: String,
    /// Where under the mount IYAKKI keeps its secrets (`iyakki/acme`); the scope and the name follow.
    pub path: String,
    /// The provider's namespace (Vault Enterprise / OpenBao), if the person uses one.
    #[serde(default)]
    pub namespace: Option<String>,
    /// The NAME of the vault secret that holds the access token. Never the token.
    pub token_secret: String,
}

fn default_mount() -> String {
    "secret".into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    pub v: u32,
    pub provider: Provider,
    #[serde(default)]
    pub external: Option<ExternalRef>,
}

impl Default for Setting {
    fn default() -> Setting {
        Setting { v: FILE_VERSION, provider: Provider::Builtin, external: None }
    }
}

fn segment_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn secret_name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'))
}

impl Setting {
    pub fn check(&self) -> Result<(), String> {
        match (self.provider, &self.external) {
            (Provider::Builtin, None) => Ok(()),
            (Provider::Builtin, Some(_)) => Err("the built-in shared vault takes no external reference: remove it or choose external".into()),
            (Provider::External, None) => Err("external needs the reference: address, path and the name of the vault secret that holds the token".into()),
            (Provider::External, Some(e)) => {
                crate::cloudvault::check_url(&e.address)?;
                if !segment_ok(&e.mount) {
                    return Err("the mount is a plain name such as secret".into());
                }
                let parts: Vec<&str> = e.path.split('/').collect();
                if parts.len() > 8 || !parts.iter().all(|p| segment_ok(p)) {
                    return Err("the path is up to eight plain names separated by / (for example iyakki/acme)".into());
                }
                if e.namespace.as_deref().is_some_and(|n| !n.split('/').all(segment_ok)) {
                    return Err("the namespace is made of plain names".into());
                }
                if !secret_name_ok(&e.token_secret) {
                    return Err("token_secret is the NAME of a vault secret (lower-case letters, digits . _ -), never the token itself".into());
                }
                Ok(())
            }
        }
    }

    /// Read the file's text. A damaged text, a newer file and a provider this version does not know are errors, never `builtin`.
    pub fn parse(text: &str) -> Result<Setting, String> {
        let fail = |why: &str| format!("the shared-vault setting cannot be used ({why}); the shared vault stays closed until it is fixed. Nothing falls back to the built-in store.");
        let raw: Value = serde_json::from_str(text).map_err(|_| fail("the file is damaged"))?;
        match raw.get("v").and_then(Value::as_u64) {
            Some(v) if v > u64::from(FILE_VERSION) => return Err(fail("it was written by a newer IYAKKI")),
            Some(v) if v >= 1 => {}
            _ => return Err(fail("it has no version")),
        }
        let s: Setting = serde_json::from_value(raw).map_err(|_| fail("an unknown provider or field: it may be from a newer IYAKKI"))?;
        s.check().map_err(|e| fail(&e))?;
        Ok(s)
    }

    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("cloud").join("shared-vault.json")
    }

    /// The setting of this data volume: `builtin` when there is no file (the default), an error when there is one that cannot be used.
    pub fn load(data_dir: &Path) -> Result<Setting, String> {
        match std::fs::read_to_string(Self::path(data_dir)) {
            Ok(t) => Setting::parse(&t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Setting::default()),
            Err(_) => Err("the shared-vault setting cannot be read; the shared vault stays closed until it is fixed".into()),
        }
    }

    /// Check, write whole and atomically (mode 0600) and put one line in the audit trail. The file holds references only.
    pub fn save(&self, data_dir: &Path, actor: &str) -> Result<(), String> {
        self.check()?;
        let file = Self::path(data_dir);
        let dir = file.parent().ok_or("no folder")?;
        std::fs::create_dir_all(dir).map_err(|_| "the setting could not be saved (the folder cannot be made)".to_string())?;
        let tmp = dir.join(".shared-vault.json.tmp");
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|_| "the setting could not be saved".to_string())?;
            f.write_all(serde_json::to_string_pretty(self).map_err(|e| e.to_string())?.as_bytes()).map_err(|_| "the setting could not be saved".to_string())?;
        }
        std::fs::rename(&tmp, &file).map_err(|_| "the setting could not be saved".to_string())?;
        audit(data_dir, &format!("shared vault: provider set to {} by {actor} ({LABEL})", self.provider.word()));
        Ok(())
    }

    /// One plain line for a page or a tool: the provider and the label. Addresses and names of tokens are not part of it.
    pub fn summary(&self) -> String {
        match self.provider {
            Provider::Builtin => format!("Shared vault: built-in ({LABEL})"),
            Provider::External => format!("Shared vault: external system ({LABEL} here; the person's own system holds the secrets)"),
        }
    }
}

/// The one line for a page: reads the file, never fails (a setting that cannot be used says so).
pub fn page_line(data_dir: &Path) -> String {
    match Setting::load(data_dir) {
        Ok(s) => s.summary(),
        Err(_) => format!("Shared vault: setting unusable, the vault stays closed ({LABEL})"),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn audit(data_dir: &Path, line: &str) {
    if let Err(e) = cloudroles::append_audit(&cloudroles::audit_path(data_dir), now(), &Change(line.to_string())) {
        log::warn!("shared vault: the audit line could not be written: {e}");
    }
}

// ---- the backends ------------------------------------------------------------------------------------------------------------------

/// What the shared vault does with secret bytes, whichever system keeps them.
pub trait Backend: Send {
    fn provider(&self) -> Provider;
    fn put(&self, scope: &Scope, name: &str, value: &[u8]) -> Result<(), String>;
    fn get(&self, scope: &Scope, name: &str) -> Result<Vec<u8>, String>;
    fn list(&self, scope: &Scope) -> Result<Vec<String>, String>;
    fn delete(&self, scope: &Scope, name: &str) -> Result<(), String>;
}

/// The built-in service (`cloudvaultd.rs`): the member's side seals and opens.
pub struct Builtin(pub Client);

impl Backend for Builtin {
    fn provider(&self) -> Provider {
        Provider::Builtin
    }
    fn put(&self, s: &Scope, n: &str, v: &[u8]) -> Result<(), String> {
        self.0.put(s, n, v)
    }
    fn get(&self, s: &Scope, n: &str) -> Result<Vec<u8>, String> {
        self.0.get(s, n)
    }
    fn list(&self, s: &Scope) -> Result<Vec<String>, String> {
        self.0.list(s)
    }
    fn delete(&self, s: &Scope, n: &str) -> Result<(), String> {
        self.0.delete(s, n)
    }
}

/// Gives the token for the NAME of a vault secret.
pub type TokenSource = Box<dyn Fn(&str) -> Result<zeroize::Zeroizing<String>, String> + Send>;

const UNREACHABLE: &str = "The external vault could not be reached, so nothing was read or written. The built-in store was not used instead. Check the address and the network, and that the external system is running.";

/// A HashiCorp-style KV version 2 system the person already runs. Nothing is kept here: not a value, not the token.
pub struct External {
    cfg: ExternalRef,
    up: Arc<dyn Upstream>,
    token: TokenSource,
}

impl External {
    pub fn new(cfg: ExternalRef, up: Arc<dyn Upstream>, token: TokenSource) -> External {
        External { cfg, up, token }
    }

    fn scope_dir(s: &Scope) -> String {
        s.name().replace(':', ".")
    }

    fn url(&self, kind: &str, s: &Scope, name: Option<&str>) -> Result<String, String> {
        if let Some(n) = name {
            if !secret_name_ok(n) && !segment_ok(n) {
                return Err("that is not a plain secret name".into());
            }
        }
        let mut u = format!("{}/v1/{}/{kind}/{}/{}", self.cfg.address.trim_end_matches('/'), self.cfg.mount, self.cfg.path, Self::scope_dir(s));
        if let Some(n) = name {
            u.push('/');
            u.push_str(n);
        }
        Ok(u)
    }

    fn call(&self, method: &str, url: &str, body: &[u8]) -> Result<(u16, Vec<u8>), String> {
        let tok = (self.token)(&self.cfg.token_secret).map_err(|_| format!("The external vault's access secret \"{}\" is not available in the vault, so nothing was read or written. The built-in store was not used instead.", self.cfg.token_secret))?;
        let mut h = vec![("X-Vault-Token".to_string(), tok.to_string())];
        if let Some(ns) = &self.cfg.namespace {
            h.push(("X-Vault-Namespace".into(), ns.clone()));
        }
        if !body.is_empty() {
            h.push(("Content-Type".into(), "application/json".into()));
        }
        // the transport's own text may carry an address or a header: it is dropped
        let r = self.up.request(method, url, &h, body).map_err(|_| UNREACHABLE.to_string())?;
        match r.status {
            401 | 403 => Err("The external vault refused the access secret, so nothing was read or written. The built-in store was not used instead.".into()),
            s => Ok((s, r.body)),
        }
    }

    fn other(status: u16) -> String {
        format!("The external vault answered with an unexpected result (HTTP {status}); nothing was changed. The built-in store was not used instead.")
    }
}

impl Backend for External {
    fn provider(&self) -> Provider {
        Provider::External
    }

    fn put(&self, s: &Scope, n: &str, v: &[u8]) -> Result<(), String> {
        let text = std::str::from_utf8(v).map_err(|_| "an external vault keeps text: this value is not text".to_string())?;
        if v.len() > MAX_VALUE {
            return Err("the value is too large (64 KiB)".into());
        }
        let body = json!({"data": {"value": text}}).to_string();
        match self.call("POST", &self.url("data", s, Some(n))?, body.as_bytes())? {
            (200 | 204, _) => Ok(()),
            (st, _) => Err(Self::other(st)),
        }
    }

    fn get(&self, s: &Scope, n: &str) -> Result<Vec<u8>, String> {
        match self.call("GET", &self.url("data", s, Some(n))?, b"")? {
            (200, b) => {
                let v: Value = serde_json::from_slice(&b).map_err(|_| "The external vault answered with something that is not a secret.".to_string())?;
                v.pointer("/data/data/value").and_then(Value::as_str).map(|t| t.as_bytes().to_vec()).ok_or_else(|| format!("There is no secret \"{n}\" with a value field in the external vault."))
            }
            (404, _) => Err(format!("There is no secret \"{n}\" in the external vault.")),
            (st, _) => Err(Self::other(st)),
        }
    }

    fn list(&self, s: &Scope) -> Result<Vec<String>, String> {
        let url = format!("{}?list=true", self.url("metadata", s, None)?);
        match self.call("GET", &url, b"")? {
            (200, b) => {
                let v: Value = serde_json::from_slice(&b).map_err(|_| "The external vault answered with something that is not a list.".to_string())?;
                Ok(v.pointer("/data/keys").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).filter(|k| !k.ends_with('/')).map(String::from).collect())
            }
            (404, _) => Ok(Vec::new()),
            (st, _) => Err(Self::other(st)),
        }
    }

    fn delete(&self, s: &Scope, n: &str) -> Result<(), String> {
        match self.call("DELETE", &self.url("metadata", s, Some(n))?, b"")? {
            (200 | 204 | 404, _) => Ok(()),
            (st, _) => Err(Self::other(st)),
        }
    }
}

// ---- the shared vault as the rest of the system uses it ---------------------------------------------------------------------------

/// The shared vault of a data volume with the provider the setting names, and the audit of every call.
pub struct SharedVault {
    backend: Box<dyn Backend>,
    data_dir: PathBuf,
    actor: String,
}

/// What `open` needs to build the provider the setting names.
pub struct Wiring {
    pub data_dir: PathBuf,
    pub actor: String,
    /// The built-in member's side; only used with `builtin`.
    pub builtin: Option<Client>,
    pub upstream: Arc<dyn Upstream>,
    pub token: TokenSource,
}

impl SharedVault {
    /// The shared vault for the setting on `w.data_dir`. A setting that cannot be used, or `builtin` with no client, is an error.
    pub fn open(w: Wiring) -> Result<SharedVault, String> {
        let setting = Setting::load(&w.data_dir)?;
        let backend: Box<dyn Backend> = match (setting.provider, setting.external) {
            (Provider::Builtin, _) => Box::new(Builtin(w.builtin.ok_or("the built-in shared vault is not configured on this machine")?)),
            (Provider::External, Some(e)) => Box::new(External::new(e, w.upstream, w.token)),
            (Provider::External, None) => return Err("external needs the reference".into()),
        };
        Ok(SharedVault { backend, data_dir: w.data_dir, actor: w.actor })
    }

    pub fn provider(&self) -> Provider {
        self.backend.provider()
    }

    fn done<T>(&self, action: &str, s: &Scope, name: &str, r: Result<T, String>) -> Result<T, String> {
        let outcome = match &r {
            Ok(_) => "ok",
            Err(e) if e.starts_with("The external vault could not be reached") => "failed: unreachable",
            Err(_) => "failed",
        };
        let name = if name == "-" || secret_name_ok(name) || segment_ok(name) { name } else { "(not a plain name)" };
        audit(&self.data_dir, &format!("shared vault: provider {} {action} {} {name} by {}: {outcome} ({LABEL})", self.provider().word(), s.name(), self.actor));
        r
    }

    pub fn put(&self, s: &Scope, name: &str, value: &[u8]) -> Result<(), String> {
        self.done("put", s, name, self.backend.put(s, name, value))
    }
    pub fn get(&self, s: &Scope, name: &str) -> Result<Vec<u8>, String> {
        self.done("get", s, name, self.backend.get(s, name))
    }
    pub fn list(&self, s: &Scope) -> Result<Vec<String>, String> {
        self.done("list", s, "-", self.backend.list(s))
    }
    pub fn delete(&self, s: &Scope, name: &str) -> Result<(), String> {
        self.done("delete", s, name, self.backend.delete(s, name))
    }
}
