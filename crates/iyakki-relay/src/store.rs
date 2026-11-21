//! The queues. Everything here is pure state plus a `now_ms` argument; the caller owns the clock and the lock.
//!
//! The limits repeat the numbers of the client side (`iyakki-agent` `devmode/relay.rs`); a test in `iyakki-agent` checks they are equal.

use std::collections::{BTreeMap, VecDeque};

use serde::Deserialize;

pub const RELAY_PROTOCOL: u32 = 1;
pub const MAX_ENVELOPE_BYTES: usize = 64 * 1024;
pub const MAX_TTL_MS: u64 = 24 * 60 * 60 * 1000;
pub const MAX_SKEW_MS: u64 = 5 * 60 * 1000;
pub const MAILBOX_PER_MINUTE: usize = 120;
pub const MAILBOX_DEPTH: usize = 256;
/// The client refuses a fetch answer with more than this many envelopes.
pub const FETCH_BATCH: usize = 64;

/// What this relay enforces; the defaults are the contract's numbers, the rest protect the machine it runs on.
#[derive(Clone, Debug)]
pub struct Limits {
    /// The longest an envelope waits here, whatever its own ttl says (24 hours; a test relay may set it short).
    pub max_wait_ms: u64,
    pub per_mailbox_per_minute: usize,
    pub mailbox_depth: usize,
    /// All queued bytes together.
    pub max_total_bytes: usize,
    /// Mailboxes that hold something at once.
    pub max_mailboxes: usize,
    /// Requests (posts plus fetches) one network address may make per minute.
    pub per_peer_per_minute: usize,
    /// Distinct network addresses remembered for that rate.
    pub max_peers: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_wait_ms: MAX_TTL_MS,
            per_mailbox_per_minute: MAILBOX_PER_MINUTE,
            mailbox_depth: MAILBOX_DEPTH,
            max_total_bytes: 256 * 1024 * 1024,
            max_mailboxes: 10_000,
            per_peer_per_minute: 1200,
            max_peers: 4096,
        }
    }
}

/// Why a request was not served: an HTTP status and a plain sentence (never a body or a key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub status: u16,
    pub why: &'static str,
}

const fn no(status: u16, why: &'static str) -> Refusal {
    Refusal { status, why }
}

#[derive(Deserialize)]
struct Head {
    v: u32,
    mailbox: String,
    from: String,
    #[allow(dead_code)]
    seq: u64,
    sent_ms: u64,
    ttl_ms: u64,
    kind: String,
    #[allow(dead_code)]
    body: String,
}

/// A mailbox id as the relay accepts it in a path or an envelope.
pub fn plain_mailbox(m: &str) -> bool {
    !m.is_empty() && m.len() <= 80 && m.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[derive(Default)]
struct Mailbox {
    /// (bytes, expires_ms)
    q: VecDeque<(Vec<u8>, u64)>,
    posted: VecDeque<u64>,
}

#[derive(Default)]
pub struct Store {
    pub limits: Limits,
    boxes: BTreeMap<String, Mailbox>,
    total: usize,
    peers: BTreeMap<String, VecDeque<u64>>,
    swept_at: u64,
    /// Counters, for the log and the tests.
    pub posted: u64,
    pub delivered: u64,
    pub expired: u64,
}

impl Store {
    pub fn new(limits: Limits) -> Store {
        Store { limits, ..Store::default() }
    }

    /// Accept one envelope for its mailbox. The relay reads the header only (it checks the shape, size, version, ttl and clock); the body
    /// is an opaque string it never decodes.
    pub fn post(&mut self, peer: &str, bytes: &[u8], now_ms: u64) -> Result<(), Refusal> {
        self.sweep(now_ms);
        self.peer_ok(peer, now_ms)?;
        if bytes.len() > MAX_ENVELOPE_BYTES {
            return Err(no(413, "the envelope is over 64 KiB"));
        }
        let h: Head = serde_json::from_slice(bytes).map_err(|_| no(400, "that is not an envelope"))?;
        if h.v != RELAY_PROTOCOL {
            return Err(no(400, "this relay speaks relay protocol 1"));
        }
        if h.ttl_ms == 0 || h.ttl_ms > MAX_TTL_MS {
            return Err(no(400, "the ttl must be between 1 ms and 24 hours"));
        }
        if !plain_mailbox(&h.mailbox) || h.from.is_empty() || h.from.len() > 80 || !matches!(h.kind.as_str(), "open" | "accept" | "msg" | "refused") {
            return Err(no(400, "that is not an envelope"));
        }
        let own_end = h.sent_ms.saturating_add(h.ttl_ms);
        if own_end <= now_ms {
            return Err(no(410, "the envelope has already expired"));
        }
        if h.sent_ms > now_ms.saturating_add(MAX_SKEW_MS) {
            return Err(no(400, "the sender's clock is ahead of the relay's"));
        }
        let until = own_end.min(now_ms.saturating_add(self.limits.max_wait_ms));
        if !self.boxes.contains_key(&h.mailbox) && self.boxes.len() >= self.limits.max_mailboxes {
            return Err(no(503, "the relay holds as many mailboxes as it allows"));
        }
        if self.total + bytes.len() > self.limits.max_total_bytes {
            return Err(no(503, "the relay is full"));
        }
        let b = self.boxes.entry(h.mailbox).or_default();
        while b.posted.front().is_some_and(|t| now_ms.saturating_sub(*t) >= 60_000) {
            b.posted.pop_front();
        }
        if b.posted.len() >= self.limits.per_mailbox_per_minute {
            return Err(no(429, "too many envelopes for this mailbox this minute"));
        }
        if b.q.len() >= self.limits.mailbox_depth {
            return Err(no(429, "this mailbox is full"));
        }
        b.posted.push_back(now_ms);
        b.q.push_back((bytes.to_vec(), until));
        self.total += bytes.len();
        self.posted += 1;
        Ok(())
    }

    /// Hand over up to [`FETCH_BATCH`] envelopes waiting in a mailbox, each once, oldest first.
    pub fn fetch(&mut self, peer: &str, mailbox: &str, now_ms: u64) -> Result<Vec<Vec<u8>>, Refusal> {
        self.sweep(now_ms);
        self.peer_ok(peer, now_ms)?;
        if !plain_mailbox(mailbox) {
            return Err(no(400, "that is not a mailbox name"));
        }
        let mut out = Vec::new();
        if let Some(b) = self.boxes.get_mut(mailbox) {
            while out.len() < FETCH_BATCH {
                let Some((bytes, until)) = b.q.pop_front() else { break };
                self.total = self.total.saturating_sub(bytes.len());
                if until <= now_ms {
                    self.expired += 1;
                } else {
                    out.push(bytes);
                }
            }
            if b.q.is_empty() && b.posted.is_empty() {
                self.boxes.remove(mailbox);
            }
        }
        self.delivered += out.len() as u64;
        Ok(out)
    }

    fn peer_ok(&mut self, peer: &str, now_ms: u64) -> Result<(), Refusal> {
        if self.peers.len() >= self.limits.max_peers && !self.peers.contains_key(peer) {
            self.peers.retain(|_, t| t.back().is_some_and(|l| now_ms.saturating_sub(*l) < 60_000));
            if self.peers.len() >= self.limits.max_peers {
                return Err(no(503, "too many network addresses at once"));
            }
        }
        let t = self.peers.entry(peer.to_string()).or_default();
        while t.front().is_some_and(|x| now_ms.saturating_sub(*x) >= 60_000) {
            t.pop_front();
        }
        if t.len() >= self.limits.per_peer_per_minute {
            return Err(no(429, "too many requests from this address this minute"));
        }
        t.push_back(now_ms);
        Ok(())
    }

    /// Drop what outlived its time, at most once a second.
    pub fn sweep(&mut self, now_ms: u64) {
        if now_ms < self.swept_at.saturating_add(1000) && self.swept_at != 0 {
            return;
        }
        self.swept_at = now_ms;
        let mut gone_bytes = 0;
        let mut gone = 0;
        for b in self.boxes.values_mut() {
            b.q.retain(|(bytes, until)| {
                let keep = *until > now_ms;
                if !keep {
                    gone_bytes += bytes.len();
                    gone += 1;
                }
                keep
            });
            while b.posted.front().is_some_and(|t| now_ms.saturating_sub(*t) >= 60_000) {
                b.posted.pop_front();
            }
        }
        self.boxes.retain(|_, b| !b.q.is_empty() || !b.posted.is_empty());
        self.total = self.total.saturating_sub(gone_bytes);
        self.expired += gone;
    }

    pub fn waiting(&self, mailbox: &str) -> usize {
        self.boxes.get(mailbox).map_or(0, |b| b.q.len())
    }

    pub fn queued_bytes(&self) -> usize {
        self.total
    }

    pub fn mailboxes(&self) -> usize {
        self.boxes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_800_000_000_000;

    fn env(mailbox: &str, seq: u64, sent: u64, ttl: u64) -> Vec<u8> {
        format!(r#"{{"v":1,"mailbox":"{mailbox}","from":"abc","seq":{seq},"sent_ms":{sent},"ttl_ms":{ttl},"kind":"msg","body":"00ff"}}"#).into_bytes()
    }

    #[test]
    fn an_envelope_is_handed_over_once_and_oldest_first() {
        let mut s = Store::default();
        s.post("p", &env("m1", 1, T0, 60_000), T0).unwrap();
        s.post("p", &env("m1", 2, T0, 60_000), T0).unwrap();
        s.post("p", &env("m2", 1, T0, 60_000), T0).unwrap();
        assert_eq!(s.waiting("m1"), 2);
        let got = s.fetch("p", "m1", T0 + 1).unwrap();
        assert_eq!(got.len(), 2);
        assert!(String::from_utf8_lossy(&got[0]).contains(r#""seq":1"#));
        assert!(s.fetch("p", "m1", T0 + 2).unwrap().is_empty(), "once");
        assert_eq!(s.waiting("m2"), 1, "another mailbox is untouched");
        assert_eq!(s.queued_bytes(), env("m2", 1, T0, 60_000).len());
    }

    #[test]
    fn a_queued_envelope_expires_at_its_ttl_and_at_the_relay_cap() {
        let mut s = Store::default();
        s.post("p", &env("m", 1, T0, 60_000), T0).unwrap();
        assert!(s.fetch("p", "m", T0 + 60_000).unwrap().is_empty(), "at the ttl it is gone");
        assert_eq!(s.expired, 1);
        assert_eq!(s.queued_bytes(), 0);
        // the relay's own cap is shorter than the envelope's ttl
        let mut s = Store::new(Limits { max_wait_ms: 3_000, ..Limits::default() });
        s.post("p", &env("m", 1, T0, 3_600_000), T0).unwrap();
        assert_eq!(s.fetch("p", "m", T0 + 2_999).unwrap().len(), 1);
        s.post("p", &env("m", 2, T0, 3_600_000), T0).unwrap();
        assert!(s.fetch("p", "m", T0 + 3_000).unwrap().is_empty());
        // the sweep alone drops it too, without any fetch
        s.post("p", &env("m", 3, T0 + 10_000, 3_600_000), T0 + 10_000).unwrap();
        s.sweep(T0 + 20_000);
        assert_eq!(s.waiting("m"), 0);
        s.sweep(T0 + 80_000);
        assert_eq!(s.mailboxes(), 0, "an empty mailbox is forgotten once its rate window has passed");
    }

    #[test]
    fn the_twenty_four_hour_ceiling_and_clock_rules() {
        let mut s = Store::default();
        assert_eq!(s.post("p", &env("m", 1, T0, MAX_TTL_MS + 1), T0).unwrap_err().status, 400);
        assert_eq!(s.post("p", &env("m", 1, T0, 0), T0).unwrap_err().status, 400);
        assert_eq!(s.post("p", &env("m", 1, T0 - 120_000, 60_000), T0).unwrap_err().status, 410, "already expired");
        assert_eq!(s.post("p", &env("m", 1, T0 + MAX_SKEW_MS + 1, 60_000), T0).unwrap_err().status, 400, "sender clock ahead");
        s.post("p", &env("m", 1, T0 + MAX_SKEW_MS, 60_000), T0).unwrap();
        s.post("p", &env("m", 2, T0, MAX_TTL_MS), T0).unwrap();
        assert_eq!(s.fetch("p", "m", T0 + MAX_TTL_MS - 1).unwrap().len(), 1, "the first one expired after six minutes; only the 24 hour one is left");
    }

    #[test]
    fn malformed_oversized_and_unknown_things_are_refused_with_a_sentence() {
        let mut s = Store::default();
        assert_eq!(s.post("p", b"not json", T0).unwrap_err().status, 400);
        assert_eq!(s.post("p", &vec![b' '; MAX_ENVELOPE_BYTES + 1], T0).unwrap_err().status, 413);
        let wrong_v = String::from_utf8(env("m", 1, T0, 1000)).unwrap().replace(r#""v":1"#, r#""v":2"#);
        assert_eq!(s.post("p", wrong_v.as_bytes(), T0).unwrap_err().status, 400);
        let wrong_kind = String::from_utf8(env("m", 1, T0, 1000)).unwrap().replace("msg", "boom");
        assert_eq!(s.post("p", wrong_kind.as_bytes(), T0).unwrap_err().status, 400);
        assert_eq!(s.post("p", &env("../etc", 1, T0, 1000), T0).unwrap_err().status, 400);
        assert_eq!(s.fetch("p", "a b", T0).unwrap_err().status, 400);
        assert!(s.fetch("p", "nobody", T0).unwrap().is_empty());
    }

    #[test]
    fn the_body_is_never_read() {
        // a body that is not hex and not anything: the relay carries it as the opaque string it is
        let e = br#"{"v":1,"mailbox":"m","from":"f","seq":1,"sent_ms":1800000000000,"ttl_ms":1000,"kind":"open","body":"zz not hex at all"}"#;
        let mut s = Store::default();
        s.post("p", e, T0).unwrap();
        assert_eq!(s.fetch("p", "m", T0).unwrap(), vec![e.to_vec()], "returned byte for byte");
    }

    #[test]
    fn depth_rate_and_total_limits() {
        let mut s = Store::new(Limits { mailbox_depth: 3, per_mailbox_per_minute: 5, ..Limits::default() });
        for i in 0..3 {
            s.post("p", &env("m", i, T0, 600_000), T0).unwrap();
        }
        assert_eq!(s.post("p", &env("m", 9, T0, 600_000), T0).unwrap_err().status, 429);
        s.fetch("p", "m", T0).unwrap();
        s.post("p", &env("m", 4, T0, 600_000), T0).unwrap();
        s.post("p", &env("m", 5, T0, 600_000), T0).unwrap();
        assert_eq!(s.post("p", &env("m", 6, T0, 600_000), T0).unwrap_err().status, 429, "five a minute");
        s.fetch("p", "m", T0 + 61_000).unwrap();
        s.post("p", &env("m", 7, T0 + 61_000, 600_000), T0 + 61_000).unwrap();
        let one = env("t", 1, T0, 1000).len();
        let mut s = Store::new(Limits { max_total_bytes: one * 2, max_mailboxes: 2, ..Limits::default() });
        s.post("p", &env("t", 1, T0, 1000), T0).unwrap();
        s.post("p", &env("t", 2, T0, 1000), T0).unwrap();
        assert_eq!(s.post("p", &env("t", 3, T0, 1000), T0).unwrap_err().status, 503);
        let mut s = Store::new(Limits { max_mailboxes: 1, ..Limits::default() });
        s.post("p", &env("a", 1, T0, 1000), T0).unwrap();
        assert_eq!(s.post("p", &env("b", 1, T0, 1000), T0).unwrap_err().status, 503);
        s.post("p", &env("a", 2, T0, 1000), T0).unwrap();
    }

    #[test]
    fn one_address_cannot_flood_and_a_batch_is_at_most_sixty_four() {
        let mut s = Store::new(Limits { per_peer_per_minute: 4, ..Limits::default() });
        for _ in 0..4 {
            s.fetch("1.2.3.4", "m", T0).unwrap();
        }
        assert_eq!(s.fetch("1.2.3.4", "m", T0).unwrap_err().status, 429);
        s.fetch("5.6.7.8", "m", T0).unwrap();
        s.fetch("1.2.3.4", "m", T0 + 60_000).unwrap();
        let mut s = Store::new(Limits { mailbox_depth: 100, per_mailbox_per_minute: 100, ..Limits::default() });
        for i in 0..70 {
            s.post("p", &env("m", i, T0, 600_000), T0).unwrap();
        }
        assert_eq!(s.fetch("p", "m", T0).unwrap().len(), FETCH_BATCH);
        assert_eq!(s.fetch("p", "m", T0).unwrap().len(), 6);
    }
}
