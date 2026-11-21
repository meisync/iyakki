//! The relay transport (F-CORE-17) end to end through an in-memory relay: a real client key, the real contract and a real (temporary)
//! developer-mode folder on the machine's side; only the relay itself is a fake (the owner runs the real one).

use std::cell::RefCell;
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use iyakki_sec::policy::{Policy, Role};
use serde_json::{json, Value};

use super::contract::Gate;
use super::relay::*;
use super::{DevKey, DevMode};
use crate::cloudkeys::hex;
use crate::contract_local::Core;
use crate::services::Services;

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

const SEED: [u8; 32] = [7; 32];
const T0: u64 = 1_800_000_000_000;

struct Rig {
    dir: std::path::PathBuf,
    dm: DevMode,
    admin: Policy,
    core: Core,
    gate: Gate,
    machine: RelayMachine,
    asked: RefCell<Vec<String>>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rig(tag: &str, relay_on: bool, scopes: bool) -> Rig {
    let dir = std::env::temp_dir().join(format!("iyakki-relay-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("devmode")).unwrap();
    let dm = DevMode::open(&dir);
    let admin = Role::Admin.template();
    dm.enable(&admin, "root", 0).unwrap();
    let vk = SigningKey::from_bytes(&SEED).verifying_key();
    let key = DevKey { name: "laptop".into(), key: hex(vk.as_bytes()), account: "root".into(), added: 1, added_by: "root".into(), last_used: 0, scopes: vec![] };
    std::fs::write(dir.join("devmode/keys.json"), serde_json::to_string(&vec![key]).unwrap()).unwrap();
    dm.set_core_enabled(&admin, "root", true).unwrap();
    if scopes {
        dm.set_core_scope(&admin, "root", "laptop", super::contract::GRANTABLE[0], true).unwrap();
    }
    if relay_on {
        dm.set_relay_enabled(&admin, "root", true).unwrap();
    }
    let svc = Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let machine = RelayMachine::open(&dm).unwrap();
    Rig { dir, dm, admin, core: Core::new(svc), gate: Gate::standard(), machine, asked: RefCell::new(vec![]) }
}

impl Rig {
    fn process(&self, e: &Envelope, now: u64) -> Result<Option<Envelope>, Dropped> {
        let audit = |_: &str, _: &str| {};
        let ask = |who: &str| self.asked.borrow_mut().push(who.to_string());
        let cx = RelayCx { dm: &self.dm, policy: &self.admin, core: &self.core, gate: &self.gate, hostname: "iyakki-test", audit: &audit, ask: &ask };
        self.machine.process(&cx, e, now)
    }

    fn client(&self) -> RelayClient {
        RelayClient::new(SEED, &self.machine.public_hex(), T0).unwrap()
    }

    /// Open a session through the relay: post, let the machine fetch and process, fetch the reply. `None`: silence.
    fn connect(&self, relay: &mut FakeRelay, c: &mut RelayClient, now: u64) -> Result<(), Dropped> {
        let open = c.open(now, 60_000).unwrap();
        relay.post(&open.encode().unwrap(), now).unwrap();
        for e in relay.fetch(&self.machine.mailbox(), now) {
            if let Some(r) = self.process(&e, now)? {
                relay.post(&r.encode().unwrap(), now).unwrap();
            }
        }
        let got = relay.fetch(&c.reply_mailbox(), now);
        let accept = got.first().ok_or(Dropped::NoSession)?;
        c.accept(accept)
    }

    /// One request and its answer, both through the relay.
    fn ask_over(&self, relay: &mut FakeRelay, c: &mut RelayClient, route: &str, body: Value, now: u64) -> Result<(u16, Value), Dropped> {
        let req = c.request(route, &body, now, 60_000).unwrap();
        relay.post(&req.encode().unwrap(), now).unwrap();
        for e in relay.fetch(&self.machine.mailbox(), now) {
            if let Some(r) = self.process(&e, now)? {
                relay.post(&r.encode().unwrap(), now).unwrap();
            }
        }
        let got = relay.fetch(&c.reply_mailbox(), now);
        c.answer(got.first().ok_or(Dropped::NoSession)?, now)
    }
}

#[test]
fn the_relay_is_off_by_default_and_a_machine_that_has_it_off_answers_nothing() {
    let r = rig("off", false, true);
    assert!(!r.dm.relay_enabled());
    assert!(r.dm.relay_summary().contains("off (the default)"));
    let mut relay = FakeRelay::new();
    let mut c = r.client();
    assert_eq!(r.connect(&mut relay, &mut c, T0), Err(Dropped::SwitchedOff));
    assert!(r.asked.borrow().is_empty(), "no question is raised while the switch is off");
    assert!(r.dm.activity(20).iter().any(|a| a.action == "relay.drop.switched_off" && !a.ok));
    // only an administrator can switch it on
    assert!(r.dm.set_relay_enabled(&Role::User.template(), "amy", true).is_err());
}

#[test]
fn the_first_contact_raises_a_question_on_the_machine_and_nothing_runs_until_an_administrator_approves_there() {
    let r = rig("first", true, true);
    let mut relay = FakeRelay::new();
    let mut c = r.client();
    assert_eq!(r.connect(&mut relay, &mut c, T0), Err(Dropped::NeedsApproval));
    assert_eq!(*r.asked.borrow(), ["laptop"], "the question names the computer");
    assert!(!c.is_open());
    assert!(c.request("call", &json!({}), T0, 1000).is_err(), "no session, no request");
    // approval is the person's, on the machine; a user account cannot give it, a computer name that is not paired cannot get it
    assert!(r.dm.approve_relay_computer(&Role::User.template(), "amy", "laptop", true).is_err());
    assert!(r.dm.approve_relay_computer(&r.admin, "root", "ghost", true).is_err());
    r.dm.approve_relay_computer(&r.admin, "root", "laptop", true).unwrap();
    let mut c = RelayClient::new(SEED, &r.machine.public_hex(), T0 + 100).unwrap();
    r.connect(&mut relay, &mut c, T0 + 1000).unwrap();
    assert!(c.is_open());
}

#[test]
fn a_client_talks_the_contract_through_the_relay_and_the_relay_sees_only_sealed_bytes() {
    let r = rig("e2e", true, true);
    r.dm.approve_relay_computer(&r.admin, "root", "laptop", true).unwrap();
    let mut relay = FakeRelay::new();
    let mut c = r.client();
    r.connect(&mut relay, &mut c, T0).unwrap();
    // hello negotiates as over the TLS channel
    let (st, v) = r.ask_over(&mut relay, &mut c, "hello", json!({"contract": {"major": 0, "minor": 1}, "wants": ["settings"]}), T0 + 10).unwrap();
    assert_eq!(st, 200);
    assert_eq!(v["ok"], true);
    assert_eq!(v["result"]["scopes"], json!([super::contract::GRANTABLE[0]]));
    // a granted call runs
    let (_, v) = r.ask_over(&mut relay, &mut c, "call", json!({"domain": "settings", "call": "schema", "args": {}}), T0 + 20).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    // a call outside the granted scopes is refused by the contract's own guard, over the relay too
    let (_, v) = r.ask_over(&mut relay, &mut c, "call", json!({"domain": "settings", "call": "set", "args": {"key": "x", "value": 1}}), T0 + 30).unwrap();
    assert_eq!(v["ok"], false);
    // a call only a person at the machine may make never completes remotely
    let (_, v) = r.ask_over(&mut relay, &mut c, "call", json!({"domain": "vault", "call": "unlock", "args": {}}), T0 + 40).unwrap();
    assert_eq!(v["ok"], false, "{v}");
    // the relay saw envelopes only: no route name, domain or call name in the clear, and the pairing key's signature body is public only
    assert!(relay.seen.len() >= 8);
    for b in &relay.seen {
        let s = String::from_utf8_lossy(b);
        for word in ["schema", "settings", "unlock", "hello"] {
            assert!(!s.contains(word), "the relay saw {word}");
        }
    }
    // the audit names the transport and the call, never a value
    let log = r.dm.activity(50);
    assert!(log.iter().any(|a| a.action == "relay.core.call" || a.action == "dev.core"), "{log:?}");
}

#[test]
fn a_replayed_envelope_a_tampered_one_and_one_from_another_machine_are_dropped() {
    let r = rig("bad", true, true);
    r.dm.approve_relay_computer(&r.admin, "root", "laptop", true).unwrap();
    let mut relay = FakeRelay::new();
    let mut c = r.client();
    r.connect(&mut relay, &mut c, T0).unwrap();
    let req = c.request("hello", &json!({"contract": {"major": 0, "minor": 1}, "wants": []}), T0 + 5, 60_000).unwrap();
    assert!(r.process(&req, T0 + 5).unwrap().is_some());
    assert_eq!(r.process(&req, T0 + 6), Err(Dropped::Replay), "the same sequence number again");
    // a flipped bit in the ciphertext does not open, and does not move the replay window
    let mut forged = c.request("hello", &json!({"contract": {"major": 0, "minor": 1}, "wants": []}), T0 + 7, 60_000).unwrap();
    let mut b = forged.body.clone().into_bytes();
    let last = b.len() - 1;
    b[last] = if b[last] == b'0' { b'1' } else { b'0' };
    let good = forged.clone();
    forged.body = String::from_utf8(b).unwrap();
    assert_eq!(r.process(&forged, T0 + 7), Err(Dropped::BadSeal));
    assert!(r.process(&good, T0 + 8).unwrap().is_some(), "the real one still works: a forgery cannot burn a sequence number");
    // a header changed after sealing does not open either (the header is the associated data)
    let mut moved = c.request("hello", &json!({"contract": {"major": 0, "minor": 1}, "wants": []}), T0 + 9, 60_000).unwrap();
    moved.ttl_ms += 1;
    assert_eq!(r.process(&moved, T0 + 9), Err(Dropped::BadSeal));
    // an envelope for another mailbox
    let mut other = c.request("hello", &json!({}), T0 + 10, 60_000).unwrap();
    other.mailbox = "somebody-else".into();
    assert_eq!(r.process(&other, T0 + 10), Err(Dropped::NotForMe));
    // a replayed `open` does not start another session
    let open = c.open(T0 + 20, 60_000).unwrap();
    assert!(r.process(&open, T0 + 20).unwrap().is_some());
    assert_eq!(r.process(&open, T0 + 21), Err(Dropped::Replay));
    // a client that pinned another machine's key never accepts this machine's answer
    let mut wrong = RelayClient::new(SEED, &hex(&[9u8; 32]), T0 + 500).unwrap();
    let open = wrong.open(T0 + 30, 60_000).unwrap();
    assert_eq!(r.process(&open, T0 + 30), Err(Dropped::NotForMe), "a different pinned key is a different mailbox");
    let mut c2 = RelayClient::new(SEED, &r.machine.public_hex(), T0 + 900).unwrap();
    let open = c2.open(T0 + 40, 60_000).unwrap();
    let accept = r.process(&open, T0 + 40).unwrap().unwrap();
    let mut impostor = RelayClient::new(SEED, &hex(&[3u8; 32]), 5).unwrap();
    impostor.open(T0, 1000).unwrap();
    assert!(impostor.accept(&accept).is_err());
}

#[test]
fn a_computer_that_is_not_paired_or_has_a_bad_signature_gets_nothing() {
    let r = rig("who", true, true);
    r.dm.approve_relay_computer(&r.admin, "root", "laptop", true).unwrap();
    let mut stranger = RelayClient::new([9; 32], &r.machine.public_hex(), T0).unwrap();
    let open = stranger.open(T0, 60_000).unwrap();
    assert_eq!(r.process(&open, T0), Err(Dropped::UnknownComputer));
    // the paired computer's name with another key's signature
    let mut c = r.client();
    let mut open = c.open(T0, 60_000).unwrap();
    open.seq += 1; // the signature covers the sequence number
    assert_eq!(r.process(&open, T0), Err(Dropped::BadSignature));
    assert!(r.asked.borrow().is_empty());
}

#[test]
fn requests_expire_after_their_ttl_at_the_relay_and_at_the_machine_and_the_client_is_told() {
    let r = rig("ttl", true, true);
    r.dm.approve_relay_computer(&r.admin, "root", "laptop", true).unwrap();
    let mut relay = FakeRelay::new();
    let mut c = r.client();
    r.connect(&mut relay, &mut c, T0).unwrap();
    // the machine is offline: the request waits
    let req = c.request("hello", &json!({"contract": {"major": 0, "minor": 1}, "wants": []}), T0 + 100, 60 * 60 * 1000).unwrap();
    relay.post(&req.encode().unwrap(), T0 + 100).unwrap();
    assert_eq!(relay.waiting(&r.machine.mailbox()), 1);
    // an hour and a second later it is gone: not delivered, and counted for the client's next look
    assert!(relay.fetch(&r.machine.mailbox(), T0 + 100 + 60 * 60 * 1000 + 1000).is_empty());
    assert_eq!(relay.take_expired(&r.machine.mailbox()), 1);
    assert_eq!(relay.take_expired(&r.machine.mailbox()), 0);
    assert_eq!(relay.waiting(&r.machine.mailbox()), 0, "nothing is kept after expiry");
    // a machine that comes back does not run a request older than its ttl, even if it was handed one
    assert_eq!(r.process(&req, T0 + 100 + 60 * 60 * 1000 + 1), Err(Dropped::Expired));
    // within 24 hours it still runs
    let req = c.request("hello", &json!({"contract": {"major": 0, "minor": 1}, "wants": []}), T0 + 200, MAX_TTL_MS).unwrap();
    relay.post(&req.encode().unwrap(), T0 + 200).unwrap();
    let got = relay.fetch(&r.machine.mailbox(), T0 + 200 + 23 * 60 * 60 * 1000);
    assert_eq!(got.len(), 1);
    assert!(r.process(&got[0], T0 + 200 + 23 * 60 * 60 * 1000).unwrap().is_some());
    // delivered once
    assert!(relay.fetch(&r.machine.mailbox(), T0 + 300).is_empty());
}

#[test]
fn envelope_limits_size_ttl_version_clock_and_the_mailbox_rate() {
    let e = Envelope { v: RELAY_PROTOCOL, mailbox: "m".into(), from: "f".into(), seq: 1, sent_ms: T0, ttl_ms: 1000, kind: "msg".into(), body: "00".into() };
    let bytes = e.encode().unwrap();
    assert_eq!(Envelope::decode(&bytes).unwrap(), e);
    let big = Envelope { body: "ab".repeat(MAX_ENVELOPE_BYTES), ..e.clone() };
    assert_eq!(big.encode(), Err(Dropped::TooBig));
    assert_eq!(Envelope::decode(&vec![b' '; MAX_ENVELOPE_BYTES + 1]), Err(Dropped::TooBig));
    assert_eq!(Envelope { ttl_ms: MAX_TTL_MS + 1, ..e.clone() }.encode().and_then(|b| Envelope::decode(&b)), Err(Dropped::BadTtl));
    assert_eq!(Envelope { ttl_ms: 0, ..e.clone() }.encode().and_then(|b| Envelope::decode(&b)), Err(Dropped::BadTtl));
    assert_eq!(Envelope { v: 2, ..e.clone() }.encode().and_then(|b| Envelope::decode(&b)), Err(Dropped::BadVersion));
    assert_eq!(Envelope { kind: "x".into(), ..e.clone() }.encode().and_then(|b| Envelope::decode(&b)), Err(Dropped::Malformed));
    assert_eq!(Envelope::decode(b"{not json"), Err(Dropped::Malformed));
    assert_eq!(e.check_time(T0 + 1000), Err(Dropped::Expired));
    assert_eq!(Envelope { sent_ms: T0 + MAX_SKEW_MS + 10, ..e.clone() }.check_time(T0), Err(Dropped::FromTheFuture));
    // the relay refuses past the per-minute rate and recovers a minute later
    let mut relay = FakeRelay::new();
    for i in 0..MAILBOX_PER_MINUTE {
        let x = Envelope { seq: i as u64, ttl_ms: 10 * 60_000, ..e.clone() };
        relay.post(&x.encode().unwrap(), T0 + 1).unwrap();
    }
    let x = Envelope { ttl_ms: 10 * 60_000, ..e.clone() };
    assert!(relay.post(&x.encode().unwrap(), T0 + 2).is_err());
    relay.fetch("m", T0 + 3);
    assert!(relay.post(&x.encode().unwrap(), T0 + 61_000).is_ok());
}

#[test]
fn the_relay_key_is_kept_private_on_the_machine_and_never_logged() {
    use std::os::unix::fs::PermissionsExt;
    let r = rig("key", true, true);
    let p = r.dir.join("devmode/relay.key");
    assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    // the same machine key after a restart: a pinned client keeps working
    let again = RelayMachine::open(&r.dm).unwrap();
    assert_eq!(again.public_hex(), r.machine.public_hex());
    let secret = std::fs::read_to_string(&p).unwrap();
    for a in r.dm.activity(200) {
        assert!(!a.detail.contains(secret.trim()) && !a.action.contains(secret.trim()));
    }
    assert_eq!(r.machine.mailbox().len(), 32);
}

// ---------------------------------------------------------------------------------------------------------- over a real network (F-CORE-33)

/// A relay as a person could host it: a loopback HTTP server over the same `FakeRelay` rules, speaking the wire of `relay_link.rs`.
fn serve_relay(stop: Arc<std::sync::atomic::AtomicBool>) -> (String, Arc<std::sync::Mutex<FakeRelay>>) {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let relay = Arc::new(std::sync::Mutex::new(FakeRelay::new()));
    let r2 = relay.clone();
    std::thread::spawn(move || {
        while !stop.load(std::sync::atomic::Ordering::SeqCst) {
            let Ok((mut s, _)) = l.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            };
            let _ = s.set_nonblocking(false);
            let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(5)));
            let mut buf = Vec::new();
            let mut tmp = [0u8; 8192];
            let (head_end, len) = loop {
                let n = s.read(&mut tmp).unwrap_or(0);
                if n == 0 {
                    break (0, 0);
                }
                buf.extend_from_slice(&tmp[..n]);
                if let Some(h) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..h]).to_ascii_lowercase();
                    let len = head.lines().find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
                    if buf.len() >= h + 4 + len {
                        break (h + 4, len);
                    }
                }
            };
            if head_end == 0 {
                continue;
            }
            let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
            let line = head.lines().next().unwrap_or("").to_string();
            let body = buf[head_end..head_end + len].to_vec();
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
            let (status, out): (u16, Vec<u8>) = if line.starts_with("POST /v1/relay/post ") {
                match r2.lock().unwrap().post(&body, now) {
                    Ok(()) => (204, vec![]),
                    Err(d) => (422, d.word().as_bytes().to_vec()),
                }
            } else if let Some(m) = line.strip_prefix("GET /v1/relay/fetch/").and_then(|r| r.split(' ').next()) {
                let list: Vec<String> = r2.lock().unwrap().fetch(m, now).iter().filter_map(|e| e.encode().ok()).map(|b| super::relay_link::to_hex(&b)).collect();
                (200, serde_json::to_vec(&list).unwrap())
            } else {
                (404, b"no such route".to_vec())
            };
            let _ = write!(s, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len());
            let _ = s.write_all(&out);
        }
    });
    (base, relay)
}

#[test]
fn a_computer_and_a_machine_talk_through_a_relay_over_real_http_and_the_machine_only_connects_out() {
    use super::relay_link::*;
    let r = rig("net", true, true);
    r.dm.approve_relay_computer(&r.admin, "root", "laptop", true).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (base, relay) = serve_relay(stop.clone());
    let up: Arc<dyn iyakki_proxy::Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(std::time::Duration::from_secs(5)));
    let link = Arc::new(HttpLink::new(up, &base).unwrap());
    assert!(HttpLink::new(Arc::new(NoUp), "relay.example").is_err() && HttpLink::new(Arc::new(NoUp), "https://relay.example/path").is_err());
    let now_ms = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let pub_hex = r.machine.public_hex();
    let l2 = link.clone();
    let client = std::thread::spawn(move || {
        let mut c = RelayClient::new(SEED, &pub_hex, now_ms()).unwrap();
        client_open(l2.as_ref(), &mut c, now_ms(), 60_000, std::time::Duration::from_secs(10))?;
        let hello = client_ask(l2.as_ref(), &mut c, "hello", &json!({"contract": {"major": 0, "minor": 1}, "wants": ["settings"]}), now_ms(), 60_000, std::time::Duration::from_secs(10))?;
        let call = client_ask(l2.as_ref(), &mut c, "call", &json!({"domain": "settings", "call": "schema", "args": {}}), now_ms(), 60_000, std::time::Duration::from_secs(10))?;
        Ok::<_, String>((hello, call))
    });
    // the machine's loop: it fetches its own mailbox and posts the replies; it never listens
    let mut passes = 0;
    while !client.is_finished() && passes < 400 {
        let audit = |_: &str, _: &str| {};
        let ask = |who: &str| r.asked.borrow_mut().push(who.to_string());
        let cx = RelayCx { dm: &r.dm, policy: &r.admin, core: &r.core, gate: &r.gate, hostname: "iyakki-test", audit: &audit, ask: &ask };
        pump_once(link.as_ref(), &r.machine, &cx, now_ms()).unwrap();
        passes += 1;
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    let ((st, h), (_, call)) = client.join().unwrap().unwrap();
    assert_eq!((st, h["ok"].as_bool()), (200, Some(true)), "{h}");
    assert_eq!(call["ok"], true, "{call}");
    // the relay held sealed bytes only
    for b in &relay.lock().unwrap().seen {
        let s = String::from_utf8_lossy(b);
        for word in ["schema", "settings", "hello"] {
            assert!(!s.contains(word), "the relay saw {word}");
        }
    }
    // a relay that cannot be reached is an error the loop waits out; asking it in time says so
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    std::thread::sleep(std::time::Duration::from_millis(100));
    let dead = HttpLink::new(Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(std::time::Duration::from_secs(1))), &base).unwrap();
    assert!(dead.fetch("abc").is_err());
    // the loop stops when told and backs off after a failure
    let flag = std::sync::atomic::AtomicBool::new(false);
    let mut calls = 0;
    run_loop(&flag, std::time::Duration::from_millis(10), &mut || {
        calls += 1;
        if calls >= 3 {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Err("down".into())
    });
    assert_eq!(calls, 3);
}

struct NoUp;
impl iyakki_proxy::Upstream for NoUp {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("no network in this test".into())
    }
}

/// A TLS relay on loopback (a certificate of its own, made for the test): fetch answers an empty list, post answers 204.
fn serve_tls_relay() -> (String, String) {
    use std::io::{Read, Write};
    let mut p = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    p.distinguished_name.push(rcgen::DnType::CommonName, "relay.test");
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = p.self_signed(&key).unwrap();
    let der = cert.der().to_vec();
    let fp = iyakki_proxy::certinfo::fingerprint(&der);
    let cfg = Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![rustls::pki_types::CertificateDer::from(der)], rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der()).unwrap())
            .unwrap(),
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            let mut s = s;
            let mut c = rustls::ServerConnection::new(cfg.clone()).unwrap();
            let mut t = rustls::Stream::new(&mut c, &mut s);
            let mut seen = Vec::new();
            let mut b = [0u8; 1];
            let mut need = None;
            loop {
                match t.read(&mut b) {
                    Ok(1) => seen.push(b[0]),
                    _ => break,
                }
                if need.is_none() && seen.ends_with(b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&seen).to_ascii_lowercase();
                    let n: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                    need = Some(seen.len() + n);
                }
                if need.is_some_and(|n| seen.len() >= n) {
                    break;
                }
            }
            let post = seen.starts_with(b"POST");
            let resp: &[u8] = if post { b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n" } else { b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]" };
            let _ = t.write_all(resp);
            let _ = t.flush();
            c.send_close_notify();
            let _ = c.complete_io(&mut s);
        }
    });
    (format!("https://localhost:{port}"), fp)
}

#[test]
fn an_https_relay_is_only_used_with_its_certificate_fingerprint_and_only_for_that_certificate() {
    use super::relay_link::*;
    let r = rig("pin", true, true);
    // nothing is connected to while the fingerprint is unset: the link is refused before any connection
    let plain: Arc<dyn iyakki_proxy::Upstream> = Arc::new(NoUp);
    assert!(link_for("https://relay.example", "", plain.clone()).err().unwrap().contains("fingerprint is not set"));
    assert!(link_for("https://relay.example", "  ", plain.clone()).is_err());
    assert!(link_for("https://relay.example", "AB:CD", plain.clone()).is_err(), "a short fingerprint is no pin");
    // the summary says why the machine connects nowhere, and the settings check the fingerprint
    r.dm.set_relay_url(&r.admin, "root", "https://relay.example").unwrap();
    assert!(r.dm.relay_summary().contains("no certificate fingerprint is set"), "{}", r.dm.relay_summary());
    assert!(r.dm.set_relay_pin(&r.admin, "root", "not a fingerprint").is_err() && r.dm.set_relay_pin(&r.admin, "root", "AB:CD").is_err());
    assert!(r.dm.set_relay_pin(&Role::User.template(), "amy", &"ab".repeat(32)).is_err(), "administrators only");
    assert_eq!(r.dm.relay_pin(), "");
    // over real TLS to a relay with a certificate of its own: the right fingerprint works for post and fetch
    let (base, fp) = serve_tls_relay();
    r.dm.set_relay_url(&r.admin, "root", &base).unwrap();
    r.dm.set_relay_pin(&r.admin, "root", &fp).unwrap();
    assert_eq!(r.dm.relay_pin().len(), 64);
    assert!(r.dm.relay_summary().contains("certificate "), "{}", r.dm.relay_summary());
    let link = link_for(&base, &r.dm.relay_pin(), plain.clone()).unwrap();
    link.post(b"sealed").unwrap();
    assert!(link.fetch("abc").unwrap().is_empty());
    // another certificate (even a valid-looking one) is refused: the pin is the only trust
    let (base2, _) = serve_tls_relay();
    let wrong = link_for(&base2, &r.dm.relay_pin(), plain.clone()).unwrap();
    assert!(wrong.fetch("abc").unwrap_err().contains("changed"), "a different certificate is refused");
    assert!(wrong.post(b"sealed").is_err());
    // a plain http address keeps using the plain transport, which is the caller's choice
    assert!(link_for("http://127.0.0.1:1", "", plain).is_ok());
    // removing the pin makes the machine stop using the https relay again
    r.dm.set_relay_pin(&r.admin, "root", "").unwrap();
    assert!(r.dm.relay_summary().contains("no certificate fingerprint is set"));
    let _ = std::fs::remove_dir_all(&r.dir);
}

// ------------------------------------------------------------------------------------------ the real relay server (iyakki-relay)

/// The real `iyakki-relay` server on loopback over TLS (a certificate made for the test, a manual clock): returns its address, the
/// certificate's fingerprint, the clock and the running handle. The listener is bound here and handed in, so the port is never freed.
fn real_relay(max_wait_ms: u64) -> (String, String, iyakki_relay::ManualClock, iyakki_relay::server::Running) {
    let mut p = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    p.distinguished_name.push(rcgen::DnType::CommonName, "relay.test");
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = p.self_signed(&key).unwrap();
    let fp = iyakki_proxy::certinfo::fingerprint(cert.der());
    let cfg = iyakki_relay::tls::config_from_pem(&cert.pem(), &key.serialize_pem()).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let clock = iyakki_relay::ManualClock::at(T0);
    let limits = iyakki_relay::store::Limits { max_wait_ms, ..Default::default() };
    let relay = Arc::new(iyakki_relay::http::Relay::new(iyakki_relay::store::Store::new(limits), Box::new(clock.clone()), Box::new(|_| {})));
    let running = iyakki_relay::server::start(listener, cfg, relay, 16).unwrap();
    (format!("https://localhost:{port}"), fp, clock, running)
}

#[test]
fn the_relay_server_constants_are_the_clients_constants() {
    use iyakki_relay::store as s;
    assert_eq!((s::RELAY_PROTOCOL, s::MAX_ENVELOPE_BYTES, s::MAX_TTL_MS, s::MAX_SKEW_MS), (RELAY_PROTOCOL, MAX_ENVELOPE_BYTES, MAX_TTL_MS, MAX_SKEW_MS));
    assert_eq!((s::MAILBOX_PER_MINUTE, s::MAILBOX_DEPTH), (MAILBOX_PER_MINUTE, MAILBOX_DEPTH));
    // everything the client can encode is accepted by the server's header check
    let mut st = s::Store::default();
    for kind in ["open", "accept", "msg", "refused"] {
        let e = Envelope { v: RELAY_PROTOCOL, mailbox: "c-0123456789abcdef".into(), from: "f".into(), seq: 1, sent_ms: T0, ttl_ms: 1000, kind: kind.into(), body: "00ff".into() };
        st.post("p", &e.encode().unwrap(), T0).unwrap();
    }
    assert_eq!(st.waiting("c-0123456789abcdef"), 4);
}

#[test]
fn a_computer_and_a_machine_use_the_real_relay_server_over_pinned_tls() {
    use super::relay_link::*;
    let r = rig("realsrv", true, true);
    let (base, fp, clock, running) = real_relay(MAX_TTL_MS);
    let plain: Arc<dyn iyakki_proxy::Upstream> = Arc::new(NoUp);
    let link = link_for(&base, &fp, plain.clone()).unwrap();
    let pump = |now: u64| {
        let audit = |_: &str, _: &str| {};
        let ask = |who: &str| r.asked.borrow_mut().push(who.to_string());
        let cx = RelayCx { dm: &r.dm, policy: &r.admin, core: &r.core, gate: &r.gate, hostname: "iyakki-test", audit: &audit, ask: &ask };
        pump_once(&link, &r.machine, &cx, now).unwrap()
    };
    let reply_of = |c: &RelayClient| -> Vec<Envelope> { link.fetch(&c.reply_mailbox()).unwrap().iter().map(|b| Envelope::decode(b).unwrap()).collect() };
    let open = |c: &mut RelayClient| {
        link.post(&c.open(clock.now_ms_for_test(), 60_000).unwrap().encode().unwrap()).unwrap();
    };

    // first contact: the machine raises its question and nothing runs; the client gets no answer
    let mut c = r.client();
    open(&mut c);
    let p = pump(clock.now_ms_for_test());
    assert_eq!((p.taken, p.replied, p.dropped), (1, 0, 1));
    assert_eq!(*r.asked.borrow(), ["laptop"]);
    assert!(reply_of(&c).is_empty());
    // the administrator approves on the machine; a fresh open is answered
    r.dm.approve_relay_computer(&r.admin, "root", "laptop", true).unwrap();
    let mut c = RelayClient::new(SEED, &r.machine.public_hex(), T0 + 100).unwrap();
    clock.advance(1000);
    open(&mut c);
    assert_eq!(pump(clock.now_ms_for_test()).replied, 1);
    let got = reply_of(&c);
    c.accept(&got[0]).unwrap();
    // a round trip
    let ask = |c: &mut RelayClient, route: &str, body: Value, ttl: u64| {
        let req = c.request(route, &body, clock.now_ms_for_test(), ttl).unwrap();
        link.post(&req.encode().unwrap()).unwrap();
        req
    };
    ask(&mut c, "call", json!({"domain": "settings", "call": "schema", "args": {}}), 60_000);
    assert_eq!(pump(clock.now_ms_for_test()).replied, 1);
    let (st, v) = c.answer(&reply_of(&c)[0], clock.now_ms_for_test()).unwrap();
    assert_eq!((st, v["ok"].as_bool()), (200, Some(true)), "{v}");
    // replay: the relay carries the same envelope twice, the machine answers once
    let req = ask(&mut c, "hello", json!({"contract": {"major": 0, "minor": 1}, "wants": []}), 60_000);
    link.post(&req.encode().unwrap()).unwrap();
    let p = pump(clock.now_ms_for_test());
    assert_eq!((p.taken, p.replied, p.dropped), (2, 1, 1), "the second copy is a replay");
    reply_of(&c);
    // tampered: one flipped character in the sealed body is dropped
    let mut forged = c.request("hello", &json!({"contract": {"major": 0, "minor": 1}, "wants": []}), clock.now_ms_for_test(), 60_000).unwrap();
    let mut b = forged.body.clone().into_bytes();
    let last = b.len() - 1;
    b[last] = if b[last] == b'0' { b'1' } else { b'0' };
    forged.body = String::from_utf8(b).unwrap();
    link.post(&forged.encode().unwrap()).unwrap();
    let p = pump(clock.now_ms_for_test());
    assert_eq!((p.taken, p.replied, p.dropped), (1, 0, 1));
    // expiry: a request queued while the machine is away is gone after its ttl, and the machine never sees it
    ask(&mut c, "hello", json!({"contract": {"major": 0, "minor": 1}, "wants": []}), 30_000);
    clock.advance(31_000);
    let p = pump(clock.now_ms_for_test());
    assert_eq!(p.taken, 0, "expired at the relay, not delivered");
    // a mailbox that was never written to is simply empty
    assert!(link.fetch("nobody-here").unwrap().is_empty());
    // the wrong certificate: another server, the first one's fingerprint
    let (base2, fp2, _clock2, running2) = real_relay(MAX_TTL_MS);
    let wrong = link_for(&base2, &fp, plain.clone()).unwrap();
    assert!(wrong.fetch("abc").unwrap_err().contains("changed"));
    assert!(wrong.post(b"x").is_err());
    assert!(link_for(&base2, &fp2, plain).unwrap().fetch("abc").unwrap().is_empty());
    running2.stop();
    running.stop();
}

trait NowForTest {
    fn now_ms_for_test(&self) -> u64;
}
impl NowForTest for iyakki_relay::ManualClock {
    fn now_ms_for_test(&self) -> u64 {
        use iyakki_relay::Clock;
        self.now_ms()
    }
}

/// A run against an `iyakki-relay` on another computer (the owner's trial setup; see docs/RELAY.md). Ignored: it needs
/// `IYAKKI_RELAY_TEST_URL` (https://localhost:PORT or any name in the certificate), `IYAKKI_RELAY_TEST_PIN` (the certificate's SHA-256
/// fingerprint) and `IYAKKI_RELAY_TEST_SHORT_URL` (a second relay started with a `--max-wait-ms` under five seconds, same certificate).
/// Both roles run here with the real clock; only the relay is remote.
#[test]
#[ignore = "needs a running iyakki-relay; see docs/RELAY.md"]
fn live_a_computer_and_a_machine_use_a_relay_on_another_computer_over_pinned_tls() {
    use super::relay_link::*;
    let (Ok(url), Ok(pin), Ok(short_url)) = (std::env::var("IYAKKI_RELAY_TEST_URL"), std::env::var("IYAKKI_RELAY_TEST_PIN"), std::env::var("IYAKKI_RELAY_TEST_SHORT_URL")) else {
        panic!("set IYAKKI_RELAY_TEST_URL, IYAKKI_RELAY_TEST_PIN and IYAKKI_RELAY_TEST_SHORT_URL");
    };
    let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let r = rig("live", true, true);
    let plain: Arc<dyn iyakki_proxy::Upstream> = Arc::new(NoUp);
    let link = link_for(&url, &pin, plain.clone()).unwrap();
    let short = link_for(&short_url, &pin, plain.clone()).unwrap();
    let pump = |l: &HttpLink| {
        let audit = |_: &str, _: &str| {};
        let ask = |who: &str| r.asked.borrow_mut().push(who.to_string());
        let cx = RelayCx { dm: &r.dm, policy: &r.admin, core: &r.core, gate: &r.gate, hostname: "iyakki-test", audit: &audit, ask: &ask };
        pump_once(l, &r.machine, &cx, now()).unwrap()
    };
    let replies = |l: &HttpLink, c: &RelayClient| -> Vec<Envelope> { l.fetch(&c.reply_mailbox()).unwrap().iter().map(|b| Envelope::decode(b).unwrap()).collect() };
    // 1. first contact needs approval on the machine
    let mut c = RelayClient::new(SEED, &r.machine.public_hex(), now()).unwrap();
    link.post(&c.open(now(), 60_000).unwrap().encode().unwrap()).unwrap();
    let p = pump(&link);
    assert_eq!((p.taken, p.replied, p.dropped), (1, 0, 1), "first contact is held for approval");
    assert_eq!(*r.asked.borrow(), ["laptop"]);
    assert!(replies(&link, &c).is_empty());
    crate::qprintln!("live: first contact held for approval on the machine: ok");
    r.dm.approve_relay_computer(&r.admin, "root", "laptop", true).unwrap();
    // 2. session and round trip
    let mut c = RelayClient::new(SEED, &r.machine.public_hex(), now()).unwrap();
    link.post(&c.open(now(), 60_000).unwrap().encode().unwrap()).unwrap();
    assert_eq!(pump(&link).replied, 1);
    c.accept(&replies(&link, &c)[0]).unwrap();
    let req = c.request("call", &json!({"domain": "settings", "call": "schema", "args": {}}), now(), 60_000).unwrap();
    link.post(&req.encode().unwrap()).unwrap();
    assert_eq!(pump(&link).replied, 1);
    let (st, v) = c.answer(&replies(&link, &c)[0], now()).unwrap();
    assert_eq!((st, v["ok"].as_bool()), (200, Some(true)), "{v}");
    crate::qprintln!("live: round trip through the relay: ok");
    // 3. replay and tampering
    let req = c.request("hello", &json!({"contract": {"major": 0, "minor": 1}, "wants": []}), now(), 60_000).unwrap();
    link.post(&req.encode().unwrap()).unwrap();
    link.post(&req.encode().unwrap()).unwrap();
    let p = pump(&link);
    assert_eq!((p.taken, p.replied, p.dropped), (2, 1, 1), "the copy is a replay");
    replies(&link, &c);
    let mut forged = c.request("hello", &json!({"contract": {"major": 0, "minor": 1}, "wants": []}), now(), 60_000).unwrap();
    let mut b = forged.body.clone().into_bytes();
    let last = b.len() - 1;
    b[last] = if b[last] == b'0' { b'1' } else { b'0' };
    forged.body = String::from_utf8(b).unwrap();
    link.post(&forged.encode().unwrap()).unwrap();
    let p = pump(&link);
    assert_eq!((p.taken, p.replied, p.dropped), (1, 0, 1), "a tampered envelope is dropped");
    crate::qprintln!("live: replay and tampered envelope dropped: ok");
    // 4. expiry at the short-lived relay: delivered in time, gone after its wait
    let req = c.request("hello", &json!({"contract": {"major": 0, "minor": 1}, "wants": []}), now(), 60_000).unwrap();
    short.post(&req.encode().unwrap()).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(7));
    let p = pump(&short);
    assert_eq!(p.taken, 0, "the queued request expired at the relay");
    let req = c.request("hello", &json!({"contract": {"major": 0, "minor": 1}, "wants": []}), now(), 60_000).unwrap();
    short.post(&req.encode().unwrap()).unwrap();
    assert_eq!(pump(&short).replied, 1, "a fresh one is delivered");
    crate::qprintln!("live: queued request expired after the relay's wait, a fresh one was delivered: ok");
    // 5. a wrong certificate is refused, for post and fetch
    let wrong = link_for(&url, &"ab".repeat(32), plain).unwrap();
    assert!(wrong.fetch("abc").unwrap_err().contains("changed"));
    assert!(wrong.post(b"x").is_err());
    crate::qprintln!("live: wrong certificate refused: ok");
}
