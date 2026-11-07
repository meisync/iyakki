//! Shared folders: the list of a person's shares (kept in `shares.json` of their own folder, mode 0600), the rules for what may
//! be shared, and the running servers (`sharedsite.rs`). Everything is off until the person creates a share, read-only until
//! they switch uploading on, and stops at sign-out. See `docs/SHARED-FOLDERS.md`.

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use serde::{Deserialize, Serialize};

use crate::sharedsite::{self, Env, Running};

pub const FIRST_PORT: u16 = 8081;
const LAST_PORT: u16 = 8180;
pub const MAX_SHARES: usize = 16;
pub const EXPIRIES: [(&str, u64); 4] = [("1 hour", 3600), ("1 day", 86400), ("7 days", 604800), ("never", 0)];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// A browsable list of files (downloads, optional uploads).
    #[default]
    Files,
    /// A static site: `index.html` is served, no list, no uploads.
    Website,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// A secret address `/s/<128-bit token>/` that can be changed to revoke it.
    #[default]
    Token,
    /// One shared password (HTTP Basic; weak on plain HTTP).
    Password,
    /// Anyone on the local network.
    Open,
    /// This machine only (127.0.0.1).
    Local,
}

impl Access {
    pub fn label(self) -> &'static str {
        match self {
            Access::Token => "secret link",
            Access::Password => "password",
            Access::Open => "anyone on the local network",
            Access::Local => "this machine only",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShareCfg {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub mode: Mode,
    pub access: Access,
    pub upload: bool,
    pub mkdir: bool,
    pub overwrite: bool,
    pub show_hidden: bool,
    pub max_upload_mb: u64,
    /// `all` or one local address.
    pub bind: String,
    pub port: u16,
    /// The secret of the link (token mode). Kept in the share list, which only its owner can read.
    pub token: String,
    /// Argon2id hash of the password (password mode); the password itself is never stored.
    pub pass_hash: String,
    /// How long a start lasts (0 = never ends).
    pub expiry_secs: u64,
    /// When the running share ends (0 = never); set at each start.
    pub expires_at: i64,
    /// Start again when the owner signs in.
    pub keep_running: bool,
    pub owner: String,
    /// A "Receive a package" share: its folder is `<data>/local/packages`, it takes only the add-on package and its checksum (see [`package_name_ok`]).
    pub package_drop: bool,
}

/// What the form sends.
#[derive(Clone, Debug, Default)]
pub struct Draft {
    pub name: String,
    pub folder: String,
    pub mode: Mode,
    pub access: Access,
    pub upload: bool,
    pub mkdir: bool,
    pub overwrite: bool,
    pub show_hidden: bool,
    pub max_upload_mb: u64,
    pub bind: String,
    /// 0 = choose one.
    pub port: u16,
    /// Password mode: the new password (empty keeps the old one when editing).
    pub password: String,
    pub expiry_secs: u64,
    pub keep_running: bool,
    /// The person ticked the card that says everyone on the network can open it.
    pub confirm_open: bool,
}

#[derive(Clone, Debug)]
pub struct ShareInfo {
    pub cfg: ShareCfg,
    pub running: bool,
    pub urls: Vec<String>,
    pub clients: usize,
    pub requests: u64,
    pub error: Option<String>,
    /// Seconds left while running with an expiry.
    pub left: Option<i64>,
}

// ------------------------------------------------------------------ small helpers

pub fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn hash_password(pw: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default().hash_password(pw.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| e.to_string())
}

pub fn verify_password(pw: &str, phc: &str) -> bool {
    PasswordHash::new(phc).is_ok_and(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
}

pub fn valid_name(n: &str) -> bool {
    let t = n.trim();
    !t.is_empty() && t.chars().count() <= 40 && t == n && n.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
}

const FORBIDDEN: [&str; 14] = ["/etc", "/proc", "/sys", "/dev", "/boot", "/root", "/run", "/usr", "/bin", "/sbin", "/lib", "/var", "/opt", "/srv"];
const TOO_BROAD: [&str; 6] = ["/", "/home", "/mnt", "/media", "/tmp", "/data"];

/// May this folder be shared? It must exist, be a folder the person can write (their home, a workspace, a mounted data
/// folder: `allow_write` is the account's policy), and not be a system folder, the vault's folder or one of the settings.
/// Returns the real path.
pub fn validate_folder(folder: &str, allow_write: &dyn Fn(&str) -> bool, deny: &[PathBuf]) -> Result<PathBuf, String> {
    let f = folder.trim();
    if f.is_empty() || f.contains('\0') || !f.starts_with('/') {
        return Err("Give the full path of a folder (it starts with /).".into());
    }
    let real = std::fs::canonicalize(f).map_err(|_| "That folder does not exist.".to_string())?;
    if !real.is_dir() {
        return Err("That is not a folder.".into());
    }
    let s = real.to_string_lossy().into_owned();
    if TOO_BROAD.contains(&s.as_str()) || FORBIDDEN.iter().any(|p| real.starts_with(p)) {
        return Err("That folder is part of the system or holds other people's files; share a folder of your own.".into());
    }
    if deny.iter().any(|d| real.starts_with(d)) {
        return Err("That folder holds the system's private data (vault and settings) and is never shared.".into());
    }
    if !allow_write(&s) {
        return Err("You can only share folders you may write to: your home folder, your workspaces or a data folder you have access to.".into());
    }
    Ok(real)
}

/// The file names a package drop accepts: the add-on's FFmpeg libraries for a known processor, and the checksum file beside it.
pub fn package_name_ok(name: &str) -> bool {
    let n = name;
    n.starts_with("ffmpeg-libs-") && (n.ends_with("-linux-musl-x86_64.tar.gz") || n.ends_with("-linux-musl-aarch64.tar.gz") || n.ends_with("-linux-musl-x86_64.tar.gz.sha256") || n.ends_with("-linux-musl-aarch64.tar.gz.sha256"))
}

/// How long a package drop stays open unless the person changes it, and the biggest package it takes.
pub const PACKAGE_DROP_SECS: u64 = 600;
pub const PACKAGE_DROP_MB: u64 = 64;

// ------------------------------------------------------------------ the manager

type AllowFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;
type LanFn = Arc<dyn Fn() -> Vec<String> + Send + Sync>;

struct Inner {
    cfgs: Vec<ShareCfg>,
    running: HashMap<String, Running>,
    errors: HashMap<String, String>,
}

pub struct Shares {
    file: PathBuf,
    owner: String,
    env: Env,
    allow_write: AllowFn,
    lan: LanFn,
    inner: Mutex<Inner>,
}

impl Shares {
    pub fn open(file: &Path, owner: &str, env: Env, allow_write: AllowFn, lan: LanFn) -> Shares {
        let cfgs: Vec<ShareCfg> = std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Shares { file: file.to_path_buf(), owner: owner.into(), env, allow_write, lan, inner: Mutex::new(Inner { cfgs, running: HashMap::new(), errors: HashMap::new() }) }
    }

    fn audit(&self, event: &str, detail: &str) {
        (self.env.audit)(event, detail);
    }

    fn save(&self, i: &Inner) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(d) = self.file.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = self.file.with_extension("tmp");
        let text = serde_json::to_string_pretty(&i.cfgs).map_err(|e| e.to_string())?;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.file).map_err(|e| e.to_string())
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    fn port_free(port: u16, local_only: bool) -> bool {
        TcpListener::bind((if local_only { "127.0.0.1" } else { "0.0.0.0" }, port)).is_ok()
    }

    fn pick_port(&self, i: &Inner, local_only: bool) -> Result<u16, String> {
        (FIRST_PORT..=LAST_PORT).find(|p| !i.cfgs.iter().any(|c| c.port == *p) && Self::port_free(*p, local_only)).ok_or_else(|| "No free port between 8081 and 8180.".to_string())
    }

    /// Check a form and make the stored settings from it. `old` is the share being edited.
    fn build(&self, i: &Inner, d: &Draft, old: Option<&ShareCfg>) -> Result<ShareCfg, String> {
        if !valid_name(&d.name) {
            return Err("The name may have letters, digits, spaces, - _ . and at most 40 characters.".into());
        }
        if i.cfgs.iter().any(|c| c.name.eq_ignore_ascii_case(&d.name) && Some(&c.id) != old.map(|o| &o.id)) {
            return Err("Another share has that name.".into());
        }
        let real = validate_folder(&d.folder, &*self.allow_write, &self.env.deny)?;
        if !EXPIRIES.iter().any(|(_, s)| *s == d.expiry_secs) {
            return Err("Choose 1 hour, 1 day, 7 days or never.".into());
        }
        if d.access == Access::Open && !d.confirm_open && old.is_none_or(|o| o.access != Access::Open) {
            return Err("Tick the box that says everyone on the network can open this share.".into());
        }
        let mut c = old.cloned().unwrap_or_default();
        c.name = d.name.clone();
        c.folder = real.to_string_lossy().into_owned();
        c.mode = d.mode;
        c.access = d.access;
        c.upload = d.upload && d.mode == Mode::Files;
        c.mkdir = c.upload && d.mkdir;
        c.overwrite = c.upload && d.overwrite;
        c.show_hidden = d.show_hidden && d.mode == Mode::Files;
        c.max_upload_mb = if d.max_upload_mb == 0 { 25 } else { d.max_upload_mb.clamp(1, 512) };
        c.expiry_secs = d.expiry_secs;
        c.keep_running = d.keep_running;
        c.owner = self.owner.clone();
        c.bind = if d.access == Access::Local { "127.0.0.1".into() } else if d.bind.trim().is_empty() { "all".into() } else { d.bind.trim().to_string() };
        if c.bind != "all" {
            let ok = c.bind.parse::<std::net::Ipv4Addr>().is_ok_and(|a| a.is_loopback() || (self.lan)().iter().any(|l| *l == c.bind));
            if !ok {
                return Err("That address is not one of this machine's addresses.".into());
            }
        }
        c.port = if d.port != 0 { d.port } else if let Some(o) = old { o.port } else { self.pick_port(i, d.access == Access::Local)? };
        if c.port < 1024 || i.cfgs.iter().any(|o| o.port == c.port && Some(&o.id) != old.map(|x| &x.id)) {
            return Err("That port is not available (use 1024 or higher, one per share).".into());
        }
        match d.access {
            Access::Token => {
                if c.token.len() != 32 {
                    c.token = random_hex(16);
                }
                c.pass_hash.clear();
            }
            Access::Password => {
                if !d.password.is_empty() {
                    if d.password.chars().count() < 8 || d.password.len() > 128 {
                        return Err("The password needs 8 to 128 characters.".into());
                    }
                    c.pass_hash = hash_password(&d.password)?;
                } else if c.pass_hash.is_empty() {
                    return Err("Choose a password for this share.".into());
                }
                c.token.clear();
            }
            _ => {
                c.token.clear();
                c.pass_hash.clear();
            }
        }
        Ok(c)
    }

    pub fn create(&self, d: &Draft) -> Result<String, String> {
        let mut i = self.inner.lock().map_err(|_| "busy".to_string())?;
        if i.cfgs.len() >= MAX_SHARES {
            return Err(format!("At most {MAX_SHARES} shares."));
        }
        let mut c = self.build(&i, d, None)?;
        c.id = random_hex(4);
        i.cfgs.push(c.clone());
        self.save(&i)?;
        self.audit("share.create", &format!("share={} mode={:?} access={:?} upload={}", c.name, c.mode, c.access, c.upload));
        Ok(c.id)
    }

    /// "Receive a package": a share of `folder` (which must be `.../local/packages`) with password access, uploads on, no overwriting and no new
    /// folders, only the add-on package's file names, at most [`PACKAGE_DROP_MB`] MB, open for [`PACKAGE_DROP_SECS`] seconds. The caller has checked that
    /// the person is an administrator; the file lands where the add-on installer looks, and nothing is installed by the upload.
    pub fn create_package_drop(&self, folder: &Path, password: &str) -> Result<String, String> {
        if !folder.ends_with("local/packages") {
            return Err("A package drop belongs to the local packages folder.".into());
        }
        if password.len() < 8 {
            return Err("Choose a longer password for the drop.".into());
        }
        let mut i = self.inner.lock().map_err(|_| "busy".to_string())?;
        if i.cfgs.len() >= MAX_SHARES {
            return Err(format!("At most {MAX_SHARES} shares."));
        }
        // one drop at a time: an old one is replaced
        let old: Vec<String> = i.cfgs.iter().filter(|c| c.package_drop).map(|c| c.id.clone()).collect();
        for id in old {
            if let Some(mut r) = i.running.remove(&id) {
                r.stop();
            }
            i.cfgs.retain(|c| c.id != id);
        }
        // a real port from the shared-folder range (not 0): the address, the curl line and the firewall preset all name it
        let port = self.pick_port(&i, false)?;
        let c = ShareCfg {
            id: random_hex(4),
            name: "Receive a package".into(),
            folder: folder.to_string_lossy().into_owned(),
            mode: Mode::default(),
            access: Access::Password,
            upload: true,
            mkdir: false,
            overwrite: false,
            show_hidden: false,
            max_upload_mb: PACKAGE_DROP_MB,
            bind: "all".into(),
            port,
            token: String::new(),
            pass_hash: hash_password(password)?,
            expiry_secs: PACKAGE_DROP_SECS,
            expires_at: 0,
            keep_running: false,
            owner: self.owner.clone(),
            package_drop: true,
        };
        let id = c.id.clone();
        i.cfgs.push(c);
        self.save(&i)?;
        self.audit("share.create", "share=Receive a package mode=package-drop access=Password upload=true");
        Ok(id)
    }

    pub fn update(&self, id: &str, d: &Draft) -> Result<(), String> {
        let mut i = self.inner.lock().map_err(|_| "busy".to_string())?;
        let pos = i.cfgs.iter().position(|c| c.id == id).ok_or("No such share.")?;
        if i.cfgs[pos].package_drop {
            return Err("A package drop is not edited: delete it and make a new one.".into());
        }
        let old = i.cfgs[pos].clone();
        let mut c = self.build(&i, d, Some(&old))?;
        c.id = old.id.clone();
        let was_running = i.running.contains_key(id);
        if let Some(mut r) = i.running.remove(id) {
            r.stop();
        }
        i.cfgs[pos] = c.clone();
        self.save(&i)?;
        self.audit("share.edit", &format!("share={} mode={:?} access={:?} upload={}", c.name, c.mode, c.access, c.upload));
        if was_running {
            self.start_locked(&mut i, id)?;
        }
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let mut i = self.inner.lock().map_err(|_| "busy".to_string())?;
        if let Some(mut r) = i.running.remove(id) {
            r.stop();
        }
        let Some(pos) = i.cfgs.iter().position(|c| c.id == id) else { return Err("No such share.".into()) };
        let c = i.cfgs.remove(pos);
        i.errors.remove(id);
        self.save(&i)?;
        self.audit("share.delete", &format!("share={}", c.name));
        Ok(())
    }

    fn start_locked(&self, i: &mut Inner, id: &str) -> Result<(), String> {
        if i.running.get(id).is_some_and(|r| r.alive()) {
            return Ok(());
        }
        i.running.remove(id);
        let pos = i.cfgs.iter().position(|c| c.id == id).ok_or("No such share.")?;
        // the folder may have moved or become unsuitable since it was chosen
        let mut cfg = i.cfgs[pos].clone();
        let checked = if cfg.package_drop {
            // the folder is fixed: the local packages folder only, however the share file was edited
            std::fs::canonicalize(&cfg.folder).ok().filter(|r| r.is_dir() && r.ends_with("local/packages")).ok_or_else(|| "The packages folder is missing.".to_string())
        } else {
            validate_folder(&cfg.folder, &*self.allow_write, &self.env.deny)
        };
        let real = checked.inspect_err(|e| {
            i.errors.insert(id.to_string(), e.clone());
        })?;
        cfg.folder = real.to_string_lossy().into_owned();
        let now = (self.env.clock)();
        cfg.expires_at = if cfg.expiry_secs > 0 { now + cfg.expiry_secs as i64 } else { 0 };
        match sharedsite::start(cfg.clone(), self.env.clone()) {
            Ok(r) => {
                i.cfgs[pos].expires_at = cfg.expires_at;
                i.errors.remove(id);
                i.running.insert(id.to_string(), r);
                let _ = self.save(i);
                self.audit("share.start", &format!("share={} port={} access={:?}", cfg.name, cfg.port, cfg.access));
                Ok(())
            }
            Err(e) => {
                i.errors.insert(id.to_string(), e.clone());
                Err(e)
            }
        }
    }

    pub fn start(&self, id: &str) -> Result<(), String> {
        let mut i = self.inner.lock().map_err(|_| "busy".to_string())?;
        self.start_locked(&mut i, id)
    }

    pub fn stop(&self, id: &str) {
        if let Ok(mut i) = self.inner.lock() {
            if let Some(mut r) = i.running.remove(id) {
                r.stop();
                self.audit("share.stop", &format!("share={}", r.site.cfg.name));
            }
        }
    }

    pub fn stop_all(&self) {
        if let Ok(mut i) = self.inner.lock() {
            let n = i.running.len();
            for (_, mut r) in i.running.drain() {
                r.stop();
            }
            if n > 0 {
                self.audit("share.stop_all", &format!("stopped={n}"));
            }
        }
    }

    pub fn running_count(&self) -> usize {
        self.inner.lock().map(|i| i.running.values().filter(|r| r.alive()).count()).unwrap_or(0)
    }

    /// Give the link a new secret: the old address stops working at once.
    pub fn new_token(&self, id: &str) -> Result<(), String> {
        let mut i = self.inner.lock().map_err(|_| "busy".to_string())?;
        let pos = i.cfgs.iter().position(|c| c.id == id).ok_or("No such share.")?;
        if i.cfgs[pos].access != Access::Token {
            return Err("Only a share with a secret link has one.".into());
        }
        i.cfgs[pos].token = random_hex(16);
        let was = i.running.remove(id);
        let running = was.is_some();
        if let Some(mut r) = was {
            r.stop();
        }
        self.save(&i)?;
        self.audit("share.token_renewed", &format!("share={}", i.cfgs[pos].name));
        if running {
            self.start_locked(&mut i, id)?;
        }
        Ok(())
    }

    /// Stop what has ended (call about once a second).
    pub fn tick(&self) {
        let now = (self.env.clock)();
        let Ok(mut i) = self.inner.lock() else { return };
        let ended: Vec<String> = i.running.iter().filter(|(_, r)| !r.alive() || { let e = r.site.expires_at.load(std::sync::atomic::Ordering::Relaxed); e > 0 && now >= e }).map(|(k, _)| k.clone()).collect();
        for k in ended {
            if let Some(mut r) = i.running.remove(&k) {
                r.stop();
            }
        }
    }

    /// At sign-in: start the shares marked "keep running" that have not ended.
    pub fn restore(&self) {
        let now = (self.env.clock)();
        let Ok(mut i) = self.inner.lock() else { return };
        let ids: Vec<String> = i.cfgs.iter().filter(|c| c.keep_running && (c.expires_at == 0 || c.expires_at > now)).map(|c| c.id.clone()).collect();
        for id in ids {
            let _ = self.start_locked(&mut i, &id);
        }
    }

    pub fn urls(&self, c: &ShareCfg, port: u16) -> Vec<String> {
        let path = if c.access == Access::Token { format!("/s/{}/", c.token) } else { "/".to_string() };
        let hosts: Vec<String> = if c.access == Access::Local {
            vec!["127.0.0.1".into()]
        } else if c.bind == "all" || c.bind.is_empty() {
            let mut v = (self.lan)();
            if v.is_empty() {
                v.push("127.0.0.1".into());
            }
            v
        } else {
            vec![c.bind.clone()]
        };
        hosts.into_iter().map(|h| format!("http://{h}:{port}{path}")).collect()
    }

    pub fn list(&self) -> Vec<ShareInfo> {
        let Ok(i) = self.inner.lock() else { return vec![] };
        let now = (self.env.clock)();
        i.cfgs
            .iter()
            .map(|c| {
                let r = i.running.get(&c.id);
                let mut cfg = c.clone();
                if let Some(r) = r {
                    cfg.expires_at = r.site.expires_at.load(std::sync::atomic::Ordering::Relaxed);
                }
                ShareInfo { urls: self.urls(c, c.port), running: r.is_some_and(|r| r.alive()), clients: r.map_or(0, |r| r.site.clients_served()), requests: r.map_or(0, |r| r.site.requests.load(std::sync::atomic::Ordering::Relaxed)), error: i.errors.get(&c.id).cloned(), left: r.and_then(|r| { let e = r.site.expires_at.load(std::sync::atomic::Ordering::Relaxed); (e > 0).then_some((e - now).max(0)) }), cfg }
            })
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<ShareCfg> {
        self.inner.lock().ok()?.cfgs.iter().find(|c| c.id == id).cloned()
    }

    pub fn addr_of(&self, id: &str) -> Option<std::net::SocketAddr> {
        self.inner.lock().ok()?.running.get(id).map(|r| r.addr)
    }
}

/// The form values to start from for an existing share (Edit).
pub fn draft_of(c: &ShareCfg) -> Draft {
    Draft { name: c.name.clone(), folder: c.folder.clone(), mode: c.mode, access: c.access, upload: c.upload, mkdir: c.mkdir, overwrite: c.overwrite, show_hidden: c.show_hidden, max_upload_mb: c.max_upload_mb, bind: c.bind.clone(), port: c.port, password: String::new(), expiry_secs: c.expiry_secs, keep_running: c.keep_running, confirm_open: c.access == Access::Open }
}

/// Where to look for the share list of `user` under the data folder.
pub fn share_file(data_dir: &Path, user: &str) -> PathBuf {
    let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).collect();
    data_dir.join("shares").join(if safe.is_empty() { "user".into() } else { safe }).join("shares.json")
}

/// Everything in the data folder except the `home` folder of window mode: never served.
pub fn deny_list(data_dir: &Path) -> Vec<PathBuf> {
    let mut v = vec![];
    if let Ok(rd) = std::fs::read_dir(data_dir) {
        for e in rd.flatten() {
            if e.file_name() != "home" {
                if let Ok(p) = std::fs::canonicalize(e.path()) {
                    v.push(p);
                }
            }
        }
    }
    v
}

// ------------------------------------------------------------------ one manager per signed-in person

type Key = (PathBuf, String);

fn registry() -> &'static Mutex<HashMap<Key, Arc<Shares>>> {
    static R: OnceLock<Mutex<HashMap<Key, Arc<Shares>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The manager of `user` (made on first use; the Settings page, the IDE action and sign-out all reach the same one).
pub fn for_user(data_dir: &Path, user: &str, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, allow_write: AllowFn) -> Arc<Shares> {
    let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
    r.entry((data_dir.to_path_buf(), user.to_string()))
        .or_insert_with(|| {
            let u = user.to_string();
            let a2 = audit.clone();
            let audit_user: Arc<dyn Fn(&str, &str) + Send + Sync> = Arc::new(move |ev, d| a2(ev, &format!("user={u} {d}")));
            let env = Env::real(deny_list(data_dir), audit_user);
            let lan: LanFn = Arc::new(lan_addresses);
            Arc::new(Shares::open(&share_file(data_dir, user), user, env, allow_write, lan))
        })
        .clone()
}

/// This machine's IPv4 addresses on the networks around it, private ones first (loopback left out).
pub fn lan_addresses() -> Vec<String> {
    let mut v: Vec<std::net::Ipv4Addr> = iyakki_devctl::ipv4_addresses().iter().filter_map(|(_, a)| a.parse().ok()).filter(|a: &std::net::Ipv4Addr| !a.is_loopback()).collect();
    v.sort_by_key(|a| !a.is_private());
    v.into_iter().map(|a| a.to_string()).collect()
}

/// Sign-out: this person's shares stop.
pub fn on_logout(user: &str) {
    let all: Vec<Arc<Shares>> = registry().lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|((_, u), _)| u == user).map(|(_, m)| m.clone()).collect();
    for m in all {
        m.stop_all();
    }
}

/// "Stop all sharing" (every person's, the machine's own switch).
pub fn stop_everything() -> usize {
    let all: Vec<Arc<Shares>> = registry().lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
    let n = all.iter().map(|m| m.running_count()).sum();
    for m in all {
        m.stop_all();
    }
    n
}

/// How many shares are running now (for the network menu).
pub fn running_total() -> usize {
    registry().lock().unwrap_or_else(|e| e.into_inner()).values().map(|m| m.running_count()).sum()
}

// ------------------------------------------------------------------ "Share this folder…" from other apps

static ASK: Mutex<Option<String>> = Mutex::new(None);

/// An app (EDIT) asks the shell to open Settings -> Shared folders with this folder filled in.
pub fn ask_share(folder: &str) {
    if let Ok(mut a) = ASK.lock() {
        *a = Some(folder.to_string());
    }
}

pub fn take_share_ask() -> Option<String> {
    ASK.lock().ok().and_then(|mut a| a.take())
}

/// The write rule for a person's shares: their account may write there (policy) and it lies inside their home, a workspace or
/// a folder they were given (never "/" or another person's home just because an administrator may write anywhere).
pub fn allow_fn(policy: iyakki_sec::policy::Policy, user: &str, data_dir: &Path) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    let (u, d) = (user.to_string(), data_dir.to_path_buf());
    Arc::new(move |p| policy.allows_write(&u, p) && crate::scope::FileRoots::new(&policy, &u, &d).roots.iter().any(|r| Path::new(p).starts_with(r)))
}
