//! Sockets for [`super::responder::Responder`] (the device announcing itself) and the resolver the developer computer uses.
//!
//! * The announcer joins 224.0.0.251 on the interfaces that have a private or link-local address only (never a public address,
//!   never loopback, never the container bridges). With no such interface it does nothing. It binds UDP 5353 with SO_REUSEADDR and
//!   SO_REUSEPORT, so it can live beside another mDNS program on the same machine.
//! * The resolver does NOT use port 5353: it asks from an ordinary port, so the answer comes back to it as a legacy unicast
//!   response (RFC 6762 section 6.7). Whatever else listens on 5353 on the developer's computer (avahi, mDNSResponder, a browser)
//!   is not disturbed and does not need to be asked.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::os::fd::FromRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::responder::{Dest, Out, Responder, State, Svc, MDNS_PORT, MDNS_V4};
use super::wire::{self, Data, Message, Question};

/// Interfaces never used (container and virtual bridges).
const SKIP: [&str; 10] = ["cni", "flannel", "veth", "docker", "br-", "podman", "virbr", "cali", "kube", "lo"];

/// One address of one interface that mDNS may use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IfAddr {
    pub iface: String,
    pub v4: Ipv4Addr,
    pub prefix: u8,
}

/// A unique-local IPv6 address (fc00::/7): private, and valid as an AAAA answer. Link-local and global ones are not announced.
pub fn is_ula(a: Ipv6Addr) -> bool {
    a.segments()[0] & 0xfe00 == 0xfc00
}

/// The private IPv4 addresses of the interfaces that are up, and the unique-local IPv6 addresses.
pub fn interfaces() -> (Vec<IfAddr>, Vec<Ipv6Addr>) {
    let (mut v4, mut v6) = (Vec::new(), Vec::new());
    // SAFETY: getifaddrs allocates a list that freeifaddrs releases; the entries are only read.
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 {
            return (v4, v6);
        }
        let mut cur = list;
        while !cur.is_null() {
            let a = &*cur;
            cur = a.ifa_next;
            if a.ifa_addr.is_null() || a.ifa_flags & (libc::IFF_UP as u32) == 0 || a.ifa_flags & (libc::IFF_LOOPBACK as u32) != 0 || a.ifa_flags & (libc::IFF_MULTICAST as u32) == 0 {
                continue;
            }
            let name = std::ffi::CStr::from_ptr(a.ifa_name).to_string_lossy().into_owned();
            if SKIP.iter().any(|p| name.starts_with(p)) {
                continue;
            }
            match i32::from((*a.ifa_addr).sa_family) {
                libc::AF_INET if !a.ifa_netmask.is_null() => {
                    let ip = Ipv4Addr::from(u32::from_be((*(a.ifa_addr as *const libc::sockaddr_in)).sin_addr.s_addr));
                    let mask = u32::from_be((*(a.ifa_netmask as *const libc::sockaddr_in)).sin_addr.s_addr);
                    if crate::net::is_private(ip) {
                        v4.push(IfAddr { iface: name, v4: ip, prefix: mask.count_ones() as u8 });
                    }
                }
                libc::AF_INET6 => {
                    let ip = Ipv6Addr::from((*(a.ifa_addr as *const libc::sockaddr_in6)).sin6_addr.s6_addr);
                    if is_ula(ip) {
                        v6.push(ip);
                    }
                }
                _ => {}
            }
        }
        libc::freeifaddrs(list);
    }
    (v4, v6)
}

fn same_net(a: Ipv4Addr, b: Ipv4Addr, prefix: u8) -> bool {
    let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix.min(32) as u32) };
    u32::from(a) & mask == u32::from(b) & mask
}

/// Is `src` on one of these networks?
pub fn on_link(src: IpAddr, nets: &[IfAddr]) -> bool {
    match src.to_canonical() {
        IpAddr::V4(s) => nets.iter().any(|n| same_net(s, n.v4, n.prefix)),
        IpAddr::V6(_) => false,
    }
}

fn setsockopt<T>(fd: i32, level: i32, name: i32, v: &T) -> bool {
    // SAFETY: a plain setsockopt with a value of the size given.
    unsafe { libc::setsockopt(fd, level, name, v as *const T as *const libc::c_void, std::mem::size_of::<T>() as u32) == 0 }
}

fn membership(group: Ipv4Addr, iface: Ipv4Addr) -> libc::ip_mreq {
    libc::ip_mreq { imr_multiaddr: libc::in_addr { s_addr: u32::from(group).to_be() }, imr_interface: libc::in_addr { s_addr: u32::from(iface).to_be() } }
}

/// A UDP socket on `0.0.0.0:5353` that several programs may share.
pub fn bind_shared(port: u16) -> Result<UdpSocket, String> {
    // SAFETY: socket() returns a new descriptor that UdpSocket takes over (closed on drop, also on the error paths below).
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(format!("socket: {}", std::io::Error::last_os_error()));
        }
        let s = UdpSocket::from_raw_fd(fd);
        let one: i32 = 1;
        setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, &one);
        setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEPORT, &one);
        let zero: i32 = 0;
        setsockopt(fd, libc::IPPROTO_IP, libc::IP_MULTICAST_LOOP, &zero);
        let ttl: i32 = 255;
        setsockopt(fd, libc::IPPROTO_IP, libc::IP_MULTICAST_TTL, &ttl);
        let addr = libc::sockaddr_in { sin_family: libc::AF_INET as u16, sin_port: port.to_be(), sin_addr: libc::in_addr { s_addr: 0 }, sin_zero: [0; 8] };
        if libc::bind(fd, &addr as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_in>() as u32) != 0 {
            return Err(format!("bind {port}: {}", std::io::Error::last_os_error()));
        }
        Ok(s)
    }
}

fn join(s: &UdpSocket, iface: Ipv4Addr) -> bool {
    use std::os::fd::AsRawFd;
    setsockopt(s.as_raw_fd(), libc::IPPROTO_IP, libc::IP_ADD_MEMBERSHIP, &membership(MDNS_V4, iface))
}

fn leave(s: &UdpSocket, iface: Ipv4Addr) {
    use std::os::fd::AsRawFd;
    setsockopt(s.as_raw_fd(), libc::IPPROTO_IP, libc::IP_DROP_MEMBERSHIP, &membership(MDNS_V4, iface));
}

fn set_out_iface(s: &UdpSocket, iface: Ipv4Addr) {
    use std::os::fd::AsRawFd;
    let a = libc::in_addr { s_addr: u32::from(iface).to_be() };
    setsockopt(s.as_raw_fd(), libc::IPPROTO_IP, libc::IP_MULTICAST_IF, &a);
}

// ------------------------------------------------------------------------------------------------------------ announcer

/// What the announcer is doing, for the Settings page and the diagnosis output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub running: bool,
    /// The name in use now (after a conflict it has a suffix).
    pub host: String,
    pub probing: bool,
    pub v4: Vec<Ipv4Addr>,
    pub v6: Vec<Ipv6Addr>,
    pub interfaces: Vec<String>,
    pub services: Vec<String>,
    pub answered: u64,
    pub dropped: u64,
    pub rate_limited: u64,
    pub error: String,
}

struct Shared {
    svcs: Vec<Svc>,
    status: Status,
}

/// The running announcer: a thread that owns the socket. [`Announcer::stop`] (or dropping it) sends the goodbyes.
pub struct Announcer {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Announcer {
    /// Start announcing `label.local` (for example `iyakki-a1b2c3`). Fails when UDP 5353 cannot be opened.
    pub fn start(label: &str, svcs: Vec<Svc>) -> Result<Announcer, String> {
        Self::start_on(label, svcs, MDNS_PORT)
    }

    pub fn start_on(label: &str, svcs: Vec<Svc>, port: u16) -> Result<Announcer, String> {
        let sock = bind_shared(port)?;
        sock.set_read_timeout(Some(Duration::from_millis(100))).map_err(|e| e.to_string())?;
        let shared = Arc::new(Mutex::new(Shared { svcs, status: Status { running: true, host: format!("{label}.local"), probing: true, ..Default::default() } }));
        let stop = Arc::new(AtomicBool::new(false));
        let (sh, st, label) = (shared.clone(), stop.clone(), label.to_string());
        let thread = std::thread::Builder::new().name("iyakki-mdns".into()).spawn(move || run(sock, &label, sh, st)).map_err(|e| e.to_string())?;
        Ok(Announcer { shared, stop, thread: Some(thread) })
    }

    pub fn set_services(&self, svcs: Vec<Svc>) {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).svcs = svcs;
    }

    pub fn status(&self) -> Status {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).status.clone()
    }

    /// Send the goodbyes and end the thread.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).status.running = false;
    }
}

impl Drop for Announcer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn send_all(sock: &UdpSocket, outs: &[Out], nets: &[IfAddr], port: u16) {
    for o in outs {
        match &o.dest {
            Dest::Unicast(to) => {
                let _ = sock.send_to(&o.bytes, to);
            }
            Dest::Multicast => {
                let mut seen = HashSet::new();
                for n in nets.iter().filter(|n| seen.insert(n.v4)) {
                    set_out_iface(sock, n.v4);
                    let _ = sock.send_to(&o.bytes, SocketAddrV4::new(MDNS_V4, port));
                }
            }
        }
    }
}

fn run(sock: UdpSocket, label: &str, shared: Arc<Mutex<Shared>>, stop: Arc<AtomicBool>) {
    let t0 = Instant::now();
    let ms = || t0.elapsed().as_millis() as u64;
    let port = sock.local_addr().map(|a| a.port()).unwrap_or(MDNS_PORT);
    let mut r = Responder::new(label, ms());
    let mut joined: HashMap<Ipv4Addr, ()> = HashMap::new();
    let mut nets: Vec<IfAddr> = Vec::new();
    let mut last_if = None::<Instant>;
    let mut buf = vec![0u8; wire::MAX_PACKET];
    loop {
        let now = ms();
        if stop.load(Ordering::SeqCst) {
            send_all(&sock, &r.stop(), &nets, port);
            break;
        }
        if last_if.is_none_or(|t| t.elapsed() >= Duration::from_secs(2)) {
            last_if = Some(Instant::now());
            let (n4, n6) = interfaces();
            for n in &n4 {
                if !joined.contains_key(&n.v4) && join(&sock, n.v4) {
                    joined.insert(n.v4, ());
                }
            }
            let gone: Vec<Ipv4Addr> = joined.keys().filter(|a| !n4.iter().any(|n| n.v4 == **a)).copied().collect();
            for a in gone {
                leave(&sock, a);
                joined.remove(&a);
            }
            nets = n4.into_iter().filter(|n| joined.contains_key(&n.v4)).collect();
            r.set_addresses(nets.iter().map(|n| n.v4).collect(), if nets.is_empty() { vec![] } else { n6 }, now);
            let svcs = shared.lock().unwrap_or_else(|e| e.into_inner()).svcs.clone();
            r.set_services(svcs, now);
        }
        match sock.recv_from(&mut buf) {
            Ok((n, SocketAddr::V4(from))) => {
                let own = nets.iter().any(|x| x.v4 == *from.ip());
                if !own && on_link(IpAddr::V4(*from.ip()), &nets) {
                    let outs = r.on_packet(&buf[..n], SocketAddr::V4(from), ms());
                    send_all(&sock, &outs, &nets, port);
                }
            }
            Ok(_) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
        send_all(&sock, &r.poll(ms()), &nets, port);
        let (v4, v6) = r.addresses();
        let mut g = shared.lock().unwrap_or_else(|e| e.into_inner());
        g.status = Status {
            running: true,
            host: r.host(),
            probing: r.state() == State::Probing,
            v4,
            v6,
            interfaces: nets.iter().map(|n| n.iface.clone()).collect(),
            services: r.services().iter().map(|s| format!("{} {}", s.kind, s.port)).collect(),
            answered: r.answered,
            dropped: r.dropped,
            rate_limited: r.rate_limited,
            error: if nets.is_empty() { "no private network to announce on".into() } else { String::new() },
        };
    }
}

// ------------------------------------------------------------------------------------------------------------ resolver

/// What the resolver may ask where. `dests` is empty by default: the multicast group on every private interface.
#[derive(Clone, Debug)]
pub struct Ask {
    pub timeout: Duration,
    /// Unicast destinations to ask instead of the multicast group (tests, or a known responder).
    pub dests: Vec<SocketAddr>,
    /// Accept loopback addresses in answers (tests only).
    pub allow_loopback: bool,
}

impl Ask {
    pub fn new(timeout: Duration) -> Ask {
        Ask { timeout, dests: vec![], allow_loopback: false }
    }
}

fn acceptable(ip: IpAddr, allow_loopback: bool) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(a) => crate::net::is_private(a) || (allow_loopback && a.is_loopback()),
        IpAddr::V6(a) => is_ula(a) || (a.segments()[0] & 0xffc0 == 0xfe80) || (allow_loopback && a.is_loopback()),
    }
}

/// Send `msgs` and collect the responses that arrive within the time. Responses are matched by id and must come from port 5353
/// (or the destination asked, in tests); everything else is thrown away.
fn exchange(msgs: &[Message], ask: &Ask, early: bool) -> Vec<(SocketAddr, Message)> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return vec![] };
    let _ = sock.set_multicast_ttl_v4(255);
    let ids: HashSet<u16> = msgs.iter().map(|m| m.id).collect();
    let packets: Vec<Vec<u8>> = msgs.iter().filter_map(|m| wire::encode(m).ok()).collect();
    if ask.dests.is_empty() {
        let (nets, _) = interfaces();
        let mut seen = HashSet::new();
        for n in nets.iter().filter(|n| seen.insert(n.v4)) {
            set_out_iface(&sock, n.v4);
            for p in &packets {
                let _ = sock.send_to(p, SocketAddrV4::new(MDNS_V4, MDNS_PORT));
            }
        }
    } else {
        for d in &ask.dests {
            for p in &packets {
                let _ = sock.send_to(p, d);
            }
        }
    }
    let mut end = Instant::now() + ask.timeout;
    let mut out = Vec::new();
    let mut buf = vec![0u8; wire::MAX_PACKET];
    while Instant::now() < end {
        let _ = sock.set_read_timeout(Some(end.saturating_duration_since(Instant::now()).max(Duration::from_millis(5))));
        let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
        if !acceptable(from.ip(), ask.allow_loopback) || (ask.dests.is_empty() && from.port() != MDNS_PORT) {
            continue;
        }
        if let Ok(m) = wire::decode(&buf[..n]) {
            if m.is_response() && ids.contains(&m.id) {
                out.push((from, m));
                if early {
                    // an answer is here: only a moment more for the other address family and other interfaces
                    end = end.min(Instant::now() + Duration::from_millis(150));
                }
            }
        }
    }
    out
}

fn ask_id() -> u16 {
    use std::sync::atomic::AtomicU16;
    static N: AtomicU16 = AtomicU16::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos() as u16 | 1);
    t.wrapping_add(N.fetch_add(2, Ordering::Relaxed))
}

/// The addresses of `name` (`iyakki-a1b2c3.local`), asked on the local network. Empty when nobody answers within the time.
pub fn resolve(name: &str, ask: &Ask) -> Vec<IpAddr> {
    let name = wire::norm(name);
    let id = ask_id();
    let q = |t: u16, id: u16| Message { id, questions: vec![Question { name: name.clone(), qtype: t, unicast: true }], ..Default::default() };
    let mut found: Vec<IpAddr> = Vec::new();
    for (_, m) in exchange(&[q(wire::TYPE_A, id), q(wire::TYPE_AAAA, id.wrapping_add(1))], ask, true) {
        for r in m.answers.iter().chain(&m.additional) {
            if r.name != name || r.ttl == 0 {
                continue;
            }
            let ip = match r.data {
                Data::A(a) => IpAddr::V4(a),
                Data::Aaaa(a) => IpAddr::V6(a),
                _ => continue,
            };
            if acceptable(ip, ask.allow_loopback) && !found.contains(&ip) {
                found.push(ip);
            }
        }
    }
    // IPv4 first: it is the one that works on every network
    found.sort_by_key(|a| a.is_ipv6());
    found
}

/// A device that announced itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// `iyakki-a1b2c3.local`
    pub host: String,
    pub addrs: Vec<IpAddr>,
    /// `(service type, port)`
    pub services: Vec<(String, u16)>,
}

/// The service types `iyakki dev scan` asks for.
pub const SERVICE_TYPES: [&str; 2] = ["_iyakki-dev._tcp", "_iyakki-web._tcp"];

/// The IYAKKI devices that answer within the time.
pub fn scan(ask: &Ask) -> Vec<Found> {
    let base = ask_id();
    let msgs: Vec<Message> = SERVICE_TYPES.iter().enumerate().map(|(i, t)| Message { id: base.wrapping_add(i as u16), questions: vec![Question { name: format!("{t}.local"), qtype: wire::TYPE_PTR, unicast: true }], ..Default::default() }).collect();
    let mut by_host: Vec<Found> = Vec::new();
    for (_, m) in exchange(&msgs, ask, false) {
        let recs: Vec<_> = m.answers.iter().chain(&m.additional).filter(|r| r.ttl > 0).collect();
        for srv in recs.iter().filter(|r| SERVICE_TYPES.iter().any(|t| r.name.ends_with(&format!(".{t}.local")))) {
            let Data::Srv { port, target, .. } = &srv.data else { continue };
            let kind = SERVICE_TYPES.iter().find(|t| srv.name.ends_with(&format!(".{t}.local"))).copied().unwrap_or("");
            let addrs: Vec<IpAddr> = recs
                .iter()
                .filter(|r| r.name == *target)
                .filter_map(|r| match r.data {
                    Data::A(a) => Some(IpAddr::V4(a)),
                    Data::Aaaa(a) => Some(IpAddr::V6(a)),
                    _ => None,
                })
                .filter(|a| acceptable(*a, ask.allow_loopback))
                .collect();
            let f = match by_host.iter_mut().find(|f| f.host == *target) {
                Some(f) => f,
                None => {
                    by_host.push(Found { host: target.clone(), addrs: vec![], services: vec![] });
                    by_host.last_mut().unwrap()
                }
            };
            for a in addrs {
                if !f.addrs.contains(&a) {
                    f.addrs.push(a);
                }
            }
            if !f.services.contains(&(kind.to_string(), *port)) {
                f.services.push((kind.to_string(), *port));
            }
        }
    }
    by_host.sort_by(|a, b| a.host.cmp(&b.host));
    by_host
}
