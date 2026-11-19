//! Timeouts for downloads: a transfer that never starts, never sends a byte, or goes quiet in the middle is a network problem
//! and is reported as one, instead of hanging a job for ever.
//!
//! [`Guarded`] wraps any [`Streamer`]:
//! * **start**: `open` must have the answer (status and headers) within [`Limits::start`]; else "network problem: no answer from
//!   <host> within N s".
//! * **first byte**: the body must give its first bytes within [`Limits::first_byte`] of the answer.
//! * **stall**: once bytes flow, a gap longer than [`Limits::stall`] ends the read with an `io::ErrorKind::TimedOut` error whose text
//!   says "network problem: no data from <host> for N s".
//!
//! The body is read by a helper thread that hands over chunks; a read that never returns leaves that one thread parked until the
//! connection dies, which is the price of being able to give up on it. Only GETs are watched: a POST that streams progress lines
//! (a model load can be silent for minutes) is passed through untouched.

use std::io::Read;
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError};
use std::time::Duration;

use crate::{StreamHead, Streamer, TlsMode};

/// How long each part of a download may take before it counts as a network problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub start: Duration,
    pub first_byte: Duration,
    pub stall: Duration,
}

impl Limits {
    /// For installs: 30 s to get an answer, 60 s to the first byte, 60 s of quiet in the middle.
    pub const DOWNLOAD: Limits = Limits { start: Duration::from_secs(30), first_byte: Duration::from_secs(60), stall: Duration::from_secs(60) };
}

pub struct Guarded<S: Streamer + 'static> {
    inner: std::sync::Arc<S>,
    limits: Limits,
}

impl<S: Streamer + 'static> Guarded<S> {
    pub fn new(inner: S, limits: Limits) -> Guarded<S> {
        Guarded { inner: std::sync::Arc::new(inner), limits }
    }

    fn watched(&self, url: &str, open: impl FnOnce(&S) -> Result<StreamHead, String> + Send + 'static) -> Result<StreamHead, String> {
        let host = host_of(url);
        let (tx, rx) = sync_channel(1);
        let inner = self.inner.clone();
        std::thread::spawn(move || {
            let _ = tx.send(open(&inner));
        });
        let head = match rx.recv_timeout(self.limits.start) {
            Ok(Ok(h)) => h,
            Ok(Err(e)) => return Err(classify(&e, &host)),
            Err(_) => return Err(format!("network problem: no answer from {host} within {} s (check the connection, the proxy or the firewall)", self.limits.start.as_secs())),
        };
        Ok(StreamHead { status: head.status, headers: head.headers, body: Box::new(Watched::new(head.body, host, self.limits)) })
    }
}

impl<S: Streamer + 'static> Streamer for Guarded<S> {
    fn open(&self, url: &str, headers: &[(String, String)]) -> Result<StreamHead, String> {
        let (u, h) = (url.to_string(), headers.to_vec());
        self.watched(url, move |s| s.open(&u, &h))
    }

    fn post(&self, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<StreamHead, String> {
        self.inner.post(url, headers, body)
    }

    fn open_tls(&self, url: &str, headers: &[(String, String)], mode: &TlsMode) -> Result<StreamHead, String> {
        let (u, h, m) = (url.to_string(), headers.to_vec(), mode.clone());
        self.watched(url, move |s| s.open_tls(&u, &h, &m))
    }

    fn peer_chain(&self, host: &str, port: u16) -> Result<Vec<Vec<u8>>, String> {
        self.inner.peer_chain(host, port)
    }
}

fn host_of(url: &str) -> String {
    url.split_once("://").map_or(url, |(_, r)| r).split(['/', '?', '#']).next().unwrap_or("").rsplit('@').next().unwrap_or("").to_string()
}

/// A transport error in plain words: the ones that are the network's fault say so.
pub fn classify(err: &str, host: &str) -> String {
    let l = err.to_ascii_lowercase();
    const NET: [&str; 12] = ["timed out", "timeout", "dns", "resolve", "connection refused", "connection reset", "unreachable", "broken pipe", "connect", "tls", "handshake", "eof"];
    if NET.iter().any(|k| l.contains(k)) && !l.starts_with("network problem") {
        format!("network problem reaching {host}: {err}")
    } else {
        err.to_string()
    }
}

enum Chunk {
    Data(Vec<u8>),
    Eof,
    Err(std::io::Error),
}

/// The body, read by a helper thread so that a silent connection can be given up on.
struct Watched {
    rx: Receiver<Chunk>,
    host: String,
    limits: Limits,
    got_any: bool,
    pending: Vec<u8>,
    at: usize,
    done: bool,
}

impl Watched {
    fn new(mut body: Box<dyn Read + Send>, host: String, limits: Limits) -> Watched {
        let (tx, rx) = sync_channel::<Chunk>(4);
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let msg = match body.read(&mut buf) {
                    Ok(0) => Chunk::Eof,
                    Ok(n) => Chunk::Data(buf[..n].to_vec()),
                    Err(e) => Chunk::Err(e),
                };
                let end = !matches!(msg, Chunk::Data(_));
                if tx.send(msg).is_err() || end {
                    break;
                }
            }
        });
        Watched { rx, host, limits, got_any: false, pending: Vec::new(), at: 0, done: false }
    }
}

impl Read for Watched {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.at >= self.pending.len() {
            if self.done {
                return Ok(0);
            }
            let wait = if self.got_any { self.limits.stall } else { self.limits.first_byte };
            match self.rx.recv_timeout(wait) {
                Ok(Chunk::Data(d)) => {
                    self.got_any = true;
                    self.pending = d;
                    self.at = 0;
                }
                Ok(Chunk::Eof) => {
                    self.done = true;
                    return Ok(0);
                }
                Ok(Chunk::Err(e)) => {
                    self.done = true;
                    return Err(std::io::Error::new(e.kind(), classify(&e.to_string(), &self.host)));
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.done = true;
                    let what = if self.got_any { "no data from" } else { "no first byte from" };
                    return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, format!("network problem: {what} {} for {} s (the connection went quiet)", self.host, wait.as_secs())));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.done = true;
                    return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, format!("network problem: the connection to {} ended unexpectedly", self.host)));
                }
            }
        }
        let n = out.len().min(self.pending.len() - self.at);
        out[..n].copy_from_slice(&self.pending[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

// ------------------------------------------------------------------------------------------------ the quick network check

/// Every message of a failed reachability check starts with this, so a retry loop can tell "there is no network" (stop at once,
/// nothing will change in a few seconds) from a hiccup worth another try. See [`is_no_network`].
pub const NO_NETWORK: &str = "No network:";

/// Does this error come from the quick reachability check ([`Preflight`])?
pub fn is_no_network(err: &str) -> bool {
    err.contains(NO_NETWORK)
}

/// Can `host:port` be reached at all? Looks the name up and opens a TCP connection, each bounded so the whole check ends within
/// about `limit`. Sends nothing. On failure the answer is a plain sentence that starts with [`NO_NETWORK`] and says what to do.
pub fn check_reachable(host: &str, port: u16, limit: Duration) -> Result<(), String> {
    use std::net::{TcpStream, ToSocketAddrs};
    let began = std::time::Instant::now();
    let name = host.to_string();
    let (tx, rx) = sync_channel(1);
    std::thread::spawn(move || {
        let _ = tx.send((name.as_str(), port).to_socket_addrs().map(|i| i.collect::<Vec<_>>()).map_err(|e| e.to_string()));
    });
    let hint = "Nothing was downloaded. Check the network cable, Wi-Fi or proxy, then press Retry (or Cancel).";
    let addrs = match rx.recv_timeout(limit) {
        Ok(Ok(a)) if !a.is_empty() => a,
        Ok(_) => return Err(format!("{NO_NETWORK} the address of {host} cannot be looked up, so the computer is probably offline. {hint}")),
        Err(_) => return Err(format!("{NO_NETWORK} looking up {host} got no answer within {} s, so the computer is probably offline. {hint}", limit.as_secs().max(1))),
    };
    let mut last = String::new();
    for a in addrs.iter().take(4) {
        let left = limit.saturating_sub(began.elapsed());
        if left.is_zero() {
            break;
        }
        match TcpStream::connect_timeout(a, left.min(Duration::from_secs(5))) {
            Ok(_) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
    }
    let l = last.to_ascii_lowercase();
    let why = if l.contains("refused") {
        format!("{host}:{port} refused the connection")
    } else if l.contains("unreachable") {
        format!("there is no route to {host}")
    } else {
        format!("{host} did not answer within {} s", limit.as_secs().max(1))
    };
    Err(format!("{NO_NETWORK} {why}. {hint}"))
}

/// Wraps a [`Streamer`] so that a download starts with a quick reachability check of the host: with no network the person gets
/// the answer within [`Preflight::limit`] instead of after the long connect and retry times. A host that was reachable a moment ago is
/// not checked again for 20 s. When a proxy is set in the environment the check is skipped (the proxy, not the host, is what must be reached).
pub struct Preflight<S: Streamer> {
    inner: S,
    pub limit: Duration,
    seen: std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
    skip: bool,
}

impl<S: Streamer> Preflight<S> {
    pub fn new(inner: S, limit: Duration) -> Preflight<S> {
        let skip = ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"].iter().any(|k| std::env::var(k).is_ok_and(|v| !v.is_empty()));
        Preflight { inner, limit, seen: std::sync::Mutex::new(std::collections::HashMap::new()), skip }
    }

    fn check(&self, url: &str) -> Result<(), String> {
        if self.skip {
            return Ok(());
        }
        let https = url.starts_with("https://");
        let hostport = host_of(url);
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h.to_string(), p.parse().unwrap_or(80)),
            _ => (hostport.clone(), if https { 443 } else { 80 }),
        };
        let key = format!("{host}:{port}");
        if let Some(t) = self.seen.lock().ok().and_then(|m| m.get(&key).copied()) {
            if t.elapsed() < Duration::from_secs(20) {
                return Ok(());
            }
        }
        check_reachable(host.trim_matches(['[', ']']), port, self.limit)?;
        if let Ok(mut m) = self.seen.lock() {
            m.insert(key, std::time::Instant::now());
        }
        Ok(())
    }
}

impl<S: Streamer> Streamer for Preflight<S> {
    fn open(&self, url: &str, headers: &[(String, String)]) -> Result<StreamHead, String> {
        self.check(url)?;
        self.inner.open(url, headers)
    }
    fn post(&self, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<StreamHead, String> {
        self.check(url)?;
        self.inner.post(url, headers, body)
    }
    fn open_tls(&self, url: &str, headers: &[(String, String)], mode: &TlsMode) -> Result<StreamHead, String> {
        self.check(url)?;
        self.inner.open_tls(url, headers, mode)
    }
    fn peer_chain(&self, host: &str, port: u16) -> Result<Vec<Vec<u8>>, String> {
        self.inner.peer_chain(host, port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Sender;
    use std::sync::Mutex;

    /// Gives `body` after `delay`, or never answers (`hang`).
    struct Fake {
        delay: Duration,
        hang: bool,
        fail: Option<String>,
        body: Mutex<Option<Box<dyn Read + Send>>>,
    }
    impl Streamer for Fake {
        fn open(&self, _: &str, _: &[(String, String)]) -> Result<StreamHead, String> {
            if self.hang {
                std::thread::sleep(Duration::from_secs(30));
            }
            std::thread::sleep(self.delay);
            if let Some(e) = &self.fail {
                return Err(e.clone());
            }
            Ok(StreamHead { status: 200, headers: vec![], body: self.body.lock().unwrap().take().unwrap() })
        }
    }

    /// A body that gives each message as it is sent and blocks in between, like a socket.
    struct Pipe(std::sync::mpsc::Receiver<Vec<u8>>);
    impl Read for Pipe {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            match self.0.recv() {
                Ok(d) => {
                    out[..d.len()].copy_from_slice(&d);
                    Ok(d.len())
                }
                Err(_) => Ok(0),
            }
        }
    }

    fn quick() -> Limits {
        Limits { start: Duration::from_millis(300), first_byte: Duration::from_millis(300), stall: Duration::from_millis(300) }
    }
    fn fake(body: impl Read + Send + 'static) -> Fake {
        Fake { delay: Duration::ZERO, hang: false, fail: None, body: Mutex::new(Some(Box::new(body))) }
    }
    fn pipe() -> (Sender<Vec<u8>>, Pipe) {
        let (tx, rx) = std::sync::mpsc::channel();
        (tx, Pipe(rx))
    }

    #[test]
    fn a_download_that_flows_is_read_whole() {
        let g = Guarded::new(fake(std::io::Cursor::new(vec![7u8; 200_000])), quick());
        let mut h = g.open("https://example.org/a.bin", &[]).unwrap();
        let mut v = Vec::new();
        h.body.read_to_end(&mut v).unwrap();
        assert_eq!(v.len(), 200_000);
    }

    #[test]
    fn a_host_that_never_answers_is_a_network_problem_after_the_start_limit() {
        let g = Guarded::new(Fake { delay: Duration::ZERO, hang: true, fail: None, body: Mutex::new(None) }, quick());
        let t = std::time::Instant::now();
        let e = g.open("https://slow.example.org/a.bin", &[]).err().expect("gives up");
        assert!(e.starts_with("network problem: no answer from slow.example.org within"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn no_first_byte_and_a_stall_in_the_middle_are_told_apart() {
        let (_tx, p) = pipe();
        let g = Guarded::new(fake(p), quick());
        let mut h = g.open("https://example.org/x", &[]).unwrap();
        let e = h.body.read(&mut [0u8; 10]).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::TimedOut);
        assert!(e.to_string().contains("no first byte from example.org"), "{e}");

        let (tx, p) = pipe();
        let g = Guarded::new(fake(p), quick());
        let mut h = g.open("https://example.org/x", &[]).unwrap();
        tx.send(vec![1, 2, 3]).unwrap();
        let mut b = [0u8; 10];
        assert_eq!(h.body.read(&mut b).unwrap(), 3);
        let e = h.body.read(&mut b).unwrap_err();
        assert!(e.to_string().contains("no data from example.org for 0 s") || e.to_string().contains("no data from example.org"), "{e}");
        assert!(e.to_string().starts_with("network problem"), "{e}");
    }

    #[test]
    fn transport_errors_that_are_the_networks_say_so_and_a_404_stays_plain() {
        assert!(classify("Connection Failed: dns error: failed to lookup", "h").starts_with("network problem reaching h"));
        assert!(classify("connection reset by peer", "h").starts_with("network problem reaching h"));
        assert_eq!(classify("the file is not found (404)", "h"), "the file is not found (404)");
        let g = Guarded::new(Fake { delay: Duration::ZERO, hang: false, fail: Some("timed out reading response".into()), body: Mutex::new(None) }, quick());
        assert!(g.open("https://example.org/x", &[]).err().unwrap().starts_with("network problem reaching example.org"));
    }

    #[test]
    fn a_pause_shorter_than_the_stall_limit_is_fine() {
        let (tx, p) = pipe();
        let g = Guarded::new(fake(p), quick());
        let mut h = g.open("https://example.org/x", &[]).unwrap();
        let t = std::thread::spawn(move || {
            tx.send(vec![1; 5]).unwrap();
            std::thread::sleep(Duration::from_millis(150));
            tx.send(vec![2; 5]).unwrap();
        });
        let mut v = Vec::new();
        h.body.read_to_end(&mut v).unwrap();
        t.join().unwrap();
        assert_eq!(v, [1, 1, 1, 1, 1, 2, 2, 2, 2, 2]);
    }

    fn closed_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    }

    #[test]
    fn a_closed_port_fails_at_once_with_a_plain_message() {
        let p = Preflight::new(fake(std::io::Cursor::new(vec![1u8])), Duration::from_secs(3));
        let t = std::time::Instant::now();
        let e = p.open(&format!("http://127.0.0.1:{}/a.bin", closed_port()), &[]).err().expect("refused");
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
        assert!(is_no_network(&e) && e.contains("refused the connection") && e.contains("Retry") && e.contains("Cancel"), "{e}");
    }

    #[test]
    fn a_name_that_cannot_be_looked_up_fails_within_the_limit() {
        let t = std::time::Instant::now();
        let e = check_reachable("no-such-host.invalid", 443, Duration::from_secs(4)).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(6), "{:?}", t.elapsed());
        assert!(is_no_network(&e) && e.contains("no-such-host.invalid"), "{e}");
    }

    /// A listener whose queue is full drops further connection attempts without an answer, which is what an address that
    /// swallows packets does: the check must give up at its limit and say so.
    #[test]
    fn an_address_that_swallows_packets_fails_at_the_limit() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        // fill the accept queue (never accepted)
        // (a refused or slow connect on a loaded machine is not "the queue is full": only a miss after a good number of held connections is)
        let mut held = Vec::new();
        let wall = std::time::Instant::now();
        let mut misses = 0;
        while held.len() < 300 && misses < 60 && wall.elapsed() < Duration::from_secs(60) {
            match std::net::TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().unwrap(), Duration::from_millis(100)) {
                Ok(s) => {
                    held.push(s);
                    misses = 0;
                }
                // the queue is full when several connects in a row time out; one slow connect on a loaded machine is not that
                Err(_) if held.len() >= 10 && misses >= 4 => break,
                Err(_) => misses += 1,
            }
        }
        if held.len() >= 300 || held.len() < 10 {
            return; // this kernel never fills the queue (or never let a connection in); nothing to test here
        }
        let t = std::time::Instant::now();
        let e = check_reachable("127.0.0.1", port, Duration::from_secs(2)).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(60), "{:?}", t.elapsed());
        assert!(is_no_network(&e) && e.contains("did not answer within 2 s"), "{e}");
        drop(held);
        drop(l);
    }

    #[test]
    fn a_reachable_host_passes_and_is_remembered() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let p = Preflight::new(fake(std::io::Cursor::new(vec![9u8; 10])), Duration::from_secs(3));
        let mut h = p.open(&format!("http://127.0.0.1:{port}/x"), &[]).unwrap();
        let mut v = Vec::new();
        h.body.read_to_end(&mut v).unwrap();
        assert_eq!(v.len(), 10);
        drop(l); // now closed, but the answer is remembered for a few seconds
        assert!(p.check(&format!("http://127.0.0.1:{port}/y")).is_ok());
    }
}
