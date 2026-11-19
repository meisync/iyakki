//! The network side of the relay transport (F-CORE-33): how a machine and a paired computer reach a relay over HTTP(S). The relay only
//! keeps sealed envelopes in mailboxes (`relay.rs`); this file is the client of it, and the machine's outgoing loop.
//!
//! The wire is small on purpose, so anyone can host a relay that speaks it:
//!   POST `{base}/v1/relay/post`            body: one encoded envelope            answer: 204, or 4xx with a plain sentence
//!   GET  `{base}/v1/relay/fetch/{mailbox}`                                        answer: 200 and a JSON list of hex envelopes, each once
//! The machine only ever connects OUT (nothing listens for the relay), the mailbox ids are hashes, and every body is sealed by `relay.rs`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iyakki_proxy::Upstream;
use serde_json::Value;

use super::relay::{Dropped, Envelope, RelayClient, RelayCx, RelayMachine};

/// A relay as the two ends use it.
pub trait RelayLink: Send + Sync {
    fn post(&self, bytes: &[u8]) -> Result<(), String>;
    /// What waits in `mailbox`, handed over once.
    fn fetch(&self, mailbox: &str) -> Result<Vec<Vec<u8>>, String>;
}

/// A relay at an address, over any HTTP transport (the real one, or a fake in a test).
pub struct HttpLink {
    pub up: Arc<dyn Upstream>,
    pub base: String,
}

fn plain_mailbox(m: &str) -> bool {
    !m.is_empty() && m.len() <= 80 && m.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl HttpLink {
    /// `base` is `https://host[:port]` (or `http://` for a relay on a trusted local network); no path, no query.
    pub fn new(up: Arc<dyn Upstream>, base: &str) -> Result<HttpLink, String> {
        let b = base.trim().trim_end_matches('/');
        let ok = (b.starts_with("https://") || b.starts_with("http://")) && !b[b.find("://").map_or(0, |i| i + 3)..].contains(['/', '?', '#', ' ']) && b.len() <= 200;
        if !ok {
            return Err("the relay address is https://HOST or https://HOST:PORT".into());
        }
        Ok(HttpLink { up, base: b.to_string() })
    }
}

impl RelayLink for HttpLink {
    fn post(&self, bytes: &[u8]) -> Result<(), String> {
        let r = self.up.request("POST", &format!("{}/v1/relay/post", self.base), &[("Content-Type".into(), "application/octet-stream".into())], bytes)?;
        match r.status {
            200..=299 => Ok(()),
            s => Err(format!("the relay refused the envelope ({s}): {}", String::from_utf8_lossy(&r.body).chars().take(120).collect::<String>())),
        }
    }

    fn fetch(&self, mailbox: &str) -> Result<Vec<Vec<u8>>, String> {
        if !plain_mailbox(mailbox) {
            return Err("that is not a mailbox name".into());
        }
        let r = self.up.request("GET", &format!("{}/v1/relay/fetch/{mailbox}", self.base), &[], b"")?;
        if r.status != 200 {
            return Err(format!("the relay answered {}", r.status));
        }
        let list: Vec<Value> = serde_json::from_slice(&r.body).map_err(|_| "the relay's answer is not the agreed list".to_string())?;
        if list.len() > 64 {
            return Err("the relay handed over too many envelopes at once".into());
        }
        list.iter().map(|v| v.as_str().and_then(unhex).ok_or_else(|| "an envelope in the relay's answer is not hex".to_string())).collect()
    }
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || s.len() > 200_000 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// What one pass of the machine's loop did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pass {
    pub taken: usize,
    pub replied: usize,
    pub dropped: usize,
}

/// One pass: fetch the machine's mailbox, let `relay.rs` judge each envelope (its checks, the switch and the first-contact question are all
/// there), post the replies. A relay that cannot be reached is an `Err` the loop waits out; nothing is lost, the relay keeps the envelopes.
pub fn pump_once(link: &dyn RelayLink, machine: &RelayMachine, cx: &RelayCx, now_ms: u64) -> Result<Pass, String> {
    let mut p = Pass::default();
    for bytes in link.fetch(&machine.mailbox())? {
        p.taken += 1;
        let Ok(env) = Envelope::decode(&bytes) else {
            p.dropped += 1;
            continue;
        };
        match machine.process(cx, &env, now_ms) {
            Ok(Some(reply)) => match reply.encode() {
                Ok(b) => {
                    link.post(&b)?;
                    p.replied += 1;
                }
                Err(_) => p.dropped += 1,
            },
            Ok(None) => {}
            Err(_) => p.dropped += 1,
        }
    }
    Ok(p)
}

/// The machine's outgoing loop: every `every` it calls `step` (which builds the machine's context and calls [`pump_once`]) until `stop`. Only
/// while the person has the relay switched on does `step` do anything (that check lives in `RelayMachine::process`); a failing relay is waited
/// out with a longer pause, at most a minute.
pub fn run_loop(stop: &AtomicBool, every: Duration, step: &mut dyn FnMut() -> Result<Pass, String>) {
    let mut pause = every;
    while !stop.load(Ordering::SeqCst) {
        pause = match step() {
            Ok(_) => every,
            Err(_) => (pause * 2).min(Duration::from_secs(60)).max(every),
        };
        let mut waited = Duration::ZERO;
        while waited < pause && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(20));
            waited += Duration::from_millis(20);
        }
    }
}

/// A paired computer's side: open a session and ask one contract question, through the relay at `link`, waiting up to `wait` for the answer.
pub fn client_open(link: &dyn RelayLink, c: &mut RelayClient, now_ms: u64, ttl_ms: u64, wait: Duration) -> Result<(), String> {
    let open = c.open(now_ms, ttl_ms)?;
    link.post(&open.encode().map_err(|d| d.word().to_string())?)?;
    let got = wait_for(link, &c.reply_mailbox(), wait)?;
    let accept = Envelope::decode(&got).map_err(|d| d.word().to_string())?;
    c.accept(&accept).map_err(|d: Dropped| format!("the machine's answer was not accepted: {}", d.word()))
}

pub fn client_ask(link: &dyn RelayLink, c: &mut RelayClient, route: &str, body: &Value, now_ms: u64, ttl_ms: u64, wait: Duration) -> Result<(u16, Value), String> {
    let req = c.request(route, body, now_ms, ttl_ms)?;
    link.post(&req.encode().map_err(|d| d.word().to_string())?)?;
    let got = wait_for(link, &c.reply_mailbox(), wait)?;
    let e = Envelope::decode(&got).map_err(|d| d.word().to_string())?;
    c.answer(&e, now_ms + wait.as_millis() as u64).map_err(|d| format!("the answer was not accepted: {}", d.word()))
}

fn wait_for(link: &dyn RelayLink, mailbox: &str, wait: Duration) -> Result<Vec<u8>, String> {
    let t = std::time::Instant::now();
    loop {
        if let Some(b) = link.fetch(mailbox)?.into_iter().next() {
            return Ok(b);
        }
        if t.elapsed() >= wait {
            return Err("nothing came back from the machine in time (it may be off, the relay switched off there, or the request expired)".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
