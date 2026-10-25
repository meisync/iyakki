//! Developer mode: install, run and watch apps on this device from a developer's own computer (`iyakki dev ...`, like `adb`).
//!
//! * Off by default. Only an administrator turns it on (Settings → Developer mode, or [`set_enabled`] from the IYAKKI menu), with an
//!   optional automatic switch-off after N hours. While it is off every request is refused.
//! * A computer pairs once: the device shows a short code and its key fingerprint, the computer proves it knows the code (see
//!   [`auth`]) and its ed25519 key is kept with a name and the account it acts as. Keys are listed and revoked in Settings.
//! * Every request is signed with the paired key and travels over the device's TLS (the certificate of the web front end).
//! * What it installs goes through the STORE's own review and install ([`ops`]), labelled **Developer (local)**, never promoted;
//!   driver packs and kernel modules are refused; the account's role decides personal or shared installs as in the STORE.
//! * Every action is written to the device's activity log here (who, what, when, result) and to the system audit log.
//!
//! The listener is `iyakki_remote::devserver`; the computer's side is [`client`] and [`cli`]. See docs/DEVELOPER-MODE.md.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use iyakki_sec::policy::Policy;
use serde::{Deserialize, Serialize};

pub mod api;
pub mod app;
pub mod auth;
pub mod bootmount;
pub mod cli;
pub mod config;
pub mod cli_adb;
pub mod debug;
pub mod files;
pub mod grants;
pub mod input;
pub mod client;
pub mod ops;
pub mod scopes;
pub mod shellexec;
pub mod tunnel;
pub mod sessions;
pub mod update;
pub mod patch;
pub mod cli_patch;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_adb;
#[cfg(test)]
mod tests_update;
#[cfg(test)]
mod tests_patch;

/// The "user" a developer screenshot is asked for: the shell answers it with whatever it is drawing (`*` is never in an account name).
pub const SCREEN_ANY: &str = "*developer*";
/// The port the device listens on while developer mode is on.
pub const DEFAULT_PORT: u16 = 7979;
/// The `source` of what developer mode installs: `developer:<key name>`.
pub const SOURCE_PREFIX: &str = "developer:";
/// A pairing code is good for this long, and for this many wrong tries.
pub const PAIR_SECS: i64 = 300;
pub const PAIR_TRIES: u32 = 5;
pub const MAX_KEYS: usize = 32;
/// The longest automatic switch-off (30 days); 0 means "stays on until switched off".
pub const MAX_AUTO_OFF_HOURS: u32 = 24 * 30;
const ACTIVITY_CAP: u64 = 512 * 1024;
const APP_LOG_CAP: u64 = 256 * 1024;

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub enabled: bool,
    /// Who switched it on, and when.
    pub by: String,
    pub since: i64,
    /// When it switches itself off (0: never).
    pub until: i64,
    /// 0 means [`DEFAULT_PORT`].
    #[serde(default)]
    pub port: u16,
}

impl State {
    pub fn port(&self) -> u16 {
        if self.port == 0 { DEFAULT_PORT } else { self.port }
    }
}

/// A developer computer allowed to act on this device.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DevKey {
    pub name: String,
    /// ed25519 public key, hex.
    pub key: String,
    /// The account of this device whose role it acts with.
    pub account: String,
    pub added: i64,
    pub added_by: String,
    #[serde(default)]
    pub last_used: i64,
    /// What this computer may do (see [`scopes`]); keys paired before scopes existed hold only `install`.
    #[serde(default = "scopes::default_scopes")]
    pub scopes: Vec<String>,
}

impl DevKey {
    /// `ab12 cd34 …`: the first 16 hex characters, for lists.
    pub fn short(&self) -> String {
        self.key.chars().take(16).collect::<Vec<_>>().chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" ")
    }
}

/// An open pairing: the code shown on the screen.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Pairing {
    pub code: String,
    pub expires: i64,
    /// The account the new key will act as (the administrator who opened the pairing).
    pub account: String,
    pub tries: u32,
    /// The administrator who opened it.
    #[serde(default)]
    pub by: Option<String>,
    /// The scopes the computer that pairs will get (shown on the pairing card; the administrator can change them while it is open).
    #[serde(default = "scopes::default_scopes")]
    pub scopes: Vec<String>,
}

/// One line of the activity log.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Activity {
    pub time: i64,
    /// The key's name, the administrator, or "?" for a refused stranger.
    pub who: String,
    pub account: String,
    pub action: String,
    pub detail: String,
    pub ok: bool,
}

impl Activity {
    pub fn line(&self) -> String {
        format!("{} {} {}{} {} {}", stamp(self.time), if self.ok { "ok " } else { "NO " }, self.who, if self.account.is_empty() { String::new() } else { format!(" ({})", self.account) }, self.action, self.detail)
    }
}

/// Developer mode of the device whose data folder is given (files in `<data>/devmode`).
#[derive(Clone)]
pub struct DevMode {
    dir: PathBuf,
    data: PathBuf,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

/// Changes to the paired keys and to an open pairing happen one at a time in this process (the listener's threads and the Settings
/// page): a revoke can never be undone by a request that was being served at the same moment, and parallel guesses of a code all count.
fn keys_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: Mutex<()> = Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| f.write_all(bytes)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn append_capped(path: &Path, line: &str, cap: u64) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > cap) {
        let _ = std::fs::rename(path, path.with_extension("1"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// `2026-10-18 14:03:09` in the device's time zone.
pub fn stamp(t: i64) -> String {
    crate::clock::format(&crate::clock::Zone::system(), t, "%F %T")
}

fn clean(s: &str, n: usize) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).take(n).collect()
}

/// A key or computer name: letters, digits, `-`, `_`, `.`, up to 40.
pub fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 40 && n.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

impl DevMode {
    pub fn open(data_dir: &Path) -> DevMode {
        DevMode { dir: data_dir.join("devmode"), data: data_dir.to_path_buf(), clock: Arc::new(crate::clock::now_unix) }
    }

    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> DevMode {
        self.clock = clock;
        self
    }

    pub fn now(&self) -> i64 {
        (self.clock)()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    fn read<T: for<'a> Deserialize<'a> + Default>(&self, f: &str) -> T {
        std::fs::read(self.dir.join(f)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn write<T: Serialize>(&self, f: &str, v: &T) -> Result<(), String> {
        write_private(&self.dir.join(f), &serde_json::to_vec_pretty(v).unwrap_or_default())
    }

    // ---------------------------------------------------------------------------------------- on and off

    /// The saved state, as written (see [`DevMode::is_on`] for whether it is on now).
    pub fn state(&self) -> State {
        self.read("state.json")
    }

    /// On right now. When the automatic switch-off time has passed it is switched off here (and that is logged).
    pub fn is_on(&self) -> bool {
        let st = self.state();
        if !st.enabled {
            return false;
        }
        if st.until > 0 && self.now() >= st.until {
            let _ = self.write("state.json", &State { enabled: false, ..st });
            self.cancel_pairing();
            sessions::end_all(&self.data);
            self.shell_forget_all();
            self.log("system", "", "devmode.off", "switched off automatically: its time was up", true);
            return false;
        }
        true
    }

    fn need_admin(policy: &Policy) -> Result<(), String> {
        if policy.manage_settings { Ok(()) } else { Err("Only an administrator can change developer mode.".into()) }
    }

    /// Switch on (administrators). `hours` > 0 switches it off by itself after that long.
    pub fn enable(&self, policy: &Policy, user: &str, hours: u32) -> Result<State, String> {
        Self::need_admin(policy)?;
        if hours > MAX_AUTO_OFF_HOURS {
            return Err(format!("the automatic switch-off can be at most {MAX_AUTO_OFF_HOURS} hours away"));
        }
        let now = self.now();
        let old = self.state();
        let st = State { enabled: true, by: user.into(), since: now, until: if hours == 0 { 0 } else { now + hours as i64 * 3600 }, port: old.port };
        self.write("state.json", &st)?;
        self.log(user, user, "devmode.on", &if hours == 0 { "until switched off".to_string() } else { format!("for {hours} h") }, true);
        Ok(st)
    }

    /// Switch off (administrators). Also closes an open pairing.
    pub fn disable(&self, policy: &Policy, user: &str) -> Result<(), String> {
        Self::need_admin(policy)?;
        let st = self.state();
        self.write("state.json", &State { enabled: false, ..st })?;
        self.cancel_pairing();
        sessions::end_all(&self.data);
        self.shell_forget_all();
        self.log(user, user, "devmode.off", "switched off", true);
        Ok(())
    }

    /// Change the port the device listens on (administrators; takes effect within a few seconds).
    pub fn set_port(&self, policy: &Policy, user: &str, port: u16) -> Result<(), String> {
        Self::need_admin(policy)?;
        if port != 0 && port < 1024 {
            return Err("pick a port of 1024 or above".into());
        }
        let st = self.state();
        self.write("state.json", &State { port, ..st })?;
        self.log(user, user, "devmode.port", &format!("{}", if port == 0 { DEFAULT_PORT } else { port }), true);
        Ok(())
    }

    /// "On · 3 h left" / "On until switched off" / "Off".
    pub fn status_text(&self) -> String {
        if !self.is_on() {
            return "Off".into();
        }
        let st = self.state();
        if st.until == 0 {
            format!("On (since {}, by {}) until switched off", stamp(st.since), st.by)
        } else {
            let left = (st.until - self.now()).max(0);
            format!("On (by {}), switches off in {} h {} min", st.by, left / 3600, (left % 3600) / 60)
        }
    }

    // ---------------------------------------------------------------------------------------- pairing

    /// Open a pairing (administrators, developer mode on): a new code, good for five minutes and five tries.
    pub fn start_pairing(&self, policy: &Policy, user: &str) -> Result<Pairing, String> {
        self.start_pairing_as(policy, user, user)
    }

    /// Open a pairing whose computer will act as `account` (an existing account of this device; its role decides what it may do).
    pub fn start_pairing_as(&self, policy: &Policy, user: &str, account: &str) -> Result<Pairing, String> {
        Self::need_admin(policy)?;
        if !iyakki_sec::policy::valid_username(account) {
            return Err(format!("{account:?} is not an account name"));
        }
        if !self.is_on() {
            return Err("Switch developer mode on first.".into());
        }
        if self.keys().len() >= MAX_KEYS {
            return Err(format!("{MAX_KEYS} computers are paired already: revoke one first"));
        }
        let p = Pairing { code: auth::new_code()?, expires: self.now() + PAIR_SECS, account: account.into(), tries: 0, by: Some(user.into()), scopes: scopes::default_scopes() };
        self.write("pairing.json", &p)?;
        self.log(user, account, "devmode.pair.open", &format!("a pairing code is on the screen (the computer will act as {account})"), true);
        Ok(p)
    }

    /// The pairing that is open, if any.
    pub fn pairing(&self) -> Option<Pairing> {
        let p: Option<Pairing> = std::fs::read(self.dir.join("pairing.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        p.filter(|p| p.expires > self.now() && p.tries < PAIR_TRIES)
    }

    pub fn cancel_pairing(&self) {
        let _ = std::fs::remove_file(self.dir.join("pairing.json"));
    }

    /// A computer sent its key and its proof of the code. `fp` is this device's certificate fingerprint. On success the key is kept and
    /// the device's answer (for the computer to check) is returned; a wrong proof uses up one of the five tries.
    pub fn complete_pairing(&self, fp: &str, name: &str, key_hex: &str, proof: &str) -> Result<(DevKey, String), String> {
        let _one_at_a_time = keys_lock();
        if !self.is_on() {
            return Err("developer mode is off on this device".into());
        }
        let Some(mut p) = self.pairing() else {
            self.log("?", "", "devmode.pair", &format!("refused {name:?}: no pairing code is open"), false);
            return Err("no pairing code is open on the device: choose Pair a computer in Settings, Developer mode".into());
        };
        let name = name.trim();
        if !valid_name(name) {
            return Err("the computer's name: letters, digits, - _ . (at most 40)".into());
        }
        let key_hex = key_hex.to_ascii_lowercase();
        if !auth::valid_key_hex(&key_hex) {
            return Err("that is not an ed25519 public key".into());
        }
        let pk = auth::pair_key(&p.code, fp)?;
        let want = auth::pair_proof(&pk, fp, &key_hex, name);
        if !auth::same(want.as_bytes(), proof.to_ascii_lowercase().as_bytes()) {
            p.tries += 1;
            let left = PAIR_TRIES.saturating_sub(p.tries);
            if left == 0 {
                self.cancel_pairing();
            } else {
                let _ = self.write("pairing.json", &p);
            }
            self.log("?", "", "devmode.pair", &format!("refused {name:?}: wrong code ({left} tries left)"), false);
            return Err(if left == 0 { "wrong code: that was the last try, open a new pairing on the device".into() } else { format!("wrong code ({left} tries left)") });
        }
        let mut keys = self.keys();
        keys.retain(|k| k.key != key_hex && k.name != name);
        let k = DevKey { name: name.into(), key: key_hex.clone(), account: p.account.clone(), added: self.now(), added_by: p.by.clone().unwrap_or_else(|| p.account.clone()), last_used: 0, scopes: p.scopes.clone() };
        keys.push(k.clone());
        self.write("keys.json", &keys)?;
        self.cancel_pairing();
        self.log(name, &k.account, "devmode.pair", &format!("paired key {} with scopes: {}", k.short(), k.scope_text()), true);
        Ok((k, auth::pair_answer(&pk, fp, &key_hex)))
    }

    // ---------------------------------------------------------------------------------------- keys

    pub fn keys(&self) -> Vec<DevKey> {
        self.read("keys.json")
    }

    pub fn find_key(&self, key_hex: &str) -> Option<DevKey> {
        self.keys().into_iter().find(|k| k.key == key_hex)
    }

    /// Note that a key was used (at most once a minute, so `logs --follow` does not rewrite the file every second).
    pub fn touch_key(&self, key_hex: &str) {
        let _one_at_a_time = keys_lock();
        let mut keys = self.keys();
        let now = self.now();
        if let Some(k) = keys.iter_mut().find(|k| k.key == key_hex && now - k.last_used >= 60) {
            k.last_used = now;
            let _ = self.write("keys.json", &keys);
        }
    }

    /// Take a computer's key away (administrators): by name or by key. Its next request is refused.
    pub fn revoke(&self, policy: &Policy, user: &str, name_or_key: &str) -> Result<DevKey, String> {
        Self::need_admin(policy)?;
        let k = {
            let _one_at_a_time = keys_lock();
            let mut keys = self.keys();
            let pos = keys.iter().position(|k| k.name == name_or_key || k.key == name_or_key.to_ascii_lowercase()).ok_or_else(|| format!("no paired computer called {name_or_key}"))?;
            let k = keys.remove(pos);
            self.write("keys.json", &keys)?;
            k
        };
        self.log(user, user, "devmode.revoke", &format!("{} ({})", k.name, k.short()), true);
        sessions::end_computer(&self.data, &k.name);
        self.shell_forget(&k.name);
        Ok(k)
    }

    // ---------------------------------------------------------------------------------------- activity

    /// Record one action (who, what, result). Never holds a secret.
    pub fn log(&self, who: &str, account: &str, action: &str, detail: &str, ok: bool) {
        let a = Activity { time: self.now(), who: clean(who, 40), account: clean(account, 40), action: clean(action, 40), detail: clean(detail, 300), ok };
        append_capped(&self.dir.join("activity.log"), &format!("{}\n", serde_json::to_string(&a).unwrap_or_default()), ACTIVITY_CAP);
    }

    /// The newest `n` actions, newest first.
    pub fn activity(&self, n: usize) -> Vec<Activity> {
        let mut v: Vec<Activity> = Vec::new();
        for f in [self.dir.join("activity.log"), self.dir.join("activity.1")] {
            let text = std::fs::read_to_string(&f).unwrap_or_default();
            v.extend(text.lines().rev().filter_map(|l| serde_json::from_str(l).ok()));
            if v.len() >= n {
                break;
            }
        }
        v.truncate(n);
        v
    }

    // ---------------------------------------------------------------------------------------- app logs

    fn app_log_path(&self, app: &str) -> Option<PathBuf> {
        crate::skills::valid_skill_name(app).then(|| self.dir.join("logs").join(format!("{app}.log")))
    }

    /// Add a line to an app's log (installs, removals, the page's console when it runs on the screen).
    pub fn app_log(&self, app: &str, line: &str) {
        if let Some(p) = self.app_log_path(app) {
            append_capped(&p, &format!("{} {}\n", stamp(self.now()), clean(line, 2000)), APP_LOG_CAP);
        }
    }

    /// The log of an app from byte `since` on, at most 64 KiB, and where to continue. A log that was rotated starts again at 0.
    pub fn read_app_log(&self, app: &str, since: u64) -> (String, u64) {
        let Some(p) = self.app_log_path(app) else { return (String::new(), 0) };
        let Ok(b) = std::fs::read(&p) else { return (String::new(), 0) };
        let len = b.len() as u64;
        let from = if since > len { 0 } else { since };
        let to = (from + 64 * 1024).min(len);
        (String::from_utf8_lossy(&b[from as usize..to as usize]).into_owned(), to)
    }
}

// -------------------------------------------------------------------------------------------- hand-offs to the shell

#[derive(Default)]
struct Handoff {
    open: HashMap<PathBuf, Vec<String>>,
    reload: HashMap<(PathBuf, String), u64>,
}

fn handoff() -> &'static Mutex<Handoff> {
    static H: OnceLock<Mutex<Handoff>> = OnceLock::new();
    H.get_or_init(Default::default)
}

/// `iyakki dev run <id>`: ask the shell of this data folder to open the app on its screen.
pub fn request_open(data_dir: &Path, name: &str) {
    if let Ok(mut h) = handoff().lock() {
        let q = h.open.entry(data_dir.to_path_buf()).or_default();
        q.retain(|n| n != name);
        q.push(name.to_string());
        q.truncate(8);
    }
}

/// The shell side: the apps it was asked to open.
pub fn take_open(data_dir: &Path) -> Vec<String> {
    handoff().lock().ok().and_then(|mut h| h.open.remove(data_dir)).unwrap_or_default()
}

/// A new build of `name` was installed: an open copy of it reloads (live reload of `iyakki dev push --watch`).
pub fn bump_reload(data_dir: &Path, name: &str) {
    if let Ok(mut h) = handoff().lock() {
        *h.reload.entry((data_dir.to_path_buf(), name.to_string())).or_default() += 1;
    }
}

/// A number that changes each time a new build of `name` is installed.
pub fn reload_stamp(data_dir: &Path, name: &str) -> u64 {
    handoff().lock().ok().and_then(|h| h.reload.get(&(data_dir.to_path_buf(), name.to_string())).copied()).unwrap_or(0)
}

// -------------------------------------------------------------------------------------------- for the IYAKKI menu and the top bar

/// The folder of the device's TLS certificate (shared with the web front end).
pub fn tls_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tls")
}

/// The SHA-256 fingerprint of the device's certificate (`AB:CD:...`), as computers see it; None until the listener first started.
pub fn cert_fingerprint(data_dir: &Path) -> Option<String> {
    let pem = std::fs::read_to_string(tls_dir(data_dir).join("cert.pem")).ok()?;
    let b64: String = pem.split("-----BEGIN CERTIFICATE-----").nth(1)?.split("-----END CERTIFICATE-----").next()?.chars().filter(|c| !c.is_whitespace()).collect();
    crate::models::b64_decode(&b64).filter(|d| !d.is_empty()).map(|d| auth::fingerprint(&d))
}

/// Is developer mode on for this data folder? (cheap: one small file)
pub fn is_on(data_dir: &Path) -> bool {
    DevMode::open(data_dir).is_on()
}

/// Switch developer mode on or off from anywhere (the IYAKKI menu): administrators only, logged. `hours` is the automatic switch-off
/// when turning on (0: never). Returns the new status in words.
pub fn set_enabled(data_dir: &Path, policy: &Policy, user: &str, on: bool, hours: u32) -> Result<String, String> {
    let d = DevMode::open(data_dir);
    if on {
        d.enable(policy, user, hours)?;
    } else {
        d.disable(policy, user)?;
    }
    Ok(d.status_text())
}

/// A short sign for the top bar while developer mode is on ("DEV" or "DEV · pairing"), None when it is off.
pub fn indicator(data_dir: &Path) -> Option<String> {
    let d = DevMode::open(data_dir);
    d.is_on().then(|| if d.pairing().is_some() { "DEV · pairing".to_string() } else { "DEV".to_string() })
}
