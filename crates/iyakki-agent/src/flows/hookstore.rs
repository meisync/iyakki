//! The webhook records (F-FLOW-15): which hooks exist, whose they are, whether they are on, the hash of each hook's secret address
//! token, when a public opening ends and the last 20 deliveries of each. One file for the whole machine (`<data>/hooks.json`,
//! written whole and renamed into place), because a request arrives with only the hook's id and token and must find its owner.
//!
//! What is kept: the SHA-256 of the token (the token itself is shown once, when it is made, and never stored), the owner, the flow,
//! the trigger's id, and for each delivery the time, the outcome, the status and the sender's address. What is never kept: the
//! token, the shared secret (it lives in the owner's vault), the body, the headers or the signature.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::devmode::auth::hex;

/// Deliveries kept per hook.
pub const KEEP: usize = 20;
/// The longest a public opening may last.
pub const PUBLIC_MAX_S: i64 = 7 * 86_400;
/// Hooks one machine keeps (a runaway tool cannot fill the file).
pub const MAX_HOOKS: usize = 200;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Delivery {
    pub at: i64,
    /// `accepted`, `bad_signature`, `stale`, `replay`, `rate_limited`, `too_large`, `wrong_method`, `bad_content_type`,
    /// `bad_payload`, `outside_network`, `owner_not_allowed`, `not_published`, `off`, `vault_locked`, `not_started`.
    pub outcome: String,
    pub http: u16,
    #[serde(default)]
    pub run: String,
    #[serde(default)]
    pub bytes: usize,
    #[serde(default)]
    pub from: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HookRec {
    pub id: String,
    pub owner: String,
    pub flow: String,
    pub trigger: String,
    /// SHA-256 of the token, hex.
    pub token_hash: String,
    /// The owner switched it on (a new hook is off).
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub created: i64,
    /// When the token was last made again.
    #[serde(default)]
    pub rotated: i64,
    /// Senders outside the local network are taken until this time (Unix seconds; 0: local network only).
    #[serde(default)]
    pub public_until: i64,
    /// A shared secret is kept in the owner's vault (`hook.<id>`).
    #[serde(default)]
    pub has_secret: bool,
    #[serde(default)]
    pub deliveries: Vec<Delivery>,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    hooks: Vec<HookRec>,
}

pub struct HookStore {
    path: PathBuf,
    inner: Mutex<BTreeMap<String, HookRec>>,
}

static STORES: OnceLock<Mutex<std::collections::HashMap<PathBuf, Arc<HookStore>>>> = OnceLock::new();

pub fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// Constant-time equality of two texts of any length (a difference in length shows, a difference in content does not).
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// A new token: 256 random bits, hex.
pub fn new_token() -> String {
    crate::sharedfolder::random_hex(32)
}

/// A new hook id: `hk-` and 10 random hex digits.
pub fn new_id() -> String {
    format!("hk-{}", crate::sharedfolder::random_hex(5))
}

/// Is this sender on the local network (loopback, private, link-local, unique-local)?
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_loopback() || a.is_private() || a.is_link_local(),
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_local(IpAddr::V4(v4));
            }
            let seg = a.segments()[0];
            a.is_loopback() || (seg & 0xfe00) == 0xfc00 || (seg & 0xffc0) == 0xfe80
        }
    }
}

impl HookStore {
    /// The one store of this data directory (every part of the process shares it).
    pub fn shared(dir: &Path) -> Arc<HookStore> {
        let path = dir.join("hooks.json");
        let m = STORES.get_or_init(|| Mutex::new(Default::default()));
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        g.entry(path.clone()).or_insert_with(|| Arc::new(HookStore::open(&path))).clone()
    }

    pub fn open(path: &Path) -> HookStore {
        let file: File = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        HookStore { path: path.to_path_buf(), inner: Mutex::new(file.hooks.into_iter().map(|h| (h.id.clone(), h)).collect()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, HookRec>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save(&self, m: &BTreeMap<String, HookRec>) -> Result<(), String> {
        let file = File { version: 1, hooks: m.values().cloned().collect() };
        let text = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
        if let Some(d) = self.path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("cannot save the webhooks: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("cannot save the webhooks: {e}"))
    }

    pub fn get(&self, id: &str) -> Option<HookRec> {
        self.lock().get(id).cloned()
    }

    pub fn of_owner(&self, owner: &str) -> Vec<HookRec> {
        self.lock().values().filter(|h| h.owner == owner).cloned().collect()
    }

    pub fn insert(&self, rec: HookRec) -> Result<(), String> {
        let mut m = self.lock();
        if m.len() >= MAX_HOOKS && !m.contains_key(&rec.id) {
            return Err(format!("this computer keeps at most {MAX_HOOKS} webhooks"));
        }
        let old = m.insert(rec.id.clone(), rec.clone());
        self.save(&m).inspect_err(|_| {
            match old {
                Some(o) => m.insert(o.id.clone(), o),
                None => m.remove(&rec.id),
            };
        })
    }

    /// Change one record; `f` returns what to do. Nothing changes when the file cannot be written.
    pub fn update(&self, id: &str, f: impl FnOnce(&mut HookRec)) -> Result<HookRec, String> {
        let mut m = self.lock();
        let Some(r) = m.get_mut(id) else { return Err(format!("there is no webhook `{id}`")) };
        let before = r.clone();
        f(r);
        let after = r.clone();
        match self.save(&m) {
            Ok(()) => Ok(after),
            Err(e) => {
                m.insert(id.to_string(), before);
                Err(e)
            }
        }
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut m = self.lock();
        let Some(old) = m.remove(id) else { return Ok(()) };
        self.save(&m).inspect_err(|_| {
            m.insert(id.to_string(), old);
        })
    }

    /// Note one delivery (the newest [`KEEP`] stay). A failed write is only logged: a request is not refused for it.
    pub fn record(&self, id: &str, d: Delivery) {
        let mut m = self.lock();
        let Some(r) = m.get_mut(id) else { return };
        r.deliveries.push(d);
        let extra = r.deliveries.len().saturating_sub(KEEP);
        r.deliveries.drain(..extra);
        if let Err(e) = self.save(&m) {
            log::warn!("webhooks: {e}");
        }
    }
}

// ----------------------------------------------------------------------------------------------- what the screen shows once

/// A new address and secret waiting to be shown on the person's screen (never given to the assistant).
#[derive(Clone)]
pub struct Reveal {
    pub hook: String,
    pub flow: String,
    /// The path and token: `/hooks/<id>/<token>`.
    pub path: String,
    /// The shared secret, when the hook is signed.
    pub secret: Option<String>,
}

impl std::fmt::Debug for Reveal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Reveal({}, hidden)", self.hook)
    }
}

static REVEALS: OnceLock<Mutex<std::collections::HashMap<String, Reveal>>> = OnceLock::new();

/// Leave a reveal for the person's screen. One waits per person; a newer one replaces it.
pub fn stash_reveal(user: &str, r: Reveal) {
    REVEALS.get_or_init(|| Mutex::new(Default::default())).lock().unwrap_or_else(|e| e.into_inner()).insert(user.to_string(), r);
}

/// The reveal waiting for this person, once.
pub fn take_reveal(user: &str) -> Option<Reveal> {
    REVEALS.get()?.lock().unwrap_or_else(|e| e.into_inner()).remove(user)
}

pub fn reveal_waiting(user: &str) -> bool {
    REVEALS.get().is_some_and(|m| m.lock().unwrap_or_else(|e| e.into_inner()).contains_key(user))
}
