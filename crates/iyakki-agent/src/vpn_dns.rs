//! The sockets and the clock of the HYBRID DNS forwarder (`iyakki_vpn::dnsroute` is the pure state machine; this is its executor).
//!
//! * **Listening:** UDP and TCP on the loopback addresses only (`127.0.0.1` and `[::1]`, the latter when the machine has IPv6).
//!   Nothing is ever bound to a LAN address, so the machine is no open resolver; a datagram or connection whose source is not
//!   loopback is dropped. The nf_tables redirect (`inet iyakki_vpn_dns`) sends the machine's own port-53 traffic here.
//! * **Asking:** the system resolvers read from `resolv.conf` (never changed) and the VPN's DNS server (reached through the
//!   tunnel by a host route the manager adds). Every upstream socket carries the forwarder's socket mark so the redirect does not
//!   catch it again. A reply is accepted only from the address that was asked, and then only if the state machine agrees (its id,
//!   its question).
//! * **Bounds:** 512 open UDP clients (older than 10 s are dropped), 16 TCP connections each with a 5 s deadline and 4 KB per
//!   message, 64 datagrams per wake-up per socket, 2 s to connect and 3 s per read/write on the TCP paths. A TCP client's query is
//!   served on the loop thread, so it can delay other queries by at most those timeouts.
//! * **Events:** every packet becomes a `Query` or `Reply` event for an [`EventSink`] (the manager's forwarder); a one-second
//!   `Tick` lets routes and open queries expire. The sink applies the route changes; this module only moves packets.
//!
//! Nothing here has run on a device behind the real redirect rule. Tests use loopback sockets and a fake upstream.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_vpn::dnsroute::{Action, Event, Upstream};

pub const MAX_UDP_CLIENTS: usize = 512;
pub const MAX_TCP_CONNS: usize = 16;
pub const CLIENT_TTL_SECS: u64 = 10;
pub const TCP_DEADLINE: Duration = Duration::from_secs(5);
pub const TCP_MAX_MSG: usize = 4096;
const UDP_BUF: usize = 4096;
const BURST: usize = 64;

/// What the loop talks to: the forwarder plus whatever applies its route changes. Returns the packets to send or answer.
pub trait EventSink: Send + Sync {
    fn event(&self, ev: Event, now: u64) -> Vec<Action>;
}

#[derive(Clone, Debug, Default)]
pub struct Upstreams {
    /// The resolvers the system used before (from `resolv.conf`).
    pub system: Vec<SocketAddr>,
    /// The VPN's DNS server.
    pub vpn: Vec<SocketAddr>,
}

impl Upstreams {
    fn of(&self, u: Upstream) -> &[SocketAddr] {
        match u {
            Upstream::System => &self.system,
            Upstream::Vpn => &self.vpn,
        }
    }
}

/// The `nameserver` lines of a `resolv.conf` as socket addresses (port 53). Scoped link-local addresses are skipped, so are the
/// forwarder's own loopback port and anything that is not an address; at most 3.
pub fn parse_nameservers(text: &str) -> Vec<SocketAddr> {
    let mut v = Vec::new();
    for l in text.lines() {
        let mut t = l.split_whitespace();
        if t.next() != Some("nameserver") {
            continue;
        }
        let Some(ip) = t.next().and_then(|a| a.parse::<IpAddr>().ok()) else { continue };
        if ip.is_unspecified() || ip.is_multicast() {
            continue;
        }
        let a = SocketAddr::new(ip, 53);
        if !v.contains(&a) {
            v.push(a);
        }
    }
    v.truncate(3);
    v
}

fn setsockopt_u32(fd: i32, level: i32, name: i32, v: u32) -> std::io::Result<()> {
    // SAFETY: v outlives the call and the length matches.
    let r = unsafe { libc::setsockopt(fd, level, name, (&v as *const u32).cast(), std::mem::size_of::<u32>() as u32) };
    if r < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn set_mark(fd: i32, mark: Option<u32>) -> Result<(), String> {
    match mark {
        Some(m) => setsockopt_u32(fd, libc::SOL_SOCKET, libc::SO_MARK, m).map_err(|e| format!("could not mark the forwarder's socket ({e}); this needs administrator rights")),
        None => Ok(()),
    }
}

/// The listening sockets. `bind` is separate from `start` so a connect can fail *before* the redirect rule goes in.
pub struct Bound {
    udp4: UdpSocket,
    udp6: Option<UdpSocket>,
    tcp4: TcpListener,
    tcp6: Option<TcpListener>,
    pub port: u16,
}

impl Bound {
    /// Binds loopback only. `port` 0 picks a free one (tests). IPv6 loopback is optional (a machine without IPv6 has none).
    pub fn bind(port: u16) -> Result<Bound, String> {
        let e = |what: &str, err: std::io::Error| format!("the DNS forwarder could not listen on {what} ({err})");
        let udp4 = UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).map_err(|x| e("127.0.0.1 (UDP)", x))?;
        let port = udp4.local_addr().map_err(|x| e("127.0.0.1", x))?.port();
        let tcp4 = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).map_err(|x| e("127.0.0.1 (TCP)", x))?;
        let udp6 = UdpSocket::bind((Ipv6Addr::LOCALHOST, port)).ok();
        let tcp6 = if udp6.is_some() { TcpListener::bind((Ipv6Addr::LOCALHOST, port)).ok() } else { None };
        for s in [&udp4] {
            s.set_nonblocking(true).map_err(|x| e("127.0.0.1", x))?;
        }
        tcp4.set_nonblocking(true).map_err(|x| e("127.0.0.1", x))?;
        if let Some(s) = &udp6 {
            let _ = s.set_nonblocking(true);
        }
        if let Some(s) = &tcp6 {
            let _ = s.set_nonblocking(true);
        }
        Ok(Bound { udp4, udp6, tcp4, tcp6, port })
    }

    /// Every address the forwarder listens on (for the audit line and the tests: all are loopback).
    pub fn addrs(&self) -> Vec<SocketAddr> {
        let mut v = vec![self.udp4.local_addr().unwrap(), self.tcp4.local_addr().unwrap()];
        v.extend(self.udp6.as_ref().and_then(|s| s.local_addr().ok()));
        v.extend(self.tcp6.as_ref().and_then(|s| s.local_addr().ok()));
        v
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn random_u16() -> u16 {
    let mut b = [0u8; 2];
    // SAFETY: b is valid for 2 bytes; getrandom does not block for so few bytes once the pool is seeded.
    let n = unsafe { libc::getrandom(b.as_mut_ptr().cast(), 2, 0) };
    if n == 2 {
        u16::from_ne_bytes(b)
    } else {
        // no entropy call: fall back to the clock's low bits mixed with the address of a local (weak, but never constant)
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        (t ^ (t >> 16) ^ (&b as *const _ as usize as u32)) as u16
    }
}

fn is_loopback(a: &SocketAddr) -> bool {
    a.ip().is_loopback()
}

/// Connect with the mark set before the first packet, with a timeout.
pub fn connect_marked(addr: SocketAddr, mark: Option<u32>, timeout: Duration) -> std::io::Result<TcpStream> {
    let fam = if addr.is_ipv4() { libc::AF_INET } else { libc::AF_INET6 };
    // SAFETY: plain syscall; the descriptor is wrapped (and closed) below on every path.
    let fd = unsafe { libc::socket(fam, libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fd is a fresh socket we own; TcpStream closes it on drop.
    let stream = unsafe { TcpStream::from_raw_fd(fd) };
    if let Some(m) = mark {
        setsockopt_u32(fd, libc::SOL_SOCKET, libc::SO_MARK, m)?;
    }
    let (sa, len): (libc::sockaddr_storage, u32) = {
        // SAFETY: an all-zero sockaddr_storage is valid; the specific address is written over its start.
        let mut st: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        match addr {
            SocketAddr::V4(a) => {
                let s = libc::sockaddr_in { sin_family: libc::AF_INET as u16, sin_port: a.port().to_be(), sin_addr: libc::in_addr { s_addr: u32::from_ne_bytes(a.ip().octets()) }, sin_zero: [0; 8] };
                // SAFETY: sockaddr_in fits in sockaddr_storage.
                unsafe { std::ptr::write((&mut st as *mut libc::sockaddr_storage).cast::<libc::sockaddr_in>(), s) };
                (st, std::mem::size_of::<libc::sockaddr_in>() as u32)
            }
            SocketAddr::V6(a) => {
                let s = libc::sockaddr_in6 { sin6_family: libc::AF_INET6 as u16, sin6_port: a.port().to_be(), sin6_flowinfo: 0, sin6_addr: libc::in6_addr { s6_addr: a.ip().octets() }, sin6_scope_id: 0 };
                // SAFETY: sockaddr_in6 fits in sockaddr_storage.
                unsafe { std::ptr::write((&mut st as *mut libc::sockaddr_storage).cast::<libc::sockaddr_in6>(), s) };
                (st, std::mem::size_of::<libc::sockaddr_in6>() as u32)
            }
        }
    };
    // SAFETY: sa is a valid sockaddr of `len` bytes.
    let r = unsafe { libc::connect(fd, (&sa as *const libc::sockaddr_storage).cast(), len) };
    if r < 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(e);
        }
        let mut p = libc::pollfd { fd, events: libc::POLLOUT, revents: 0 };
        // SAFETY: p is one valid pollfd.
        let n = unsafe { libc::poll(&mut p, 1, timeout.as_millis().min(i32::MAX as u128) as i32) };
        if n <= 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timed out"));
        }
        let mut err: i32 = 0;
        let mut l = std::mem::size_of::<i32>() as u32;
        // SAFETY: err and l are valid for the call.
        unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_ERROR, (&mut err as *mut i32).cast(), &mut l) };
        if err != 0 {
            return Err(std::io::Error::from_raw_os_error(err));
        }
    }
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    Ok(stream)
}

/// One DNS-over-TCP exchange with an upstream: length-prefixed request, length-prefixed reply, bounded.
pub fn exchange_tcp(addr: SocketAddr, mark: Option<u32>, packet: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut s = connect_marked(addr, mark, Duration::from_secs(2))?;
    let mut out = (packet.len() as u16).to_be_bytes().to_vec();
    out.extend_from_slice(packet);
    s.write_all(&out)?;
    let mut l = [0u8; 2];
    s.read_exact(&mut l)?;
    let n = u16::from_be_bytes(l) as usize;
    if !(12..=TCP_MAX_MSG).contains(&n) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "implausible DNS message length"));
    }
    let mut r = vec![0u8; n];
    s.read_exact(&mut r)?;
    Ok(r)
}

struct UdpClient {
    addr: SocketAddr,
    v6: bool,
    at: u64,
}

struct TcpConn {
    stream: TcpStream,
    buf: Vec<u8>,
    born: Instant,
}

struct UpSock {
    sock: UdpSocket,
    kind: Upstream,
}

struct Loop {
    b: Bound,
    ups: Upstreams,
    mark: Option<u32>,
    sink: Arc<dyn EventSink>,
    upsocks: Vec<UpSock>,
    clients: HashMap<u64, UdpClient>,
    next_client: u64,
    conns: Vec<TcpConn>,
    last_tick: u64,
}

/// A running forwarder; dropping it stops the thread, waits for it and closes every socket.
pub struct Runner {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Starts the loop. `mark` is put on every upstream socket (required on a real machine; `None` only in tests).
pub fn start(b: Bound, ups: Upstreams, mark: Option<u32>, sink: Arc<dyn EventSink>) -> Result<Runner, String> {
    let mut upsocks = Vec::new();
    for kind in [Upstream::System, Upstream::Vpn] {
        let list = ups.of(kind);
        for v6 in [false, true] {
            if !list.iter().any(|a| a.is_ipv6() == v6) {
                continue;
            }
            let bind: SocketAddr = if v6 { (Ipv6Addr::UNSPECIFIED, 0).into() } else { (Ipv4Addr::UNSPECIFIED, 0).into() };
            let sock = UdpSocket::bind(bind).map_err(|e| format!("could not open the DNS forwarder's outgoing socket ({e})"))?;
            set_mark(sock.as_raw_fd(), mark)?;
            sock.set_nonblocking(true).map_err(|e| e.to_string())?;
            upsocks.push(UpSock { sock, kind });
        }
    }
    // the TCP path marks its own sockets; prove now that marking works so a failure is reported before the redirect exists
    if let Some(m) = mark {
        let probe = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
        set_mark(probe.as_raw_fd(), Some(m))?;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let mut lp = Loop { b, ups, mark, sink, upsocks, clients: HashMap::new(), next_client: 1, conns: Vec::new(), last_tick: 0 };
    let s2 = stop.clone();
    let handle = std::thread::Builder::new().name("vpn-dns".into()).spawn(move || lp.run(&s2)).map_err(|e| e.to_string())?;
    Ok(Runner { stop, handle: Some(handle) })
}

impl Loop {
    fn run(&mut self, stop: &AtomicBool) {
        while !stop.load(Ordering::SeqCst) {
            let mut fds: Vec<libc::pollfd> = Vec::new();
            let mut roles: Vec<Role> = Vec::new();
            let add = |fd: i32, r: Role, fds: &mut Vec<libc::pollfd>, roles: &mut Vec<Role>| {
                fds.push(libc::pollfd { fd, events: libc::POLLIN, revents: 0 });
                roles.push(r);
            };
            add(self.b.udp4.as_raw_fd(), Role::Udp(false), &mut fds, &mut roles);
            if let Some(s) = &self.b.udp6 {
                add(s.as_raw_fd(), Role::Udp(true), &mut fds, &mut roles);
            }
            add(self.b.tcp4.as_raw_fd(), Role::Accept(false), &mut fds, &mut roles);
            if let Some(s) = &self.b.tcp6 {
                add(s.as_raw_fd(), Role::Accept(true), &mut fds, &mut roles);
            }
            for (i, u) in self.upsocks.iter().enumerate() {
                add(u.sock.as_raw_fd(), Role::Up(i), &mut fds, &mut roles);
            }
            for (i, c) in self.conns.iter().enumerate() {
                add(c.stream.as_raw_fd(), Role::Conn(i), &mut fds, &mut roles);
            }
            // SAFETY: fds is a valid array of pollfd of the given length.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 200) };
            let now = now_secs();
            if n > 0 {
                let mut dead: Vec<usize> = Vec::new();
                for (f, r) in fds.iter().zip(roles.iter()) {
                    if f.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) == 0 {
                        continue;
                    }
                    match *r {
                        Role::Udp(v6) => self.udp_in(v6, now),
                        Role::Accept(v6) => self.accept(v6),
                        Role::Up(i) => self.up_in(i, now),
                        Role::Conn(i) => {
                            if self.conn_in(i, now) {
                                dead.push(i);
                            }
                        }
                    }
                }
                dead.sort_unstable();
                for i in dead.into_iter().rev() {
                    self.conns.remove(i);
                }
            }
            // expire TCP connections and open UDP clients; tick the forwarder once a second
            self.conns.retain(|c| c.born.elapsed() < TCP_DEADLINE);
            if now != self.last_tick {
                self.last_tick = now;
                self.clients.retain(|_, c| now.saturating_sub(c.at) <= CLIENT_TTL_SECS);
                let acts = self.sink.event(Event::Tick { now }, now);
                self.run_actions(acts, None, now);
            }
        }
    }

    fn udp_in(&mut self, v6: bool, now: u64) {
        let mut buf = vec![0u8; UDP_BUF];
        for _ in 0..BURST {
            let r = if v6 { self.b.udp6.as_ref().map(|s| s.recv_from(&mut buf)) } else { Some(self.b.udp4.recv_from(&mut buf)) };
            let Some(Ok((n, from))) = r else { return };
            if !is_loopback(&from) || n < 12 {
                continue; // never answer anything that did not come from this machine
            }
            if self.clients.len() >= MAX_UDP_CLIENTS {
                continue;
            }
            let cid = self.next_client;
            self.next_client += 1;
            self.clients.insert(cid, UdpClient { addr: from, v6, at: now });
            let acts = self.sink.event(Event::Query { client: cid, packet: buf[..n].to_vec(), upstream_id: random_u16(), now }, now);
            self.run_actions(acts, None, now);
        }
    }

    fn accept(&mut self, v6: bool) {
        for _ in 0..BURST {
            let r = if v6 { self.b.tcp6.as_ref().map(|l| l.accept()) } else { Some(self.b.tcp4.accept()) };
            let Some(Ok((s, from))) = r else { return };
            if !is_loopback(&from) || self.conns.len() >= MAX_TCP_CONNS || s.set_nonblocking(true).is_err() {
                continue;
            }
            self.conns.push(TcpConn { stream: s, buf: Vec::new(), born: Instant::now() });
        }
    }

    /// Reads what is there; when a whole message has arrived, answers it (blocking, bounded) and reports the connection done.
    fn conn_in(&mut self, i: usize, now: u64) -> bool {
        let mut tmp = [0u8; 2048];
        loop {
            match self.conns[i].stream.read(&mut tmp) {
                Ok(0) => return true,
                Ok(n) => {
                    self.conns[i].buf.extend_from_slice(&tmp[..n]);
                    if self.conns[i].buf.len() > TCP_MAX_MSG + 2 {
                        return true;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => return true,
            }
        }
        let buf = &self.conns[i].buf;
        if buf.len() < 2 {
            return false;
        }
        let want = u16::from_be_bytes([buf[0], buf[1]]) as usize;
        if !(12..=TCP_MAX_MSG).contains(&want) {
            return true;
        }
        if buf.len() < want + 2 {
            return false;
        }
        let packet = buf[2..2 + want].to_vec();
        let cid = self.next_client;
        self.next_client += 1;
        let acts = self.sink.event(Event::Query { client: cid, packet, upstream_id: random_u16(), now }, now);
        let reply = self.run_actions(acts, Some(cid), now);
        if let Some(r) = reply {
            let s = &mut self.conns[i].stream;
            let _ = s.set_nonblocking(false);
            let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
            let mut out = (r.len() as u16).to_be_bytes().to_vec();
            out.extend_from_slice(&r);
            let _ = s.write_all(&out);
        }
        true
    }

    fn up_in(&mut self, i: usize, now: u64) {
        let mut buf = vec![0u8; UDP_BUF];
        for _ in 0..BURST {
            let kind = self.upsocks[i].kind;
            let Ok((n, from)) = self.upsocks[i].sock.recv_from(&mut buf) else { return };
            // a reply counts only from the address that was asked
            if !self.ups.of(kind).contains(&from) {
                continue;
            }
            let acts = self.sink.event(Event::Reply { from: kind, packet: buf[..n].to_vec(), now }, now);
            self.run_actions(acts, None, now);
        }
    }

    /// Carries out the actions. With `tcp` set, the query came over TCP: upstream is asked over TCP too and the answer for that
    /// client is returned instead of sent.
    fn run_actions(&mut self, first: Vec<Action>, tcp: Option<u64>, _now: u64) -> Option<Vec<u8>> {
        let mut queue: std::collections::VecDeque<Action> = first.into();
        let mut answer = None;
        let mut guard = 0;
        while let Some(a) = queue.pop_front() {
            guard += 1;
            if guard > 64 {
                break;
            }
            match a {
                Action::Send { to, packet } => {
                    let list = self.ups.of(to).to_vec();
                    let Some(target) = list.first().copied() else { continue };
                    if tcp.is_some() {
                        if let Ok(r) = exchange_tcp(target, self.mark, &packet) {
                            queue.extend(self.sink.event(Event::Reply { from: to, packet: r, now: now_secs() }, now_secs()));
                        }
                    } else if let Some(u) = self.upsocks.iter().find(|u| u.kind == to && u.sock.local_addr().is_ok_and(|l| l.is_ipv6() == target.is_ipv6())) {
                        let _ = u.sock.send_to(&packet, target);
                    }
                }
                Action::Respond { client, packet } => {
                    if tcp == Some(client) {
                        answer = Some(packet);
                    } else if let Some(c) = self.clients.remove(&client) {
                        let s = if c.v6 { self.b.udp6.as_ref() } else { Some(&self.b.udp4) };
                        if let Some(s) = s {
                            let _ = s.send_to(&packet, c.addr);
                        }
                    }
                }
                // route changes are applied by the sink before it returns; these never reach the loop
                Action::RouteAdd { .. } | Action::RouteDel { .. } | Action::Note(_) => {}
            }
        }
        answer
    }
}

#[derive(Clone, Copy)]
enum Role {
    Udp(bool),
    Accept(bool),
    Up(usize),
    Conn(usize),
}

#[cfg(test)]
#[path = "vpn_dns_tests.rs"]
mod tests;
