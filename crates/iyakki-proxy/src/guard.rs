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
}
