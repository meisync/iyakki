//! The SFTP server of this computer: settings, authorised keys, the running listener, the firewall opening and the audit lines.
//! The protocol is in `sftp_proto.rs`, the confined files in `sftp_fs.rs` and `sftp_session.rs`, the SSH layer in `ssh_server.rs`;
//! the design and its limits are in `docs/development/SFTP-SERVER.md`.
//!
//! * **Off by default.** Nothing listens until an administrator enables it (the tool and the Settings page ask first). It does not come
//!   back by itself after a restart or a reboot: enabling is a deliberate act each time.
//! * **Who:** only the accounts on the allow-list, and each sees only what the account's file rules give (`FileRoots`, the same rules as
//!   the file tools). Login by an authorised Ed25519 key (public keys are kept in `<data>/sftp/keys/<account>.pub`, one line each) or, if
//!   switched on, by the account's password checked by the account code (`Authenticator`: Argon2id, throttled). The password is passed
//!   straight to that check and dropped; it is never stored or logged.
//! * **Where from:** only the local network (private, loopback and link-local addresses), by a check on every connection and by firewall
//!   rules for those networks only, with an expiry.
//! * **Limits:** 8 connections at once, 10 wrong passwords from one address in 5 minutes block it for 5 minutes, 4 failed logins end a
//!   connection, 30 seconds to log in.

use std::collections::HashMap;
use std::net::{IpAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_core_api::service::Authenticator;
use serde::{Deserialize, Serialize};

use crate::sftp_fs::Jail;
use crate::sftp_session::Session;
use crate::ssh_server::{self, Gate, Server};
use crate::sshkey::{fingerprint_of_blob, KeyPair};

pub const DEFAULT_PORT: u16 = 2222;
pub const MAX_CONNECTIONS: usize = 8;
pub const MAX_KEYS_PER_USER: usize = 20;
pub const RULE_COMMENT: &str = "IYAKKI SFTP server";
pub const EXPIRIES: [(&str, u64); 4] = [("1 hour", 3600), ("1 day", 86400), ("7 days", 604800), ("never", 0)];

pub type Audit = Arc<dyn Fn(&str, &str) + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub port: u16,
    /// Accounts that may log in (empty: nobody).
    pub users: Vec<String>,
    /// Allow the account password as well as keys (off by default).
    pub password_login: bool,
    /// Seconds a start lasts (0: until stopped).
    pub expiry_secs: u64,
}

impl Default for Config {
    fn default() -> Config {
        Config { port: DEFAULT_PORT, users: Vec::new(), password_login: false, expiry_secs: 86400 }
    }
}

pub fn valid_port(p: u16) -> Result<u16, String> {
    if p < 1024 || (8081..=8180).contains(&p) {
        return Err("choose a port from 1024 to 65535 outside 8081-8180 (those belong to Shared folders)".into());
    }
    Ok(p)
}

// ------------------------------------------------------------------ authorised keys

/// A public key line (`ssh-ed25519 AAAA… comment`) as the blob and the comment. Only Ed25519 is accepted.
pub fn parse_key_line(line: &str) -> Result<(Vec<u8>, String), String> {
    let line = line.trim();
    if line.len() > 600 || line.contains('\n') || line.contains('\0') || line.contains("PRIVATE") {
        return Err("that is not a single public key line (never paste a private key)".into());
    }
    let mut it = line.splitn(3, char::is_whitespace);
    let ty = it.next().unwrap_or("");
    let b64 = it.next().unwrap_or("");
    let comment = it.next().unwrap_or("").trim().chars().filter(|c| !c.is_control()).take(80).collect::<String>();
    if ty != "ssh-ed25519" {
        return Err("only ssh-ed25519 public keys are accepted (make one with: ssh-keygen -t ed25519)".into());
    }
    let blob = crate::sshkey::unb64(b64).ok_or("the key text does not decode")?;
    let mut r = crate::sshkey::Reader { b: &blob };
    if r.string() != Some(b"ssh-ed25519") || r.string().map(<[u8]>::len) != Some(32) || !r.b.is_empty() {
        return Err("the key text is not an Ed25519 public key".into());
    }
    Ok((blob, comment))
}

fn keys_dir(dir: &Path) -> PathBuf {
    dir.join("sftp").join("keys")
}

fn key_file(dir: &Path, user: &str) -> Result<PathBuf, String> {
    if !iyakki_sec::policy::valid_username(user) {
        return Err("that is not an account name".into());
    }
    Ok(keys_dir(dir).join(format!("{user}.pub")))
}

/// The keys of an account as (blob, comment).
pub fn user_keys(dir: &Path, user: &str) -> Vec<(Vec<u8>, String)> {
    let Ok(f) = key_file(dir, user) else { return Vec::new() };
    std::fs::read_to_string(f).unwrap_or_default().lines().filter_map(|l| parse_key_line(l).ok()).take(MAX_KEYS_PER_USER).collect()
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700));
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
    f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

pub fn add_key(dir: &Path, user: &str, line: &str) -> Result<String, String> {
    let (blob, comment) = parse_key_line(line)?;
    let keys = user_keys(dir, user);
    let fp = fingerprint_of_blob(&blob);
    if keys.iter().any(|(b, _)| *b == blob) {
        return Ok(fp);
    }
    if keys.len() >= MAX_KEYS_PER_USER {
        return Err(format!("at most {MAX_KEYS_PER_USER} keys per account"));
    }
    let mut text: String = keys.iter().map(|(b, c)| format!("ssh-ed25519 {} {}\n", crate::sshkey::b64(b), c)).collect();
    text.push_str(&format!("ssh-ed25519 {} {}\n", crate::sshkey::b64(&blob), comment));
    write_private(&key_file(dir, user)?, &text)?;
    Ok(fp)
}

pub fn remove_key(dir: &Path, user: &str, fingerprint: &str) -> Result<usize, String> {
    let keys = user_keys(dir, user);
    let keep: Vec<_> = keys.iter().filter(|(b, _)| fingerprint_of_blob(b) != fingerprint.trim()).collect();
    let removed = keys.len() - keep.len();
    if removed == 0 {
        return Err("no key with that fingerprint (sftp_status lists them)".into());
    }
    let text: String = keep.iter().map(|(b, c)| format!("ssh-ed25519 {} {}\n", crate::sshkey::b64(b), c)).collect();
    write_private(&key_file(dir, user)?, &text)?;
    Ok(removed)
}

/// The server's own host key, made on first use and kept in `<data>/sftp/host_key` (mode 0600).
pub fn host_key(dir: &Path) -> Result<KeyPair, String> {
    let path = dir.join("sftp").join("host_key");
    if let Ok(t) = std::fs::read_to_string(&path) {
        return KeyPair::from_openssh(&t, None).map_err(|e| format!("the SFTP host key is damaged: {e}"));
    }
    let k = KeyPair::generate("iyakki-sftp-host")?;
    write_private(&path, &k.to_openssh())?;
    Ok(k)
}

// ------------------------------------------------------------------ rate limit

/// Wrong passwords per address: `MAX` in `WINDOW` seconds block the address for `BLOCK` seconds.
#[derive(Default)]
pub struct Limiter {
    seen: HashMap<IpAddr, (u32, i64, i64)>,
}

impl Limiter {
    const MAX: u32 = 10;
    const WINDOW: i64 = 300;
    const BLOCK: i64 = 300;

    pub fn blocked(&self, ip: IpAddr, now: i64) -> bool {
        self.seen.get(&ip).is_some_and(|(_, _, until)| *until > now)
    }
    pub fn failed(&mut self, ip: IpAddr, now: i64) {
        if self.seen.len() > 4096 {
            self.seen.retain(|_, (_, start, until)| *until > now || now - *start < Self::WINDOW);
        }
        let e = self.seen.entry(ip).or_insert((0, now, 0));
        if now - e.1 > Self::WINDOW {
            *e = (0, now, 0);
        }
        e.0 += 1;
        if e.0 >= Self::MAX {
            *e = (0, now, now + Self::BLOCK);
        }
    }
}

/// The account system as two closures (the shell passes its user store), so this crate needs no account types.
pub struct FnAuth {
    login: Arc<dyn Fn(&str, &str) -> Result<iyakki_sec::policy::Policy, String> + Send + Sync>,
    lookup: Arc<dyn Fn(&str) -> Option<iyakki_sec::policy::Policy> + Send + Sync>,
}

impl FnAuth {
    pub fn new(login: Arc<dyn Fn(&str, &str) -> Result<iyakki_sec::policy::Policy, String> + Send + Sync>, lookup: Arc<dyn Fn(&str) -> Option<iyakki_sec::policy::Policy> + Send + Sync>) -> FnAuth {
        FnAuth { login, lookup }
    }
}

fn principal(user: &str, policy: iyakki_sec::policy::Policy) -> iyakki_core_api::service::Principal {
    iyakki_core_api::service::Principal { user: user.to_string(), role: "user".into(), admin: false, policy }
}

impl Authenticator for FnAuth {
    fn login(&self, name: &str, password: &str) -> iyakki_core_api::error::ApiResult<iyakki_core_api::service::Principal> {
        (self.login)(name, password).map(|p| principal(name, p)).map_err(iyakki_core_api::error::ApiError::Denied)
    }
    fn lookup(&self, user: &str) -> Option<iyakki_core_api::service::Principal> {
        (self.lookup)(user).map(|p| principal(user, p))
    }
}

// ------------------------------------------------------------------ the gate

pub struct RealGate {
    pub dir: PathBuf,
    pub cfg: Config,
    pub auth: Arc<dyn Authenticator>,
    pub audit: Audit,
    pub limiter: Arc<Mutex<Limiter>>,
    /// A fixed home folder instead of the account's usual one (tests and portable set-ups).
    pub home: Option<PathBuf>,
}

impl RealGate {
    fn allowed(&self, user: &str) -> bool {
        self.cfg.users.iter().any(|u| u == user)
    }
}

impl Gate for RealGate {
    fn key_ok(&self, user: &str, blob: &[u8], _peer: IpAddr) -> bool {
        self.allowed(user) && user_keys(&self.dir, user).iter().any(|(b, _)| b.as_slice() == blob)
    }

    fn password_ok(&self, user: &str, password: &str, peer: IpAddr) -> bool {
        let now = crate::clock::now_unix();
        if !self.cfg.password_login || !self.allowed(user) {
            return false;
        }
        if self.limiter.lock().unwrap_or_else(|e| e.into_inner()).blocked(peer, now) {
            return false;
        }
        let ok = self.auth.login(user, password).is_ok_and(|p| p.user == user);
        if !ok {
            self.limiter.lock().unwrap_or_else(|e| e.into_inner()).failed(peer, now);
        }
        ok
    }

    fn session(&self, user: &str, peer: IpAddr) -> Option<Session> {
        if !self.allowed(user) {
            return None;
        }
        // the account as it is now: its role and rules may have changed since it was put on the list
        let p = self.auth.lookup(user)?;
        let jail = jail_for(&p.policy, user, &self.dir, self.home.clone());
        let (audit, who) = (self.audit.clone(), format!("{user}@{peer}"));
        Some(Session::new(jail, Box::new(move |e, d| audit(e, &format!("{d} by {who}")))))
    }

    fn audit(&self, event: &str, detail: &str) {
        (self.audit)(event, detail);
    }
}

/// The jail of an account: its home, every other readable root as `/+name`, the account's own read and write rules, and the system data
/// folder never reachable unless it holds the home itself.
pub fn jail_for(policy: &iyakki_sec::policy::Policy, user: &str, data: &Path, home: Option<PathBuf>) -> Jail {
    let roots = match home {
        Some(h) => crate::scope::FileRoots::with_home(policy, user, data, h),
        None => crate::scope::FileRoots::new(policy, user, data),
    };
    let home = roots.home.clone();
    let others: Vec<PathBuf> = roots.roots.iter().filter(|r| **r != home).cloned().collect();
    let deny = std::fs::canonicalize(data).unwrap_or_else(|_| data.to_path_buf());
    let (r1, p1, u1, h1, d1) = (roots.clone(), policy.clone(), user.to_string(), home.clone(), deny.clone());
    let read: crate::sftp_fs::Rule = Arc::new(move |p: &Path| r1.read_ok(&p1, &u1, p) && (p.starts_with(&h1) || !p.starts_with(&d1)));
    let (r2, p2, u2, h2, d2) = (roots, policy.clone(), user.to_string(), home.clone(), deny);
    let write: crate::sftp_fs::Rule = Arc::new(move |p: &Path| r2.contains(p) && r2.write_ok(&p2, &u2, p) && (p.starts_with(&h2) || !p.starts_with(&d2)));
    Jail::new(home, others, read, write)
}

// ------------------------------------------------------------------ firewall

/// Opens and closes the port for the local networks. The real one is the firewall engine; tests use a fake.
pub trait Firewall: Send + Sync {
    /// Allow TCP `port` from the local networks until `expires` (Unix seconds, 0: until closed).
    fn open(&self, port: u16, expires: i64) -> Result<(), String>;
    fn close(&self) -> Result<(), String>;
}

pub const LOCAL_NETWORKS: [&str; 4] = ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16"];

/// The settings with the SFTP rules for `port` (earlier ones of ours replaced); `None` removes them. Pure.
pub fn with_rules(mut c: iyakki_fw::model::Config, open: Option<(u16, i64)>) -> iyakki_fw::model::Config {
    c.rules.retain(|r| r.comment != RULE_COMMENT);
    if let Some((port, expires)) = open {
        for net in LOCAL_NETWORKS {
            let mut r = iyakki_fw::model::Rule::new(iyakki_fw::model::Kind::Cidr, net);
            r.proto = iyakki_fw::model::Proto::Tcp;
            r.ports = port.to_string();
            r.comment = RULE_COMMENT.to_string();
            r.expires = expires;
            c.add(r);
        }
    }
    c
}

pub struct RealFirewall;

impl RealFirewall {
    fn apply(open: Option<(u16, i64)>) -> Result<(), String> {
        let mut e = iyakki_fw::service::engine();
        let new = with_rules(e.config().clone(), open);
        if new == *e.config() {
            return Ok(());
        }
        e.propose(new, "sftp")?;
        // a change that only lets more in cannot lock anyone out and is kept at once; closing keeps the usual confirmation unless it is ours alone
        let _ = e.keep("sftp");
        Ok(())
    }
}

impl Firewall for RealFirewall {
    fn open(&self, port: u16, expires: i64) -> Result<(), String> {
        Self::apply(Some((port, expires)))
    }
    fn close(&self) -> Result<(), String> {
        Self::apply(None)
    }
}

// ------------------------------------------------------------------ the manager

struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    port: u16,
    until: i64,
    by: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub running: bool,
    pub port: u16,
    pub until: i64,
    pub by: String,
    pub config: Config,
    pub host_fingerprint: String,
    pub keys: Vec<(String, Vec<String>)>,
    pub addresses: Vec<String>,
}

pub struct Sftp {
    dir: PathBuf,
    auth: Arc<dyn Authenticator>,
    audit: Audit,
    fw: Arc<dyn Firewall>,
    running: Mutex<Option<Running>>,
    limiter: Arc<Mutex<Limiter>>,
    home: Option<PathBuf>,
}

fn config_path(dir: &Path) -> PathBuf {
    dir.join("sftp").join("config.json")
}

pub fn load_config(dir: &Path) -> Config {
    std::fs::read_to_string(config_path(dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_config(dir: &Path, c: &Config) -> Result<(), String> {
    write_private(&config_path(dir), &serde_json::to_string_pretty(c).map_err(|e| e.to_string())?)
}

static SHARED: Mutex<Option<Arc<Sftp>>> = Mutex::new(None);
/// The port of the running server of this process (0: not running); lets a page show the state without owning the manager.
static LISTENING: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The lines Settings -> Remote access shows about the SFTP server: state, port, login way, accounts and how to change it. Reads the
/// settings file and the process state only; no secrets. (Changing it is by the assistant's `sftp_set` tool, which asks.)
pub fn status_lines(dir: &Path) -> Vec<String> {
    let c = load_config(dir);
    let port = LISTENING.load(Ordering::Relaxed);
    let mut v = vec![if port != 0 { format!("SFTP file server: ON at port {port} (local network only)") } else { "SFTP file server: off, nothing listens".to_string() }];
    v.push(format!("Login: {}. Accounts allowed: {}.", if c.password_login { "keys or account password" } else { "keys only" }, if c.users.is_empty() { "none".to_string() } else { c.users.join(", ") }));
    v.push("Each account sees only what its own file rules allow. Ask the assistant to turn it on or off (it asks you first); it stops by itself after the chosen time.".into());
    v
}

impl Sftp {
    pub fn new(dir: &Path, auth: Arc<dyn Authenticator>, audit: Audit, fw: Arc<dyn Firewall>) -> Sftp {
        Sftp { dir: dir.to_path_buf(), auth, audit, fw, running: Mutex::new(None), limiter: Arc::new(Mutex::new(Limiter::default())), home: None }
    }

    /// Use this folder as every account's home (tests and portable set-ups).
    pub fn with_home(mut self, home: PathBuf) -> Sftp {
        self.home = Some(home);
        self
    }

    /// The server of this process (made on first use, so every window talks to the same listener).
    pub fn shared(dir: &Path, auth: Arc<dyn Authenticator>, audit: Audit) -> Arc<Sftp> {
        let mut g = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(|| Arc::new(Sftp::new(dir, auth, audit, Arc::new(RealFirewall)))).clone()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn config(&self) -> Config {
        load_config(&self.dir)
    }

    fn log(&self, e: &str, d: &str) {
        (self.audit)(e, d);
    }

    pub fn status(&self) -> Status {
        self.tick();
        let cfg = self.config();
        let run = self.running.lock().unwrap_or_else(|e| e.into_inner());
        let fp = host_key(&self.dir).map(|k| k.fingerprint()).unwrap_or_default();
        let keys = cfg.users.iter().map(|u| (u.clone(), user_keys(&self.dir, u).iter().map(|(b, _)| fingerprint_of_blob(b)).collect())).collect();
        let port = run.as_ref().map_or(cfg.port, |r| r.port);
        let addresses = if run.is_some() { crate::sharedfolder::lan_addresses() } else { Vec::new() };
        Status { running: run.is_some(), port, until: run.as_ref().map_or(0, |r| r.until), by: run.as_ref().map(|r| r.by.clone()).unwrap_or_default(), config: cfg, host_fingerprint: fp, keys, addresses }
    }

    /// Change the settings (not the running state). Account names must be valid; the port must be allowed.
    pub fn configure(&self, f: impl FnOnce(&mut Config) -> Result<(), String>) -> Result<Config, String> {
        let mut c = self.config();
        f(&mut c)?;
        valid_port(c.port).or_else(|e| if c.port == 0 && cfg!(test) { Ok(0) } else { Err(e) })?;
        if c.users.iter().any(|u| !iyakki_sec::policy::valid_username(u)) || c.users.len() > 32 {
            return Err("the allow-list holds account names (at most 32)".into());
        }
        c.users.sort();
        c.users.dedup();
        save_config(&self.dir, &c)?;
        Ok(c)
    }

    /// Why the server could not be useful now (no way to log in), or `None`.
    pub fn problem(&self) -> Option<String> {
        let c = self.config();
        if c.users.is_empty() {
            return Some("no account is allowed to log in yet: allow an account first".into());
        }
        if !c.password_login && c.users.iter().all(|u| user_keys(&self.dir, u).is_empty()) {
            return Some("nobody could log in: add a public key for an allowed account, or allow password login".into());
        }
        None
    }

    pub fn enable(&self, by: &str) -> Result<Status, String> {
        self.tick();
        if self.running.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
            return Err("the SFTP server is already running".into());
        }
        if let Some(p) = self.problem() {
            return Err(p);
        }
        let cfg = self.config();
        let key = host_key(&self.dir)?;
        let listener = TcpListener::bind(("0.0.0.0", cfg.port)).map_err(|e| format!("cannot listen on port {}: {e}", cfg.port))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let now = crate::clock::now_unix();
        let until = if cfg.expiry_secs == 0 { 0 } else { now + cfg.expiry_secs as i64 };
        if let Err(e) = self.fw.open(port, until) {
            return Err(format!("the firewall would not open the port: {e}"));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(RealGate { dir: self.dir.clone(), cfg: cfg.clone(), auth: self.auth.clone(), audit: self.audit.clone(), limiter: self.limiter.clone(), home: self.home.clone() });
        let (stop2, limiter) = (stop.clone(), self.limiter.clone());
        let thread = std::thread::spawn(move || accept_loop(listener, key, gate, stop2, limiter, until));
        LISTENING.store(port as u32, Ordering::Relaxed);
        *self.running.lock().unwrap_or_else(|e| e.into_inner()) = Some(Running { stop, thread: Some(thread), port, until, by: by.to_string() });
        self.log("sftp.enable", &format!("by {by}, port {port}, {}", if until == 0 { "until stopped".to_string() } else { format!("until {until}") }));
        Ok(self.status())
    }

    pub fn disable(&self, by: &str) -> bool {
        let run = self.running.lock().unwrap_or_else(|e| e.into_inner()).take();
        LISTENING.store(0, Ordering::Relaxed);
        let Some(mut r) = run else {
            // a rule left behind by a crash is removed even when nothing runs
            let _ = self.fw.close();
            return false;
        };
        r.stop.store(true, Ordering::Relaxed);
        if let Some(t) = r.thread.take() {
            let _ = t.join();
        }
        let _ = self.fw.close();
        self.log("sftp.disable", &format!("by {by}"));
        true
    }

    /// Stop by itself when the time is up.
    pub fn tick(&self) {
        let expired = self.running.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|r| r.until != 0 && crate::clock::now_unix() >= r.until);
        if expired {
            self.disable("expiry");
        }
    }
}

fn accept_loop(listener: TcpListener, key: KeyPair, gate: Arc<RealGate>, stop: Arc<AtomicBool>, limiter: Arc<Mutex<Limiter>>, until: i64) {
    let live = Arc::new(AtomicUsize::new(0));
    while !stop.load(Ordering::Relaxed) {
        if until != 0 && crate::clock::now_unix() >= until {
            stop.store(true, Ordering::Relaxed);
            LISTENING.store(0, Ordering::Relaxed);
            break;
        }
        match listener.accept() {
            Ok((s, peer)) => {
                let now = crate::clock::now_unix();
                if !ssh_server::peer_allowed(&peer) {
                    gate.audit("sftp.refused_address", &peer.ip().to_string());
                    continue;
                }
                if limiter.lock().unwrap_or_else(|e| e.into_inner()).blocked(peer.ip(), now) || live.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    continue;
                }
                let _ = s.set_nonblocking(false);
                if ssh_server::prepare(&s).is_err() {
                    continue;
                }
                live.fetch_add(1, Ordering::Relaxed);
                let (key, gate, stop, live) = (key.clone(), gate.clone(), stop.clone(), live.clone());
                std::thread::spawn(move || {
                    let srv = Server { host_key: &key, gate: &*gate, peer: peer.ip() };
                    let _ = ssh_server::serve(s, &srv, stop);
                    live.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}
