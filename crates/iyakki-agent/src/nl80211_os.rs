//! The operating-system side of the Wi-Fi hotspot's radio control (plan: docs/development/HOTSPOT-PLAN.md, phase P1).
//!
//! `iyakki-hotspot` is `#![forbid(unsafe_code)]`, so the real sockets live here, with a minimal `unsafe` block per system call:
//!
//! * [`Nl80211Socket`]: a generic netlink socket that implements [`NetlinkTransport`]. It sends a request, collects the answers
//!   that carry the request's sequence number up to the acknowledgement (or the end of a dump, or the one reply of a family
//!   lookup), keeps every other message (events, replies of other requests) in a queue, and gives up after a time-out.
//!   It resolves the `nl80211` family, joins its multicast groups (`config`, `mlme`) and hands out events.
//! * [`PacketEapol`]: an `AF_PACKET` channel for EAPOL frames (ether type 0x888e) on the access point's interface, behind the
//!   [`EapolChannel`] trait (the control-port route of the nl80211 is the other candidate, see the plan).
//! * small helpers: `poll` over several descriptors and a UDP socket bound to one interface.
//!
//! The framing is tested without a kernel (a datagram socket pair plays the kernel); the real thing is tested on virtual radios
//! with `crates/iyakki-hotspot/tools/hwsim-test.sh`.

use iyakki_hotspot::nl80211::{self, NetlinkTransport, NlError, NLMSG_DONE, NLMSG_ERROR};
use iyakki_hotspot::Mac;
use std::collections::VecDeque;
use std::net::UdpSocket;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::{Duration, Instant};

const NETLINK_GENERIC: i32 = 16;
const SOL_NETLINK: i32 = 270;
const NETLINK_ADD_MEMBERSHIP: i32 = 1;
const NETLINK_EXT_ACK: i32 = 11;
const CTRL_ATTR_MCAST_GROUPS: u16 = 7;
const CTRL_ATTR_MCAST_GRP_NAME: u16 = 1;
const CTRL_ATTR_MCAST_GRP_ID: u16 = 2;
const NLMSGERR_ATTR_MSG: u16 = 1;
/// Sequence numbers of the socket's own requests (the control layer above counts up from 1).
const OWN_SEQ_BASE: u32 = 0x4000_0000;

/// The `nl80211` family as the controller describes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Family {
    pub id: u16,
    /// Multicast groups by name (`config`, `scan`, `regulatory`, `mlme`, `vendor`, `nan`, `testmode`).
    pub groups: Vec<(String, u32)>,
}

impl Family {
    pub fn group(&self, name: &str) -> Option<u32> {
        self.groups.iter().find(|(n, _)| n == name).map(|(_, id)| *id)
    }
}

/// The family id and the multicast groups out of the answer to `CTRL_CMD_GETFAMILY`.
pub fn parse_family(reply: &[u8]) -> Option<Family> {
    for m in nl80211::split_messages(reply)? {
        if m.ty != nl80211::GENL_ID_CTRL {
            continue;
        }
        let (_, a) = nl80211::genl(m.payload)?;
        let a = nl80211::attrs(a)?;
        let id = a.iter().find(|(t, _)| *t == nl80211::CTRL_ATTR_FAMILY_ID).and_then(|(_, v)| <[u8; 2]>::try_from(*v).ok()).map(u16::from_le_bytes)?;
        let mut fam = Family { id, groups: vec![] };
        if let Some((_, g)) = a.iter().find(|(t, _)| *t == CTRL_ATTR_MCAST_GROUPS) {
            for (_, one) in nl80211::attrs(g)? {
                let f = nl80211::attrs(one)?;
                let name = f.iter().find(|(t, _)| *t == CTRL_ATTR_MCAST_GRP_NAME).map(|(_, v)| String::from_utf8_lossy(v.split(|b| *b == 0).next().unwrap_or(&[])).into_owned());
                let gid = f.iter().find(|(t, _)| *t == CTRL_ATTR_MCAST_GRP_ID).and_then(|(_, v)| <[u8; 4]>::try_from(*v).ok()).map(u32::from_le_bytes);
                if let (Some(n), Some(i)) = (name, gid) {
                    fam.groups.push((n, i));
                }
            }
        }
        return Some(fam);
    }
    None
}

/// The text the kernel adds to an error acknowledgement (extended ack), if the message has one.
fn ext_ack_text(payload: &[u8]) -> Option<String> {
    // errno (4) | the request's header (16, and the request body unless the kernel capped it) | attributes
    let orig_len = u32::from_le_bytes(payload.get(4..8)?.try_into().ok()?) as usize;
    let capped = u16::from_le_bytes(payload.get(10..12)?.try_into().ok()?) & 0x100 != 0;
    let skip = if capped { 20 } else { 4 + ((orig_len + 3) & !3) };
    let a = nl80211::attrs(payload.get(skip..)?)?;
    a.iter().find(|(t, _)| *t == NLMSGERR_ATTR_MSG).map(|(_, v)| String::from_utf8_lossy(v.split(|b| *b == 0).next().unwrap_or(&[])).into_owned())
}

/// One message cut out of a datagram, with its raw bytes.
struct Raw<'a> {
    ty: u16,
    seq: u32,
    bytes: &'a [u8],
}

fn cut(mut b: &[u8]) -> Option<Vec<Raw<'_>>> {
    let mut v = Vec::new();
    while !b.is_empty() {
        if b.len() < 16 {
            return None;
        }
        let len = u32::from_le_bytes(b[0..4].try_into().ok()?) as usize;
        if len < 16 || len > b.len() {
            return None;
        }
        v.push(Raw { ty: u16::from_le_bytes([b[4], b[5]]), seq: u32::from_le_bytes(b[8..12].try_into().ok()?), bytes: &b[..len] });
        b = b.get(((len + 3) & !3)..).unwrap_or(&[]);
    }
    Some(v)
}

fn io_err(what: &str) -> NlError {
    NlError::Io(format!("{what}: {}", std::io::Error::last_os_error()))
}

/// Wait until one of the descriptors is readable; returns which ones are. `timeout_ms` below 0 waits for ever.
pub fn poll_readable(fds: &[RawFd], timeout_ms: i32) -> Vec<bool> {
    let mut p: Vec<libc::pollfd> = fds.iter().map(|fd| libc::pollfd { fd: *fd, events: libc::POLLIN, revents: 0 }).collect();
    // SAFETY: `p` is a valid array of `p.len()` pollfd structures for the whole call.
    let r = unsafe { libc::poll(p.as_mut_ptr(), p.len() as libc::nfds_t, timeout_ms) };
    if r <= 0 {
        return vec![false; fds.len()];
    }
    p.iter().map(|x| x.revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP) != 0).collect()
}

fn recv_datagram(fd: RawFd, timeout_ms: i32) -> Result<Option<Vec<u8>>, NlError> {
    if !poll_readable(&[fd], timeout_ms)[0] {
        return Ok(None);
    }
    let mut buf = vec![0u8; 65536];
    // SAFETY: `buf` is valid for writes of `buf.len()` bytes.
    let n = unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), libc::MSG_DONTWAIT) };
    if n < 0 {
        let e = std::io::Error::last_os_error();
        return if e.kind() == std::io::ErrorKind::WouldBlock { Ok(None) } else { Err(NlError::Io(format!("reading from the Wi-Fi driver: {e}"))) };
    }
    buf.truncate(n as usize);
    Ok(Some(buf))
}

/// A generic netlink socket speaking to the `nl80211` family (or, in tests, to a fake kernel on a datagram socket pair).
pub struct Nl80211Socket {
    fd: OwnedFd,
    own_seq: u32,
    timeout: Duration,
    events: VecDeque<Vec<u8>>,
    family: Option<Family>,
    ext_ack: Option<String>,
}

impl Nl80211Socket {
    /// Open and bind a `NETLINK_GENERIC` socket (needs the right to control the Wi-Fi card, normally root).
    pub fn open(timeout: Duration) -> Result<Nl80211Socket, NlError> {
        // SAFETY: plain system call with constant arguments.
        let raw = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, NETLINK_GENERIC) };
        if raw < 0 {
            return Err(io_err("opening the netlink socket"));
        }
        // SAFETY: `raw` is a freshly opened descriptor that nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: an all-zero sockaddr_nl is valid; the port id 0 lets the kernel pick one.
        let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        sa.nl_family = libc::AF_NETLINK as u16;
        // SAFETY: `sa` is a valid sockaddr_nl and the size passed is its size.
        let r = unsafe { libc::bind(raw, (&sa as *const libc::sockaddr_nl).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32) };
        if r < 0 {
            return Err(io_err("binding the netlink socket"));
        }
        let one: i32 = 1;
        // SAFETY: `one` outlives the call and the size is that of an i32. A kernel without extended acks answers an error; ignored.
        unsafe { libc::setsockopt(raw, SOL_NETLINK, NETLINK_EXT_ACK, (&one as *const i32).cast(), 4) };
        Ok(Nl80211Socket::from_fd(fd, timeout))
    }

    /// Use any datagram socket as the kernel side (tests).
    pub fn from_fd(fd: OwnedFd, timeout: Duration) -> Nl80211Socket {
        Nl80211Socket { fd, own_seq: OWN_SEQ_BASE, timeout, events: VecDeque::new(), family: None, ext_ack: None }
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// The kernel's text for the last refusal (extended acknowledgement), if it gave one.
    pub fn last_ext_ack(&self) -> Option<&str> {
        self.ext_ack.as_deref()
    }

    fn send(&self, b: &[u8]) -> Result<(), NlError> {
        // SAFETY: `b` is valid for reads of its length.
        let n = unsafe { libc::send(self.fd.as_raw_fd(), b.as_ptr().cast(), b.len(), 0) };
        if n < 0 || n as usize != b.len() {
            return Err(io_err("sending to the Wi-Fi driver"));
        }
        Ok(())
    }

    /// Ask the controller for the `nl80211` family once (id and multicast groups).
    pub fn family(&mut self) -> Result<Family, NlError> {
        if let Some(f) = &self.family {
            return Ok(f.clone());
        }
        let seq = self.next_seq();
        let r = self.exchange(&nl80211::get_family_request(seq))?;
        nl80211::check_ack(&r)?;
        let f = parse_family(&r).ok_or(NlError::Malformed)?;
        self.family = Some(f.clone());
        Ok(f)
    }

    /// A sequence number for a request the control layer did not build.
    pub fn next_seq(&mut self) -> u32 {
        self.own_seq = self.own_seq.wrapping_add(1);
        self.own_seq
    }

    /// Join a multicast group by id (see [`Family::group`]).
    pub fn join_group(&mut self, id: u32) -> Result<(), NlError> {
        let g = id as i32;
        // SAFETY: `g` outlives the call and the size is that of an i32.
        let r = unsafe { libc::setsockopt(self.fd.as_raw_fd(), SOL_NETLINK, NETLINK_ADD_MEMBERSHIP, (&g as *const i32).cast(), 4) };
        if r < 0 {
            return Err(io_err("joining a Wi-Fi event group"));
        }
        Ok(())
    }

    /// Join the `config` and `mlme` groups (station added and removed, frames of the access point).
    pub fn join_ap_groups(&mut self) -> Result<(), NlError> {
        let fam = self.family()?;
        for name in ["config", "mlme"] {
            if let Some(id) = fam.group(name) {
                self.join_group(id)?;
            }
        }
        Ok(())
    }

    /// Build a request for the resolved family with a fresh sequence number, send it and check the acknowledgement.
    pub fn run(&mut self, build: impl FnOnce(u16, u32) -> Vec<u8>) -> Result<(), NlError> {
        let fam = self.family()?.id;
        let seq = self.next_seq();
        let r = self.exchange(&build(fam, seq))?;
        nl80211::check_ack(&r)
    }

    /// The next event message (one message per call), waiting up to `wait_ms`.
    pub fn next_event(&mut self, wait_ms: i32) -> Result<Option<Vec<u8>>, NlError> {
        if let Some(e) = self.events.pop_front() {
            return Ok(Some(e));
        }
        let Some(d) = recv_datagram(self.fd.as_raw_fd(), wait_ms)? else { return Ok(None) };
        for m in cut(&d).ok_or(NlError::Malformed)? {
            self.events.push_back(m.bytes.to_vec());
        }
        Ok(self.events.pop_front())
    }
}

impl NetlinkTransport for Nl80211Socket {
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>, NlError> {
        let seq = u32::from_le_bytes(request.get(8..12).ok_or(NlError::Malformed)?.try_into().map_err(|_| NlError::Malformed)?);
        self.send(request)?;
        let deadline = Instant::now() + self.timeout;
        let mut out = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(NlError::Io("the Wi-Fi driver did not answer in time".into()));
            }
            let Some(d) = recv_datagram(self.fd.as_raw_fd(), left.as_millis().clamp(1, i32::MAX as u128) as i32)? else { continue };
            let mut finished = false;
            for m in cut(&d).ok_or(NlError::Malformed)? {
                if m.seq != seq {
                    self.events.push_back(m.bytes.to_vec());
                    continue;
                }
                out.extend_from_slice(m.bytes);
                if m.ty == NLMSG_ERROR {
                    self.ext_ack = ext_ack_text(&m.bytes[16..]);
                    finished = true;
                } else if m.ty == NLMSG_DONE || m.ty == nl80211::GENL_ID_CTRL {
                    finished = true;
                }
            }
            if finished {
                return Ok(out);
            }
        }
    }
}

/// A netlink transport that several owners hold: the control layer (`ApControl`) and the event loop use the same socket,
/// because frames registered by a socket are delivered to that socket.
#[derive(Clone)]
pub struct SharedNl(pub std::rc::Rc<std::cell::RefCell<Nl80211Socket>>);

impl SharedNl {
    pub fn new(s: Nl80211Socket) -> SharedNl {
        SharedNl(std::rc::Rc::new(std::cell::RefCell::new(s)))
    }
}

impl NetlinkTransport for SharedNl {
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>, NlError> {
        self.0.borrow_mut().exchange(request)
    }
}

// ---- EAPOL over AF_PACKET ----

/// Sends and receives EAPOL frames (the bytes after the Ethernet header, starting with the EAPOL version byte).
pub trait EapolChannel {
    fn send(&mut self, sta: Mac, eapol: &[u8]) -> Result<(), String>;
    /// The next frame and the client it came from, waiting up to `wait_ms`.
    fn recv(&mut self, wait_ms: i32) -> Result<Option<(Mac, Vec<u8>)>, String>;
    fn raw_fd(&self) -> RawFd;
}

/// EAPOL (ether type 0x888e) on one interface through a packet socket. The kernel builds and strips the Ethernet header
/// (`SOCK_DGRAM`); the source of the frame is the interface's own address, which is the access point's BSSID.
pub struct PacketEapol {
    fd: OwnedFd,
    ifindex: i32,
}

const ETH_P_PAE_BE: u16 = nl80211::ETH_P_PAE.to_be();

impl PacketEapol {
    pub fn open(ifindex: u32) -> Result<PacketEapol, String> {
        // SAFETY: plain system call with constant arguments.
        let raw = unsafe { libc::socket(libc::AF_PACKET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, ETH_P_PAE_BE as i32) };
        if raw < 0 {
            return Err(format!("opening the EAPOL socket: {}", std::io::Error::last_os_error()));
        }
        // SAFETY: `raw` is a freshly opened descriptor that nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: an all-zero sockaddr_ll is valid.
        let mut sa: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
        sa.sll_family = libc::AF_PACKET as u16;
        sa.sll_protocol = ETH_P_PAE_BE;
        sa.sll_ifindex = ifindex as i32;
        // SAFETY: `sa` is a valid sockaddr_ll and the size passed is its size.
        let r = unsafe { libc::bind(raw, (&sa as *const libc::sockaddr_ll).cast(), std::mem::size_of::<libc::sockaddr_ll>() as u32) };
        if r < 0 {
            return Err(format!("binding the EAPOL socket: {}", std::io::Error::last_os_error()));
        }
        Ok(PacketEapol { fd, ifindex: ifindex as i32 })
    }
}

impl EapolChannel for PacketEapol {
    fn send(&mut self, sta: Mac, eapol: &[u8]) -> Result<(), String> {
        // SAFETY: an all-zero sockaddr_ll is valid.
        let mut sa: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
        sa.sll_family = libc::AF_PACKET as u16;
        sa.sll_protocol = ETH_P_PAE_BE;
        sa.sll_ifindex = self.ifindex;
        sa.sll_halen = 6;
        sa.sll_addr[..6].copy_from_slice(&sta);
        // SAFETY: `eapol` is valid for reads of its length; `sa` is a valid sockaddr_ll of the size passed.
        let n = unsafe { libc::sendto(self.fd.as_raw_fd(), eapol.as_ptr().cast(), eapol.len(), 0, (&sa as *const libc::sockaddr_ll).cast(), std::mem::size_of::<libc::sockaddr_ll>() as u32) };
        if n < 0 || n as usize != eapol.len() {
            return Err(format!("sending an EAPOL frame: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn recv(&mut self, wait_ms: i32) -> Result<Option<(Mac, Vec<u8>)>, String> {
        if !poll_readable(&[self.fd.as_raw_fd()], wait_ms)[0] {
            return Ok(None);
        }
        let mut buf = vec![0u8; 2048];
        // SAFETY: an all-zero sockaddr_ll is valid; the kernel fills it in.
        let mut sa: libc::sockaddr_ll = unsafe { std::mem::zeroed() };
        let mut sl = std::mem::size_of::<libc::sockaddr_ll>() as u32;
        // SAFETY: `buf` is valid for writes of its length; `sa` and `sl` are valid for the kernel to fill in.
        let n = unsafe { libc::recvfrom(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), libc::MSG_DONTWAIT, (&mut sa as *mut libc::sockaddr_ll).cast(), &mut sl) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            return if e.kind() == std::io::ErrorKind::WouldBlock { Ok(None) } else { Err(format!("reading an EAPOL frame: {e}")) };
        }
        // outgoing copies of our own frames are not wanted (packet type 4 = PACKET_OUTGOING)
        if sa.sll_pkttype == 4 || sa.sll_halen != 6 {
            return Ok(None);
        }
        buf.truncate(n as usize);
        let mut mac = [0u8; 6];
        mac.copy_from_slice(&sa.sll_addr[..6]);
        Ok(Some((mac, buf)))
    }

    fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

/// A UDP socket bound to one interface (the DHCP server must hear only the hotspot's clients) that may broadcast.
pub fn udp_on_interface(ifname: &str, port: u16) -> Result<UdpSocket, String> {
    let s = UdpSocket::bind(("0.0.0.0", port)).map_err(|e| format!("binding UDP {port}: {e}"))?;
    s.set_broadcast(true).map_err(|e| format!("allowing broadcast: {e}"))?;
    s.set_nonblocking(true).map_err(|e| format!("non-blocking socket: {e}"))?;
    let name = ifname.as_bytes();
    // SAFETY: `name` is valid for reads of its length; the kernel copies the interface name.
    let r = unsafe { libc::setsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_BINDTODEVICE, name.as_ptr().cast(), name.len() as u32) };
    if r < 0 {
        return Err(format!("binding to {ifname}: {}", std::io::Error::last_os_error()));
    }
    Ok(s)
}

// ---- capability probe (generic: the kernel's answer decides, the driver name is only shown) ----

use iyakki_hotspot::caps::{self, PhyCaps};

/// Every radio the kernel exposes, with the regulatory country in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub phys: Vec<PhyCaps>,
    pub country: String,
}

/// The driver's name for a radio, from sysfs, for the explanation shown to a person only. Never used to decide anything.
pub fn driver_name(phy: &str) -> Option<String> {
    if phy.is_empty() || !phy.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
        return None;
    }
    let l = std::fs::read_link(format!("/sys/class/ieee80211/{phy}/device/driver")).ok()?;
    Some(l.file_name()?.to_string_lossy().into_owned())
}

/// True when a PCI network controller of the wireless kind (class 0x0280) exists: a Wi-Fi chip is there even if no radio is.
pub fn wifi_hardware_seen() -> bool {
    std::fs::read_dir("/sys/bus/pci/devices").map(|d| d.flatten().any(|e| std::fs::read_to_string(e.path().join("class")).is_ok_and(|c| c.trim().starts_with("0x0280")))).unwrap_or(false)
}

/// Ask the kernel what every radio can do and which country rules apply.
pub fn probe(s: &mut Nl80211Socket) -> Result<Probe, NlError> {
    let fam = s.family()?.id;
    let seq = s.next_seq();
    let r = s.exchange(&caps::get_wiphy_split_dump(fam, seq))?;
    nl80211::check_ack(&r)?;
    let mut phys = caps::parse_phy_dump(&r).ok_or(NlError::Malformed)?;
    for p in &mut phys {
        p.driver = driver_name(&p.name);
    }
    let seq = s.next_seq();
    let r = s.exchange(&caps::get_reg_request(fam, seq))?;
    let country = caps::parse_country(&r).unwrap_or_else(|| "00".to_string());
    Ok(Probe { phys, country })
}

/// Set the regulatory country (`iw reg set`); a radio whose firmware decides the rules may ignore it.
pub fn set_country(s: &mut Nl80211Socket, alpha2: &str) -> Result<(), NlError> {
    let fam = s.family()?.id;
    let seq = s.next_seq();
    let req = caps::req_set_reg(fam, seq, alpha2).ok_or(NlError::Malformed)?;
    let r = s.exchange(&req)?;
    nl80211::check_ack(&r)
}

#[cfg(test)]
#[path = "nl80211_os_tests.rs"]
mod tests;
