//! The privileged helper of the VPN manager: a Unix socket in front of the **system layer** ([`crate::vpn_sys::Sys`]).
//!
//! The split: the manager (profiles, vault, state machine, DNS forwarder decisions) can run in the signed-in session; the only
//! code that must be privileged is "change the machine", which is exactly the [`Sys`] trait. [`serve`] runs the real
//! [`crate::vpn_sys::OsSys`] behind a socket, [`RemoteSys`] is a `Sys` that talks to it, and the manager does not know the difference.
//!
//! * **Protocol:** one JSON object per line, request `{"op": ...}`, answer `{"ok": true, "value": ...}` or `{"ok": false, "error": ...}`.
//!   Lines are at most 256 KB; a connection idles out after 60 s; at most 4 connections.
//! * **Who may talk:** the peer's user id is read with `SO_PEERCRED` on every accepted connection and must be in the allow-list
//!   ([`PeerPolicy`]); anyone else is closed on without a word. The socket file is also `0600` and owned by the allowed user when the
//!   helper can chown it, but the credential check is the gate, not the file mode.
//! * **What may be asked:** the helper validates every request again (it does not trust the manager): `ip` only in the five forms
//!   the plans produce (the interface named `iyvpn0` for link and address changes), the guard and DNS tables are rebuilt by the
//!   helper from typed specs and can only be `iyakki_vpn` / `iyakki_vpn_dns`, `resolv.conf` text is bounded, WireGuard
//!   configuration only for `iyvpn0`.
//! * **Secrets:** the private and preshared keys travel in a `wg_configure` request (base64 inside one local socket line) and are
//!   never logged; the helper's own log lists operation names only.
//!
//! Status: the protocol, the server, the peer check, the client and the tests are finished. **Not done:** nothing starts the helper
//! on an image yet (an init service running it as root with the session user's id in the allow-list), the session still runs as
//! root there, and the setting that selects it ([`helper_enabled`]) is off. See `docs/development/VPN-CONNECTOR-PLAN.md`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_vpn::net::Cidr;
use iyakki_vpn::plan::{DnsCaptureSpec, KillSwitchSpec};
use iyakki_vpn::profile::PublicKey;
use iyakki_vpn::secret::Secret;
use iyakki_vpn::wgnl::{valid_ifname, DeviceSet, DeviceStatus, PeerSet, PeerStatus};
use iyakki_vpn::b64;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::vpn_mgr::IFNAME;
use crate::vpn_sys::{NetInfo, Sys};

pub const MAX_LINE: usize = 256 * 1024;
pub const MAX_CONNS: usize = 4;
pub const IDLE: Duration = Duration::from_secs(60);
/// Where the helper listens on an image.
pub const DEFAULT_SOCKET: &str = "/run/iyakki/vpn/helper.sock";

// ------------------------------------------------------------------------------------------------ the setting

/// The setting that makes the manager use the helper: a file `helper.json` in the VPN data directory, `{"enabled": false}` unless
/// the person (or the image maker) turned it on. **Off by default.**
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HelperSetting {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_socket")]
    pub socket: String,
}

fn default_socket() -> String {
    DEFAULT_SOCKET.into()
}

impl Default for HelperSetting {
    fn default() -> Self {
        HelperSetting { enabled: false, socket: default_socket() }
    }
}

pub fn read_setting(dir: &Path) -> HelperSetting {
    std::fs::read_to_string(dir.join("helper.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn write_setting(dir: &Path, s: &HelperSetting) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let p = dir.join("helper.json");
    std::fs::write(&p, serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    Ok(())
}

pub fn helper_enabled(dir: &Path) -> bool {
    read_setting(dir).enabled
}

// ------------------------------------------------------------------------------------------------ the peer check

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerCred {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

/// The credentials the kernel recorded when the peer connected (`SO_PEERCRED`); they cannot be forged by the peer.
pub fn peer_cred(s: &UnixStream) -> std::io::Result<PeerCred> {
    let mut c = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as u32;
    // SAFETY: c and len are valid for the call and len is the size of c.
    let r = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut c as *mut libc::ucred).cast(), &mut len) };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(PeerCred { pid: c.pid, uid: c.uid, gid: c.gid })
}

/// Who may use the helper. An empty list allows nobody.
#[derive(Clone, Debug, Default)]
pub struct PeerPolicy {
    pub allowed_uids: Vec<u32>,
}

impl PeerPolicy {
    pub fn only(uid: u32) -> PeerPolicy {
        PeerPolicy { allowed_uids: vec![uid] }
    }
    pub fn allows(&self, c: &PeerCred) -> bool {
        self.allowed_uids.contains(&c.uid)
    }
}

// ------------------------------------------------------------------------------------------------ the wire

/// A WireGuard device as it crosses the socket. Keys are base64. Deliberately no `Debug`.
#[derive(Serialize, Deserialize)]
pub struct WgWire {
    pub ifname: String,
    pub private_key: Option<String>,
    pub listen_port: Option<u16>,
    pub fwmark: Option<u32>,
    pub replace_peers: bool,
    pub peers: Vec<PeerWire>,
}

#[derive(Serialize, Deserialize)]
pub struct PeerWire {
    pub public_key: String,
    pub preshared: Option<String>,
    pub endpoint: Option<SocketAddr>,
    pub keepalive: Option<u16>,
    pub allowed_ips: Vec<Cidr>,
    pub replace_allowed_ips: bool,
    pub remove: bool,
}

#[derive(Serialize, Deserialize)]
struct StatusWire {
    ifname: String,
    public_key: Option<String>,
    listen_port: u16,
    peers: Vec<PeerStatusWire>,
}

#[derive(Serialize, Deserialize)]
struct PeerStatusWire {
    public_key: String,
    endpoint: Option<SocketAddr>,
    keepalive: u16,
    last_handshake: Option<i64>,
    rx_bytes: u64,
    tx_bytes: u64,
    allowed_ips: Vec<Cidr>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Req {
    Ping,
    NetInfo,
    Resolve { host: String },
    Ip { args: Vec<String> },
    LinkExists { ifname: String },
    WgConfigure { dev: WgWire },
    WgStatus { ifname: String },
    ReadResolv,
    WriteResolv { text: String },
    ReadBackup,
    WriteBackup { text: String },
    DropBackup,
    InstallGuard { spec: KillSwitchSpec },
    RemoveGuard,
    GuardPresent,
    InstallCapture { spec: DnsCaptureSpec },
    RemoveCapture,
    GuardSet { ip: IpAddr, expires_in: Option<u64>, add: bool },
}

impl Req {
    /// The name that goes in the log. Never a value.
    pub fn name(&self) -> &'static str {
        match self {
            Req::Ping => "ping",
            Req::NetInfo => "net_info",
            Req::Resolve { .. } => "resolve",
            Req::Ip { .. } => "ip",
            Req::LinkExists { .. } => "link_exists",
            Req::WgConfigure { .. } => "wg_configure",
            Req::WgStatus { .. } => "wg_status",
            Req::ReadResolv => "read_resolv",
            Req::WriteResolv { .. } => "write_resolv",
            Req::ReadBackup => "read_backup",
            Req::WriteBackup { .. } => "write_backup",
            Req::DropBackup => "drop_backup",
            Req::InstallGuard { .. } => "install_guard",
            Req::RemoveGuard => "remove_guard",
            Req::GuardPresent => "guard_present",
            Req::InstallCapture { .. } => "install_capture",
            Req::RemoveCapture => "remove_capture",
            Req::GuardSet { .. } => "guard_set",
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Resp {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub value: Value,
}

impl Resp {
    fn ok(v: Value) -> Resp {
        Resp { ok: true, error: None, value: v }
    }
    fn err(e: impl Into<String>) -> Resp {
        Resp { ok: false, error: Some(e.into()), value: Value::Null }
    }
}

// ------------------------------------------------------------------------------------------------ validation

fn token_ok(t: &str) -> bool {
    !t.is_empty() && t.len() <= 64 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b".:/_-".contains(&b)) && !t.starts_with('-')
}

/// The only `ip` command lines the helper runs (exactly what the plans produce):
/// `link add dev iyvpn0 type wireguard`, `link set dev iyvpn0 [mtu N] up`, `link del dev iyvpn0`,
/// `address add CIDR dev iyvpn0`, `route add|del CIDR [via IP] dev IFNAME`.
pub fn validate_ip_args(args: &[String]) -> Result<(), String> {
    if args.is_empty() || args.len() > 10 || !args.iter().all(|a| token_ok(a)) {
        return Err("not an allowed ip command".into());
    }
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let ok = match a.as_slice() {
        ["link", "add", "dev", n, "type", "wireguard"] => *n == IFNAME,
        ["link", "del", "dev", n] => *n == IFNAME,
        ["link", "set", "dev", n, "up"] => *n == IFNAME,
        ["link", "set", "dev", n, "mtu", m, "up"] => *n == IFNAME && m.parse::<u16>().is_ok_and(|m| (576..=9000).contains(&m)),
        ["address", "add", c, "dev", n] => *n == IFNAME && Cidr::parse(c).is_some(),
        ["route", op @ ("add" | "del"), c, "dev", n] => {
            let _ = op;
            Cidr::parse(c).is_some() && valid_ifname(n)
        }
        ["route", "add" | "del", c, "via", g, "dev", n] => Cidr::parse(c).is_some() && g.parse::<IpAddr>().is_ok() && valid_ifname(n),
        _ => false,
    };
    // a route may never replace the default route, whatever device it names
    let default_route = a.first() == Some(&"route") && matches!(a.get(2), Some(&"default") | Some(&"0.0.0.0/0") | Some(&"::/0"));
    if ok && !default_route {
        Ok(())
    } else {
        Err("not an allowed ip command".into())
    }
}

fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && h.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_:".contains(&b))
}

// ------------------------------------------------------------------------------------------------ the server

fn to_wg(d: &WgWire) -> Result<(Option<Secret>, Vec<(PublicKey, Option<Secret>)>), String> {
    let key = match &d.private_key {
        Some(k) => Some(Secret::from_key_b64(k).ok_or("the private key is not 32 bytes of base64")?),
        None => None,
    };
    let mut peers = Vec::new();
    for p in &d.peers {
        let pk = PublicKey::parse(&p.public_key).ok_or("a public key is not 32 bytes of base64")?;
        let psk = match &p.preshared {
            Some(k) => Some(Secret::from_key_b64(k).ok_or("a preshared key is not 32 bytes of base64")?),
            None => None,
        };
        peers.push((pk, psk));
    }
    Ok((key, peers))
}

/// Carries out one request on `sys`. Pure of sockets, so the tests call it directly too.
pub fn dispatch(sys: &dyn Sys, req: Req) -> Resp {
    fn val<T: Serialize>(v: T) -> Resp {
        match serde_json::to_value(v) {
            Ok(v) => Resp::ok(v),
            Err(e) => Resp::err(e.to_string()),
        }
    }
    fn unit(r: Result<(), String>) -> Resp {
        match r {
            Ok(()) => Resp::ok(Value::Null),
            Err(e) => Resp::err(e),
        }
    }
    match req {
        Req::Ping => Resp::ok(json!("pong")),
        Req::NetInfo => match sys.net_info() {
            Ok(n) => val(n),
            Err(e) => Resp::err(e),
        },
        Req::Resolve { host } => {
            if !host_ok(&host) {
                return Resp::err("not a host name");
            }
            match sys.resolve(&host) {
                Ok(v) => val(v),
                Err(e) => Resp::err(e),
            }
        }
        Req::Ip { args } => match validate_ip_args(&args) {
            Ok(()) => unit(sys.ip(&args)),
            Err(e) => Resp::err(e),
        },
        Req::LinkExists { ifname } => val(valid_ifname(&ifname) && sys.link_exists(&ifname)),
        Req::WgConfigure { dev } => {
            if dev.ifname != IFNAME {
                return Resp::err("WireGuard may only be configured on the VPN interface");
            }
            let (key, peers) = match to_wg(&dev) {
                Ok(x) => x,
                Err(e) => return Resp::err(e),
            };
            let mut ps: Vec<PeerSet> = Vec::new();
            for (i, p) in dev.peers.iter().enumerate() {
                let mut x = PeerSet::new(&peers[i].0, &p.allowed_ips);
                x.preshared = peers[i].1.as_ref();
                x.endpoint = p.endpoint;
                x.keepalive = p.keepalive;
                x.replace_allowed_ips = p.replace_allowed_ips;
                x.remove = p.remove;
                ps.push(x);
            }
            let set = DeviceSet { ifname: IFNAME, private_key: key.as_ref(), listen_port: dev.listen_port, fwmark: dev.fwmark, replace_peers: dev.replace_peers, peers: ps };
            unit(sys.wg_configure(&set))
        }
        Req::WgStatus { ifname } => {
            if ifname != IFNAME {
                return Resp::err("status only for the VPN interface");
            }
            match sys.wg_status(&ifname) {
                Ok(d) => val(StatusWire {
                    ifname: d.ifname,
                    public_key: d.public_key.map(|k| k.to_string()),
                    listen_port: d.listen_port,
                    peers: d.peers.into_iter().map(|p| PeerStatusWire { public_key: p.public_key.to_string(), endpoint: p.endpoint, keepalive: p.keepalive, last_handshake: p.last_handshake, rx_bytes: p.rx_bytes, tx_bytes: p.tx_bytes, allowed_ips: p.allowed_ips }).collect(),
                }),
                Err(e) => Resp::err(e),
            }
        }
        Req::ReadResolv => val(sys.read_resolv()),
        Req::WriteResolv { text } => {
            if text.len() > 4096 {
                return Resp::err("resolv.conf text too long");
            }
            unit(sys.write_resolv(&text))
        }
        Req::ReadBackup => val(sys.read_backup()),
        Req::WriteBackup { text } => {
            if text.len() > 4096 {
                return Resp::err("resolv.conf text too long");
            }
            unit(sys.write_backup(&text))
        }
        Req::DropBackup => {
            sys.drop_backup();
            Resp::ok(Value::Null)
        }
        Req::InstallGuard { spec } => unit(sys.install_guard(&spec)),
        Req::RemoveGuard => {
            sys.remove_guard();
            Resp::ok(Value::Null)
        }
        Req::GuardPresent => val(sys.guard_present()),
        Req::InstallCapture { spec } => unit(sys.install_dns_capture(&spec)),
        Req::RemoveCapture => {
            sys.remove_dns_capture();
            Resp::ok(Value::Null)
        }
        Req::GuardSet { ip, expires_in, add } => unit(sys.guard_set(ip, expires_in.map(|s| s.min(86_400)), add)),
    }
}

/// Makes the socket: removes a stale file (only if it is a socket nobody answers on), binds, sets `0600`, and gives it to
/// `owner_uid` when the helper may chown. The directory is created `0755`; the credential check does the real gating.
pub fn bind_socket(path: &Path, owner_uid: Option<u32>) -> std::io::Result<UnixListener> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    if let Ok(md) = std::fs::symlink_metadata(path) {
        use std::os::unix::fs::FileTypeExt;
        if !md.file_type().is_socket() {
            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "the helper's socket path is taken by something that is not a socket"));
        }
        if UnixStream::connect(path).is_ok() {
            return Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "another helper is already listening"));
        }
        std::fs::remove_file(path)?;
    }
    let l = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    if let Some(uid) = owner_uid {
        let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| std::io::Error::other("bad path"))?;
        // SAFETY: c is a valid NUL-terminated path; gid -1 leaves the group alone. Failure (not root) is fine: the peer check gates.
        let _ = unsafe { libc::chown(c.as_ptr(), uid, u32::MAX) };
    }
    Ok(l)
}

/// What a running helper is, for tests and for the caller that wants to stop it.
pub struct Helper {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    pub path: PathBuf,
    /// Operation names only, in order (never a value).
    pub log: Arc<Mutex<Vec<String>>>,
    active: Arc<AtomicUsize>,
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        // connection threads notice the stop flag within one read timeout
        for _ in 0..40 {
            if self.active.load(Ordering::SeqCst) == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Runs the helper on a thread until dropped.
pub fn serve(listener: UnixListener, path: PathBuf, policy: PeerPolicy, sys: Arc<dyn Sys>) -> std::io::Result<Helper> {
    listener.set_nonblocking(true)?;
    let stop = Arc::new(AtomicBool::new(false));
    let log = Arc::new(Mutex::new(Vec::new()));
    let (st, lg) = (stop.clone(), log.clone());
    let active = Arc::new(AtomicUsize::new(0));
    let act = active.clone();
    let handle = std::thread::Builder::new().name("vpn-helper".into()).spawn(move || {
        let active = act;
        while !st.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((s, _)) => {
                    let cred = peer_cred(&s);
                    let allowed = cred.as_ref().is_ok_and(|c| policy.allows(c));
                    if !allowed {
                        lg.lock().unwrap().push("refused: peer not allowed".into());
                        continue; // dropping the stream closes it without a word
                    }
                    if active.load(Ordering::SeqCst) >= MAX_CONNS {
                        lg.lock().unwrap().push("refused: too many connections".into());
                        continue;
                    }
                    active.fetch_add(1, Ordering::SeqCst);
                    let (sys, lg, st, active) = (sys.clone(), lg.clone(), st.clone(), active.clone());
                    std::thread::spawn(move || {
                        conn(s, &*sys, &lg, &st);
                        active.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    })?;
    Ok(Helper { stop, handle: Some(handle), path, log, active })
}

fn conn(s: UnixStream, sys: &dyn Sys, log: &Mutex<Vec<String>>, stop: &AtomicBool) {
    let _ = s.set_nonblocking(false);
    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(10)));
    let Ok(mut w) = s.try_clone() else { return };
    let mut r = BufReader::new(s);
    let mut idle = Duration::ZERO;
    // kept across wake-ups: a request that arrives in pieces is not lost when a read times out
    let mut line = Vec::new();
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match read_line_bounded(&mut r, &mut line) {
            Ok(Some(())) => idle = Duration::ZERO,
            Ok(None) => return,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {
                idle += Duration::from_millis(500);
                if idle >= IDLE {
                    return;
                }
                continue;
            }
            Err(_) => return,
        }
        let parsed = serde_json::from_slice::<Req>(&line);
        line.clear();
        let resp = match parsed {
            Ok(req) => {
                log.lock().unwrap().push(req.name().to_string());
                dispatch(sys, req)
            }
            Err(_) => {
                log.lock().unwrap().push("bad request".into());
                Resp::err("not a request")
            }
        };
        let mut out = serde_json::to_vec(&resp).unwrap_or_else(|_| b"{\"ok\":false}".to_vec());
        out.push(b'\n');
        if w.write_all(&out).is_err() {
            return;
        }
    }
}

/// One line, at most [`MAX_LINE`] bytes. `Ok(None)` when the peer closed; an over-long line is an error (the connection ends).
fn read_line_bounded<R: BufRead>(r: &mut R, out: &mut Vec<u8>) -> std::io::Result<Option<()>> {
    let mut limited = r.take(MAX_LINE as u64 + 1);
    let n = limited.read_until(b'\n', out)?;
    if out.len() > MAX_LINE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "line too long"));
    }
    if n == 0 {
        return Ok(None);
    }
    if out.last() != Some(&b'\n') {
        // a piece of a line (the peer is slow or closed): the caller keeps `out` and reads on; a close shows as n == 0 next time
        return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "incomplete line"));
    }
    out.pop();
    Ok(Some(()))
}

// ------------------------------------------------------------------------------------------------ the client

/// A [`Sys`] that asks the helper. One connection, re-opened when it breaks; every call has a timeout.
pub struct RemoteSys {
    path: PathBuf,
    conn: Mutex<Option<BufReader<UnixStream>>>,
}

impl RemoteSys {
    pub fn new(path: impl Into<PathBuf>) -> RemoteSys {
        RemoteSys { path: path.into(), conn: Mutex::new(None) }
    }

    fn open(&self) -> Result<BufReader<UnixStream>, String> {
        let s = UnixStream::connect(&self.path).map_err(|e| format!("the VPN helper is not reachable ({e})"))?;
        s.set_read_timeout(Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;
        s.set_write_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
        Ok(BufReader::new(s))
    }

    fn call(&self, req: &Req) -> Result<Value, String> {
        let mut line = serde_json::to_vec(req).map_err(|e| e.to_string())?;
        line.push(b'\n');
        if line.len() > MAX_LINE {
            return Err("request too large".into());
        }
        let mut guard = self.conn.lock().unwrap();
        for attempt in 0..2 {
            if guard.is_none() {
                *guard = Some(self.open()?);
            }
            let c = guard.as_mut().unwrap();
            let sent = c.get_mut().write_all(&line);
            let mut buf = Vec::new();
            let got = sent.and_then(|_| c.by_ref().take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf));
            match got {
                Ok(n) if n > 0 && buf.last() == Some(&b'\n') => {
                    let r: Resp = serde_json::from_slice(&buf).map_err(|_| "the VPN helper sent something that is not an answer".to_string())?;
                    return if r.ok { Ok(r.value) } else { Err(r.error.unwrap_or_else(|| "the VPN helper refused".into())) };
                }
                _ => {
                    *guard = None; // broken or closed: try once more on a fresh connection
                    if attempt == 1 {
                        return Err("the VPN helper did not answer".into());
                    }
                }
            }
        }
        Err("the VPN helper did not answer".into())
    }

    fn typed<T: for<'de> Deserialize<'de>>(&self, req: Req) -> Result<T, String> {
        serde_json::from_value(self.call(&req)?).map_err(|e| format!("unexpected answer from the VPN helper ({e})"))
    }
}

fn wire_of(d: &DeviceSet) -> WgWire {
    WgWire {
        ifname: d.ifname.to_string(),
        private_key: d.private_key.map(|k| b64::encode(k.expose())),
        listen_port: d.listen_port,
        fwmark: d.fwmark,
        replace_peers: d.replace_peers,
        peers: d
            .peers
            .iter()
            .map(|p| PeerWire { public_key: p.public_key.to_string(), preshared: p.preshared.map(|k| b64::encode(k.expose())), endpoint: p.endpoint, keepalive: p.keepalive, allowed_ips: p.allowed_ips.to_vec(), replace_allowed_ips: p.replace_allowed_ips, remove: p.remove })
            .collect(),
    }
}

impl Sys for RemoteSys {
    fn net_info(&self) -> Result<NetInfo, String> {
        self.typed(Req::NetInfo)
    }
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, String> {
        self.typed(Req::Resolve { host: host.into() })
    }
    fn ip(&self, args: &[String]) -> Result<(), String> {
        self.call(&Req::Ip { args: args.to_vec() }).map(|_| ())
    }
    fn link_exists(&self, ifname: &str) -> bool {
        self.typed(Req::LinkExists { ifname: ifname.into() }).unwrap_or(false)
    }
    fn wg_configure(&self, d: &DeviceSet) -> Result<(), String> {
        self.call(&Req::WgConfigure { dev: wire_of(d) }).map(|_| ())
    }
    fn wg_status(&self, ifname: &str) -> Result<DeviceStatus, String> {
        let w: StatusWire = self.typed(Req::WgStatus { ifname: ifname.into() })?;
        let bad = || "the VPN helper sent a key that is not valid".to_string();
        let mut peers = Vec::new();
        for p in w.peers {
            peers.push(PeerStatus { public_key: PublicKey::parse(&p.public_key).ok_or_else(bad)?, endpoint: p.endpoint, keepalive: p.keepalive, last_handshake: p.last_handshake, rx_bytes: p.rx_bytes, tx_bytes: p.tx_bytes, allowed_ips: p.allowed_ips });
        }
        Ok(DeviceStatus { ifname: w.ifname, public_key: w.public_key.as_deref().and_then(PublicKey::parse), listen_port: w.listen_port, peers })
    }
    fn read_resolv(&self) -> Option<String> {
        self.typed::<Option<String>>(Req::ReadResolv).ok().flatten()
    }
    fn write_resolv(&self, text: &str) -> Result<(), String> {
        self.call(&Req::WriteResolv { text: text.into() }).map(|_| ())
    }
    fn read_backup(&self) -> Option<String> {
        self.typed::<Option<String>>(Req::ReadBackup).ok().flatten()
    }
    fn write_backup(&self, text: &str) -> Result<(), String> {
        self.call(&Req::WriteBackup { text: text.into() }).map(|_| ())
    }
    fn drop_backup(&self) {
        let _ = self.call(&Req::DropBackup);
    }
    fn install_guard(&self, spec: &KillSwitchSpec) -> Result<(), String> {
        self.call(&Req::InstallGuard { spec: spec.clone() }).map(|_| ())
    }
    fn remove_guard(&self) {
        let _ = self.call(&Req::RemoveGuard);
    }
    fn guard_present(&self) -> bool {
        self.typed(Req::GuardPresent).unwrap_or(false)
    }
    fn install_dns_capture(&self, spec: &DnsCaptureSpec) -> Result<(), String> {
        self.call(&Req::InstallCapture { spec: spec.clone() }).map(|_| ())
    }
    fn remove_dns_capture(&self) {
        let _ = self.call(&Req::RemoveCapture);
    }
    fn guard_set(&self, ip: IpAddr, expires_in: Option<u64>, add: bool) -> Result<(), String> {
        self.call(&Req::GuardSet { ip, expires_in, add }).map(|_| ())
    }
}

/// The `iyakki vpn-helper` entry: listen at `socket`, allow only `uid`, run the real system layer until the process is stopped.
pub fn cli_main(args: &[String]) -> i32 {
    let mut socket = DEFAULT_SOCKET.to_string();
    let mut uid: Option<u32> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--socket" => socket = it.next().cloned().unwrap_or_default(),
            "--allow-uid" => uid = it.next().and_then(|v| v.parse().ok()),
            _ => {
                eprintln!("usage: iyakki vpn-helper --allow-uid UID [--socket PATH]");
                return 2;
            }
        }
    }
    let Some(uid) = uid else {
        eprintln!("usage: iyakki vpn-helper --allow-uid UID [--socket PATH]");
        return 2;
    };
    let path = PathBuf::from(&socket);
    let listener = match bind_socket(&path, Some(uid)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("could not listen at {socket}: {e}");
            return 1;
        }
    };
    let h = match serve(listener, path, PeerPolicy::only(uid), Arc::new(crate::vpn_sys::OsSys::default())) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("could not start: {e}");
            return 1;
        }
    };
    eprintln!("VPN helper listening at {socket} for user id {uid}");
    loop {
        std::thread::sleep(Duration::from_secs(3600));
        let _ = &h;
    }
}

#[cfg(test)]
#[path = "vpn_helper_tests.rs"]
mod tests;
