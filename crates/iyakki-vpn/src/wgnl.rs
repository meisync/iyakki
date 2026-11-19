//! WireGuard configuration over generic netlink, written from the kernel's `wireguard.h` UAPI description (our own code;
//! no `wg` tool). The messages are built and decoded by pure functions and tested against byte vectors; the conversation
//! with the kernel goes through [`GenlTransport`], which tests replace with [`FakeGenl`]. The real socket transport
//! (AF_NETLINK, NETLINK_GENERIC) is a later step and is not here.
//!
//! The device (`wgN`) is created and brought up with `ip` (see [`crate::plan`]); this only sets its key, port and peers and
//! reads its state. Attribute values are in the machine's byte order except ports, which are big endian inside sockaddrs.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::error::VpnError;
use crate::net::Cidr;
use crate::profile::PublicKey;
use crate::secret::{Secret, SecretBuf};

const NLM_F_REQUEST: u16 = 1;
const NLM_F_ACK: u16 = 4;
const NLM_F_DUMP: u16 = 0x300;
const NLA_F_NESTED: u16 = 0x8000;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;

pub const GENL_ID_CTRL: u16 = 0x10;
const CTRL_CMD_GETFAMILY: u8 = 3;
const CTRL_ATTR_FAMILY_ID: u16 = 1;
const CTRL_ATTR_FAMILY_NAME: u16 = 2;

pub const WG_GENL_NAME: &str = "wireguard";
const WG_GENL_VERSION: u8 = 1;
pub const WG_CMD_GET_DEVICE: u8 = 0;
pub const WG_CMD_SET_DEVICE: u8 = 1;

const WGDEVICE_A_IFNAME: u16 = 2;
const WGDEVICE_A_PRIVATE_KEY: u16 = 3;
const WGDEVICE_A_PUBLIC_KEY: u16 = 4;
const WGDEVICE_A_FLAGS: u16 = 5;
const WGDEVICE_A_LISTEN_PORT: u16 = 6;
const WGDEVICE_A_FWMARK: u16 = 7;
const WGDEVICE_A_PEERS: u16 = 8;

const WGPEER_A_PUBLIC_KEY: u16 = 1;
const WGPEER_A_PRESHARED_KEY: u16 = 2;
const WGPEER_A_FLAGS: u16 = 3;
const WGPEER_A_ENDPOINT: u16 = 4;
const WGPEER_A_PERSISTENT_KEEPALIVE_INTERVAL: u16 = 5;
const WGPEER_A_LAST_HANDSHAKE_TIME: u16 = 6;
const WGPEER_A_RX_BYTES: u16 = 7;
const WGPEER_A_TX_BYTES: u16 = 8;
const WGPEER_A_ALLOWEDIPS: u16 = 9;

const WGALLOWEDIP_A_FAMILY: u16 = 1;
const WGALLOWEDIP_A_IPADDR: u16 = 2;
const WGALLOWEDIP_A_CIDR_MASK: u16 = 3;

pub const WGDEVICE_F_REPLACE_PEERS: u32 = 1;
pub const WGPEER_F_REMOVE_ME: u32 = 1;
pub const WGPEER_F_REPLACE_ALLOWEDIPS: u32 = 2;

const AF_INET: u16 = 2;
const AF_INET6: u16 = 10;

fn pad4(v: &mut Vec<u8>) {
    while v.len() % 4 != 0 {
        v.push(0);
    }
}

fn attr(buf: &mut Vec<u8>, ty: u16, data: &[u8]) {
    buf.extend_from_slice(&((4 + data.len()) as u16).to_ne_bytes());
    buf.extend_from_slice(&ty.to_ne_bytes());
    buf.extend_from_slice(data);
    pad4(buf);
}

fn nest(buf: &mut Vec<u8>, ty: u16, f: impl FnOnce(&mut Vec<u8>)) {
    let start = buf.len();
    buf.extend_from_slice(&[0, 0]);
    buf.extend_from_slice(&(ty | NLA_F_NESTED).to_ne_bytes());
    f(buf);
    let len = (buf.len() - start) as u16;
    buf[start..start + 2].copy_from_slice(&len.to_ne_bytes());
}

fn cstr(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

/// Netlink header and generic netlink header, then the attributes written by `f`.
fn genl(family: u16, flags: u16, seq: u32, cmd: u8, version: u8, f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&family.to_ne_bytes());
    out.extend_from_slice(&flags.to_ne_bytes());
    out.extend_from_slice(&seq.to_ne_bytes());
    out.extend_from_slice(&0u32.to_ne_bytes()); // port id: the kernel fills the sender
    out.extend_from_slice(&[cmd, version, 0, 0]);
    f(&mut out);
    let len = out.len() as u32;
    out[0..4].copy_from_slice(&len.to_ne_bytes());
    out
}

/// Interface names the kernel accepts and that cannot smuggle anything: 1 to 15 bytes of `[A-Za-z0-9_.-]`.
pub fn valid_ifname(n: &str) -> bool {
    !n.is_empty() && n.len() <= 15 && n.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

fn ifname_check(n: &str) -> Result<(), VpnError> {
    if valid_ifname(n) {
        Ok(())
    } else {
        Err(VpnError::Netlink("not a valid interface name".into()))
    }
}

/// `CTRL_CMD_GETFAMILY` for "wireguard": asks the generic netlink controller for the family number.
pub fn get_family_request(seq: u32) -> Vec<u8> {
    genl(GENL_ID_CTRL, NLM_F_REQUEST | NLM_F_ACK, seq, CTRL_CMD_GETFAMILY, 1, |b| attr(b, CTRL_ATTR_FAMILY_NAME, &cstr(WG_GENL_NAME)))
}

/// One peer to set.
#[derive(Clone, Debug)]
pub struct PeerSet<'a> {
    pub public_key: &'a PublicKey,
    pub preshared: Option<&'a Secret>,
    pub endpoint: Option<SocketAddr>,
    pub keepalive: Option<u16>,
    pub allowed_ips: &'a [Cidr],
    pub replace_allowed_ips: bool,
    pub remove: bool,
}

impl<'a> PeerSet<'a> {
    pub fn new(public_key: &'a PublicKey, allowed_ips: &'a [Cidr]) -> PeerSet<'a> {
        PeerSet { public_key, preshared: None, endpoint: None, keepalive: None, allowed_ips, replace_allowed_ips: false, remove: false }
    }
}

/// What `WG_CMD_SET_DEVICE` sets.
#[derive(Clone, Debug)]
pub struct DeviceSet<'a> {
    pub ifname: &'a str,
    pub private_key: Option<&'a Secret>,
    pub listen_port: Option<u16>,
    pub fwmark: Option<u32>,
    /// Remove every peer that is not listed.
    pub replace_peers: bool,
    pub peers: Vec<PeerSet<'a>>,
}

fn sockaddr(a: &SocketAddr) -> Vec<u8> {
    let mut v = Vec::with_capacity(28);
    match a {
        SocketAddr::V4(s) => {
            v.extend_from_slice(&AF_INET.to_ne_bytes());
            v.extend_from_slice(&s.port().to_be_bytes());
            v.extend_from_slice(&s.ip().octets());
            v.extend_from_slice(&[0; 8]);
        }
        SocketAddr::V6(s) => {
            v.extend_from_slice(&AF_INET6.to_ne_bytes());
            v.extend_from_slice(&s.port().to_be_bytes());
            v.extend_from_slice(&s.flowinfo().to_ne_bytes());
            v.extend_from_slice(&s.ip().octets());
            v.extend_from_slice(&s.scope_id().to_ne_bytes());
        }
    }
    v
}

/// `WG_CMD_SET_DEVICE`. The message carries the private key, so it is returned in a buffer that wipes itself.
pub fn set_device(family: u16, seq: u32, d: &DeviceSet) -> Result<SecretBuf, VpnError> {
    ifname_check(d.ifname)?;
    if d.private_key.is_some_and(|k| k.as_key().is_none()) || d.peers.iter().any(|p| p.preshared.is_some_and(|k| k.as_key().is_none())) {
        return Err(VpnError::Netlink("a key is not 32 bytes".into()));
    }
    let msg = genl(family, NLM_F_REQUEST | NLM_F_ACK, seq, WG_CMD_SET_DEVICE, WG_GENL_VERSION, |b| {
        attr(b, WGDEVICE_A_IFNAME, &cstr(d.ifname));
        if d.replace_peers {
            attr(b, WGDEVICE_A_FLAGS, &WGDEVICE_F_REPLACE_PEERS.to_ne_bytes());
        }
        if let Some(k) = d.private_key.and_then(Secret::as_key) {
            attr(b, WGDEVICE_A_PRIVATE_KEY, k);
        }
        if let Some(p) = d.listen_port {
            attr(b, WGDEVICE_A_LISTEN_PORT, &p.to_ne_bytes());
        }
        if let Some(m) = d.fwmark {
            attr(b, WGDEVICE_A_FWMARK, &m.to_ne_bytes());
        }
        if !d.peers.is_empty() {
            nest(b, WGDEVICE_A_PEERS, |b| {
                for (i, p) in d.peers.iter().enumerate() {
                    nest(b, i as u16, |b| peer(b, p));
                }
            });
        }
    });
    Ok(SecretBuf(msg))
}

fn peer(b: &mut Vec<u8>, p: &PeerSet) {
    attr(b, WGPEER_A_PUBLIC_KEY, &p.public_key.0);
    let mut flags = 0;
    if p.remove {
        flags |= WGPEER_F_REMOVE_ME;
    }
    if p.replace_allowed_ips {
        flags |= WGPEER_F_REPLACE_ALLOWEDIPS;
    }
    if flags != 0 {
        attr(b, WGPEER_A_FLAGS, &flags.to_ne_bytes());
    }
    if let Some(k) = p.preshared.and_then(Secret::as_key) {
        attr(b, WGPEER_A_PRESHARED_KEY, k);
    }
    if let Some(e) = &p.endpoint {
        attr(b, WGPEER_A_ENDPOINT, &sockaddr(e));
    }
    if let Some(k) = p.keepalive {
        attr(b, WGPEER_A_PERSISTENT_KEEPALIVE_INTERVAL, &k.to_ne_bytes());
    }
    if !p.allowed_ips.is_empty() {
        nest(b, WGPEER_A_ALLOWEDIPS, |b| {
            for (i, c) in p.allowed_ips.iter().enumerate() {
                nest(b, i as u16, |b| {
                    attr(b, WGALLOWEDIP_A_FAMILY, &(if c.is_v4() { AF_INET } else { AF_INET6 }).to_ne_bytes());
                    match c.addr {
                        IpAddr::V4(a) => attr(b, WGALLOWEDIP_A_IPADDR, &a.octets()),
                        IpAddr::V6(a) => attr(b, WGALLOWEDIP_A_IPADDR, &a.octets()),
                    }
                    attr(b, WGALLOWEDIP_A_CIDR_MASK, &[c.prefix]);
                });
            }
        });
    }
}

/// `WG_CMD_GET_DEVICE` as a dump request for one interface.
pub fn get_device(family: u16, seq: u32, ifname: &str) -> Result<Vec<u8>, VpnError> {
    ifname_check(ifname)?;
    Ok(genl(family, NLM_F_REQUEST | NLM_F_DUMP, seq, WG_CMD_GET_DEVICE, WG_GENL_VERSION, |b| attr(b, WGDEVICE_A_IFNAME, &cstr(ifname))))
}

// ---- decoding -------------------------------------------------------------------------------------------------------

fn bad(what: &str) -> VpnError {
    VpnError::Netlink(format!("malformed netlink reply: {what}"))
}

/// Split a receive buffer into netlink messages (each with its header).
pub fn split_messages(buf: &[u8]) -> Result<Vec<&[u8]>, VpnError> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        if buf.len() - i < 16 {
            return Err(bad("short header"));
        }
        let len = u32::from_ne_bytes(buf[i..i + 4].try_into().unwrap()) as usize;
        if len < 16 || i + len > buf.len() {
            return Err(bad("bad message length"));
        }
        out.push(&buf[i..i + len]);
        i += (len + 3) & !3;
    }
    Ok(out)
}

fn msg_type(m: &[u8]) -> u16 {
    u16::from_ne_bytes([m[4], m[5]])
}

/// Walk attributes: `(type without the nested/byte-order flags, payload)`.
fn attrs(mut b: &[u8]) -> Result<Vec<(u16, &[u8])>, VpnError> {
    let mut out = Vec::new();
    while !b.is_empty() {
        if b.len() < 4 {
            return Err(bad("short attribute"));
        }
        let len = u16::from_ne_bytes([b[0], b[1]]) as usize;
        let ty = u16::from_ne_bytes([b[2], b[3]]) & 0x3fff;
        if len < 4 || len > b.len() {
            return Err(bad("bad attribute length"));
        }
        out.push((ty, &b[4..len]));
        let adv = ((len + 3) & !3).min(b.len());
        b = &b[adv..];
    }
    Ok(out)
}

/// A kernel acknowledgement or error: `Ok` for errno 0.
pub fn check_ack(msg: &[u8]) -> Result<(), VpnError> {
    if msg.len() >= 20 && msg_type(msg) == NLMSG_ERROR {
        let e = i32::from_ne_bytes(msg[16..20].try_into().unwrap());
        if e != 0 {
            return Err(VpnError::Netlink(errno_text(-e)));
        }
    }
    Ok(())
}

fn errno_text(e: i32) -> String {
    match e {
        1 => "operation not permitted (CAP_NET_ADMIN needed)".into(),
        19 => "no such device (is the interface up and of type wireguard?)".into(),
        22 => "invalid argument".into(),
        95 => "operation not supported (is the wireguard module loaded?)".into(),
        n => format!("kernel error {n}"),
    }
}

/// The family number out of the controller's reply.
pub fn parse_family_id(reply: &[Vec<u8>]) -> Result<u16, VpnError> {
    for m in reply {
        check_ack(m)?;
        if m.len() >= 20 && msg_type(m) == GENL_ID_CTRL {
            for (t, v) in attrs(&m[20..])? {
                if t == CTRL_ATTR_FAMILY_ID && v.len() >= 2 {
                    return Ok(u16::from_ne_bytes([v[0], v[1]]));
                }
            }
        }
    }
    Err(VpnError::Netlink("the wireguard netlink family is not available (module not loaded?)".into()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerStatus {
    pub public_key: PublicKey,
    pub endpoint: Option<SocketAddr>,
    pub keepalive: u16,
    /// Seconds since the epoch of the last handshake; `None` when there has been none.
    pub last_handshake: Option<i64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub allowed_ips: Vec<Cidr>,
}

/// State of a device. The private and preshared keys that the kernel returns are **dropped here**: status carries public data only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceStatus {
    pub ifname: String,
    pub public_key: Option<PublicKey>,
    pub listen_port: u16,
    pub peers: Vec<PeerStatus>,
}

fn read_sockaddr(v: &[u8]) -> Option<SocketAddr> {
    let fam = u16::from_ne_bytes(v.get(0..2)?.try_into().ok()?);
    let port = u16::from_be_bytes(v.get(2..4)?.try_into().ok()?);
    match fam {
        AF_INET if v.len() >= 8 => Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(&v[4..8]).ok()?)), port)),
        AF_INET6 if v.len() >= 24 => Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&v[8..24]).ok()?)), port)),
        _ => None,
    }
}

fn read_allowed(v: &[u8]) -> Result<Vec<Cidr>, VpnError> {
    let mut out = Vec::new();
    for (_, el) in attrs(v)? {
        let (mut fam, mut addr, mut mask) = (0u16, None, None);
        for (t, p) in attrs(el)? {
            match t {
                WGALLOWEDIP_A_FAMILY if p.len() >= 2 => fam = u16::from_ne_bytes([p[0], p[1]]),
                WGALLOWEDIP_A_IPADDR => addr = Some(p),
                WGALLOWEDIP_A_CIDR_MASK if !p.is_empty() => mask = Some(p[0]),
                _ => {}
            }
        }
        let ip = match (fam, addr) {
            (AF_INET, Some(a)) if a.len() == 4 => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(a).unwrap())),
            (AF_INET6, Some(a)) if a.len() == 16 => IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(a).unwrap())),
            _ => return Err(bad("allowed ip")),
        };
        out.push(Cidr::new(ip, mask.ok_or_else(|| bad("allowed ip without a prefix"))?).ok_or_else(|| bad("allowed ip prefix"))?);
    }
    Ok(out)
}

fn read_peer(v: &[u8]) -> Result<PeerStatus, VpnError> {
    let (mut key, mut endpoint, mut keepalive, mut hs, mut rx, mut tx, mut allowed) = (None, None, 0u16, None, 0u64, 0u64, Vec::new());
    for (t, p) in attrs(v)? {
        match t {
            WGPEER_A_PUBLIC_KEY => key = <[u8; 32]>::try_from(p).ok().map(PublicKey),
            WGPEER_A_ENDPOINT => endpoint = read_sockaddr(p),
            WGPEER_A_PERSISTENT_KEEPALIVE_INTERVAL if p.len() >= 2 => keepalive = u16::from_ne_bytes([p[0], p[1]]),
            WGPEER_A_LAST_HANDSHAKE_TIME if p.len() >= 8 => {
                let s = i64::from_ne_bytes(p[0..8].try_into().unwrap());
                hs = (s > 0).then_some(s);
            }
            WGPEER_A_RX_BYTES if p.len() >= 8 => rx = u64::from_ne_bytes(p[0..8].try_into().unwrap()),
            WGPEER_A_TX_BYTES if p.len() >= 8 => tx = u64::from_ne_bytes(p[0..8].try_into().unwrap()),
            WGPEER_A_ALLOWEDIPS => allowed = read_allowed(p)?,
            // WGPEER_A_PRESHARED_KEY and the rest: deliberately not read
            _ => {}
        }
    }
    Ok(PeerStatus { public_key: key.ok_or_else(|| bad("peer without a key"))?, endpoint, keepalive, last_handshake: hs, rx_bytes: rx, tx_bytes: tx, allowed_ips: allowed })
}

/// Decode the replies of a `GET_DEVICE` dump (one or several messages; a peer whose allowed IPs did not fit continues in
/// the next message and is merged by its key).
pub fn decode_device(replies: &[Vec<u8>]) -> Result<DeviceStatus, VpnError> {
    let mut st = DeviceStatus { ifname: String::new(), public_key: None, listen_port: 0, peers: Vec::new() };
    let mut any = false;
    for m in replies {
        if m.len() < 16 {
            return Err(bad("short message"));
        }
        check_ack(m)?;
        match msg_type(m) {
            NLMSG_DONE | NLMSG_ERROR => continue,
            _ => {}
        }
        if m.len() < 20 {
            return Err(bad("short message"));
        }
        any = true;
        for (t, v) in attrs(&m[20..])? {
            match t {
                WGDEVICE_A_IFNAME => st.ifname = String::from_utf8_lossy(v.split(|&b| b == 0).next().unwrap_or(&[])).into_owned(),
                WGDEVICE_A_PUBLIC_KEY => st.public_key = <[u8; 32]>::try_from(v).ok().map(PublicKey),
                WGDEVICE_A_LISTEN_PORT if v.len() >= 2 => st.listen_port = u16::from_ne_bytes([v[0], v[1]]),
                WGDEVICE_A_PEERS => {
                    for (_, el) in attrs(v)? {
                        let p = read_peer(el)?;
                        match st.peers.iter_mut().find(|q| q.public_key == p.public_key) {
                            Some(q) => q.allowed_ips.extend(p.allowed_ips),
                            None => st.peers.push(p),
                        }
                    }
                }
                // WGDEVICE_A_PRIVATE_KEY: deliberately not read
                _ => {}
            }
        }
    }
    if !any {
        return Err(VpnError::Netlink("no answer for the device".into()));
    }
    Ok(st)
}

// ---- transport ------------------------------------------------------------------------------------------------------

/// Sends one request and returns the reply messages (each a whole netlink message) up to and including the ack or the
/// end of the dump. The real one wraps a netlink socket; [`FakeGenl`] answers in memory.
pub trait GenlTransport {
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<Vec<u8>>, VpnError>;
}

/// The high-level calls on top of a transport. The family number is resolved once.
pub struct WgClient<T: GenlTransport> {
    t: T,
    family: Option<u16>,
    seq: u32,
}

impl<T: GenlTransport> WgClient<T> {
    pub fn new(t: T) -> WgClient<T> {
        WgClient { t, family: None, seq: 0 }
    }
    pub fn transport(&self) -> &T {
        &self.t
    }
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.t
    }
    fn next_seq(&mut self) -> u32 {
        self.seq += 1;
        self.seq
    }
    fn family(&mut self) -> Result<u16, VpnError> {
        if let Some(f) = self.family {
            return Ok(f);
        }
        let seq = self.next_seq();
        let reply = self.t.exchange(&get_family_request(seq))?;
        let f = parse_family_id(&reply)?;
        self.family = Some(f);
        Ok(f)
    }
    /// Set key, port and peers; fails with the kernel's reason.
    pub fn configure(&mut self, d: &DeviceSet) -> Result<(), VpnError> {
        let fam = self.family()?;
        let seq = self.next_seq();
        let msg = set_device(fam, seq, d)?;
        let reply = self.t.exchange(msg.as_bytes())?;
        drop(msg);
        reply.iter().try_for_each(|m| check_ack(m))
    }
    pub fn status(&mut self, ifname: &str) -> Result<DeviceStatus, VpnError> {
        let fam = self.family()?;
        let seq = self.next_seq();
        let reply = self.t.exchange(&get_device(fam, seq, ifname)?)?;
        decode_device(&reply)
    }
}

/// An in-memory stand-in for the kernel's wireguard family, for tests. It decodes what it is sent with the same attribute
/// reader, keeps a device per interface name and answers dumps. `fail_next` makes the next SET fail with an errno.
#[derive(Default)]
pub struct FakeGenl {
    pub family_id: u16,
    pub devices: Vec<FakeDevice>,
    pub requests: Vec<Vec<u8>>,
    pub fail_next: Option<i32>,
    /// Split dump replies so each peer is its own message.
    pub split_peers: bool,
    /// Pretend the module is not loaded.
    pub no_family: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FakeDevice {
    pub ifname: String,
    pub has_private_key: bool,
    pub listen_port: u16,
    pub fwmark: u32,
    pub peers: Vec<PeerStatus>,
    pub preshared_for: Vec<PublicKey>,
}

impl FakeGenl {
    pub fn new() -> FakeGenl {
        FakeGenl { family_id: 0x1c, ..Default::default() }
    }
    pub fn add_device(&mut self, name: &str) {
        self.devices.push(FakeDevice { ifname: name.to_string(), ..Default::default() });
    }
    pub fn device(&self, name: &str) -> Option<&FakeDevice> {
        self.devices.iter().find(|d| d.ifname == name)
    }
    fn error_msg(seq: u32, errno: i32) -> Vec<u8> {
        let mut m = Vec::new();
        m.extend_from_slice(&36u32.to_ne_bytes());
        m.extend_from_slice(&NLMSG_ERROR.to_ne_bytes());
        m.extend_from_slice(&0u16.to_ne_bytes());
        m.extend_from_slice(&seq.to_ne_bytes());
        m.extend_from_slice(&0u32.to_ne_bytes());
        m.extend_from_slice(&(-errno).to_ne_bytes());
        m.extend_from_slice(&[0; 16]); // the echoed request header
        m
    }
}

impl GenlTransport for FakeGenl {
    fn exchange(&mut self, req: &[u8]) -> Result<Vec<Vec<u8>>, VpnError> {
        self.requests.push(req.to_vec());
        let m = split_messages(req)?.into_iter().next().ok_or_else(|| bad("empty request"))?;
        let seq = u32::from_ne_bytes(m[8..12].try_into().unwrap());
        let ty = msg_type(m);
        if ty == GENL_ID_CTRL {
            if self.no_family {
                return Ok(vec![Self::error_msg(seq, 2)]);
            }
            let reply = genl(GENL_ID_CTRL, 0, seq, 1, 1, |b| {
                attr(b, CTRL_ATTR_FAMILY_ID, &self.family_id.to_ne_bytes());
                attr(b, CTRL_ATTR_FAMILY_NAME, &cstr(WG_GENL_NAME));
            });
            return Ok(vec![reply]);
        }
        if ty != self.family_id {
            return Ok(vec![Self::error_msg(seq, 22)]);
        }
        let cmd = m[16];
        let at = attrs(&m[20..])?;
        let name = at.iter().find(|(t, _)| *t == WGDEVICE_A_IFNAME).map(|(_, v)| String::from_utf8_lossy(v.split(|&b| b == 0).next().unwrap_or(&[])).into_owned()).unwrap_or_default();
        let Some(idx) = self.devices.iter().position(|d| d.ifname == name) else {
            return Ok(vec![Self::error_msg(seq, 19)]);
        };
        match cmd {
            WG_CMD_SET_DEVICE => {
                if let Some(e) = self.fail_next.take() {
                    return Ok(vec![Self::error_msg(seq, e)]);
                }
                let dev = &mut self.devices[idx];
                let mut replace = false;
                for (t, v) in &at {
                    match *t {
                        WGDEVICE_A_FLAGS if v.len() >= 4 => replace = u32::from_ne_bytes(v[0..4].try_into().unwrap()) & WGDEVICE_F_REPLACE_PEERS != 0,
                        _ => {}
                    }
                }
                if replace {
                    dev.peers.clear();
                }
                for (t, v) in &at {
                    match *t {
                        WGDEVICE_A_PRIVATE_KEY if v.len() == 32 => dev.has_private_key = true,
                        WGDEVICE_A_LISTEN_PORT if v.len() >= 2 => dev.listen_port = u16::from_ne_bytes([v[0], v[1]]),
                        WGDEVICE_A_FWMARK if v.len() >= 4 => dev.fwmark = u32::from_ne_bytes(v[0..4].try_into().unwrap()),
                        WGDEVICE_A_PEERS => {
                            for (_, el) in attrs(v)? {
                                let mut flags = 0u32;
                                let mut psk = false;
                                for (pt, pv) in attrs(el)? {
                                    if pt == WGPEER_A_FLAGS && pv.len() >= 4 {
                                        flags = u32::from_ne_bytes(pv[0..4].try_into().unwrap());
                                    }
                                    if pt == WGPEER_A_PRESHARED_KEY {
                                        psk = true;
                                    }
                                }
                                let mut p = read_peer(el)?;
                                if flags & WGPEER_F_REMOVE_ME != 0 {
                                    dev.peers.retain(|q| q.public_key != p.public_key);
                                    continue;
                                }
                                if psk && !dev.preshared_for.contains(&p.public_key) {
                                    dev.preshared_for.push(p.public_key);
                                }
                                match dev.peers.iter_mut().find(|q| q.public_key == p.public_key) {
                                    Some(q) => {
                                        if p.endpoint.is_some() {
                                            q.endpoint = p.endpoint;
                                        }
                                        if p.keepalive != 0 {
                                            q.keepalive = p.keepalive;
                                        }
                                        if flags & WGPEER_F_REPLACE_ALLOWEDIPS != 0 {
                                            q.allowed_ips.clear();
                                        }
                                        q.allowed_ips.append(&mut p.allowed_ips);
                                    }
                                    None => dev.peers.push(p),
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Ok(vec![Self::error_msg(seq, 0)])
            }
            WG_CMD_GET_DEVICE => {
                let dev = &self.devices[idx];
                let dev_attrs = |b: &mut Vec<u8>| {
                    attr(b, WGDEVICE_A_IFNAME, &cstr(&dev.ifname));
                    attr(b, WGDEVICE_A_PRIVATE_KEY, &[0x55; 32]); // the kernel does return it; the decoder must not keep it
                    attr(b, WGDEVICE_A_PUBLIC_KEY, &[0xAB; 32]);
                    attr(b, WGDEVICE_A_LISTEN_PORT, &dev.listen_port.to_ne_bytes());
                };
                let write_peer = |b: &mut Vec<u8>, p: &PeerStatus| {
                    attr(b, WGPEER_A_PUBLIC_KEY, &p.public_key.0);
                    attr(b, WGPEER_A_PRESHARED_KEY, &[0x66; 32]);
                    if let Some(e) = &p.endpoint {
                        attr(b, WGPEER_A_ENDPOINT, &sockaddr(e));
                    }
                    attr(b, WGPEER_A_PERSISTENT_KEEPALIVE_INTERVAL, &p.keepalive.to_ne_bytes());
                    let mut ts = p.last_handshake.unwrap_or(0).to_ne_bytes().to_vec();
                    ts.extend_from_slice(&0i64.to_ne_bytes());
                    attr(b, WGPEER_A_LAST_HANDSHAKE_TIME, &ts);
                    attr(b, WGPEER_A_RX_BYTES, &p.rx_bytes.to_ne_bytes());
                    attr(b, WGPEER_A_TX_BYTES, &p.tx_bytes.to_ne_bytes());
                    if !p.allowed_ips.is_empty() {
                        nest(b, WGPEER_A_ALLOWEDIPS, |b| {
                            for (i, c) in p.allowed_ips.iter().enumerate() {
                                nest(b, i as u16, |b| {
                                    attr(b, WGALLOWEDIP_A_FAMILY, &(if c.is_v4() { AF_INET } else { AF_INET6 }).to_ne_bytes());
                                    match c.addr {
                                        IpAddr::V4(a) => attr(b, WGALLOWEDIP_A_IPADDR, &a.octets()),
                                        IpAddr::V6(a) => attr(b, WGALLOWEDIP_A_IPADDR, &a.octets()),
                                    }
                                    attr(b, WGALLOWEDIP_A_CIDR_MASK, &[c.prefix]);
                                });
                            }
                        });
                    }
                };
                let mut out = Vec::new();
                if self.split_peers && !dev.peers.is_empty() {
                    for p in &dev.peers {
                        out.push(genl(self.family_id, 2, seq, WG_CMD_GET_DEVICE, 1, |b| {
                            dev_attrs(b);
                            nest(b, WGDEVICE_A_PEERS, |b| nest(b, 0, |b| write_peer(b, p)));
                        }));
                    }
                } else {
                    out.push(genl(self.family_id, 2, seq, WG_CMD_GET_DEVICE, 1, |b| {
                        dev_attrs(b);
                        if !dev.peers.is_empty() {
                            nest(b, WGDEVICE_A_PEERS, |b| {
                                for (i, p) in dev.peers.iter().enumerate() {
                                    nest(b, i as u16, |b| write_peer(b, p));
                                }
                            });
                        }
                    }));
                }
                // NLMSG_DONE
                let mut done = Vec::new();
                done.extend_from_slice(&20u32.to_ne_bytes());
                done.extend_from_slice(&NLMSG_DONE.to_ne_bytes());
                done.extend_from_slice(&2u16.to_ne_bytes());
                done.extend_from_slice(&seq.to_ne_bytes());
                done.extend_from_slice(&0u32.to_ne_bytes());
                done.extend_from_slice(&0i32.to_ne_bytes());
                out.push(done);
                Ok(out)
            }
            _ => Ok(vec![Self::error_msg(seq, 95)]),
        }
    }
}
