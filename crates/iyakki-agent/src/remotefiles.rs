//! The remote files client (F-FILE-02): saved connections, sessions, host key pins and the transfer queue behind the REMOTE FILES tile
//! (`crates/iyakki-tile-remotefiles`) and the assistant's tools (`remotefilestools.rs`). One [`Manager`] per signed-in person, shared by
//! both, so a session the person opened in the tile can be listed by the assistant and the other way round.
//!
//! * Protocols: SFTP over the in-house SSH client (`rf_ssh.rs`, `rf_sftp.rs`), FTPS with explicit TLS and plain FTP (`rf_ftp.rs`). Plain
//!   FTP only for a connection on which the person ticked the warning (`Conn::plain_ok`).
//! * Secrets: a password lives in the person's vault under `rf.<name>`; a key is a vault key made in the CLI tile (`ssh.<x>`). The saved
//!   connection list (`<data>/remotefiles/<user>/connections.json`, mode 0600) holds no secret.
//! * Host keys: pinned on first use. `open` ends with [`Opened::Unknown`] (a card for the person) or [`Opened::Changed`] (refused), before
//!   any password or key signature is sent; only [`Manager::trust`] writes a pin, and it only accepts the fingerprint the server showed.
//!   The assistant has no tool that trusts a key.
//! * Transfers run on their own connections (so browsing stays quick), at most two at a time, with progress, cancel, and resume:
//!   downloads go to `name.part` and are renamed when complete; uploads go to `name.part` on the server and are renamed there.
//! * Local files obey the person's file rules through the `local` check the caller supplies.

use std::collections::HashMap;
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::sshkey::KeyPair;
pub use iyakki_sshfs::remotefs::{clean_path, Entry, RemoteFs};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Proto {
    #[default]
    Sftp,
    Ftps,
    Ftp,
}

impl Proto {
    pub fn label(self) -> &'static str {
        match self {
            Proto::Sftp => "SFTP",
            Proto::Ftps => "FTPS",
            Proto::Ftp => "FTP (plain)",
        }
    }
    pub fn default_port(self) -> u16 {
        if self == Proto::Sftp { 22 } else { 21 }
    }
    pub fn parse(s: &str) -> Option<Proto> {
        match s.to_ascii_lowercase().as_str() {
            "sftp" => Some(Proto::Sftp),
            "ftps" => Some(Proto::Ftps),
            "ftp" => Some(Proto::Ftp),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Auth {
    #[default]
    Password,
    Key,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Conn {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub proto: Proto,
    pub auth: Auth,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub plain_ok: bool,
    /// Filled in when listing: a password is in the vault.
    #[serde(skip)]
    pub has_password: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostCard {
    pub host: String,
    pub port: u16,
    pub kind: String,
    pub fingerprint: String,
    pub pinned: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opened {
    Ready(u64),
    Unknown(HostCard),
    Changed(HostCard),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    Open(String),
    List(u64, String),
    Mkdir(u64, String),
    Rename(u64, String, String),
    Delete(u64, String, bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobResult {
    Opened(Opened),
    Listing(String, Vec<Entry>),
    Done(String),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferReq {
    pub session: u64,
    pub download: bool,
    pub remote: String,
    pub local: String,
    pub overwrite: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl State {
    pub fn finished(self) -> bool {
        matches!(self, State::Done | State::Failed | State::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferInfo {
    pub id: u64,
    pub download: bool,
    pub remote: String,
    pub local: String,
    pub done: u64,
    pub total: u64,
    pub state: State,
    pub note: String,
    pub rate: u64,
    pub resumed: bool,
}

pub enum Cred {
    Password(Zeroizing<String>),
    Key(Box<KeyPair>),
}

pub enum ConnectErr {
    /// Not the pinned identity (or none pinned): the kind and fingerprint it showed.
    Untrusted { kind: String, fingerprint: String },
    Auth(String),
    Other(String),
}

pub trait Connector: Send + Sync {
    /// `pin` is the pinned fingerprint of this server, if any.
    fn connect(&self, conn: &Conn, cred: &Cred, pin: Option<&str>, stop: Arc<AtomicBool>) -> Result<Box<dyn RemoteFs>, ConnectErr>;
}

pub trait Secrets: Send + Sync {
    fn names(&self) -> Vec<String>;
    fn get(&self, name: &str) -> Result<Zeroizing<String>, String>;
    fn set(&self, name: &str, value: &str) -> Result<(), String>;
    fn delete(&self, name: &str) -> Result<(), String>;
}

/// The person's vault.
pub struct VaultSecrets {
    pub svc: Arc<crate::services::Services>,
    pub user: String,
}

impl Secrets for VaultSecrets {
    fn names(&self) -> Vec<String> {
        self.svc.user_names(&self.user).unwrap_or_default()
    }
    fn get(&self, name: &str) -> Result<Zeroizing<String>, String> {
        self.svc.user_get(&self.user, name).map_err(|e| format!("could not read `{name}` from your vault: {e}"))
    }
    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.svc.user_set(&self.user, name, value).map_err(|e| format!("could not save to your vault: {e}"))
    }
    fn delete(&self, name: &str) -> Result<(), String> {
        self.svc.user_delete(&self.user, name).map_err(|e| format!("could not remove from your vault: {e}"))
    }
}

/// The real network.
pub struct NetConnector;

fn tcp(host: &str, port: u16, stop: &AtomicBool) -> Result<std::net::TcpStream, String> {
    crate::rf_ftp::connect_tcp(host, port, stop)
}

impl Connector for NetConnector {
    fn connect(&self, conn: &Conn, cred: &Cred, pin: Option<&str>, stop: Arc<AtomicBool>) -> Result<Box<dyn RemoteFs>, ConnectErr> {
        match conn.proto {
            Proto::Sftp => {
                let s = tcp(&conn.host, conn.port, &stop).map_err(ConnectErr::Other)?;
                let (pw, key) = match cred {
                    Cred::Password(p) => (Some(p.as_str()), None),
                    Cred::Key(k) => (None, Some(k.as_ref())),
                };
                let ssh = crate::rf_ssh::connect(s, &conn.user, pw, key, &|_k, fp| pin == Some(fp), stop).map_err(|e| match e {
                    crate::rf_ssh::SshErr::HostKey { kind, fingerprint } => ConnectErr::Untrusted { kind, fingerprint },
                    crate::rf_ssh::SshErr::Auth(m) => ConnectErr::Auth(m),
                    crate::rf_ssh::SshErr::Other(m) => ConnectErr::Other(m),
                })?;
                Ok(Box::new(crate::rf_sftp::SftpFs::start(ssh).map_err(ConnectErr::Other)?))
            }
            Proto::Ftps | Proto::Ftp => {
                let pw = match cred {
                    Cred::Password(p) => p.as_str(),
                    Cred::Key(_) => return Err(ConnectErr::Other("keys are for SFTP".into())),
                };
                let f = crate::rf_ftp::connect(&conn.host, conn.port, &conn.user, pw, conn.proto == Proto::Ftps, &|fp| pin == Some(fp), stop).map_err(|e| match e {
                    crate::rf_ftp::FtpErr::Cert { fingerprint } => ConnectErr::Untrusted { kind: "tls-certificate".into(), fingerprint },
                    crate::rf_ftp::FtpErr::Auth(m) => ConnectErr::Auth(m),
                    crate::rf_ftp::FtpErr::Other(m) => ConnectErr::Other(m),
                })?;
                Ok(Box::new(f))
            }
        }
    }
}

// ------------------------------------------------------------------ paths and names

pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

pub fn parent(p: &str) -> String {
    let c = clean_path(p);
    match c.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => c[..i].to_string(),
    }
}

fn valid_host(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && !h.starts_with(['-', '.']) && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
}

/// The vault name of a connection's password: `rf.<lower-case name with - for spaces>`.
pub fn pw_name(conn: &str) -> String {
    let n: String = conn.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect();
    format!("rf.{n}")
}

impl Conn {
    pub fn validate(&self) -> Result<(), String> {
        let n = &self.name;
        if n.is_empty() || n.len() > 40 || !n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ')) || n.starts_with(' ') || n.ends_with(' ') {
            return Err("the name is 1 to 40 letters, digits, spaces, `.`, `_` or `-`".into());
        }
        if !valid_host(&self.host) {
            return Err(format!("`{}` is not a host name or address", self.host));
        }
        if self.port == 0 {
            return Err("the port is 1 to 65535".into());
        }
        if self.user.is_empty() || self.user.len() > 64 || self.user.contains(|c: char| c.is_control() || c.is_whitespace()) {
            return Err("the user name is required (up to 64 characters, no spaces)".into());
        }
        if self.proto == Proto::Ftp && !self.plain_ok {
            return Err("plain FTP sends the password and the files unencrypted; it needs the warning accepted on the connection. Use SFTP or FTPS if the server offers them.".into());
        }
        match self.auth {
            Auth::Key if self.proto != Proto::Sftp => return Err("keys are for SFTP; FTP and FTPS use a password".into()),
            Auth::Key if !crate::ssh::valid_key_name(&self.key) => return Err("choose a key from your vault (named ssh.<name>)".into()),
            _ => {}
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ files

fn write_private(path: &std::path::Path, data: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("could not create {}: {e}", d.display()))?;
        let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700));
    }
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    f.write_all(data).map_err(|e| e.to_string())?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Pin {
    kind: String,
    fingerprint: String,
}

fn pin_key(c: &Conn) -> String {
    format!("{}:{}:{}", if c.proto == Proto::Sftp { "sftp" } else { "ftps" }, c.host.to_ascii_lowercase(), c.port)
}

/// Check a path on this computer against the person's file rules: the real path to use, or why not. `true`: it will be written.
pub type LocalCheck = Arc<dyn Fn(&str, bool) -> Result<PathBuf, String> + Send + Sync>;

struct Hooks {
    secrets: Arc<dyn Secrets>,
    local: LocalCheck,
    audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

struct Session {
    conn: Conn,
    fs: Mutex<Box<dyn RemoteFs>>,
}

struct Tr {
    id: u64,
    conn: Conn,
    req: TransferReq,
    st: Mutex<TrState>,
    cancel: Arc<AtomicBool>,
}

struct TrState {
    state: State,
    done: u64,
    total: u64,
    note: String,
    rate: u64,
    resumed: bool,
    mark: Option<(Instant, u64)>,
}

#[derive(Default)]
struct Inner {
    sessions: HashMap<u64, Arc<Session>>,
    jobs: HashMap<u64, Option<JobResult>>,
    transfers: Vec<Arc<Tr>>,
    seen: HashMap<String, (String, String)>,
    next: u64,
}

pub struct Manager {
    dir: PathBuf,
    connector: Arc<dyn Connector>,
    hooks: Mutex<Arc<Hooks>>,
    download_dir: Mutex<String>,
    inner: Mutex<Inner>,
    stop: Arc<AtomicBool>,
}

const MAX_SESSIONS: usize = 8;
const MAX_RUNNING: usize = 2;
const MAX_TRANSFERS: usize = 200;

fn registry() -> &'static Mutex<HashMap<String, Arc<Manager>>> {
    static R: OnceLock<Mutex<HashMap<String, Arc<Manager>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// The person's manager (made the first time, shared afterwards). The hooks are replaced on every call, so they always follow the
/// person's current vault and file rules.
pub fn shared(data: &std::path::Path, user: &str, secrets: Arc<dyn Secrets>, local: LocalCheck, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, download_dir: &str) -> Arc<Manager> {
    let key = format!("{}\u{0}{}", data.display(), user);
    let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
    let m = r.entry(key).or_insert_with(|| Arc::new(Manager::new(data, user, Arc::new(NetConnector), secrets.clone(), local.clone(), audit.clone(), download_dir)));
    *m.hooks.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(Hooks { secrets, local, audit });
    *m.download_dir.lock().unwrap_or_else(|e| e.into_inner()) = download_dir.to_string();
    m.clone()
}

impl Manager {
    pub fn new(data: &std::path::Path, user: &str, connector: Arc<dyn Connector>, secrets: Arc<dyn Secrets>, local: LocalCheck, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, download_dir: &str) -> Manager {
        let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).collect();
        Manager { dir: data.join("remotefiles").join(safe), connector, hooks: Mutex::new(Arc::new(Hooks { secrets, local, audit })), download_dir: Mutex::new(download_dir.into()), inner: Mutex::new(Inner { next: 1, ..Default::default() }), stop: Arc::new(AtomicBool::new(false)) }
    }

    fn hooks(&self) -> Arc<Hooks> {
        self.hooks.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn audit(&self, e: &str, d: &str) {
        (self.hooks().audit)(e, d);
    }

    pub fn download_dir(&self) -> String {
        self.download_dir.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    // -------------------------------------------------------------- saved connections

    fn conns_path(&self) -> PathBuf {
        self.dir.join("connections.json")
    }
    fn pins_path(&self) -> PathBuf {
        self.dir.join("hostpins.json")
    }

    fn read_conns(&self) -> Vec<Conn> {
        let mut v: Vec<Conn> = std::fs::read(self.conns_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        v.sort_by_key(|c| c.name.to_ascii_lowercase());
        v
    }

    pub fn connections(&self) -> Vec<Conn> {
        let names = self.hooks().secrets.names();
        self.read_conns().into_iter().map(|c| Conn { has_password: names.contains(&pw_name(&c.name)), ..c }).collect()
    }

    pub fn connection(&self, name: &str) -> Option<Conn> {
        self.connections().into_iter().find(|c| c.name == name)
    }

    pub fn key_names(&self) -> Vec<String> {
        self.hooks().secrets.names().into_iter().filter(|n| crate::ssh::valid_key_name(n)).collect()
    }

    /// `password`: None keeps what is stored, Some("") removes it, Some(text) stores it in the vault.
    pub fn save(&self, c: &Conn, password: Option<&str>, replacing: Option<&str>) -> Result<(), String> {
        let c = Conn { host: c.host.trim().to_ascii_lowercase(), name: c.name.trim().to_string(), plain_ok: c.proto == Proto::Ftp && c.plain_ok, ..c.clone() };
        c.validate()?;
        let mut all = self.read_conns();
        let norm = pw_name(&c.name);
        if all.iter().any(|x| (x.name.eq_ignore_ascii_case(&c.name) || pw_name(&x.name) == norm) && Some(x.name.as_str()) != replacing) {
            return Err(format!("there is already a connection called {}", c.name));
        }
        if let Some(old) = replacing {
            if !all.iter().any(|x| x.name == old) {
                return Err(format!("no connection called {old}"));
            }
        } else if all.len() >= 200 {
            return Err("at most 200 saved connections".into());
        }
        let secrets = self.hooks().secrets.clone();
        match password {
            Some(p) if !p.is_empty() => {
                if p.len() > 1024 || p.contains(['\r', '\n', '\0']) {
                    return Err("the password is one line of at most 1024 characters".into());
                }
                if c.auth != Auth::Password {
                    return Err("this connection signs in with a key, not a password".into());
                }
                secrets.set(&norm, p)?;
            }
            Some(_) => {
                let _ = secrets.delete(&norm);
            }
            None => {}
        }
        if let Some(old) = replacing {
            let old_pw = pw_name(old);
            if old_pw != norm && password.is_none() {
                if let Ok(v) = secrets.get(&old_pw) {
                    secrets.set(&norm, &v)?;
                }
            }
            if old_pw != norm {
                let _ = secrets.delete(&old_pw);
            }
            all.retain(|x| x.name != old);
        }
        all.push(c.clone());
        write_private(&self.conns_path(), &serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?)?;
        self.audit("remotefiles.save", &format!("{} {} {}@{}:{}", c.name, c.proto.label(), c.user, c.host, c.port));
        Ok(())
    }

    pub fn remove(&self, name: &str) -> Result<(), String> {
        let mut all = self.read_conns();
        let n = all.len();
        all.retain(|x| x.name != name);
        if all.len() == n {
            return Err(format!("no connection called {name}"));
        }
        write_private(&self.conns_path(), &serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?)?;
        let _ = self.hooks().secrets.delete(&pw_name(name));
        self.audit("remotefiles.remove", name);
        Ok(())
    }

    // -------------------------------------------------------------- host pins

    fn pins(&self) -> HashMap<String, Pin> {
        std::fs::read(self.pins_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Pin the identity the server showed last time for this connection, if `fingerprint` is exactly that.
    pub fn trust(&self, name: &str, fingerprint: &str) -> Result<(), String> {
        let c = self.connection(name).ok_or_else(|| format!("no connection called {name}"))?;
        let seen = self.inner().seen.get(name).cloned().ok_or("the server has not shown its identity yet; connect first")?;
        if seen.1 != fingerprint {
            return Err("the server shows a different identity than the one you confirmed".into());
        }
        let mut pins = self.pins();
        if pins.get(&pin_key(&c)).is_some_and(|p| p.fingerprint != seen.1) {
            return Err("the server's identity changed; forget the old one first (only if you know why it changed)".into());
        }
        pins.insert(pin_key(&c), Pin { kind: seen.0.clone(), fingerprint: seen.1.clone() });
        write_private(&self.pins_path(), &serde_json::to_vec_pretty(&pins).map_err(|e| e.to_string())?)?;
        self.audit("remotefiles.trust", &format!("{} {} {}", name, seen.0, seen.1));
        Ok(())
    }

    pub fn forget_host(&self, name: &str) -> Result<(), String> {
        let c = self.connection(name).ok_or_else(|| format!("no connection called {name}"))?;
        let mut pins = self.pins();
        if pins.remove(&pin_key(&c)).is_none() {
            return Err("no identity is pinned for this connection".into());
        }
        write_private(&self.pins_path(), &serde_json::to_vec_pretty(&pins).map_err(|e| e.to_string())?)?;
        self.inner().seen.remove(name);
        self.audit("remotefiles.forget_host", name);
        Ok(())
    }

    // -------------------------------------------------------------- sessions

    fn cred(&self, c: &Conn) -> Result<Cred, String> {
        let secrets = self.hooks().secrets.clone();
        match c.auth {
            Auth::Password => match secrets.get(&pw_name(&c.name)) {
                Ok(p) => Ok(Cred::Password(p)),
                Err(_) if c.proto != Proto::Sftp && matches!(c.user.as_str(), "anonymous" | "ftp") => Ok(Cred::Password(Zeroizing::new("anonymous@".into()))),
                Err(_) => Err(format!("no password is stored for {}: edit the connection and enter one", c.name)),
            },
            Auth::Key => {
                let text = secrets.get(&c.key)?;
                let pass = secrets.get(&format!("{}.passphrase", c.key)).ok();
                KeyPair::from_openssh(&text, pass.as_ref().map(|p| p.as_str())).map(|k| Cred::Key(Box::new(k))).map_err(|e| format!("the key {}: {e}", c.key))
            }
        }
    }

    fn connect(&self, c: &Conn, cred: &Cred) -> Result<Result<Box<dyn RemoteFs>, (String, String)>, String> {
        let pin = self.pins().get(&pin_key(c)).map(|p| p.fingerprint.clone());
        match self.connector.connect(c, cred, pin.as_deref(), self.stop.clone()) {
            Ok(fs) => Ok(Ok(fs)),
            Err(ConnectErr::Untrusted { kind, fingerprint }) => Ok(Err((kind, fingerprint))),
            Err(ConnectErr::Auth(m)) | Err(ConnectErr::Other(m)) => Err(m),
        }
    }

    /// Open a session on a saved connection. Blocks until connected; the tile and the tools call it on a worker thread.
    pub fn open(&self, name: &str) -> Result<Opened, String> {
        let c = self.connection(name).ok_or_else(|| format!("no connection called {name}"))?;
        if self.inner().sessions.len() >= MAX_SESSIONS {
            return Err("too many open connections: close one first".into());
        }
        let cred = self.cred(&c)?;
        match self.connect(&c, &cred)? {
            Ok(fs) => {
                let mut i = self.inner();
                let id = i.next;
                i.next += 1;
                i.sessions.insert(id, Arc::new(Session { conn: c.clone(), fs: Mutex::new(fs) }));
                drop(i);
                self.audit("remotefiles.open", &format!("{} {}@{}:{}", c.name, c.user, c.host, c.port));
                Ok(Opened::Ready(id))
            }
            Err((kind, fingerprint)) => {
                let pinned = self.pins().get(&pin_key(&c)).map(|p| p.fingerprint.clone());
                self.inner().seen.insert(name.to_string(), (kind.clone(), fingerprint.clone()));
                let card = HostCard { host: c.host.clone(), port: c.port, kind, fingerprint: fingerprint.clone(), pinned: pinned.clone() };
                if pinned.is_some() {
                    self.audit("remotefiles.identity_changed", &format!("{} now {}", c.name, fingerprint));
                    Ok(Opened::Changed(card))
                } else {
                    Ok(Opened::Unknown(card))
                }
            }
        }
    }

    fn session(&self, id: u64) -> Result<Arc<Session>, String> {
        self.inner().sessions.get(&id).cloned().ok_or_else(|| "that connection is closed".to_string())
    }

    pub fn close(&self, id: u64) {
        self.inner().sessions.remove(&id);
    }

    pub fn sessions(&self) -> Vec<(u64, String)> {
        let mut v: Vec<(u64, String)> = self.inner().sessions.iter().map(|(i, s)| (*i, s.conn.name.clone())).collect();
        v.sort();
        v
    }

    fn with_fs<T>(&self, id: u64, f: impl FnOnce(&mut dyn RemoteFs) -> Result<T, String>) -> Result<T, String> {
        let s = self.session(id)?;
        let mut g = s.fs.lock().unwrap_or_else(|e| e.into_inner());
        let r = f(g.as_mut());
        if let Err(e) = &r {
            if e.contains("connection") && (e.contains("lost") || e.contains("closed")) {
                drop(g);
                self.close(id);
                return Err(format!("{e}; the connection was closed, open it again"));
            }
        }
        r
    }

    pub fn list(&self, id: u64, path: &str) -> Result<(String, Vec<Entry>), String> {
        check_remote(path)?;
        self.with_fs(id, |fs| fs.list(path))
    }

    pub fn mkdir(&self, id: u64, path: &str) -> Result<(), String> {
        check_remote(path)?;
        self.with_fs(id, |fs| fs.mkdir(path))?;
        self.audit("remotefiles.mkdir", path);
        Ok(())
    }

    pub fn rename(&self, id: u64, from: &str, to: &str) -> Result<(), String> {
        check_remote(from)?;
        check_remote(to)?;
        self.with_fs(id, |fs| fs.rename(from, to))?;
        self.audit("remotefiles.rename", &format!("{from} -> {to}"));
        Ok(())
    }

    pub fn delete(&self, id: u64, path: &str, dir: bool) -> Result<(), String> {
        check_remote(path)?;
        if clean_path(path) == "/" {
            return Err("the root folder is not deleted".into());
        }
        self.with_fs(id, |fs| if dir { fs.remove_dir(path) } else { fs.remove_file(path) })?;
        self.audit("remotefiles.delete", path);
        Ok(())
    }

    /// Start a job on a worker thread; poll [`Manager::take`] with the ticket.
    pub fn submit(self: &Arc<Self>, job: Job) -> u64 {
        let ticket = {
            let mut i = self.inner();
            if i.jobs.len() > 64 {
                // old answers nobody collected
                let keep: Vec<u64> = i.jobs.iter().filter(|(_, v)| v.is_none()).map(|(k, _)| *k).collect();
                i.jobs.retain(|k, _| keep.contains(k));
            }
            let t = i.next;
            i.next += 1;
            i.jobs.insert(t, None);
            t
        };
        let m = self.clone();
        let spawned = std::thread::Builder::new().name("remotefiles-job".into()).spawn(move || {
            let r = match &job {
                Job::Open(n) => m.open(n).map(JobResult::Opened),
                Job::List(s, p) => m.list(*s, p).map(|(p, e)| JobResult::Listing(p, e)),
                Job::Mkdir(s, p) => m.mkdir(*s, p).map(|_| JobResult::Done(format!("Made the folder {p}"))),
                Job::Rename(s, a, b) => m.rename(*s, a, b).map(|_| JobResult::Done("Renamed".into())),
                Job::Delete(s, p, d) => m.delete(*s, p, *d).map(|_| JobResult::Done(format!("Deleted {p}"))),
            };
            m.inner().jobs.insert(ticket, Some(r.unwrap_or_else(JobResult::Failed)));
        });
        if spawned.is_err() {
            self.inner().jobs.insert(ticket, Some(JobResult::Failed("could not start a worker".into())));
        }
        ticket
    }

    pub fn take(&self, ticket: u64) -> Option<JobResult> {
        let mut i = self.inner();
        match i.jobs.get(&ticket) {
            Some(Some(_)) => i.jobs.remove(&ticket).flatten(),
            Some(None) => None,
            None => Some(JobResult::Failed("that request is gone".into())),
        }
    }

    // -------------------------------------------------------------- transfers

    pub fn start_transfer(self: &Arc<Self>, req: &TransferReq) -> Result<u64, String> {
        let s = self.session(req.session)?;
        check_remote(&req.remote)?;
        let hooks = self.hooks();
        // refuse early what the person's file rules refuse
        (hooks.local)(&req.local, req.download).map_err(|e| format!("{}: {e}", req.local))?;
        let id = {
            let mut i = self.inner();
            if i.transfers.iter().filter(|t| !t.st.lock().map(|s| s.state.finished()).unwrap_or(true)).count() >= MAX_TRANSFERS {
                return Err("too many transfers are waiting".into());
            }
            if i.transfers.len() >= MAX_TRANSFERS {
                i.transfers.retain(|t| !t.st.lock().map(|s| s.state.finished()).unwrap_or(true));
            }
            let id = i.next;
            i.next += 1;
            i.transfers.push(Arc::new(Tr { id, conn: s.conn.clone(), req: req.clone(), st: Mutex::new(TrState { state: State::Queued, done: 0, total: 0, note: String::new(), rate: 0, resumed: false, mark: None }), cancel: Arc::new(AtomicBool::new(false)) }));
            id
        };
        self.audit(if req.download { "remotefiles.download" } else { "remotefiles.upload" }, &format!("{} {} -> {}", s.conn.name, if req.download { &req.remote } else { &req.local }, if req.download { &req.local } else { &req.remote }));
        self.pump();
        Ok(id)
    }

    fn pump(self: &Arc<Self>) {
        let next = {
            let i = self.inner();
            let running = i.transfers.iter().filter(|t| t.st.lock().map(|s| s.state == State::Running).unwrap_or(false)).count();
            if running >= MAX_RUNNING {
                return;
            }
            i.transfers.iter().find(|t| t.st.lock().map(|s| s.state == State::Queued).unwrap_or(false)).cloned()
        };
        let Some(t) = next else { return };
        if let Ok(mut s) = t.st.lock() {
            s.state = State::Running;
        }
        let m = self.clone();
        let t2 = t.clone();
        if std::thread::Builder::new().name("remotefiles-transfer".into()).spawn(move || {
            let r = m.run_transfer(&t2);
            if let Ok(mut s) = t2.st.lock() {
                match r {
                    Ok(()) => {
                        s.state = State::Done;
                        s.done = s.total.max(s.done);
                        s.rate = 0;
                    }
                    Err(e) if e == "cancelled" => {
                        s.state = State::Cancelled;
                        s.note = "cancelled; the part already moved is kept and can be resumed".into();
                    }
                    Err(e) => {
                        s.state = State::Failed;
                        s.note = e;
                    }
                }
            }
            m.pump();
        })
        .is_err()
        {
            if let Ok(mut s) = t.st.lock() {
                s.state = State::Failed;
                s.note = "could not start a worker".into();
            }
        }
    }

    pub fn transfers(&self) -> Vec<TransferInfo> {
        self.inner()
            .transfers
            .iter()
            .map(|t| {
                let s = t.st.lock().unwrap_or_else(|e| e.into_inner());
                TransferInfo { id: t.id, download: t.req.download, remote: t.req.remote.clone(), local: t.req.local.clone(), done: s.done, total: s.total, state: s.state, note: s.note.clone(), rate: s.rate, resumed: s.resumed }
            })
            .collect()
    }

    pub fn cancel_transfer(&self, id: u64) {
        for t in self.inner().transfers.iter().filter(|t| t.id == id) {
            t.cancel.store(true, Ordering::Relaxed);
            if let Ok(mut s) = t.st.lock() {
                if s.state == State::Queued {
                    s.state = State::Cancelled;
                    s.note = "cancelled".into();
                }
            }
        }
    }

    pub fn retry_transfer(self: &Arc<Self>, id: u64) -> Result<u64, String> {
        let t = self.inner().transfers.iter().find(|t| t.id == id).cloned().ok_or("no such transfer")?;
        if !t.st.lock().map(|s| matches!(s.state, State::Failed | State::Cancelled)).unwrap_or(false) {
            return Err("only a failed or cancelled transfer can be continued".into());
        }
        self.start_transfer(&t.req)
    }

    pub fn clear_finished(&self) {
        self.inner().transfers.retain(|t| !t.st.lock().map(|s| s.state.finished()).unwrap_or(false));
    }

    fn progress(t: &Tr, n: u64) -> bool {
        if let Ok(mut s) = t.st.lock() {
            s.done += n;
            let now = Instant::now();
            match s.mark {
                None => s.mark = Some((now, s.done)),
                Some((at, b)) if now.duration_since(at).as_millis() >= 1000 => {
                    let secs = now.duration_since(at).as_secs_f64().max(0.001);
                    s.rate = ((s.done - b) as f64 / secs) as u64;
                    s.mark = Some((now, s.done));
                }
                _ => {}
            }
        }
        !t.cancel.load(Ordering::Relaxed)
    }

    fn run_transfer(&self, t: &Tr) -> Result<(), String> {
        if t.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let cred = self.cred(&t.conn)?;
        let mut fs = match self.connect(&t.conn, &cred)? {
            Ok(fs) => fs,
            Err(_) => return Err("the server's identity is not the one you trusted; open the connection again".into()),
        };
        let local = (self.hooks().local)(&t.req.local, t.req.download).map_err(|e| format!("{}: {e}", t.req.local))?;
        let set = |f: &dyn Fn(&mut TrState)| {
            if let Ok(mut s) = t.st.lock() {
                f(&mut s)
            }
        };
        if t.req.download {
            let total = fs.size(&t.req.remote)?.ok_or_else(|| format!("{} is not on the server", t.req.remote))?;
            set(&|s| s.total = total);
            let part = part_of(&local)?;
            if local.exists() && !t.req.overwrite {
                return Err(format!("{} already exists here; choose Replace to overwrite it", local.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
            }
            if std::fs::symlink_metadata(&local).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err("the target is a link; it is not written through".into());
            }
            let have = match std::fs::symlink_metadata(&part) {
                Ok(m) if m.file_type().is_symlink() || !m.is_file() => return Err("the .part file is not a plain file".into()),
                Ok(m) => m.len(),
                Err(_) => 0,
            };
            let offset = if have > 0 && have <= total { have } else { 0 };
            set(&|s| {
                s.done = offset;
                s.resumed = offset > 0;
            });
            let mut f = if offset > 0 { std::fs::OpenOptions::new().append(true).open(&part) } else { std::fs::File::create(&part) }.map_err(|e| format!("could not write {}: {}", part.display(), e.kind()))?;
            if offset < total || total == 0 {
                fs.download(&t.req.remote, offset, &mut f, &mut |n| Self::progress(t, n))?;
            }
            f.flush().map_err(|e| e.to_string())?;
            drop(f);
            let got = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            if got != total {
                return Err(format!("the file is incomplete ({got} of {total} bytes); it can be continued"));
            }
            std::fs::rename(&part, &local).map_err(|e| format!("could not finish the file: {}", e.kind()))?;
            Ok(())
        } else {
            let meta = std::fs::metadata(&local).map_err(|e| format!("could not read {}: {}", local.display(), e.kind()))?;
            if !meta.is_file() {
                return Err("only files are uploaded (not folders)".into());
            }
            let total = meta.len();
            set(&|s| s.total = total);
            let part = format!("{}.part", t.req.remote);
            let existing = fs.size(&t.req.remote)?;
            if existing.is_some() && !t.req.overwrite {
                return Err("a file with that name is already on the server; choose Replace to overwrite it".into());
            }
            let have = fs.size(&part)?.unwrap_or(0);
            let offset = if have > 0 && have <= total { have } else { 0 };
            set(&|s| {
                s.done = offset;
                s.resumed = offset > 0;
            });
            let mut f = std::fs::File::open(&local).map_err(|e| format!("could not open {}: {}", local.display(), e.kind()))?;
            f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
            if offset < total || total == 0 {
                fs.upload(&part, offset, &mut f, &mut |n| Self::progress(t, n))?;
            }
            if fs.size(&part)? != Some(total) {
                return Err("the server holds a different size than was sent; the part is kept for another try".into());
            }
            if existing.is_some() {
                fs.remove_file(&t.req.remote)?;
            }
            fs.rename(&part, &t.req.remote)?;
            Ok(())
        }
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn part_of(local: &std::path::Path) -> Result<PathBuf, String> {
    let name = local.file_name().ok_or("the target has no file name")?.to_string_lossy().into_owned();
    if name.len() > 250 {
        return Err("the file name is too long".into());
    }
    Ok(local.with_file_name(format!("{name}.part")))
}

/// A path to send to a server: no NUL, CR or LF, not absurdly long.
pub fn check_remote(p: &str) -> Result<(), String> {
    if p.is_empty() || p.len() > 4096 || p.contains(['\0', '\r', '\n']) {
        return Err("that is not a usable path on the server".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "remotefiles_tests.rs"]
mod tests;
