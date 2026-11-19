//! The fleet over the relay (F-FLEET-10, docs/development/FLEET-AND-MODEL-ROUTER-PLAN.md section 3.2; the relay itself is the mailbox of
//! F-CORE-17, `devmode/relay.rs`).
//!
//! A member that cannot be reached directly (behind a router with no address to give) names a mailbox instead of `host:port`: the address
//! `relay://MAILBOX`. A message to it is the SAME signed and group-sealed member message as on the LAN, carried as the body of a relay
//! envelope; the receiver's own relay loop fetches its mailbox, hands each message to the hub exactly as if it had come in over TCP, and posts
//! the answer back to the sender's mailbox. The answer is sealed with the group key too, because unlike a TCP answer it passes the relay.
//! The relay sees which mailbox writes to which and when, never what.
//!
//! What goes this way: member messages only (heartbeat, record push, assignment, answers, status, shared context, model query). A join is
//! not offered through a relay (its code is for two people at the same place), and a bundle chunk is too big for an envelope (64 KiB), so a
//! hand-back with a bundle needs a direct address (decision D-FLEET-10 in docs/development/ALPHA-7-DECISIONS.md).

use std::sync::atomic::AtomicU64;
use std::sync::{Condvar, MutexGuard};

use sha2::{Digest, Sha256};

use super::*;
use crate::devmode::relay::{Envelope, FakeRelay, MAX_ENVELOPE_BYTES, RELAY_PROTOCOL};

pub const SCHEME: &str = "relay://";
/// How long a posted message waits at the relay for its answer to come back.
const TTL_MS: u64 = 5 * 60 * 1000;

/// The relay as the fleet needs it: post an envelope, fetch what waits for a mailbox. The real one dials the owner's relay (KI-725, not
/// built); [`FakeRelay`] keeps the same rules in memory for the tests.
pub trait Post: Send + Sync {
    fn post(&self, envelope: &[u8], now_ms: u64) -> Result<(), String>;
    fn fetch(&self, mailbox: &str, now_ms: u64) -> Result<Vec<Envelope>, String>;
}

impl Post for Mutex<FakeRelay> {
    fn post(&self, envelope: &[u8], now_ms: u64) -> Result<(), String> {
        lock(self).post(envelope, now_ms).map_err(|d| format!("the relay refused the message ({})", d.word()))
    }
    fn fetch(&self, mailbox: &str, now_ms: u64) -> Result<Vec<Envelope>, String> {
        Ok(lock(self).fetch(mailbox, now_ms))
    }
}

/// The mailbox of a member: a hash of its signing key, so it names no one.
pub fn mailbox_of(sign_id: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"iyakki-fleet-mailbox-v1");
    h.update(sign_id.as_bytes());
    hex(&h.finalize()[..16])
}

/// Whether text is a mailbox id as this module makes them.
pub fn valid_mailbox(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The link of one hub to its relay.
pub struct RelayLink {
    post: Arc<dyn Post>,
    mailbox: String,
    seq: AtomicU64,
    wait: Duration,
    /// Answers that came back, by the nonce of the message they answer: (fleet, generation, sealed answer).
    replies: Mutex<BTreeMap<String, (String, u64, String)>>,
    arrived: Condvar,
    now_ms: Mutex<Arc<dyn Fn() -> u64 + Send + Sync>>,
}

impl RelayLink {
    pub(super) fn mailbox_text(&self) -> String {
        self.mailbox.clone()
    }

    fn ms(&self) -> u64 {
        (lock(&self.now_ms))()
    }

    fn envelope(&self, to: &str, payload: &Value) -> Result<Vec<u8>, String> {
        let body = hex(payload.to_string().as_bytes());
        let e = Envelope { v: RELAY_PROTOCOL, mailbox: to.into(), from: self.mailbox.clone(), seq: self.seq.fetch_add(1, Ordering::SeqCst) + 1, sent_ms: self.ms(), ttl_ms: TTL_MS, kind: "msg".into(), body };
        e.encode().map_err(|d| if d == crate::devmode::relay::Dropped::TooBig { format!("that message is bigger than a relay carries ({} KiB): use a direct address for it", MAX_ENVELOPE_BYTES / 1024) } else { format!("the relay envelope is not valid ({})", d.word()) })
    }

    fn wait_for(&self, nonce: &str) -> Result<(String, u64, String), String> {
        let mut g: MutexGuard<'_, BTreeMap<String, (String, u64, String)>> = lock(&self.replies);
        let deadline = std::time::Instant::now() + self.wait;
        loop {
            if let Some(r) = g.remove(nonce) {
                return Ok(r);
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err("the member did not answer through the relay in time (it may be offline; the message is dropped when it expires)".into());
            }
            g = self.arrived.wait_timeout(g, left).unwrap_or_else(std::sync::PoisonError::into_inner).0;
        }
    }
}

/// One message to a member by its mailbox, and its answer. `key` opens the answer.
pub(super) fn exchange_relay(link: Option<&Arc<RelayLink>>, mailbox: &str, env: &Env, key: &dyn Fn(&str, u64, &str) -> Result<Vec<u8>, String>) -> Result<Reply, String> {
    let link = link.ok_or("this member is reached through the relay, which is not switched on here")?;
    if !valid_mailbox(mailbox) {
        return Err("that relay address is not valid".into());
    }
    let bytes = link.envelope(mailbox, &json!({"req": env, "reply_to": link.mailbox}))?;
    link.post.post(&bytes, link.ms())?;
    let (fleet, generation, sealed) = link.wait_for(&env.nonce)?;
    if fleet != env.fleet {
        return Err("the answer is for another fleet".into());
    }
    let plain = key(&fleet, generation, &sealed)?;
    serde_json::from_slice(&plain).map_err(|_| "the answer through the relay is not readable".into())
}

impl Hub {
    /// Switch the relay on for this hub: the mailbox of this system is made from its key and returned. `post` is the relay (the fake one in
    /// tests); `wait` is how long a message waits for its answer. Nothing is sent until a member has a `relay://` address.
    pub fn set_relay(&self, post: Arc<dyn Post>, wait: Duration) -> String {
        let mailbox = mailbox_of(&lock(&self.inner).keys.public().sign);
        let clock = lock(&self.clock).clone();
        let link = Arc::new(RelayLink { post, mailbox: mailbox.clone(), seq: AtomicU64::new(0), wait, replies: Mutex::new(BTreeMap::new()), arrived: Condvar::new(), now_ms: Mutex::new(Arc::new(move || clock() * 1000)) });
        *lock(&self.relay) = Some(link);
        self.audit("fleet.relay.on", &format!("mailbox={}", &mailbox[..8]));
        mailbox
    }

    pub fn relay_off(&self) {
        *lock(&self.relay) = None;
        self.audit("fleet.relay.off", "");
    }

    /// This system's mailbox when the relay is on: what another member is given as `relay://MAILBOX`.
    pub fn relay_address(&self) -> Option<String> {
        lock(&self.relay).as_ref().map(|l| format!("{SCHEME}{}", l.mailbox))
    }

    pub(super) fn relay_link(&self) -> Option<Arc<RelayLink>> {
        lock(&self.relay).clone()
    }

    /// The group key opens an answer that came through the relay.
    pub(super) fn open_relayed(&self, fleet: &str, generation: u64, sealed: &str) -> Result<Vec<u8>, String> {
        let i = lock(&self.inner);
        let f = i.fleets.get(fleet).ok_or("that fleet is gone")?;
        if generation != f.rec.generation {
            return Err("the group key generation differs (the record is being updated: try again)".into());
        }
        f.key.open(fleet, "relay_reply", generation, sealed)
    }

    /// One look at the mailbox: every request waiting is handled as if it had come in over TCP and answered; every answer is given to the
    /// message that waits for it. Returns how many envelopes were dealt with. The relay loop of [`Hub::start_relay`] calls this.
    pub fn relay_pump(&self) -> usize {
        let Some(link) = self.relay_link() else { return 0 };
        let Ok(list) = link.post.fetch(&link.mailbox, link.ms()) else { return 0 };
        let n = list.len();
        for e in list {
            let Some(payload) = unhex_text(&e.body).and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
            if let Some(rep) = payload.get("rep") {
                let nonce = rep["corr"].as_str().unwrap_or("").to_string();
                let (fleet, generation, sealed) = (rep["fleet"].as_str().unwrap_or("").to_string(), rep["generation"].as_u64().unwrap_or(0), rep["sealed"].as_str().unwrap_or("").to_string());
                if nonce.len() <= 64 {
                    lock(&link.replies).insert(nonce, (fleet, generation, sealed));
                    link.arrived.notify_all();
                }
                continue;
            }
            let (Some(req), Some(to)) = (payload.get("req").and_then(|r| serde_json::from_value::<Env>(r.clone()).ok()), payload["reply_to"].as_str().map(str::to_string)) else { continue };
            if !valid_mailbox(&to) || !is_member_kind(&req.kind) {
                continue;
            }
            // there is no peer address behind a relay: the unspecified one tells the handlers not to learn an address from it
            let reply = self.handle(req.clone(), SocketAddr::from(([0, 0, 0, 0], 0)));
            let body = {
                let i = lock(&self.inner);
                match i.fleets.get(&req.fleet) {
                    Some(f) => {
                        let plain = serde_json::to_string(&reply).unwrap_or_default();
                        f.key.seal(&req.fleet, "relay_reply", f.rec.generation, plain.as_bytes()).ok().map(|s| json!({"rep": {"corr": req.nonce, "fleet": req.fleet, "generation": f.rec.generation, "sealed": s}}))
                    }
                    None => None,
                }
            };
            // an answer to a fleet this system does not know cannot be sealed, so it is not sent: the sender's wait ends by its own timeout
            if let Some(b) = body {
                if let Ok(bytes) = link.envelope(&to, &b) {
                    let _ = link.post.post(&bytes, link.ms());
                }
            }
        }
        n
    }

    /// Run the relay loop: look at the mailbox every `poll` until [`Hub::stop_relay`]. Call [`Hub::set_relay`] first.
    pub fn start_relay(self: &Arc<Self>, poll: Duration) -> Result<(), String> {
        let mut t = lock(&self.relay_thread);
        if t.is_some() {
            return Ok(());
        }
        if self.relay_link().is_none() {
            return Err("switch the relay on first".into());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (me, s2) = (Arc::downgrade(self), stop.clone());
        let h = std::thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                let Some(hub) = me.upgrade() else { break };
                hub.relay_pump();
                drop(hub);
                std::thread::sleep(poll);
            }
        });
        *t = Some((stop, h));
        Ok(())
    }

    pub fn stop_relay(&self) {
        if let Some((stop, h)) = lock(&self.relay_thread).take() {
            stop.store(true, Ordering::SeqCst);
            let _ = h.join();
        }
    }
}

fn is_member_kind(k: &str) -> bool {
    matches!(k, "record_push" | "heartbeat" | "assign" | "assign_reply" | "leave_notice" | "task_status" | "task_answer" | "context" | "models_query")
}

fn unhex_text(s: &str) -> Option<String> {
    let b: Option<Vec<u8>> = s.as_bytes().chunks(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok()).collect();
    String::from_utf8(b?).ok()
}
