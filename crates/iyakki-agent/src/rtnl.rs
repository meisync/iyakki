//! Link and address set-up through the kernel's routing netlink (`NETLINK_ROUTE`), in our own code, in place of interface ioctls and the
//! `ip` program (F-HOTSPOT-18): switch an interface up or down, add or remove an IPv4 address. The messages are plain bytes built by pure
//! functions (golden-byte tests); [`Rtnl`] sends one through any [`RtnlTransport`], and [`RouteSocket`] is the real one (Linux only).
// SPDX-License-Identifier: Apache-2.0

use std::net::Ipv4Addr;

const RTM_NEWLINK: u16 = 16;
const RTM_NEWADDR: u16 = 20;
const RTM_DELADDR: u16 = 21;
const NLMSG_ERROR: u16 = 2;
const NLM_F_REQUEST: u16 = 1;
const NLM_F_ACK: u16 = 4;
const NLM_F_REPLACE: u16 = 0x100;
const NLM_F_CREATE: u16 = 0x400;
const IFF_UP: u32 = 1;
const AF_INET: u8 = 2;
const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const IFA_BROADCAST: u16 = 4;

/// Something that sends one request to the kernel and returns what it answered.
pub trait RtnlTransport {
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>, String>;
}

fn header(len: usize, ty: u16, flags: u16, seq: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(16);
    h.extend_from_slice(&(len as u32).to_ne_bytes());
    h.extend_from_slice(&ty.to_ne_bytes());
    h.extend_from_slice(&flags.to_ne_bytes());
    h.extend_from_slice(&seq.to_ne_bytes());
    h.extend_from_slice(&0u32.to_ne_bytes());
    h
}

fn attr(out: &mut Vec<u8>, ty: u16, data: &[u8]) {
    out.extend_from_slice(&((4 + data.len()) as u16).to_ne_bytes());
    out.extend_from_slice(&ty.to_ne_bytes());
    out.extend_from_slice(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn finish(ty: u16, flags: u16, seq: u32, body: Vec<u8>) -> Vec<u8> {
    let mut m = header(16 + body.len(), ty, flags, seq);
    m.extend(body);
    m
}

/// `RTM_NEWLINK`: set the interface up or down (only the up flag is changed).
pub fn link_set(seq: u32, ifindex: u32, up: bool) -> Vec<u8> {
    let mut b = vec![0u8, 0, 0, 0]; // family, pad, type
    b.extend_from_slice(&(ifindex as i32).to_ne_bytes());
    b.extend_from_slice(&(if up { IFF_UP } else { 0 }).to_ne_bytes()); // flags
    b.extend_from_slice(&IFF_UP.to_ne_bytes()); // change mask: this flag only
    finish(RTM_NEWLINK, NLM_F_REQUEST | NLM_F_ACK, seq, b)
}

fn addr_body(ifindex: u32, ip: Ipv4Addr, prefix: u8, with_broadcast: bool) -> Vec<u8> {
    let mut b = vec![AF_INET, prefix, 0, 0]; // family, prefix length, flags, scope (universe)
    b.extend_from_slice(&ifindex.to_ne_bytes());
    attr(&mut b, IFA_LOCAL, &ip.octets());
    attr(&mut b, IFA_ADDRESS, &ip.octets());
    if with_broadcast {
        let host = if prefix >= 32 { 0 } else { u32::MAX >> prefix };
        attr(&mut b, IFA_BROADCAST, &(u32::from(ip) | host).to_be_bytes());
    }
    b
}

/// `RTM_NEWADDR`: give the interface an IPv4 address (an address already there is replaced, so it can be repeated).
pub fn addr_add(seq: u32, ifindex: u32, ip: Ipv4Addr, prefix: u8) -> Result<Vec<u8>, String> {
    if prefix == 0 || prefix > 32 {
        return Err(format!("a prefix of {prefix} bits is not an address range"));
    }
    Ok(finish(RTM_NEWADDR, NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_REPLACE, seq, addr_body(ifindex, ip, prefix, true)))
}

/// `RTM_DELADDR`: take the address off the interface.
pub fn addr_del(seq: u32, ifindex: u32, ip: Ipv4Addr, prefix: u8) -> Result<Vec<u8>, String> {
    if prefix == 0 || prefix > 32 {
        return Err(format!("a prefix of {prefix} bits is not an address range"));
    }
    Ok(finish(RTM_DELADDR, NLM_F_REQUEST | NLM_F_ACK, seq, addr_body(ifindex, ip, prefix, false)))
}

/// The acknowledgement: `Ok` for an ack (error number 0), otherwise the kernel's error in words.
pub fn check_ack(reply: &[u8]) -> Result<(), String> {
    let mut rest = reply;
    while rest.len() >= 16 {
        let len = u32::from_ne_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        let ty = u16::from_ne_bytes([rest[4], rest[5]]);
        if len < 16 || len > rest.len() {
            return Err("the kernel's answer is cut short".into());
        }
        if ty == NLMSG_ERROR {
            let e = rest.get(16..20).ok_or("the kernel's answer is cut short")?;
            let errno = i32::from_ne_bytes([e[0], e[1], e[2], e[3]]);
            return if errno == 0 { Ok(()) } else { Err(errno_text(-errno)) };
        }
        rest = &rest[(len + 3) & !3..].get(..).unwrap_or(&[]);
    }
    Err("the kernel did not answer".into())
}

fn errno_text(e: i32) -> String {
    match e {
        1 => "not allowed (administrator rights are needed)".into(),
        2 => "there is no such interface".into(),
        17 => "it is already there".into(),
        19 => "there is no such device".into(),
        22 => "the kernel refused the request as invalid".into(),
        99 => "the address cannot be assigned".into(),
        n => format!("the kernel refused it (error {n})"),
    }
}

/// The calls, over any transport.
pub struct Rtnl<T: RtnlTransport> {
    t: T,
    seq: u32,
}

impl<T: RtnlTransport> Rtnl<T> {
    pub fn new(t: T) -> Rtnl<T> {
        Rtnl { t, seq: 0 }
    }

    pub fn transport(&self) -> &T {
        &self.t
    }

    fn next(&mut self) -> u32 {
        self.seq += 1;
        self.seq
    }

    pub fn link_set(&mut self, ifindex: u32, up: bool) -> Result<(), String> {
        let s = self.next();
        check_ack(&self.t.exchange(&link_set(s, ifindex, up))?)
    }

    pub fn addr_add(&mut self, ifindex: u32, ip: Ipv4Addr, prefix: u8) -> Result<(), String> {
        let s = self.next();
        check_ack(&self.t.exchange(&addr_add(s, ifindex, ip, prefix)?)?)
    }

    pub fn addr_del(&mut self, ifindex: u32, ip: Ipv4Addr, prefix: u8) -> Result<(), String> {
        let s = self.next();
        check_ack(&self.t.exchange(&addr_del(s, ifindex, ip, prefix)?)?)
    }
}

/// The real routing socket.
#[cfg(target_os = "linux")]
pub struct RouteSocket {
    fd: std::os::fd::OwnedFd,
}

#[cfg(target_os = "linux")]
impl RouteSocket {
    pub fn open() -> Result<RouteSocket, String> {
        use std::os::fd::FromRawFd;
        // SAFETY: plain socket calls; the descriptor is owned by the returned value, the address and timeout structs outlive their calls.
        unsafe {
            let fd = libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, 0);
            if fd < 0 {
                return Err(format!("the routing socket could not be opened: {}", std::io::Error::last_os_error()));
            }
            let owned = std::os::fd::OwnedFd::from_raw_fd(fd);
            let mut sa: libc::sockaddr_nl = std::mem::zeroed();
            sa.nl_family = libc::AF_NETLINK as u16;
            if libc::bind(fd, &sa as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_nl>() as u32) < 0 {
                return Err(format!("the routing socket could not be bound: {}", std::io::Error::last_os_error()));
            }
            let tv = libc::timeval { tv_sec: 3, tv_usec: 0 };
            libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_RCVTIMEO, &tv as *const _ as *const libc::c_void, std::mem::size_of::<libc::timeval>() as u32);
            Ok(RouteSocket { fd: owned })
        }
    }
}

#[cfg(target_os = "linux")]
impl RtnlTransport for RouteSocket {
    fn exchange(&mut self, request: &[u8]) -> Result<Vec<u8>, String> {
        use std::os::fd::AsRawFd;
        let fd = self.fd.as_raw_fd();
        // SAFETY: the buffers are valid for their lengths for the duration of the calls.
        unsafe {
            if libc::send(fd, request.as_ptr() as *const libc::c_void, request.len(), 0) < 0 {
                return Err(format!("sending to the kernel failed: {}", std::io::Error::last_os_error()));
            }
            let mut buf = vec![0u8; 8192];
            let n = libc::recv(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0);
            if n < 0 {
                return Err(format!("the kernel did not answer: {}", std::io::Error::last_os_error()));
            }
            buf.truncate(n as usize);
            Ok(buf)
        }
    }
}

/// Bring an interface up or down through the routing socket. `Err` is a plain line.
#[cfg(target_os = "linux")]
pub fn set_link(name: &str, up: bool) -> Result<(), String> {
    let idx = ifindex(name)?;
    Rtnl::new(RouteSocket::open()?).link_set(idx, up).map_err(|e| format!("{name}: {e}"))
}

/// Give an interface an IPv4 address through the routing socket.
#[cfg(target_os = "linux")]
pub fn add_address(name: &str, ip: Ipv4Addr, prefix: u8) -> Result<(), String> {
    let idx = ifindex(name)?;
    Rtnl::new(RouteSocket::open()?).addr_add(idx, ip, prefix).map_err(|e| format!("{name}: {e}"))
}

#[cfg(not(target_os = "linux"))]
pub fn set_link(name: &str, _up: bool) -> Result<(), String> {
    Err(format!("{name}: changing a network link needs Linux"))
}

#[cfg(not(target_os = "linux"))]
pub fn add_address(name: &str, _ip: Ipv4Addr, _prefix: u8) -> Result<(), String> {
    Err(format!("{name}: changing a network address needs Linux"))
}

/// The kernel's number of an interface (from `/sys/class/net/<name>/ifindex`); the name is checked first.
pub fn ifindex(name: &str) -> Result<u32, String> {
    if !iyakki_shared::sharing::valid_iface(name) {
        return Err(format!("{name:?} is not an interface name"));
    }
    iyakki_core_local::net::iface_attr(name, "ifindex").and_then(|s| s.trim().parse().ok()).ok_or_else(|| format!("there is no network device called {name}"))
}

#[cfg(test)]
#[path = "rtnl_tests.rs"]
mod tests;
