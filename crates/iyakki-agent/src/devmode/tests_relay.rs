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
