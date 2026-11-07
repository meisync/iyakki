//! Ping and the port checker. Both run on background threads and fill a shared result list the page redraws from, so the screen
//! never waits; both check only the host the person typed (the port list is capped at [`MAX_PORTS`]).
//!
//! Ping sends ICMP echo requests on an unprivileged datagram socket (no ping program, no root). Where the system does not allow
//! that (`net.ipv4.ping_group_range`), it times a TCP connect to port 443, then 80, and says so.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The most ports one check takes.
pub const MAX_PORTS: usize = 256;
/// How long one connect may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
/// Checks run side by side.
const WORKERS: usize = 16;

/// The shared state of a running check.
#[derive(Default)]
pub struct Job {
    /// (sort key, line): shown in key order.
    lines: Mutex<Vec<(u32, String)>>,
    done: AtomicBool,
    stop: AtomicBool,
}

impl Job {
    fn push(&self, key: u32, line: String) {
        self.lines.lock().unwrap_or_else(|e| e.into_inner()).push((key, line));
    }
    pub fn finished(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
    /// The lines so far, in order.
    pub fn text(&self) -> String {
        let mut v = self.lines.lock().unwrap_or_else(|e| e.into_inner()).clone();
        v.sort_by_key(|(k, _)| *k);
        v.into_iter().map(|(_, l)| l).collect::<Vec<_>>().join("\n")
    }
}

/// "22,80,443", "8000-8010" or a mix, deduplicated and in order.
pub fn parse_ports(spec: &str) -> Result<Vec<u16>, String> {
    let mut out: Vec<u16> = Vec::new();
    for part in spec.split([',', ' ', ';']).filter(|p| !p.is_empty()) {
        let num = |s: &str| s.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| format!("'{s}' is not a port (1 to 65535)"));
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (num(a)?, num(b)?),
            None => (num(part)?, num(part)?),
        };
        if b < a {
            return Err(format!("The range {part} is backwards"));
        }
        if out.len() + (b - a) as usize + 1 > MAX_PORTS * 4 {
            return Err(format!("Too many ports: at most {MAX_PORTS} at a time"));
        }
        out.extend(a..=b);
    }
    out.sort_unstable();
    out.dedup();
    if out.is_empty() {
        return Err("Type ports like 22,80,443 or a range like 8000-8010.".into());
    }
    if out.len() > MAX_PORTS {
        return Err(format!("{} ports is too many: at most {MAX_PORTS} at a time.", out.len()));
    }
    Ok(out)
}

fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let h = host.trim();
    if h.is_empty() || h.contains(char::is_whitespace) || h.len() > 253 {
        return Err("Type one host name or IP address.".into());
    }
    let a: Vec<SocketAddr> = (h, port).to_socket_addrs().map_err(|e| format!("Cannot find '{h}': {e}"))?.collect();
    if a.is_empty() { Err(format!("Cannot find '{h}'")) } else { Ok(a) }
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

// ---- port checker ---------------------------------------------------------------------------------------------------

/// One port: ("open" | "closed" | "filtered", detail).
pub fn check_port(addrs: &[SocketAddr], port: u16, timeout: Duration) -> (&'static str, String) {
    let t = Instant::now();
    let mut last = None;
    for a in addrs {
        let mut a = *a;
        a.set_port(port);
        match TcpStream::connect_timeout(&a, timeout) {
            Ok(_) => return ("open", ms(t.elapsed())),
            Err(e) => last = Some(e),
        }
    }
    match last.map(|e| (e.kind(), e)) {
        Some((std::io::ErrorKind::ConnectionRefused, _)) => ("closed", format!("refused after {}", ms(t.elapsed()))),
        Some((std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock, _)) => ("filtered", format!("no answer in {} (a firewall may drop it)", ms(timeout))),
        Some((_, e)) => ("filtered", format!("{e}")),
        None => ("filtered", "no address".into()),
    }
}

pub fn start_ports(host: &str, spec: &str) -> Result<Arc<Job>, String> {
    let ports = parse_ports(if spec.trim().is_empty() { "22,80,443" } else { spec })?;
    let addrs = resolve(host, 0)?;
    let job = Arc::new(Job::default());
    job.push(0, format!("Checking {} port{} on {} ({})", ports.len(), if ports.len() == 1 { "" } else { "s" }, host.trim(), addrs[0].ip()));
    let next = Arc::new(Mutex::new(0usize));
    let left = Arc::new(Mutex::new(WORKERS.min(ports.len())));
    for _ in 0..WORKERS.min(ports.len()) {
        let (job, addrs, ports, next, left) = (job.clone(), addrs.clone(), ports.clone(), next.clone(), left.clone());
        std::thread::spawn(move || {
            loop {
                let i = {
                    let mut n = next.lock().unwrap_or_else(|e| e.into_inner());
                    *n += 1;
                    *n - 1
                };
                if i >= ports.len() || job.stopped() {
                    break;
                }
                let (state, detail) = check_port(&addrs, ports[i], CONNECT_TIMEOUT);
                job.push(ports[i] as u32 + 1, format!("{:>5}  {:<9} {}", ports[i], state, detail));
            }
            let mut l = left.lock().unwrap_or_else(|e| e.into_inner());
            *l -= 1;
            if *l == 0 {
                let text = job.text();
                let count = |w: &str| text.lines().filter(|l| l.get(7..).is_some_and(|r| r.starts_with(w))).count();
                job.push(70_000, format!("{} open, {} closed, {} filtered{}", count("open"), count("closed"), count("filtered"), if job.stopped() { " (stopped)" } else { "" }));
                job.done.store(true, Ordering::Relaxed);
            }
        });
    }
    Ok(job)
}

// ---- ping -------------------------------------------------------------------------------------------------------------

fn checksum(b: &[u8]) -> u16 {
    let mut s: u32 = b.chunks(2).map(|c| ((c[0] as u32) << 8) | *c.get(1).unwrap_or(&0) as u32).sum();
    while s >> 16 != 0 {
        s = (s & 0xffff) + (s >> 16);
    }
    !(s as u16)
}

/// An ICMP echo socket for IPv4 that needs no privilege, or None where the system forbids it.
fn icmp_socket(timeout: Duration) -> Option<i32> {
    // SAFETY: plain socket calls with valid arguments; the descriptor is closed by the caller.
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, libc::IPPROTO_ICMP);
        if fd < 0 {
            return None;
        }
        let tv = libc::timeval { tv_sec: timeout.as_secs() as _, tv_usec: timeout.subsec_micros() as _ };
        libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_RCVTIMEO, &tv as *const _ as *const libc::c_void, std::mem::size_of::<libc::timeval>() as u32);
        Some(fd)
    }
}

/// One echo: Some(round trip) when the reply came back in time.
fn echo(fd: i32, ip: std::net::Ipv4Addr, seq: u16) -> Option<Duration> {
    let mut pkt = vec![8u8, 0, 0, 0, 0, 0, (seq >> 8) as u8, seq as u8];
    pkt.extend((0..32u8).map(|i| b'a' + i % 26));
    let c = checksum(&pkt);
    pkt[2] = (c >> 8) as u8;
    pkt[3] = c as u8;
    // SAFETY: sockaddr_in is plain data; the buffers outlive the calls.
    unsafe {
        let mut sa: libc::sockaddr_in = std::mem::zeroed();
        sa.sin_family = libc::AF_INET as _;
        sa.sin_addr.s_addr = u32::from_ne_bytes(ip.octets());
        let t = Instant::now();
        if libc::sendto(fd, pkt.as_ptr() as *const libc::c_void, pkt.len(), 0, &sa as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_in>() as u32) < 0 {
            return None;
        }
        let mut buf = [0u8; 256];
        while t.elapsed() < Duration::from_secs(2) {
            let n = libc::recv(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0);
            if n < 8 {
                return None;
            }
            // type 0 is the echo reply; the kernel picks the id, so the sequence says whose it is
            if buf[0] == 0 && u16::from_be_bytes([buf[6], buf[7]]) == seq {
                return Some(t.elapsed());
            }
        }
        None
    }
}

fn summary(sent: usize, times: &[Duration]) -> String {
    let lost = sent - times.len();
    let mut s = format!("{sent} sent, {} received, {}% loss", times.len(), if sent == 0 { 0 } else { lost * 100 / sent });
    if !times.is_empty() {
        let tot: Duration = times.iter().sum();
        s.push_str(&format!("\nmin / avg / max = {} / {} / {}", ms(*times.iter().min().unwrap_or(&Duration::ZERO)), ms(tot / times.len() as u32), ms(*times.iter().max().unwrap_or(&Duration::ZERO))));
    }
    s
}

/// Which method a ping would use for `host`: "icmp" or "tcp".
pub fn ping_method(host: &str) -> &'static str {
    let ok = resolve(host, 0).ok().is_some_and(|a| a.iter().any(|x| x.is_ipv4())) && icmp_socket(Duration::from_millis(100)).map(|fd| unsafe { libc::close(fd) }).is_some();
    if ok { "icmp" } else { "tcp" }
}

pub fn start_ping(host: &str, count: usize) -> Result<Arc<Job>, String> {
    let addrs = resolve(host, 0)?;
    let count = count.clamp(1, 20);
    let job = Arc::new(Job::default());
    let host = host.trim().to_string();
    let v4 = addrs.iter().find_map(|a| if let SocketAddr::V4(v) = a { Some(*v.ip()) } else { None });
    let fd = v4.and_then(|_| icmp_socket(Duration::from_secs(2)));
    let (j, a) = (job.clone(), addrs.clone());
    std::thread::spawn(move || {
        let mut times = Vec::new();
        let mut sent = 0;
        match (fd, v4) {
            (Some(fd), Some(ip)) => {
                j.push(0, format!("PING {host} ({ip}) with ICMP echo, {count} times"));
                for seq in 1..=count as u16 {
                    if j.stopped() {
                        break;
                    }
                    sent += 1;
                    match echo(fd, ip, seq) {
                        Some(d) => {
                            times.push(d);
                            j.push(seq as u32, format!("reply from {ip}: seq={seq} time={}", ms(d)));
                        }
                        None => j.push(seq as u32, format!("seq={seq}: no reply (timed out)")),
                    }
                    std::thread::sleep(Duration::from_millis(400));
                }
                // SAFETY: fd came from socket() above and is closed once.
                unsafe { libc::close(fd) };
            }
            _ => {
                j.push(0, format!("ICMP is not allowed for this account here (or {host} has no IPv4 address), so this times a TCP connect to port 443, then 80, instead. A refused connection still shows the host answers."));
                for seq in 1..=count as u32 {
                    if j.stopped() {
                        break;
                    }
                    sent += 1;
                    let t = Instant::now();
                    let hit = [443u16, 80].iter().find_map(|p| {
                        let (state, _) = check_port(&a, *p, CONNECT_TIMEOUT);
                        (state != "filtered").then_some((*p, state))
                    });
                    match hit {
                        Some((p, state)) => {
                            times.push(t.elapsed());
                            j.push(seq, format!("seq={seq}: port {p} {state}, time={}", ms(t.elapsed())));
                        }
                        None => j.push(seq, format!("seq={seq}: no answer (timed out)")),
                    }
                    std::thread::sleep(Duration::from_millis(400));
                }
            }
        }
        j.push(1000, format!("--- {host} ---\n{}{}", summary(sent, &times), if j.stopped() { " (stopped)" } else { "" }));
        j.done.store(true, Ordering::Relaxed);
    });
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn wait(j: &Job) -> String {
        let t = Instant::now();
        while !j.finished() && t.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(j.finished(), "{}", j.text());
        j.text()
    }

    #[test]
    fn port_lists_and_ranges_are_read_and_capped() {
        assert_eq!(parse_ports("443, 22 80,22").unwrap(), [22, 80, 443]);
        assert_eq!(parse_ports("8000-8003,9000").unwrap(), [8000, 8001, 8002, 8003, 9000]);
        assert!(parse_ports("1-257").unwrap_err().contains("at most 256"));
        assert_eq!(parse_ports("1-256").unwrap().len(), 256);
        for bad in ["", "0", "70000", "9-3", "web"] {
            assert!(parse_ports(bad).is_err(), "{bad}");
        }
        assert!(parse_ports("1-65535").is_err());
    }

    #[test]
    fn an_open_and_a_closed_port_on_this_machine() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let open = l.local_addr().unwrap().port();
        let closed = {
            let t = TcpListener::bind("127.0.0.1:0").unwrap();
            t.local_addr().unwrap().port()
        };
        let j = start_ports("127.0.0.1", &format!("{open},{closed}")).unwrap();
        let t = wait(&j);
        assert!(t.contains(&format!("{open}  open")), "{t}");
        assert!(t.contains(&format!("{closed}  closed")), "{t}");
        assert!(t.ends_with("1 open, 1 closed, 0 filtered"), "{t}");
        assert!(start_ports("127.0.0.1", "99999").is_err());
        assert!(start_ports("", "80").is_err());
    }

    #[test]
    fn stop_ends_a_check_early() {
        let j = start_ports("127.0.0.1", "10000-10200").unwrap();
        j.stop();
        let t = wait(&j);
        assert!(t.contains("(stopped)"), "{t}");
    }

    #[test]
    fn ping_localhost_by_icmp_where_allowed_and_by_tcp_otherwise() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        drop(l);
        let method = ping_method("127.0.0.1");
        let j = start_ping("127.0.0.1", 2).unwrap();
        let t = wait(&j);
        if method == "icmp" {
            assert!(t.contains("with ICMP echo") && t.contains("reply from 127.0.0.1"), "{t}");
            assert!(t.contains("2 sent, 2 received, 0% loss") && t.contains("min / avg / max"), "{t}");
        } else {
            assert!(t.contains("times a TCP connect"), "{t}");
            assert!(t.contains("2 sent"), "{t}");
        }
    }

    #[test]
    fn the_tcp_fallback_counts_a_refusal_as_an_answer() {
        let (state, _) = check_port(&["127.0.0.1:1".parse().unwrap()], 1, Duration::from_millis(500));
        assert_eq!(state, "closed");
        assert!(checksum(&[8, 0, 0, 0, 0, 1, 0, 1]) != 0);
        assert!(summary(4, &[Duration::from_millis(2)]).contains("75% loss"));
    }
}
