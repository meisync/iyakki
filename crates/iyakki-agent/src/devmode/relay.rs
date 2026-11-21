//! The cloud relay transport of the core contract (F-CORE-17; design: docs/development/CORE-CONTRACT.md, "Relay transport").
//!
//! A relay is a mailbox, not a computer. A client and a machine that cannot reach each other both make OUTGOING connections to it and
//! exchange small sealed envelopes addressed to a mailbox id (a hash of the machine's relay public key). The relay sees who writes to
//! which mailbox and when, never what: the body is encrypted between the client and the machine.
//!
//! What is here is everything except the relay itself (which the owner runs, KI-725): the envelope and its limits, the keys, the
//! sealing, the replay and expiry rules, the machine's three switches and first-use question, the client, and [`FakeRelay`], an
//! in-memory relay with the limits and the 24 hour queue a real one must keep, which the tests (and a future relay's own tests) use.
//!
//! Keys. The machine has a static X25519 key made the first time the relay is switched on; its public half and the mailbox id are what a
//! paired computer is given to pin. A session starts with an `open` envelope: the client's fresh X25519 key, signed with the Ed25519 key
//! of its developer-mode pairing. The machine answers `accept` with its own fresh key and a sealed confirmation. Both sides derive two
//! keys (one per direction) with HKDF-SHA256 from the two Diffie-Hellman results (client ephemeral with the machine's static key, and
//! ephemeral with ephemeral) and the transcript; only a machine that holds the static key can make a confirmation that opens. Every
//! message is XChaCha20-Poly1305 with the envelope header as associated data and a nonce built from the direction and the sequence number.
//!
//! Rules the machine keeps (each has a test): the relay is OFF by default (a third switch beside developer mode and the contract
//! switch); only a paired computer is answered; the first contact of a computer raises a question on the machine's screen and nothing
//! runs until an administrator approves it there (a remote client cannot); the contract's own scopes, rate limit and audit apply
//! unchanged; a sequence number that does not rise is dropped (replay); an envelope past its ttl is dropped; nothing is written to the
//! log but names, kinds and reasons, never bodies or keys.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Mutex;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use iyakki_core_api::contract::Transport;
use iyakki_sec::policy::Policy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::contract::{handle_over, CoreCx};
use super::DevMode;
use crate::cloudkeys::{hex, hkdf, random, unhex};

/// The relay's own tiny protocol version (`relay 1`).
pub const RELAY_PROTOCOL: u32 = 1;
/// The largest envelope a relay carries, as bytes of its encoding.
pub const MAX_ENVELOPE_BYTES: usize = 64 * 1024;
/// The longest an envelope may wait at the relay: 24 hours.
pub const MAX_TTL_MS: u64 = 24 * 60 * 60 * 1000;
/// How far a sender's clock may be ahead of the receiver's before an envelope is dropped.
pub const MAX_SKEW_MS: u64 = 5 * 60 * 1000;
/// Envelopes a mailbox accepts per minute.
pub const MAILBOX_PER_MINUTE: usize = 120;
/// Envelopes a mailbox holds at most.
pub const MAILBOX_DEPTH: usize = 256;

const SIGN_CONTEXT: &[u8] = b"iyakki-relay-open-v1";
const KEY_INFO: &[u8] = b"iyakki-relay-keys-v1";

/// One sealed message, as the relay sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u32,
    /// Where it goes: a machine's mailbox id, or `c-<client key id>` for a reply.
    pub mailbox: String,
    /// The sender's key id (a client: its pairing key in hex; the machine: its mailbox id).
    pub from: String,
    /// Strictly increasing per sender.
    pub seq: u64,
    pub sent_ms: u64,
    pub ttl_ms: u64,
    /// `open`, `accept`, `msg` or `refused`.
    pub kind: String,
    /// Hex. For `msg` the AEAD ciphertext of one contract message; for `open` and `accept` a small public body.
    pub body: String,
}

/// Why an envelope was refused or dropped. Names only; never a body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    TooBig,
    Malformed,
    BadVersion,
    BadTtl,
    Expired,
    FromTheFuture,
    Replay,
    /// Not addressed to this machine.
    NotForMe,
    /// The relay is off on this machine.
    SwitchedOff,
    UnknownComputer,
    BadSignature,
    NoSession,
    BadSeal,
    /// The first contact of this computer: the question is on the machine's screen and waits for an administrator there.
    NeedsApproval,
}

impl Dropped {
    pub fn word(self) -> &'static str {
        match self {
            Dropped::TooBig => "too_big",
            Dropped::Malformed => "malformed",
            Dropped::BadVersion => "bad_version",
            Dropped::BadTtl => "bad_ttl",
            Dropped::Expired => "expired",
            Dropped::FromTheFuture => "from_the_future",
            Dropped::Replay => "replay",
            Dropped::NotForMe => "not_for_me",
            Dropped::SwitchedOff => "switched_off",
            Dropped::UnknownComputer => "unknown_computer",
            Dropped::BadSignature => "bad_signature",
            Dropped::NoSession => "no_session",
            Dropped::BadSeal => "bad_seal",
            Dropped::NeedsApproval => "needs_approval",
        }
    }
}

impl Envelope {
    /// The wire form. Fails when it would be over the limit.
    pub fn encode(&self) -> Result<Vec<u8>, Dropped> {
        let b = serde_json::to_vec(self).map_err(|_| Dropped::Malformed)?;
        if b.len() > MAX_ENVELOPE_BYTES {
            return Err(Dropped::TooBig);
        }
        Ok(b)
    }

    /// Read and check one: size, shape, version and ttl (not the clock; see [`Envelope::check_time`]).
    pub fn decode(bytes: &[u8]) -> Result<Envelope, Dropped> {
        if bytes.len() > MAX_ENVELOPE_BYTES {
            return Err(Dropped::TooBig);
        }
        let e: Envelope = serde_json::from_slice(bytes).map_err(|_| Dropped::Malformed)?;
        if e.v != RELAY_PROTOCOL {
            return Err(Dropped::BadVersion);
        }
        if e.ttl_ms == 0 || e.ttl_ms > MAX_TTL_MS {
            return Err(Dropped::BadTtl);
        }
        if e.mailbox.is_empty() || e.mailbox.len() > 80 || e.from.is_empty() || e.from.len() > 80 || !matches!(e.kind.as_str(), "open" | "accept" | "msg" | "refused") {
            return Err(Dropped::Malformed);
        }
        Ok(e)
    }

    pub fn check_time(&self, now_ms: u64) -> Result<(), Dropped> {
        if self.sent_ms.saturating_add(self.ttl_ms) <= now_ms {
            return Err(Dropped::Expired);
        }
        if self.sent_ms > now_ms.saturating_add(MAX_SKEW_MS) {
            return Err(Dropped::FromTheFuture);
        }
        Ok(())
    }

    /// What the sealing binds the ciphertext to: everything but the body.
    fn aad(&self) -> Vec<u8> {
        format!("{}|{}|{}|{}|{}|{}|{}", self.v, self.mailbox, self.from, self.seq, self.sent_ms, self.ttl_ms, self.kind).into_bytes()
    }
}

/// The mailbox id of a machine: a hash of its relay public key.
pub fn mailbox_of(machine_public: &[u8; 32]) -> String {
    hex(&Sha256::digest(machine_public)[..16])
}

/// The mailbox replies to a client go to.
pub fn client_mailbox(from: &str) -> String {
    format!("c-{}", &from[..from.len().min(32)])
}

// ----------------------------------------------------------------------------------------------------------------- the keys

#[derive(Clone)]
struct Keys {
    c2m: [u8; 32],
    m2c: [u8; 32],
}

fn derive(ss1: &[u8; 32], ss2: &[u8; 32], transcript: &[u8]) -> Keys {
    let mut ikm = [0u8; 64];
    ikm[..32].copy_from_slice(ss1);
    ikm[32..].copy_from_slice(ss2);
    let mut out = [0u8; 64];
    hkdf(transcript, &ikm, KEY_INFO, &mut out);
    let mut k = Keys { c2m: [0; 32], m2c: [0; 32] };
    k.c2m.copy_from_slice(&out[..32]);
    k.m2c.copy_from_slice(&out[32..]);
    k
}

fn transcript(from: &str, epk_c: &[u8; 32], epk_m: &[u8; 32], machine_public: &[u8; 32]) -> Vec<u8> {
    let mut t = Vec::new();
    t.extend_from_slice(b"iyakki-relay-1|");
    t.extend_from_slice(from.as_bytes());
    t.extend_from_slice(epk_c);
    t.extend_from_slice(epk_m);
    t.extend_from_slice(machine_public);
    t
}

fn nonce(direction: u8, seq: u64) -> [u8; 24] {
    let mut n = [0u8; 24];
    n[0] = direction;
    n[16..].copy_from_slice(&seq.to_le_bytes());
    n
}

fn seal(key: &[u8; 32], direction: u8, e: &Envelope, plain: &[u8]) -> Result<String, Dropped> {
    let ct = XChaCha20Poly1305::new(Key::from_slice(key)).encrypt(XNonce::from_slice(&nonce(direction, e.seq)), Payload { msg: plain, aad: &e.aad() }).map_err(|_| Dropped::BadSeal)?;
    Ok(hex(&ct))
}

fn unseal(key: &[u8; 32], direction: u8, e: &Envelope) -> Result<Vec<u8>, Dropped> {
    let ct = unhex(&e.body).ok_or(Dropped::Malformed)?;
    XChaCha20Poly1305::new(Key::from_slice(key)).decrypt(XNonce::from_slice(&nonce(direction, e.seq)), Payload { msg: &ct, aad: &e.aad() }).map_err(|_| Dropped::BadSeal)
}

fn pub32(s: &str) -> Option<[u8; 32]> {
    unhex(s).and_then(|b| <[u8; 32]>::try_from(b).ok())
}

fn open_signature_input(mailbox: &str, epk: &[u8; 32], sent_ms: u64, seq: u64) -> Vec<u8> {
    let mut m = SIGN_CONTEXT.to_vec();
    m.extend_from_slice(mailbox.as_bytes());
    m.extend_from_slice(epk);
    m.extend_from_slice(&sent_ms.to_le_bytes());
    m.extend_from_slice(&seq.to_le_bytes());
    m
}

// ----------------------------------------------------------------------------------------------------------- the machine

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
struct RelayState {
    /// "Allow connections through the relay": off by default.
    enabled: bool,
    /// Computers an administrator approved on this machine after their first contact.
    approved: Vec<String>,
    /// Where the relay is (`https://host[:port]`); empty until the person gives it (F-CORE-33).
    #[serde(default)]
    url: String,
    /// SHA-256 fingerprint of the relay's own certificate (64 hex digits): an https relay is trusted for that certificate only, and
    /// without it the machine connects nowhere (F-CORE-33). Empty until the person gives it.
    #[serde(default)]
    pin: String,
}

struct Session {
    keys: Keys,
    last_seq: u64,
    out_seq: u64,
    computer: String,
}

/// What the machine needs to serve a call, as the developer-mode channel has it.
pub struct RelayCx<'a> {
    pub dm: &'a DevMode,
    pub policy: &'a Policy,
    pub core: &'a crate::contract_local::Core,
    pub gate: &'a super::contract::Gate,
    pub hostname: &'a str,
    pub audit: &'a dyn Fn(&str, &str),
    /// Raise the first-contact question on the machine's screen (the name of the computer).
    pub ask: &'a dyn Fn(&str),
}

pub struct RelayMachine {
    secret: [u8; 32],
    public: [u8; 32],
    sessions: Mutex<HashMap<String, Session>>,
    /// Highest `open` sequence seen per computer: an `open` is never replayed.
    opens: Mutex<HashMap<String, u64>>,
}

impl DevMode {
    fn relay_state(&self) -> RelayState {
        self.read("relay.json")
    }

    /// "Allow connections through the relay" (off by default).
    pub fn relay_enabled(&self) -> bool {
        self.relay_state().enabled
    }

    /// Switch the relay on or off (administrators, on the device). Off ends nothing already queued at a relay; the machine simply stops answering.
    pub fn set_relay_enabled(&self, policy: &Policy, user: &str, on: bool) -> Result<(), String> {
        Self::need_admin(policy)?;
        let _one = super::keys_lock();
        let mut st = self.relay_state();
        st.enabled = on;
        self.write("relay.json", &st)?;
        self.log(user, user, if on { "devmode.relay.on" } else { "devmode.relay.off" }, "connections through the relay", true);
        Ok(())
    }

    /// Approve a paired computer's first contact through the relay (administrators, on the device: there is no remote way to this).
    pub fn approve_relay_computer(&self, policy: &Policy, user: &str, computer: &str, on: bool) -> Result<(), String> {
        Self::need_admin(policy)?;
        let _one = super::keys_lock();
        if !self.keys().iter().any(|k| k.name == computer) {
            return Err(format!("no paired computer called {computer}"));
        }
        let mut st = self.relay_state();
        st.approved.retain(|c| c != computer);
        if on {
            st.approved.push(computer.to_string());
        }
        self.write("relay.json", &st)?;
        self.log(user, user, if on { "devmode.relay.approve" } else { "devmode.relay.revoke" }, computer, true);
        Ok(())
    }

    /// The relay's address: where the machine connects out to (administrators; asks the same card as the switch). Empty takes it away.
    pub fn set_relay_url(&self, policy: &Policy, user: &str, url: &str) -> Result<(), String> {
        Self::need_admin(policy)?;
        let u = url.trim();
        if !u.is_empty() {
            super::relay_link::HttpLink::new(std::sync::Arc::new(NoNet), u)?;
        }
        let _one = super::keys_lock();
        let mut st = self.relay_state();
        st.url = u.trim_end_matches('/').to_string();
        self.write("relay.json", &st)?;
        self.log(user, user, "devmode.relay.address", if u.is_empty() { "removed" } else { "set" }, true);
        Ok(())
    }

    pub fn relay_url(&self) -> String {
        self.relay_state().url
    }

    /// The fingerprint of the relay's certificate that an https relay must show (empty: none given, so an https relay is not used).
    pub fn relay_pin(&self) -> String {
        self.relay_state().pin
    }

    /// Set the fingerprint of the relay's certificate (administrators; asks the same card as the switch). Empty takes it away. Checked: 64 hex digits.
    pub fn set_relay_pin(&self, policy: &Policy, user: &str, pin: &str) -> Result<(), String> {
        Self::need_admin(policy)?;
        let p = iyakki_proxy::certinfo::norm_fingerprint(pin.trim());
        if !p.is_empty() && (p.len() != 64 || pin.trim().chars().any(|c| !(c.is_ascii_hexdigit() || c == ':' || c == ' '))) {
            return Err("the certificate fingerprint is 64 hexadecimal digits (SHA-256), with or without colons".into());
        }
        let _one = super::keys_lock();
        let mut st = self.relay_state();
        st.pin = p;
        self.write("relay.json", &st)?;
        self.log(user, user, "devmode.relay.pin", if st.pin.is_empty() { "removed" } else { "set" }, true);
        Ok(())
    }

    pub fn relay_approved(&self, computer: &str) -> bool {
        self.relay_state().approved.iter().any(|c| c == computer)
    }

    /// What the person sees: the switch and the computers that may use it.
    pub fn relay_summary(&self) -> String {
        let st = self.relay_state();
        format!(
            "The relay is {}{}. Approved computers: {}.{}",
            if st.enabled { "on" } else { "off (the default)" },
            if st.url.is_empty() {
                " (no address set: the machine connects nowhere)".to_string()
            } else if st.url.starts_with("https://") && st.pin.is_empty() {
                format!(" (address {}, but no certificate fingerprint is set: an https relay is only used for the certificate you name, so the machine connects nowhere)", st.url)
            } else if st.url.starts_with("https://") {
                format!(" (address {}, certificate {})", st.url, iyakki_proxy::certinfo::fingerprint_groups(&st.pin).split(' ').next().unwrap_or(""))
            } else {
                format!(" (address {})", st.url)
            },
            if st.approved.is_empty() { "none".to_string() } else { st.approved.join(", ") },
            if st.enabled { "" } else { " Nothing reaches this machine through a relay while it is off." }
        )
    }
}

/// Only to check an address's shape.
struct NoNet;

impl iyakki_proxy::Upstream for NoNet {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("no network".into())
    }
}

impl RelayMachine {
    /// Open (or make) the machine's relay key in the developer-mode folder.
    pub fn open(dm: &DevMode) -> Result<RelayMachine, String> {
        let path = dm.data_dir().join("devmode").join("relay.key");
        let secret = match std::fs::read_to_string(&path).ok().and_then(|s| pub32(s.trim())) {
            Some(s) => s,
            None => {
                let s = random::<32>()?;
                super::write_private(&path, hex(&s).as_bytes())?;
                s
            }
        };
        let public = x25519_dalek::x25519(secret, x25519_dalek::X25519_BASEPOINT_BYTES);
        Ok(RelayMachine { secret, public, sessions: Mutex::new(HashMap::new()), opens: Mutex::new(HashMap::new()) })
    }

    /// What a paired computer pins: the machine's relay public key (hex).
    pub fn public_hex(&self) -> String {
        hex(&self.public)
    }

    pub fn mailbox(&self) -> String {
        mailbox_of(&self.public)
    }

    fn note(&self, cx: &RelayCx, who: &str, what: &str, ok: bool) {
        cx.dm.log(who, "", &format!("relay.{what}"), "", ok);
    }

    /// Take one envelope off the relay. `Ok(reply)` (maybe none); `Err` says why it was dropped (and the log has the reason, never a body).
    pub fn process(&self, cx: &RelayCx, env: &Envelope, now_ms: u64) -> Result<Option<Envelope>, Dropped> {
        let r = self.process_inner(cx, env, now_ms);
        if let Err(d) = &r {
            self.note(cx, &env.from.chars().take(12).collect::<String>(), &format!("drop.{}", d.word()), false);
        }
        r
    }

    fn process_inner(&self, cx: &RelayCx, env: &Envelope, now_ms: u64) -> Result<Option<Envelope>, Dropped> {
        if env.mailbox != self.mailbox() {
            return Err(Dropped::NotForMe);
        }
        env.check_time(now_ms)?;
        // the third switch: while off, nothing is answered and nothing is learned about the machine
        if !cx.dm.relay_enabled() {
            return Err(Dropped::SwitchedOff);
        }
        let key = cx.dm.find_key(&env.from).ok_or(Dropped::UnknownComputer)?;
        match env.kind.as_str() {
            "open" => self.open_session(cx, env, &key, now_ms),
            "msg" => self.serve(cx, env, &key, now_ms),
            _ => Err(Dropped::Malformed),
        }
    }

    fn open_session(&self, cx: &RelayCx, env: &Envelope, key: &super::DevKey, now_ms: u64) -> Result<Option<Envelope>, Dropped> {
        let body: Value = serde_json::from_slice(&unhex(&env.body).ok_or(Dropped::Malformed)?).map_err(|_| Dropped::Malformed)?;
        let epk_c = pub32(body["epk"].as_str().unwrap_or("")).ok_or(Dropped::Malformed)?;
        let sig = unhex(body["sig"].as_str().unwrap_or("")).and_then(|b| <[u8; 64]>::try_from(b).ok()).ok_or(Dropped::Malformed)?;
        let vk = pub32(&key.key).and_then(|b| VerifyingKey::from_bytes(&b).ok()).ok_or(Dropped::UnknownComputer)?;
        vk.verify(&open_signature_input(&env.mailbox, &epk_c, env.sent_ms, env.seq), &Signature::from_bytes(&sig)).map_err(|_| Dropped::BadSignature)?;
        {
            let mut opens = self.opens.lock().unwrap_or_else(|e| e.into_inner());
            let last = opens.entry(env.from.clone()).or_insert(0);
            if env.seq <= *last {
                return Err(Dropped::Replay);
            }
            *last = env.seq;
        }
        // first use: a question on this machine's screen; nothing runs until an administrator there answers it
        if !cx.dm.relay_approved(&key.name) {
            (cx.ask)(&key.name);
            self.note(cx, &key.name, "first_contact", true);
            let reply = Envelope { v: RELAY_PROTOCOL, mailbox: client_mailbox(&env.from), from: self.mailbox(), seq: env.seq, sent_ms: now_ms, ttl_ms: 60_000, kind: "refused".into(), body: hex(b"needs_approval") };
            let _ = reply; // a plain notice would tell a relay that this machine is awake and who asked; the client sees silence until approved
            return Err(Dropped::NeedsApproval);
        }
        let eph = random::<32>().map_err(|_| Dropped::BadSeal)?;
        let epk_m = x25519_dalek::x25519(eph, x25519_dalek::X25519_BASEPOINT_BYTES);
        let ss1 = x25519_dalek::x25519(self.secret, epk_c);
        let ss2 = x25519_dalek::x25519(eph, epk_c);
        if ss1 == [0u8; 32] || ss2 == [0u8; 32] {
            return Err(Dropped::Malformed);
        }
        let keys = derive(&ss1, &ss2, &transcript(&env.from, &epk_c, &epk_m, &self.public));
        let mut reply = Envelope { v: RELAY_PROTOCOL, mailbox: client_mailbox(&env.from), from: self.mailbox(), seq: 1, sent_ms: now_ms, ttl_ms: 5 * 60_000, kind: "accept".into(), body: String::new() };
        let confirm = seal(&keys.m2c, 1, &reply, b"iyakki relay ok")?;
        reply.body = hex(&serde_json::to_vec(&json!({"epk": hex(&epk_m), "confirm": confirm})).unwrap_or_default());
        self.sessions.lock().unwrap_or_else(|e| e.into_inner()).insert(env.from.clone(), Session { keys, last_seq: 0, out_seq: 1, computer: key.name.clone() });
        self.note(cx, &key.name, "open", true);
        Ok(Some(reply))
    }

    fn serve(&self, cx: &RelayCx, env: &Envelope, key: &super::DevKey, now_ms: u64) -> Result<Option<Envelope>, Dropped> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let s = sessions.get_mut(&env.from).ok_or(Dropped::NoSession)?;
        if !cx.dm.relay_approved(&s.computer) {
            return Err(Dropped::NeedsApproval);
        }
        if env.seq <= s.last_seq {
            return Err(Dropped::Replay);
        }
        let plain = unseal(&s.keys.c2m, 0, env)?;
        // only a message that opened is counted: a forged one cannot move the window
        s.last_seq = env.seq;
        let req: Value = serde_json::from_slice(&plain).map_err(|_| Dropped::Malformed)?;
        let route = req["route"].as_str().unwrap_or("");
        let body = serde_json::to_vec(&req["body"]).unwrap_or_default();
        let core_cx = CoreCx { dm: cx.dm, key, policy: cx.policy, core: cx.core, gate: cx.gate, hostname: cx.hostname, audit: cx.audit };
        let out = handle_over(&core_cx, Transport::Relay, "POST", &format!("/v1/dev/core/{route}"), &body).unwrap_or_else(|| super::api::Out::err(404, "no such route"));
        let answer = json!({ "status": out.status, "body": serde_json::from_slice::<Value>(&out.body).unwrap_or(Value::Null) });
        s.out_seq += 1;
        let mut reply = Envelope { v: RELAY_PROTOCOL, mailbox: client_mailbox(&env.from), from: self.mailbox(), seq: s.out_seq, sent_ms: now_ms, ttl_ms: env.ttl_ms.min(MAX_TTL_MS), kind: "msg".into(), body: String::new() };
        reply.body = seal(&s.keys.m2c, 1, &reply, &serde_json::to_vec(&answer).unwrap_or_default())?;
        Ok(Some(reply))
    }
}

// ------------------------------------------------------------------------------------------------------------- the client

/// The client's end: a paired computer that holds its pairing key and has pinned the machine's relay public key.
pub struct RelayClient {
    sign: SigningKey,
    from: String,
    machine_public: [u8; 32],
    seq: u64,
    eph: Option<[u8; 32]>,
    keys: Option<Keys>,
    last_in: u64,
}

impl RelayClient {
    /// `sign_seed` is the 32-byte seed of the pairing key; `machine_public_hex` what the machine showed; `start_seq` any number above
    /// every sequence this client used before (the current time in milliseconds will do).
    pub fn new(sign_seed: [u8; 32], machine_public_hex: &str, start_seq: u64) -> Result<RelayClient, String> {
        let sign = SigningKey::from_bytes(&sign_seed);
        let from = hex(sign.verifying_key().as_bytes());
        Ok(RelayClient { sign, from, machine_public: pub32(machine_public_hex).ok_or("the machine's relay key is not 32 bytes of hex")?, seq: start_seq, eph: None, keys: None, last_in: 0 })
    }

    pub fn mailbox(&self) -> String {
        mailbox_of(&self.machine_public)
    }

    /// Where the answers to this client arrive.
    pub fn reply_mailbox(&self) -> String {
        client_mailbox(&self.from)
    }

    pub fn is_open(&self) -> bool {
        self.keys.is_some()
    }

    /// Start a session.
    pub fn open(&mut self, now_ms: u64, ttl_ms: u64) -> Result<Envelope, String> {
        let eph = random::<32>()?;
        let epk = x25519_dalek::x25519(eph, x25519_dalek::X25519_BASEPOINT_BYTES);
        self.seq += 1;
        let mailbox = self.mailbox();
        let sig = self.sign.sign(&open_signature_input(&mailbox, &epk, now_ms, self.seq));
        self.eph = Some(eph);
        self.keys = None;
        Ok(Envelope { v: RELAY_PROTOCOL, mailbox, from: self.from.clone(), seq: self.seq, sent_ms: now_ms, ttl_ms, kind: "open".into(), body: hex(&serde_json::to_vec(&json!({"epk": hex(&epk), "sig": hex(&sig.to_bytes())})).unwrap_or_default()) })
    }

    /// Take the machine's `accept`. Fails when the confirmation does not open: not the machine that was pinned.
    pub fn accept(&mut self, e: &Envelope) -> Result<(), Dropped> {
        let eph = self.eph.ok_or(Dropped::NoSession)?;
        if e.kind != "accept" || e.mailbox != self.reply_mailbox() || e.from != self.mailbox() {
            return Err(Dropped::Malformed);
        }
        let body: Value = serde_json::from_slice(&unhex(&e.body).ok_or(Dropped::Malformed)?).map_err(|_| Dropped::Malformed)?;
        let epk_m = pub32(body["epk"].as_str().unwrap_or("")).ok_or(Dropped::Malformed)?;
        let epk_c = x25519_dalek::x25519(eph, x25519_dalek::X25519_BASEPOINT_BYTES);
        let ss1 = x25519_dalek::x25519(eph, self.machine_public);
        let ss2 = x25519_dalek::x25519(eph, epk_m);
        if ss1 == [0u8; 32] || ss2 == [0u8; 32] {
            return Err(Dropped::Malformed);
        }
        let keys = derive(&ss1, &ss2, &transcript(&self.from, &epk_c, &epk_m, &self.machine_public));
        let proof = Envelope { body: body["confirm"].as_str().unwrap_or("").to_string(), ..e.clone() };
        if unseal(&keys.m2c, 1, &proof)? != b"iyakki relay ok" {
            return Err(Dropped::BadSeal);
        }
        self.keys = Some(keys);
        self.eph = None;
        self.last_in = e.seq;
        Ok(())
    }

    /// One contract request (`route` is `hello`, `call`, `stream/read` or `stream/close`; `body` as over the TLS channel).
    pub fn request(&mut self, route: &str, body: &Value, now_ms: u64, ttl_ms: u64) -> Result<Envelope, String> {
        let keys = self.keys.clone().ok_or("no open session: open one first")?;
        self.seq += 1;
        let mut e = Envelope { v: RELAY_PROTOCOL, mailbox: self.mailbox(), from: self.from.clone(), seq: self.seq, sent_ms: now_ms, ttl_ms: ttl_ms.min(MAX_TTL_MS), kind: "msg".into(), body: String::new() };
        e.body = seal(&keys.c2m, 0, &e, &serde_json::to_vec(&json!({"route": route, "body": body})).unwrap_or_default()).map_err(|d| d.word().to_string())?;
        e.encode().map_err(|d| d.word().to_string())?;
        Ok(e)
    }

    /// Read the machine's answer: `(status, body)`.
    pub fn answer(&mut self, e: &Envelope, now_ms: u64) -> Result<(u16, Value), Dropped> {
        let keys = self.keys.clone().ok_or(Dropped::NoSession)?;
        e.check_time(now_ms)?;
        if e.kind != "msg" || e.mailbox != self.reply_mailbox() || e.from != self.mailbox() {
            return Err(Dropped::Malformed);
        }
        if e.seq <= self.last_in {
            return Err(Dropped::Replay);
        }
        let v: Value = serde_json::from_slice(&unseal(&keys.m2c, 1, e)?).map_err(|_| Dropped::Malformed)?;
        self.last_in = e.seq;
        Ok((v["status"].as_u64().unwrap_or(0) as u16, v["body"].clone()))
    }
}

// ----------------------------------------------------------------------------------------------------------- a fake relay

/// An in-memory relay that keeps the rules a real one must: size, ttl, a per-mailbox rate and depth, delivery once, and nothing kept
/// after delivery or expiry. It also counts what expired, per mailbox, so a client can be told "expired" at its next look.
#[derive(Default)]
pub struct FakeRelay {
    boxes: BTreeMap<String, VecDeque<(Vec<u8>, u64)>>,
    posted: BTreeMap<String, VecDeque<u64>>,
    expired: BTreeMap<String, usize>,
    /// Every envelope as the relay saw it, for tests that check it never holds plaintext.
    pub seen: Vec<Vec<u8>>,
}

impl FakeRelay {
    pub fn new() -> FakeRelay {
        FakeRelay::default()
    }

    /// Accept one envelope for its mailbox.
    pub fn post(&mut self, bytes: &[u8], now_ms: u64) -> Result<(), Dropped> {
        let e = Envelope::decode(bytes)?;
        let until = e.sent_ms.saturating_add(e.ttl_ms);
        if until <= now_ms {
            return Err(Dropped::Expired);
        }
        let times = self.posted.entry(e.mailbox.clone()).or_default();
        while times.front().is_some_and(|t| now_ms.saturating_sub(*t) >= 60_000) {
            times.pop_front();
        }
        let q = self.boxes.entry(e.mailbox.clone()).or_default();
        if times.len() >= MAILBOX_PER_MINUTE || q.len() >= MAILBOX_DEPTH {
            return Err(Dropped::TooBig);
        }
        times.push_back(now_ms);
        self.seen.push(bytes.to_vec());
        q.push_back((bytes.to_vec(), until));
        Ok(())
    }

    /// Hand over what waits for a mailbox, once; what outlived its ttl is dropped and counted.
    pub fn fetch(&mut self, mailbox: &str, now_ms: u64) -> Vec<Envelope> {
        let mut out = Vec::new();
        let mut gone = 0;
        if let Some(q) = self.boxes.get_mut(mailbox) {
            for (b, until) in q.drain(..) {
                if until <= now_ms {
                    gone += 1;
                } else if let Ok(e) = Envelope::decode(&b) {
                    out.push(e);
                }
            }
        }
        if gone > 0 {
            *self.expired.entry(mailbox.to_string()).or_default() += gone;
        }
        out
    }

    /// How many requests to this mailbox expired unanswered since the last call (the client is told at its next look).
    pub fn take_expired(&mut self, mailbox: &str) -> usize {
        self.expired.remove(mailbox).unwrap_or(0)
    }

    /// Envelopes waiting now.
    pub fn waiting(&self, mailbox: &str) -> usize {
        self.boxes.get(mailbox).map_or(0, VecDeque::len)
    }
}
