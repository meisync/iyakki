//! SSH tunnels: see a web page that sits behind a front-facing SSH host. The model, the file, the command line and the words for
//! what went wrong. The running side (the manager that starts, watches and restarts) is [`crate::sshtunnel_mgr`]; the page is
//! Settings → Network → SSH tunnels; the design is in `docs/SSH-TUNNELS.md`.
//!
//! * **A profile** ([`Tunnel`]) is a name, the front-facing host (port, user, an optional chain of hops before it), a kind
//!   ([`Kind::LocalForward`]: `127.0.0.1:PORT` reaches one `HOST:PORT` behind the host; [`Kind::DynamicSocks`]: `127.0.0.1:PORT` is a SOCKS
//!   proxy to everything the host's network reaches), the vault name of the key and an enabled flag. The list is one JSON file per person
//!   next to the SSH folder. **No secret is in it**: the key is a name of the person's Git and SSH keys (the vault), the private half
//!   reaches the client through IYAKKI's in-memory ssh-agent.
//! * **The command line** is built by [`build_args`], a pure function, and every line is checked again by [`check_args`], an allow-list
//!   of the few shapes this feature ever uses. A `ProxyCommand`, `LocalCommand`, `Include`, `-F`, `-S`, a file typed by the person, a
//!   remote forward, a bind to anything but `127.0.0.1`, or a password is not expressible.
//! * **Host keys** follow `ssh.rs`: the person's own `known_hosts`, `StrictHostKeyChecking=yes`; a host that is not in it is refused
//!   ("trust it first", with the fingerprint to confirm), a changed key is a hard refusal.
//! * **Errors** of the client are mapped to plain words by [`classify`]; the raw text is not shown.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MAX_TUNNELS: usize = 32;
pub const MAX_JUMPS: usize = 3;
/// Local ports below this need a privileged user; none of the tunnels runs as one.
pub const MIN_LOCAL_PORT: u16 = 1024;
const BIND: &str = "127.0.0.1";

fn default_port() -> u16 {
    22
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// `127.0.0.1:local_port` to `target_host:target_port` through the host: one internal page.
    #[default]
    LocalForward,
    /// `127.0.0.1:local_port` is a SOCKS proxy: any site the host's network reaches.
    DynamicSocks,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::LocalForward => "one page (local forward)",
            Kind::DynamicSocks => "any site (SOCKS proxy)",
        }
    }
}

/// A host in front of the final one (`-J`).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Hop {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
}

impl Hop {
    fn validate(&self, what: &str) -> Result<(), String> {
        valid_host(&self.host).map_err(|e| format!("{what}: {e}"))?;
        valid_port(self.port).map_err(|e| format!("{what}: {e}"))?;
        valid_user(&self.user).map_err(|e| format!("{what}: {e}"))
    }

    fn jump_word(&self) -> String {
        format!("{}@{}:{}", self.user, self.host, self.port)
    }
}

/// One tunnel. No secret.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Tunnel {
    /// `t1`, `t2`, … (names a folder at run time, so it is checked on load too).
    pub id: String,
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    /// The name of a key in Settings → Git and SSH keys (vault entry `ssh.<key>`).
    pub key: String,
    /// The key was made for this tunnel, so deleting the tunnel deletes it from the vault too. A shared key is never deleted.
    pub own_key: bool,
    /// Hops before `host`, in order.
    pub jump: Vec<Hop>,
    pub kind: Kind,
    /// Where it listens on this device (127.0.0.1 only). 0 only before it is saved: saving picks a free one.
    pub local_port: u16,
    /// `LocalForward` only.
    pub target_host: String,
    pub target_port: u16,
    /// Enabled tunnels start with the device and reconnect; disabled ones are kept and never started.
    pub enabled: bool,
}

impl Tunnel {
    pub fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.id) {
            return Err("the tunnel has no valid id".into());
        }
        self.validate_fields()?;
        if self.local_port < MIN_LOCAL_PORT {
            return Err(format!("the local port is {MIN_LOCAL_PORT} to 65535"));
        }
        Ok(())
    }

    /// Everything but the id and a zero local port (still to be picked).
    pub fn validate_fields(&self) -> Result<(), String> {
        let n = &self.name;
        if n.trim().is_empty() || n.chars().count() > 40 || !n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ')) {
            return Err("the name is 1 to 40 letters, digits, spaces, `.`, `_` or `-`".into());
        }
        valid_host(&self.host).map_err(|e| format!("host: {e}"))?;
        valid_port(self.port).map_err(|e| format!("host port: {e}"))?;
        valid_user(&self.user).map_err(|e| format!("user: {e}"))?;
        if !crate::gitkeys::valid_key_id(&self.key) {
            return Err("choose a key (Settings > Git and SSH keys) or let the tunnel make one".into());
        }
        if self.jump.len() > MAX_JUMPS {
            return Err(format!("at most {MAX_JUMPS} jump hosts"));
        }
        for (i, h) in self.jump.iter().enumerate() {
            h.validate(&format!("jump host {}", i + 1))?;
        }
        if self.local_port != 0 && self.local_port < MIN_LOCAL_PORT {
            return Err(format!("the local port is {MIN_LOCAL_PORT} to 65535"));
        }
        match self.kind {
            Kind::LocalForward => {
                valid_host(&self.target_host).map_err(|e| format!("page host: {e}"))?;
                valid_port(self.target_port).map_err(|e| format!("page port: {e}"))?;
            }
            Kind::DynamicSocks => {
                if !self.target_host.is_empty() || self.target_port != 0 {
                    return Err("a SOCKS proxy has no single target".into());
                }
            }
        }
        Ok(())
    }

    /// The hosts whose keys must be known: the jumps, then the host itself.
    pub fn hops(&self) -> Vec<(String, u16)> {
        let mut v: Vec<(String, u16)> = self.jump.iter().map(|h| (h.host.to_ascii_lowercase(), h.port)).collect();
        v.push((self.host.to_ascii_lowercase(), self.port));
        v
    }

    /// What the Browser opens: `http://127.0.0.1:PORT/` for a local forward, `socks5h://127.0.0.1:PORT` (a proxy address) for SOCKS.
    pub fn address(&self) -> String {
        match self.kind {
            Kind::LocalForward => format!("http://{BIND}:{}/", self.local_port),
            Kind::DynamicSocks => format!("socks5h://{BIND}:{}", self.local_port),
        }
    }

    /// `you@host:22` through `a`, `b`.
    pub fn route_text(&self) -> String {
        let mut s = String::new();
        for h in &self.jump {
            s.push_str(&format!("{} > ", h.host));
        }
        s.push_str(&format!("{}@{}:{}", self.user, self.host, self.port));
        s
    }
}

pub fn valid_id(id: &str) -> bool {
    id.len() >= 2 && id.len() <= 5 && id.starts_with('t') && id[1..].chars().all(|c| c.is_ascii_digit())
}

/// A DNS name or IPv4 address: letters, digits, `.`, `-`, `_`; no `:` (IPv6 literals are not accepted), no control character, no space.
pub fn valid_host(h: &str) -> Result<(), String> {
    if h.is_empty() {
        return Err("it is empty".into());
    }
    if h.len() > 253 || h.starts_with(['-', '.']) || h.ends_with('.') || h.contains("..") || !h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
        return Err(format!("`{}` is not a host name or IPv4 address (letters, digits, `.`, `-`)", h.chars().filter(|c| !c.is_control()).take(40).collect::<String>()));
    }
    Ok(())
}

pub fn valid_port(p: u16) -> Result<(), String> {
    if p == 0 { Err("the port is 1 to 65535".into()) } else { Ok(()) }
}

pub fn valid_user(u: &str) -> Result<(), String> {
    if u.is_empty() || u.len() > 64 || u.starts_with('-') || !u.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err("the user name is 1 to 64 letters, digits, `.`, `_` or `-`".into());
    }
    Ok(())
}

// ------------------------------------------------------------------ the file

/// The list of tunnels of one person: `tunnels.json`, 0600, written whole and renamed into place.
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(path: &Path) -> Store {
        Store { path: path.into() }
    }

    pub fn list(&self) -> Vec<Tunnel> {
        let mut v: Vec<Tunnel> = std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        v.retain(|t| valid_id(&t.id));
        v.sort_by_key(|t| t.name.to_ascii_lowercase());
        v
    }

    pub fn get(&self, id: &str) -> Option<Tunnel> {
        self.list().into_iter().find(|t| t.id == id)
    }

    fn write(&self, all: &[Tunnel]) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("could not create {}: {e}", d.display()))?;
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("tmp");
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| format!("could not write {}: {e}", self.path.display()))?;
        f.write_all(&serde_json::to_vec_pretty(all).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        drop(f);
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    /// Adds a tunnel (empty `id`: a new one is given) or replaces the one with that id. Checks the fields, a repeated name, the count and
    /// a local port another tunnel already owns (enabled or not: a disabled tunnel keeps its port). `local_port` must be set by now.
    pub fn put(&self, mut t: Tunnel) -> Result<Tunnel, String> {
        t.name = t.name.trim().to_string();
        t.host = t.host.to_ascii_lowercase();
        for h in &mut t.jump {
            h.host = h.host.to_ascii_lowercase();
        }
        let mut all = self.list();
        if t.id.is_empty() {
            if all.len() >= MAX_TUNNELS {
                return Err(format!("at most {MAX_TUNNELS} tunnels"));
            }
            t.id = (1u32..).map(|n| format!("t{n}")).find(|id| !all.iter().any(|x| &x.id == id)).unwrap_or_default();
        } else if !all.iter().any(|x| x.id == t.id) {
            return Err("that tunnel is gone".into());
        }
        t.validate()?;
        if let Some(o) = all.iter().find(|x| x.id != t.id && x.name.eq_ignore_ascii_case(&t.name)) {
            return Err(format!("there is already a tunnel called {}", o.name));
        }
        if let Some(o) = all.iter().find(|x| x.id != t.id && x.local_port == t.local_port) {
            return Err(format!("port {} is already used by the tunnel {}", t.local_port, o.name));
        }
        all.retain(|x| x.id != t.id);
        all.push(t.clone());
        self.write(&all)?;
        Ok(t)
    }

    pub fn remove(&self, id: &str) -> Result<Tunnel, String> {
        let mut all = self.list();
        let t = all.iter().find(|t| t.id == id).cloned().ok_or("that tunnel is gone")?;
        all.retain(|t| t.id != id);
        self.write(&all)?;
        Ok(t)
    }
}

// ------------------------------------------------------------------ local ports

/// Asks the machine about local ports.
pub trait Ports: Send + Sync {
    /// Could `127.0.0.1:port` be listened on right now?
    fn is_free(&self, port: u16) -> bool;
    /// A free port, chosen by the operating system.
    fn pick(&self) -> Result<u16, String>;
}

pub struct OsPorts;

impl Ports for OsPorts {
    fn is_free(&self, port: u16) -> bool {
        std::net::TcpListener::bind((BIND, port)).is_ok()
    }
    fn pick(&self) -> Result<u16, String> {
        let l = std::net::TcpListener::bind((BIND, 0)).map_err(|e| format!("no free local port: {e}"))?;
        l.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
    }
}

// ------------------------------------------------------------------ the command line

/// The options every tunnel runs with, in this order. The known-hosts path is the only value that is not fixed.
fn fixed_options(known_hosts: &str) -> Vec<String> {
    [
        "BatchMode=yes",
        "StrictHostKeyChecking=yes",
        "GlobalKnownHostsFile=/dev/null",
        "UpdateHostKeys=no",
        "VerifyHostKeyDNS=no",
        "PreferredAuthentications=publickey",
        "PubkeyAuthentication=yes",
        "PasswordAuthentication=no",
        "KbdInteractiveAuthentication=no",
        "IdentitiesOnly=no",
        "ForwardAgent=no",
        "ForwardX11=no",
        "PermitLocalCommand=no",
        "ControlMaster=no",
        "ControlPath=none",
        "ExitOnForwardFailure=yes",
        "ServerAliveInterval=15",
        "ServerAliveCountMax=3",
        "ConnectTimeout=15",
        "LogLevel=ERROR",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain(std::iter::once(format!("UserKnownHostsFile={known_hosts}")))
    .collect()
}

/// `-o` options and their only allowed values (`None`: a number in the range).
const OPTION_RULES: &[(&str, Option<&str>)] = &[
    ("BatchMode", Some("yes")),
    ("StrictHostKeyChecking", Some("yes")),
    ("GlobalKnownHostsFile", Some("/dev/null")),
    ("UpdateHostKeys", Some("no")),
    ("VerifyHostKeyDNS", Some("no")),
    ("PreferredAuthentications", Some("publickey")),
    ("PubkeyAuthentication", Some("yes")),
    ("PasswordAuthentication", Some("no")),
    ("KbdInteractiveAuthentication", Some("no")),
    ("IdentitiesOnly", Some("no")),
    ("ForwardAgent", Some("no")),
    ("ForwardX11", Some("no")),
    ("PermitLocalCommand", Some("no")),
    ("ControlMaster", Some("no")),
    ("ControlPath", Some("none")),
    ("ExitOnForwardFailure", Some("yes")),
    ("LogLevel", Some("ERROR")),
    ("ServerAliveInterval", None),
    ("ServerAliveCountMax", None),
    ("ConnectTimeout", None),
];

/// The arguments of the OpenSSH client for this tunnel (without the program). Pure: the same profile gives the same line, and the
/// private key is not part of it (the client finds the in-memory agent through `SSH_AUTH_SOCK`, set by the runner).
pub fn build_args(t: &Tunnel, known_hosts: &Path) -> Result<Vec<String>, String> {
    t.validate()?;
    let kh = known_hosts.display().to_string();
    if kh.is_empty() || kh.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | '%' | '$' | '~')) {
        return Err("the folder of the known hosts file has a character the client cannot take safely in a path".into());
    }
    let mut a: Vec<String> = vec!["-N".into(), "-T".into()];
    for o in fixed_options(&kh) {
        a.push("-o".into());
        a.push(o);
    }
    a.push("-p".into());
    a.push(t.port.to_string());
    if !t.jump.is_empty() {
        a.push("-J".into());
        a.push(t.jump.iter().map(Hop::jump_word).collect::<Vec<_>>().join(","));
    }
    match t.kind {
        Kind::LocalForward => {
            a.push("-L".into());
            a.push(format!("{BIND}:{}:{}:{}", t.local_port, t.target_host, t.target_port));
        }
        Kind::DynamicSocks => {
            a.push("-D".into());
            a.push(format!("{BIND}:{}", t.local_port));
        }
    }
    a.push("--".into());
    a.push(format!("{}@{}", t.user, t.host));
    check_args(&a, known_hosts)?;
    Ok(a)
}

fn num_in(v: &str, lo: u32, hi: u32) -> bool {
    v.parse::<u32>().is_ok_and(|n| (lo..=hi).contains(&n))
}

fn port_word(v: &str) -> Result<u16, String> {
    v.parse::<u16>().ok().filter(|p| *p >= 1).ok_or_else(|| format!("`{v}` is not a port"))
}

fn local_port_word(v: &str) -> Result<u16, String> {
    port_word(v).and_then(|p| if p >= MIN_LOCAL_PORT { Ok(p) } else { Err(format!("local port {p} is below {MIN_LOCAL_PORT}")) })
}

fn dest_word(w: &str) -> Result<(), String> {
    let (u, h) = w.split_once('@').ok_or("the destination is user@host")?;
    valid_user(u)?;
    valid_host(h)
}

/// The allow-list. A line passes only when every word is one of: `-N`, `-T`, `-p PORT`, `-J user@host:port[,…]`, exactly one of
/// `-D 127.0.0.1:PORT` or `-L 127.0.0.1:PORT:HOST:PORT`, `-o Option=Value` from [`OPTION_RULES`] (and `UserKnownHostsFile` equal to
/// `known_hosts`), then `--` and `user@host`. Everything else is refused by name: no `-F`, `-S`, `-R`, `-W`, `-i`, `-E`, `-w`, `-f`,
/// `-A`, `-X`, `-o ProxyCommand`, `-o LocalCommand`, `-o Include`, `-o IdentityFile`, no bind other than 127.0.0.1.
pub fn check_args(args: &[String], known_hosts: &Path) -> Result<(), String> {
    let kh = known_hosts.display().to_string();
    let (mut forwards, mut jumps, mut ports, mut seen_kh, mut i) = (0, 0, 0, false, 0);
    let mut dest = false;
    let refuse = |w: &str| Err(format!("`{}` is not allowed in a tunnel command", w.chars().filter(|c| !c.is_control()).take(60).collect::<String>()));
    while i < args.len() {
        let w = args[i].as_str();
        let val = args.get(i + 1).map(String::as_str);
        match w {
            "-N" | "-T" => i += 1,
            "-p" => {
                ports += 1;
                port_word(val.ok_or("-p needs a port")?)?;
                i += 2;
            }
            "-J" => {
                jumps += 1;
                let list = val.ok_or("-J needs the hosts")?;
                let hops: Vec<&str> = list.split(',').collect();
                if hops.len() > MAX_JUMPS {
                    return Err("too many jump hosts".into());
                }
                for h in hops {
                    let (ud, port) = h.rsplit_once(':').ok_or("a jump host is user@host:port")?;
                    port_word(port)?;
                    dest_word(ud)?;
                }
                i += 2;
            }
            "-D" => {
                forwards += 1;
                local_port_word(val.and_then(|v| v.strip_prefix("127.0.0.1:")).ok_or("-D binds 127.0.0.1 only: 127.0.0.1:PORT")?)?;
                i += 2;
            }
            "-L" => {
                forwards += 1;
                let rest = val.and_then(|v| v.strip_prefix("127.0.0.1:")).ok_or("-L binds 127.0.0.1 only: 127.0.0.1:PORT:HOST:PORT")?;
                let p: Vec<&str> = rest.split(':').collect();
                if p.len() != 3 {
                    return Err("-L is 127.0.0.1:PORT:HOST:PORT".into());
                }
                local_port_word(p[0])?;
                valid_host(p[1])?;
                port_word(p[2])?;
                i += 2;
            }
            "-o" => {
                let kv = val.ok_or("-o needs an option")?;
                let (k, v) = kv.split_once('=').ok_or("an option is Name=Value")?;
                if k == "UserKnownHostsFile" {
                    if v != kh || seen_kh {
                        return Err("the known hosts file is the person's own, once".into());
                    }
                    seen_kh = true;
                } else {
                    match OPTION_RULES.iter().find(|(n, _)| *n == k) {
                        Some((_, Some(fixed))) if v == *fixed => {}
                        Some((n, None)) if num_in(v, 1, if *n == "ServerAliveCountMax" { 10 } else { 600 }) => {}
                        _ => return refuse(kv),
                    }
                }
                i += 2;
            }
            "--" => {
                let d = args.get(i + 1).ok_or("the destination is missing")?;
                if args.len() != i + 2 {
                    return refuse("words after the destination");
                }
                dest_word(d)?;
                dest = true;
                i += 2;
            }
            other => return refuse(other),
        }
    }
    if forwards != 1 || ports != 1 || jumps > 1 || !dest || !seen_kh {
        return Err("a tunnel command has one forward, one port, the known hosts file and a destination".into());
    }
    Ok(())
}

// ------------------------------------------------------------------ what went wrong, in words

/// Why a client run ended, sorted by what the person can do about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The host is not in the known hosts: trust it first.
    NotTrusted,
    /// The host presented another key than the recorded one.
    HostKeyChanged,
    PermissionDenied,
    Refused,
    Timeout,
    NoRoute,
    NoName,
    AddressInUse,
    /// An established connection ended (the host closed it, the network dropped).
    Dropped,
    /// The key could not be opened (missing, needs its passphrase).
    KeyUnavailable,
    Other,
}

impl Fault {
    /// Worth trying again by itself? The others wait for the person (a retry loop on a changed host key or a refused login would only
    /// annoy the host).
    pub fn retryable(self) -> bool {
        matches!(self, Fault::Refused | Fault::Timeout | Fault::NoRoute | Fault::NoName | Fault::Dropped | Fault::Other)
    }

    pub fn words(self) -> &'static str {
        match self {
            Fault::NotTrusted => "This host is not trusted yet. Press Test to see its fingerprint and trust it first.",
            Fault::HostKeyChanged => "The host key has CHANGED since you trusted it. Nothing was sent. Ask the host's owner; if the change is genuine, forget the old key in Settings > Git and SSH keys and trust the host again.",
            Fault::PermissionDenied => "The host refused the key. Add the public key (Settings > Git and SSH keys) to the user's authorized keys on the host, or check the user name.",
            Fault::Refused => "The connection was refused: nothing listens on that address and port, or a firewall says no.",
            Fault::Timeout => "The host did not answer in time. Check the address, the network, and that the host is up.",
            Fault::NoRoute => "There is no route to the host from this network.",
            Fault::NoName => "The host name is not known (check the spelling and your DNS).",
            Fault::AddressInUse => "The local port is already used by another program. Edit the tunnel and pick another port.",
            Fault::Dropped => "The connection dropped. It will try again.",
            Fault::KeyUnavailable => "The key could not be opened. Unlock it (Settings > Git and SSH keys) or choose another.",
            Fault::Other => "The tunnel stopped for a reason the client did not explain. It will try again.",
        }
    }
}

/// Sorts the error text of OpenSSH 9/10 (`LogLevel=ERROR`, `LC_ALL=C`) into a [`Fault`]. Order matters: the host key lines come first,
/// because the same run may end with a `Permission denied` line too.
pub fn classify(stderr: &str) -> Fault {
    let t = stderr.to_ascii_lowercase();
    let has = |s: &str| t.contains(s);
    if has("remote host identification has changed") || has("host key for") && has("has changed") || has("offending") && has("key in") {
        Fault::HostKeyChanged
    } else if has("host key verification failed") || has("no ") && has("host key is known") || has("strict checking") {
        Fault::NotTrusted
    } else if has("address already in use") || has("could not request local forwarding") || has("cannot listen to port") || has("bind [") && has("failed") {
        Fault::AddressInUse
    } else if has("permission denied") || has("too many authentication failures") || has("no more authentication methods") {
        Fault::PermissionDenied
    } else if has("connection refused") {
        Fault::Refused
    } else if has("timed out") || has("timeout") && !has("server") {
        Fault::Timeout
    } else if has("no route to host") || has("network is unreachable") {
        Fault::NoRoute
    } else if has("could not resolve hostname") || has("name or service not known") || has("temporary failure in name resolution") {
        Fault::NoName
    } else if has("connection closed") || has("closed by remote host") || has("connection reset") || has("broken pipe") || has("timeout, server") || has("not responding") || has("kex_exchange_identification") {
        Fault::Dropped
    } else {
        Fault::Other
    }
}

/// How long ago, for a screen: `just now`, `4 min`, `3 h`, `2 days`.
pub fn ago(secs: u64) -> String {
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min", secs / 60),
        3600..=86399 => format!("{} h", secs / 3600),
        _ => format!("{} days", secs / 86400),
    }
}

// ------------------------------------------------------------------ the seam for the Browser

/// A tunnel the Browser should use for a page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// For a local forward the address to open instead; for SOCKS the proxy address.
    pub address: String,
}

/// **The seam for the Browser** (not wired yet). Given a URL, the id of the tunnel it should go through, if any. The planned
/// implementation is the same hybrid filter (domain name, URL, CIDR) as the VPN's `FilterSet` in `crates/iyakki-vpn`, with a tunnel
/// named by each rule; until then [`NoSiteRouter`] says "direct" for everything. The Browser asks
/// [`crate::sshtunnel_mgr::Manager::route`], which also checks that the tunnel is connected, so a page never goes to a dead port.
pub trait SiteRouter: Send + Sync {
    fn tunnel_for(&self, url: &str) -> Option<String>;
}

pub struct NoSiteRouter;

impl SiteRouter for NoSiteRouter {
    fn tunnel_for(&self, _url: &str) -> Option<String> {
        None
    }
}
