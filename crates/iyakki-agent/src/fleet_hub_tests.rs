//! The hub over real loopback sockets: two or three systems in one process, each with its own folder, key and listener (F-FLEET-01, 02, 07).
//! What is real: the TCP exchange, the signatures, the sealing, the files. What is not: other machines, a LAN, a clock that differs.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use super::*;
use crate::fleet::{FixedSource, Heartbeat, HeartbeatSource};

fn hub(tag: &str) -> Arc<Hub> {
    Arc::new(Hub::load(&crate::testdir::outside_project("fleet", tag)).unwrap())
}

fn hb(n: u64) -> Heartbeat {
    Heartbeat { disk_free_mb: 1000 * n, disk_total_mb: 9000, cpu_load_pct: 10 * n as u32, cpus: 4, mem_used_pct: 20 + n as u32, mem_total_mb: 8000, tasks_running: n as u32, models: vec![format!("model-{n}")], battery_pct: None }
}

fn src(n: u64) -> Arc<dyn HeartbeatSource> {
    Arc::new(FixedSource(hb(n)))
}

fn up(h: &Arc<Hub>, n: u64) -> String {
    h.start_node("127.0.0.1:0", src(n)).unwrap().to_string()
}

/// `a` creates a fleet and `b` joins it through the real exchange. Returns the fleet id and a's address.
fn pair(a: &Arc<Hub>, b: &Arc<Hub>, a_addr: &str, name: &str) -> String {
    let fid = if a.status().fleets.is_empty() { a.create_fleet("lab", "alpha").unwrap() } else { a.status().fleets[0].id.clone() };
    let (code, _) = a.offer_code(&fid).unwrap();
    let t = b.join_start(a_addr, &code, name).unwrap();
    b.poll_pending();
    assert_eq!(b.status().pending[0].state, "waiting");
    let req = a.status().requests;
    assert_eq!(req.len(), 1);
    assert_eq!(req[0].fingerprint, fingerprint(&b.my_id()));
    a.approve_join(&req[0].id).unwrap();
    b.poll_pending();
    assert_eq!(b.status().pending[0].state, "offered");
    b.join_accept(&t).unwrap();
    a.settle();
    fid
}

#[test]
fn two_systems_pair_with_a_one_time_code_each_side_seeing_the_other_and_both_hold_the_same_signed_record() {
    let (a, b) = (hub("pa"), hub("pb"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let fid = pair(&a, &b, &a_addr, "bravo");
    let (sa, sb) = (a.status(), b.status());
    assert_eq!(sa.fleets[0].members.len(), 2);
    assert_eq!(sb.fleets[0].members.len(), 2);
    assert_eq!((sa.fleets[0].version, sb.fleets[0].version), (2, 2));
    assert!(sb.fleets[0].id == fid && !sb.fleets[0].i_am_admin && sa.fleets[0].i_am_admin);
    let (ra, rb) = (lock(&a.inner).fleets[&fid].rec.clone(), lock(&b.inner).fleets[&fid].rec.clone());
    assert_eq!(ra, rb, "the very same signed record");
    rb.verify_self().unwrap();
    // both can open the group key
    let (ka, kb) = (lock(&a.inner).keys.clone(), lock(&b.inner).keys.clone());
    let sealed = ra.open_key(&ka).unwrap().seal(&fid, "x", 0, b"hi").unwrap();
    assert_eq!(rb.open_key(&kb).unwrap().open(&fid, "x", 0, &sealed).unwrap(), b"hi");
    assert!(sa.offers.is_empty(), "the offer was used up by the join");
}

#[test]
fn a_code_works_once_and_only_for_ten_minutes_and_wrong_codes_lock_joining() {
    let (a, b) = (hub("ca"), hub("cb"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let fid = a.create_fleet("lab", "alpha").unwrap();
    let (code, life) = a.offer_code(&fid).unwrap();
    assert_eq!(life, 600);
    // the code is not in the status of the fleet or of anything the chat reads except the offer list the screen shows
    let t = b.join_start(&a_addr, &code, "bravo").unwrap();
    assert!(!t.is_empty());
    let again = b.join_start(&a_addr, &code, "bravo").unwrap_err();
    assert!(again.contains("not valid"), "{again}");
    // a new offer, then it expires
    let now = Arc::new(AtomicU64::new(now_secs()));
    let n2 = now.clone();
    a.set_clock(Arc::new(move || n2.load(Ordering::SeqCst)));
    let (code2, _) = a.offer_code(&fid).unwrap();
    now.fetch_add(601, Ordering::SeqCst);
    // the joiner's clock is the real one, within the five minute skew? no: 601 s is outside, so the message itself is refused
    let e = b.join_start(&a_addr, &code2, "charlie").unwrap_err();
    assert!(e.contains("too old") || e.contains("not valid"), "{e}");
    now.store(now_secs(), Ordering::SeqCst);
    let (_c3, _) = a.offer_code(&fid).unwrap();
    // one wrong code was counted above; four more make five
    for n in 0..4 {
        let e = b.join_start(&a_addr, &format!("00000000{n}"), "x").unwrap_err();
        assert!(e.contains("not valid"), "{e}");
    }
    let e = b.join_start(&a_addr, "123456789", "x").unwrap_err();
    assert!(e.contains("locked"), "after five wrong codes: {e}");
    assert!(a.status().offers.is_empty(), "the open offers were dropped by the lock");
}

#[test]
fn a_join_request_nobody_approves_gets_nothing_and_a_refused_one_is_told() {
    let (a, b) = (hub("ra"), hub("rb"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let fid = a.create_fleet("lab", "alpha").unwrap();
    let (code, _) = a.offer_code(&fid).unwrap();
    b.join_start(&a_addr, &code, "bravo").unwrap();
    b.poll_pending();
    assert_eq!(b.status().pending[0].state, "waiting");
    assert!(b.status().fleets.is_empty(), "nothing is joined before an administrator approves and the person accepts");
    let r = a.status().requests[0].id.clone();
    a.decline_join(&r).unwrap();
    b.poll_pending();
    assert_eq!(b.status().pending[0].state, "declined");
    assert!(b.join_accept(&b.status().pending[0].ticket).is_err());
    assert_eq!(a.status().fleets[0].members.len(), 1);
}

#[test]
fn an_offered_record_is_not_kept_until_the_person_accepts_and_can_be_dropped() {
    let (a, b) = (hub("oa"), hub("ob"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let fid = a.create_fleet("lab", "alpha").unwrap();
    let (code, _) = a.offer_code(&fid).unwrap();
    let t = b.join_start(&a_addr, &code, "bravo").unwrap();
    a.approve_join(&a.status().requests[0].id).unwrap();
    b.poll_pending();
    let card = b.offer_card(&t).unwrap();
    assert!(card.contains("lab") && card.contains(&fingerprint(&a.my_id())) && card.contains("administrator"), "{card}");
    assert!(b.status().fleets.is_empty(), "offered, not joined");
    b.join_decline(&t).unwrap();
    assert!(b.join_accept(&t).is_err() && b.status().fleets.is_empty());
}

#[test]
fn heartbeats_carry_the_numbers_to_every_member_and_a_third_member_learns_the_address_of_the_others() {
    let (a, b, c) = (hub("ha"), hub("hb"), hub("hc"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    up(&c, 3);
    pair(&a, &b, &a_addr, "bravo");
    pair(&a, &c, &a_addr, "charlie");
    // b learned c only from the record; the addresses come with the heartbeats
    a.heartbeat_tick(&FixedSource(hb(1)));
    b.heartbeat_tick(&FixedSource(hb(2)));
    c.heartbeat_tick(&FixedSource(hb(3)));
    a.heartbeat_tick(&FixedSource(hb(1))); // now a knows both addresses and tells each the other's
    b.heartbeat_tick(&FixedSource(hb(2)));
    c.heartbeat_tick(&FixedSource(hb(3)));
    let sb = b.status();
    let ms = &sb.fleets[0].members;
    assert_eq!(ms.len(), 3);
    for m in ms {
        assert!(m.heartbeat.is_some(), "{} has numbers on b", m.name);
        assert!(m.state == "up" || m.state == "this computer", "{} {}", m.name, m.state);
        assert!(m.me || m.address_known);
    }
    let on_b_about_c = ms.iter().find(|m| m.name == "charlie").unwrap().heartbeat.clone().unwrap();
    assert_eq!((on_b_about_c.cpu_load_pct, on_b_about_c.tasks_running, on_b_about_c.models.clone()), (30, 3, vec!["model-3".to_string()]));
    // a heartbeat is only numbers: nothing of the message is a path or content
    let all = serde_json::to_string(&sb).unwrap();
    assert!(!all.contains("/home"));
}

#[test]
fn the_node_sends_heartbeats_by_itself_and_a_silent_member_is_shown_as_lost() {
    let (a, b) = (hub("na"), hub("nb"));
    a.set_interval(Duration::from_millis(60));
    b.set_interval(Duration::from_millis(60));
    let a_addr = up(&a, 1);
    up(&b, 2);
    pair(&a, &b, &a_addr, "bravo");
    let t0 = std::time::Instant::now();
    loop {
        let s = a.status();
        let m = s.fleets[0].members.iter().find(|m| m.name == "bravo").unwrap();
        if m.heartbeat.is_some() {
            assert_eq!(m.heartbeat.as_ref().unwrap().tasks_running, 2);
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "no heartbeat arrived");
        std::thread::sleep(Duration::from_millis(30));
    }
    // b stops: after three intervals (with a fake clock far ahead) it is lost
    b.stop_node();
    let now = Arc::new(AtomicU64::new(now_secs() + 1));
    let n2 = now.clone();
    a.set_clock(Arc::new(move || n2.load(Ordering::SeqCst)));
    a.set_interval(Duration::from_secs(10));
    now.fetch_add(100, Ordering::SeqCst);
    let s = a.status();
    assert_eq!(s.fleets[0].members.iter().find(|m| m.name == "bravo").unwrap().state, "lost");
    a.stop_node();
}

#[test]
fn removing_a_member_changes_the_key_tells_it_and_its_next_message_is_refused() {
    let (a, b, c) = (hub("ea"), hub("eb"), hub("ec"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    up(&c, 3);
    let fid = pair(&a, &b, &a_addr, "bravo");
    pair(&a, &c, &a_addr, "charlie");
    a.heartbeat_tick(&FixedSource(hb(1)));
    b.heartbeat_tick(&FixedSource(hb(2)));
    // b is removed by a (a tells everyone, including b)
    let old_gen = a.status().fleets[0].generation;
    a.remove_member(&fid, "bravo").unwrap();
    a.settle();
    let sa = a.status();
    assert_eq!((sa.fleets[0].members.len(), sa.fleets[0].generation), (2, old_gen + 1));
    assert!(b.status().fleets.is_empty(), "b was told and forgot the fleet");
    assert!(!lock(&b.inner).fleets.contains_key(&fid));
    let sc = c.status();
    assert_eq!((sc.fleets[0].members.len(), sc.fleets[0].generation), (2, old_gen + 1), "c took the new record");
    // c and a can still talk with the new key
    c.heartbeat_tick(&FixedSource(hb(3)));
    let m = a.status().fleets[0].members.iter().find(|m| m.name == "charlie").cloned().unwrap();
    assert!(m.heartbeat.is_some() && m.state == "up");
    assert!(sc.fleets[0].members.iter().all(|m| m.name != "bravo"));
}

#[test]
fn a_removed_member_that_never_heard_of_it_is_refused_at_its_next_message_and_sees_that_it_is_refused() {
    let (a, b) = (hub("sa"), hub("sb"));
    let a_addr = up(&a, 1);
    let b_addr = up(&b, 2);
    let fid = pair(&a, &b, &a_addr, "bravo");
    a.heartbeat_tick(&FixedSource(hb(1)));
    b.heartbeat_tick(&FixedSource(hb(2)));
    // b is off the network when the removal happens
    b.stop_node();
    a.remove_member(&fid, "bravo").unwrap();
    a.settle();
    assert!(lock(&b.inner).fleets.contains_key(&fid), "b still holds the old record: it was not told");
    // b sends a heartbeat with its old key and old record: a refuses
    b.heartbeat_tick(&FixedSource(hb(2)));
    let st = b.status();
    assert_eq!(st.fleets[0].members.iter().find(|m| !m.me).unwrap().state, "refused");
    // and a direct message is refused too, whatever it is
    let env = {
        let i = lock(&b.inner);
        b.sealed(&i, &fid, "assign", &json!({})).unwrap()
    };
    let r = exchange(&a_addr, &env).unwrap();
    assert!(!r.ok && r.error.contains("not a member"), "{r:?}");
    let _ = b_addr;
}

#[test]
fn a_member_that_leaves_is_removed_by_the_administrator_without_anyone_pressing_anything() {
    let (a, b) = (hub("la"), hub("lb"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let fid = pair(&a, &b, &a_addr, "bravo");
    a.heartbeat_tick(&FixedSource(hb(1)));
    b.heartbeat_tick(&FixedSource(hb(2)));
    b.leave(&fid).unwrap();
    b.settle();
    assert!(b.status().fleets.is_empty());
    let t0 = std::time::Instant::now();
    while a.status().fleets[0].members.len() != 1 {
        assert!(t0.elapsed() < Duration::from_secs(10), "a did not remove the member that left");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(a.status().fleets[0].generation, 1, "the key changed");
}

#[test]
fn the_only_administrator_cannot_leave_while_others_remain_but_can_after_handing_over_or_alone() {
    let (a, b) = (hub("xa"), hub("xb"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let fid = pair(&a, &b, &a_addr, "bravo");
    assert!(a.leave(&fid).unwrap_err().contains("only administrator"));
    a.set_admin(&fid, "bravo", true).unwrap();
    a.settle();
    assert!(b.status().fleets[0].i_am_admin, "b took the new record that made it an administrator");
    a.leave(&fid).unwrap();
    a.settle();
}

#[test]
fn a_replayed_stale_tampered_or_unsigned_message_is_refused() {
    let (a, b) = (hub("ya"), hub("yb"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let fid = pair(&a, &b, &a_addr, "bravo");
    let env = {
        let i = lock(&b.inner);
        b.sealed(&i, &fid, "heartbeat", &json!(HbBody { hb: hb(2), port: 1, known: BTreeMap::new(), tls: String::new(), relay: String::new() })).unwrap()
    };
    assert!(exchange(&a_addr, &env).unwrap().ok);
    let again = exchange(&a_addr, &env).unwrap();
    assert!(!again.ok && again.error.contains("seen before"), "replay: {again:?}");
    // tampered body
    let mut t = env.clone();
    t.nonce = "ab".repeat(16);
    assert!(!exchange(&a_addr, &t).unwrap().ok, "the nonce is signed, so changing it breaks the signature");
    let mut t = {
        let i = lock(&b.inner);
        b.sealed(&i, &fid, "heartbeat", &json!({})).unwrap()
    };
    t.body.push('0');
    let r = exchange(&a_addr, &t).unwrap();
    assert!(!r.ok && r.error.contains("signature"), "{r:?}");
    // an old message
    let old = {
        let i = lock(&b.inner);
        let mut e = b.sealed(&i, &fid, "heartbeat", &json!({})).unwrap();
        e.ts -= 4000;
        e.sig = i.keys.sign_text(DOMAIN_MSG, e.text().as_bytes());
        e
    };
    let r = exchange(&a_addr, &old).unwrap();
    assert!(!r.ok && r.error.contains("too old"), "{r:?}");
    // signed by somebody else's key
    let forged = {
        let i = lock(&b.inner);
        let mut e = b.sealed(&i, &fid, "heartbeat", &json!({})).unwrap();
        let other = MemberKeys::from_seed([77; 32]);
        e.sig = other.sign_text(DOMAIN_MSG, e.text().as_bytes());
        e
    };
    assert!(exchange(&a_addr, &forged).unwrap().error.contains("signature"));
    // a body sealed with the wrong key does not open
    let wrong = {
        let i = lock(&b.inner);
        let k = ScopeKey::generate().unwrap();
        let body = k.seal(&fid, "heartbeat", 0, b"{}").unwrap();
        b.signed(&i, &fid, "heartbeat", 0, body)
    };
    let r = exchange(&a_addr, &wrong).unwrap();
    assert!(!r.ok && r.error.contains("group key"), "{r:?}");
    // garbage
    let mut s = TcpStream::connect(&a_addr).unwrap();
    s.write_all(&[0, 0, 0, 3, b'x', b'y', b'z']).unwrap();
    let r: Reply = read_frame(&mut s).unwrap();
    assert!(!r.ok);
}

#[test]
fn a_record_push_is_accepted_only_from_an_administrator_and_only_when_newer() {
    let (a, b) = (hub("qa"), hub("qb"));
    let a_addr = up(&a, 1);
    let b_addr = up(&b, 2);
    let fid = pair(&a, &b, &a_addr, "bravo");
    // b is a plain member: a record "signed" by b is refused by a
    let (rec, bk) = {
        let i = lock(&b.inner);
        let f = &i.fleets[&fid];
        (f.rec.clone(), i.keys.clone())
    };
    let ka = rec.open_key(&bk).unwrap();
    let forged = rec.with_member(&bk, &ka, Member::of(&MemberKeys::from_seed([5; 32]), "evil", false));
    assert!(forged.is_err(), "a plain member cannot even make the version");
    // an old record pushed again by the real administrator is ignored
    let old = rec.clone();
    let env = {
        let i = lock(&a.inner);
        a.signed(&i, &fid, "record_push", old.generation, serde_json::to_string(&old).unwrap())
    };
    let r = exchange(&b_addr, &env).unwrap();
    assert!(!r.ok && r.error.contains("not newer"), "{r:?}");
}

#[test]
fn work_is_queued_trusted_or_refused_by_the_receivers_own_setting_and_the_answer_comes_back() {
    let (a, b) = (hub("wa"), hub("wb"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let fid = pair(&a, &b, &a_addr, "bravo");
    a.heartbeat_tick(&FixedSource(hb(1)));
    b.heartbeat_tick(&FixedSource(hb(2)));
    let body = |n: u8, title: &str| AssignBody { id: format!("{n:02x}").repeat(16), title: title.into(), goal: "make the parser faster".into(), base_branch: "main".into(), base_commit: "abc123".into(), rows: vec!["F-1".into()], context_digest: "dead".into(), budget_tokens: 5000, model_class: "small".into(), after: vec![] };
    // default: ask
    assert_eq!(a.assign(&fid, "bravo", &body(1, "one")).unwrap(), "waiting");
    let inbox = b.status().inbox;
    assert_eq!((inbox.len(), inbox[0].state.as_str(), inbox[0].from_name.as_str()), (1, "waiting", "alpha"));
    assert_eq!(a.status().outbox[0].state, "waiting");
    b.inbox_decide(&inbox[0].id, true).unwrap();
    b.settle();
    assert_eq!(a.status().outbox[0].state, "accepted");
    assert!(b.inbox_decide(&inbox[0].id, false).is_err(), "answered once");
    // never
    b.set_accept(Accept::Never);
    let e = a.assign(&fid, "bravo", &body(2, "two")).unwrap_err();
    assert!(e.contains("does not accept"), "{e}");
    assert!(a.status().outbox[1].state.starts_with("failed"));
    // members: trusted ones are accepted at once, only if the receiver says so
    b.set_accept(Accept::Members);
    assert_eq!(a.assign(&fid, "bravo", &body(3, "three")).unwrap(), "waiting");
    b.trust(&fid, "alpha", true).unwrap();
    assert_eq!(a.assign(&fid, "bravo", &body(4, "four")).unwrap(), "accepted");
    assert_eq!(b.status().inbox.iter().filter(|x| x.state == "accepted").count(), 2);
    // the same task twice is refused
    assert!(a.assign(&fid, "bravo", &body(4, "four")).unwrap_err().contains("already here"));
    // the work and the audit survive a restart of the receiver
    let dir = b.dir.clone();
    b.stop_node();
    let b2 = Hub::load(&dir).unwrap();
    assert_eq!(b2.status().inbox.len(), 3);
    assert_eq!(b2.status().accept, Accept::Members);
    assert_eq!(b2.status().fleets[0].members.len(), 2, "the record is loaded again and its key opens");
}

#[test]
fn an_assignment_with_a_secret_an_extra_field_or_too_much_is_refused_on_both_ends() {
    let (a, b) = (hub("ka"), hub("kb"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let fid = pair(&a, &b, &a_addr, "bravo");
    a.heartbeat_tick(&FixedSource(hb(1)));
    b.heartbeat_tick(&FixedSource(hb(2)));
    let ok = AssignBody { id: "11".repeat(16), title: "t".into(), goal: "g".into(), base_branch: String::new(), base_commit: String::new(), rows: vec![], context_digest: String::new(), budget_tokens: 0, model_class: String::new(), after: vec![] };
    assert!(a.assign(&fid, "bravo", &AssignBody { goal: "use api_key sk-abcdefghijklmnopqrstuv to call it".into(), ..ok.clone() }).unwrap_err().contains("secret"));
    assert!(a.assign(&fid, "bravo", &AssignBody { goal: "the password is hunter2".into(), ..ok.clone() }).unwrap_err().contains("secret"));
    assert!(a.assign(&fid, "bravo", &AssignBody { goal: "x".repeat(4001), ..ok.clone() }).is_err());
    assert!(a.assign(&fid, "bravo", &AssignBody { base_commit: "a b".into(), ..ok.clone() }).is_err());
    assert!(a.assign(&fid, "bravo", &AssignBody { budget_tokens: u64::MAX, ..ok.clone() }).is_err());
    assert!(a.assign(&fid, "bravo", &AssignBody { title: "two\nlines".into(), ..ok.clone() }).is_err());
    assert!(b.status().inbox.is_empty(), "nothing reached the other side");
    // a modified sender that skips its own check still meets the receiver's: craft the envelope by hand
    for plain in [json!({"id": "22".repeat(16), "title": "t", "goal": "the token= abc is", "approve": true}), json!({"id": "22".repeat(16), "title": "t", "goal": "g", "capabilities": ["files.write"]}), json!({"id": "22".repeat(16), "title": "t", "goal": "my password is x"})] {
        let env = {
            let i = lock(&a.inner);
            a.sealed(&i, &fid, "assign", &plain).unwrap()
        };
        let r = exchange(&lock(&a.inner).fleets[&fid].peers.values().find_map(|p| p.addr.clone()).unwrap(), &env).unwrap();
        assert!(!r.ok, "{plain}: {r:?}");
    }
    assert!(b.status().inbox.is_empty());
}

#[test]
fn nobody_outside_the_fleet_gets_a_record_or_an_answer_and_a_plain_stranger_is_refused() {
    let (a, s) = (hub("za"), hub("zs"));
    let a_addr = up(&a, 1);
    up(&s, 2);
    let fid = a.create_fleet("lab", "alpha").unwrap();
    // a stranger knows the fleet id (it is not a secret) but has no key in the record
    let (srec, _) = Record::create(&lock(&s.inner).keys, "stranger", "lab").unwrap();
    s.settle();
    let env = {
        let i = lock(&s.inner);
        s.signed(&i, &fid, "heartbeat", 0, "00".into())
    };
    let r = exchange(&a_addr, &env).unwrap();
    assert!(!r.ok && r.error.contains("not a member"));
    // a join poll with a ticket that does not exist, or one that belongs to another key
    let e = s.join_env("join_poll", json!({"ticket": "00".repeat(16), "wrap": lock(&s.inner).keys.public().wrap}));
    let r = exchange(&a_addr, &e).unwrap();
    assert!(!r.ok);
    let _ = srec;
}

#[test]
fn the_member_secret_is_kept_private_on_disk_and_never_shown() {
    let a = hub("pk");
    a.create_fleet("lab", "alpha").unwrap();
    let dir = a.dir.join("fleet");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(dir.join("identity.key")).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    let seed = std::fs::read_to_string(dir.join("identity.key")).unwrap();
    let shown = serde_json::to_string(&a.status()).unwrap();
    assert!(!shown.contains(seed.trim()));
    let rec = std::fs::read_to_string(dir.join("records").read_dir().unwrap().next().unwrap().unwrap().path()).unwrap();
    assert!(!rec.contains(seed.trim()), "the record holds public keys and wrapped keys only");
    // the same folder gives the same identity after a restart
    let id = a.my_id();
    assert_eq!(Hub::load(&a.dir).unwrap().my_id(), id);
}

#[test]
fn a_hub_that_cannot_listen_says_so_and_stopping_closes_the_port() {
    let a = hub("sp");
    let addr = a.start_node("127.0.0.1:0", src(1)).unwrap();
    assert!(a.listening().is_some());
    let b = hub("sq");
    assert!(b.start_node(&addr.to_string(), src(1)).is_err(), "the port is taken");
    a.stop_node();
    assert!(a.listening().is_none());
    assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_err(), "closed");
}

// ------------------------------------------------------------------------------------------------ F-FLEET-08: beyond the LAN

fn tls_hub(tag: &str) -> (Arc<Hub>, String) {
    let h = hub(tag);
    let id = crate::webservice::tls::self_signed(&crate::testdir::outside_project("fleet-tls", tag), &[]).unwrap();
    h.set_tls(id.config, &id.fingerprint);
    (h, id.fingerprint)
}

fn tls_addr(listen: &str, fp: &str) -> String {
    format!("tls://{listen}#{fp}")
}

/// A tap between two systems: forwards every byte both ways and keeps a copy of what went through.
fn tap(to: String) -> (String, Arc<Mutex<Vec<u8>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let at = l.local_addr().unwrap().to_string();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for client in l.incoming().flatten() {
            let (Ok(up), s3) = (TcpStream::connect(&to), s2.clone()) else { continue };
            let (mut c1, mut u1, mut c2, mut u2) = (client.try_clone().unwrap(), up.try_clone().unwrap(), client, up);
            let s4 = s3.clone();
            std::thread::spawn(move || {
                let mut b = [0u8; 4096];
                while let Ok(n) = c1.read(&mut b) {
                    if n == 0 || u1.write_all(&b[..n]).is_err() {
                        break;
                    }
                    s3.lock().unwrap().extend_from_slice(&b[..n]);
                }
                let _ = u1.shutdown(std::net::Shutdown::Write);
            });
            std::thread::spawn(move || {
                let mut b = [0u8; 4096];
                while let Ok(n) = u2.read(&mut b) {
                    if n == 0 || c2.write_all(&b[..n]).is_err() {
                        break;
                    }
                    s4.lock().unwrap().extend_from_slice(&b[..n]);
                }
                let _ = c2.shutdown(std::net::Shutdown::Write);
            });
        }
    });
    (at, seen)
}

fn has(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

#[test]
fn an_explicit_address_is_parsed_with_its_pin_or_refused_and_a_plain_address_stays_plain() {
    let fp = "AB".repeat(32);
    let colon = tls::colon(&fp);
    assert_eq!(colon.len(), 95);
    assert_eq!(Target::parse("10.0.0.5:7671").unwrap(), Target::Plain("10.0.0.5:7671".into()));
    let t = Target::parse(&format!("tls://example.org:7671#{colon}")).unwrap();
    assert_eq!(t, Target::Tls { addr: "example.org:7671".into(), fingerprint: fp.clone() });
    assert_eq!(t.text(), format!("tls://example.org:7671#{colon}"));
    assert_eq!(Target::parse(&format!("tls://example.org:7671#{}", fp.to_lowercase())).unwrap(), t, "case and colons do not matter");
    for bad in ["tls://example.org:7671", "tls://example.org:7671#abcd", "tls://example.org#".to_string().as_str(), &format!("tls://:7671#{fp}"), &format!("tls://host:0#{fp}"), &format!("tls://host:99999#{fp}"), &format!("tls://ho st:1#{fp}"), &format!("tls://host:1#{}#x", fp)] {
        assert!(Target::parse(bad).is_err() || matches!(Target::parse(bad), Ok(Target::Tls { .. })) && bad.contains("#x"), "{bad} must be refused");
    }
    assert!(Target::parse(&format!("tls://[::1]:7671#{fp}")).is_ok());
    assert!(tls::advertised("::1".parse().unwrap(), 5, &fp).starts_with("tls://[::1]:5#AB:AB"));
}

#[test]
fn two_systems_pair_and_trade_heartbeats_over_real_tls_on_loopback_with_the_certificate_pinned() {
    let ((a, fa), (b, fb)) = (tls_hub("ta"), tls_hub("tb"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let fid = pair(&a, &b, &tls_addr(&a_addr, &fa), "bravo");
    let (sa, sb) = (a.status(), b.status());
    assert_eq!(sa.fleets[0].members.len(), 2);
    assert_eq!(sb.fleets[0].id, fid);
    // each side reaches the other over TLS, and the pin is the certificate the other showed
    let me_b = b.my_id();
    let pa = lock(&a.inner).fleets[&fid].peers[&me_b].addr.clone().unwrap();
    assert!(pa.starts_with("tls://127.0.0.1:") && pa.ends_with(&tls::colon(&fb)), "{pa}");
    // heartbeats cross the TLS channel: each sees the other's numbers, and the status says how they are reached
    a.heartbeat_tick(&FixedSource(hb(1)));
    b.heartbeat_tick(&FixedSource(hb(2)));
    let other = |h: &Arc<Hub>| h.status().fleets[0].members.iter().find(|m| !m.me).cloned().unwrap();
    assert_eq!(other(&a).heartbeat.unwrap().models, vec!["model-2".to_string()]);
    assert_eq!(other(&b).heartbeat.unwrap().models, vec!["model-1".to_string()]);
    assert_eq!((other(&a).transport.as_str(), other(&b).transport.as_str()), ("tls", "tls"));
    // work travels the same way
    a.set_accept(Accept::Never);
    b.set_accept(Accept::Ask);
    let body = AssignBody { id: "1".repeat(32), title: "t".into(), goal: "g".into(), base_branch: "main".into(), base_commit: "0".repeat(40), rows: vec![], context_digest: String::new(), budget_tokens: 0, model_class: String::new(), after: vec![] };
    a.assign(&fid, &me_b, &body).unwrap();
    assert_eq!(b.status().inbox.len(), 1, "the assignment reached the other system over TLS");
}

#[test]
fn a_wrong_certificate_is_refused_before_anything_is_sent_and_a_plain_client_gets_nothing_from_a_tls_node() {
    let ((a, fa), (b, _)) = (tls_hub("wa"), tls_hub("wb"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let fid = a.create_fleet("lab", "alpha").unwrap();
    let (code, _) = a.offer_code(&fid).unwrap();
    // a certificate that is not A's
    let other = "CD".repeat(32);
    let e = b.join_start(&tls_addr(&a_addr, &other), &code, "bravo").unwrap_err();
    assert!(e.contains("certificate"), "{e}");
    assert!(a.status().requests.is_empty(), "A never saw a message");
    assert!(b.status().pending.is_empty());
    // a plain client talking to a TLS node: no answer it can read, no request
    let plain = Hub::load(&crate::testdir::outside_project("fleet", "wp")).unwrap();
    let e = plain.join_start(&a_addr, &code, "charlie").unwrap_err();
    assert!(!e.is_empty());
    assert!(a.status().requests.is_empty());
    // the code is still good: the right pin works
    b.join_start(&tls_addr(&a_addr, &fa), &code, "bravo").unwrap();
    assert_eq!(a.status().requests.len(), 1);
    // a TLS client talking to a plain node
    let (c, _) = (hub("wc"), ());
    let c_addr = up(&c, 3);
    let cf = c.create_fleet("lab2", "gamma").unwrap();
    let (code2, _) = c.offer_code(&cf).unwrap();
    let e = b.join_start(&tls_addr(&c_addr, &fa), &code2, "bravo").unwrap_err();
    assert!(!e.is_empty());
    assert!(c.status().requests.is_empty());
}

#[test]
fn a_tap_between_the_systems_sees_the_plain_channel_in_the_clear_and_the_tls_channel_not_at_all() {
    // plain: the kinds of the messages and the fleet's id are readable by anyone on the path
    let (a, b) = (hub("pa2"), hub("pb2"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let (via, seen) = tap(a_addr.clone());
    let fid = a.create_fleet("lab", "alpha").unwrap();
    let (code, _) = a.offer_code(&fid).unwrap();
    b.join_start(&via, &code, "bravo-name").unwrap();
    let plain = seen.lock().unwrap().clone();
    assert!(has(&plain, "join_request") && has(&plain, "bravo-name"), "the control: the tap does see a plain exchange");
    // TLS: the same exchange through the same kind of tap shows a handshake and noise
    let ((a, fa), (b, _)) = (tls_hub("ta2"), tls_hub("tb2"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    let (via, seen) = tap(a_addr);
    let fid = a.create_fleet("lab", "alpha").unwrap();
    let (code, _) = a.offer_code(&fid).unwrap();
    b.join_start(&tls_addr(&via, &fa), &code, "bravo-name").unwrap();
    let wire = seen.lock().unwrap().clone();
    assert!(wire.len() > 200 && wire[0] == 0x16 && wire[1] == 0x03, "a TLS handshake, not a frame");
    for secret in ["join_request", "bravo-name", &code.replace(' ', ""), "lab", "alpha", &fid[..16]] {
        assert!(!has(&wire, secret), "{secret:?} is readable on the wire");
    }
    // and a tap that changes the certificate (a man in the middle with its own key) is refused by the pin
    let evil = crate::webservice::tls::self_signed(&crate::testdir::outside_project("fleet-tls", "evil"), &[]).unwrap();
    let mitm = hub("mitm");
    mitm.set_tls(evil.config, &evil.fingerprint);
    let m_addr = up(&mitm, 9);
    let e = b.join_start(&tls_addr(&m_addr, &fa), &code, "bravo-name").unwrap_err();
    assert!(e.contains("certificate"), "{e}");
}

#[test]
fn an_address_a_person_gives_is_kept_and_saved_and_only_the_pin_follows_the_members_own_signed_heartbeat() {
    let ((a, fa), (b, fb)) = (tls_hub("ka"), tls_hub("kb"));
    let a_addr = up(&a, 1);
    let b_addr = up(&b, 2);
    let fid = pair(&a, &b, &tls_addr(&a_addr, &fa), "bravo");
    let me_a = a.my_id();
    let port_a = a_addr.rsplit(':').next().unwrap();
    // the person says A is at a name, not the address the connection came from
    let given = format!("tls://localhost:{port_a}#{}", tls::colon(&fa));
    let said = b.set_member_address(&fid, &fingerprint(&me_a), &given).unwrap();
    assert_eq!(said, given);
    a.heartbeat_tick(&FixedSource(hb(1)));
    assert_eq!(lock(&b.inner).fleets[&fid].peers[&me_a].addr.as_deref(), Some(given.as_str()), "a heartbeat does not overwrite what the person gave");
    // the same address works: B reaches A through the name
    b.heartbeat_tick(&FixedSource(hb(2)));
    assert!(a.status().fleets[0].members.iter().any(|m| !m.me && m.heartbeat.is_some()));
    // a new certificate on A: its own heartbeat (signed and sealed) moves the pin, not the name
    let fresh = crate::webservice::tls::self_signed(&crate::testdir::outside_project("fleet-tls", "ka-new"), &[]).unwrap();
    a.set_tls(fresh.config, &fresh.fingerprint);
    a.heartbeat_tick(&FixedSource(hb(1)));
    let now = lock(&b.inner).fleets[&fid].peers[&me_a].addr.clone().unwrap();
    assert!(now.starts_with(&format!("tls://localhost:{port_a}#")) && now.ends_with(&tls::colon(&fresh.fingerprint)), "{now}");
    // it is saved: a reloaded hub still has it
    let again = Hub::load(&b.dir).unwrap();
    assert_eq!(again.status().fleets[0].members.iter().find(|m| !m.me).unwrap().transport, "tls");
    let _ = (b_addr, fb);
    // refusals
    for (who, addr, why) in [(fingerprint(&me_a), "tls://localhost:1".to_string(), "fingerprint"), (fingerprint(&me_a), "nonsense".to_string(), "not valid"), ("nobody".to_string(), given.clone(), "not a member"), (fingerprint(&b.my_id()), given.clone(), "this computer")] {
        let e = b.set_member_address(&fid, &who, &addr).unwrap_err();
        assert!(e.contains(why), "{addr}: {e}");
    }
    assert!(b.set_member_address("no-such-fleet", &fingerprint(&me_a), &given).is_err());
}

#[test]
fn a_mixed_fleet_works_a_plain_member_and_a_tls_member_reach_each_other_by_what_each_advertises() {
    let (a, b) = (hub("ma"), hub("mb"));
    let (b, fb) = {
        let id = crate::webservice::tls::self_signed(&crate::testdir::outside_project("fleet-tls", "mb"), &[]).unwrap();
        b.set_tls(id.config, &id.fingerprint);
        (b, id.fingerprint)
    };
    let a_addr = up(&a, 1);
    up(&b, 2);
    // B (TLS) joins A (plain) over the plain address; A learns to reach B over TLS from the join itself
    let fid = pair(&a, &b, &a_addr, "bravo");
    let me_b = b.my_id();
    let pa = lock(&a.inner).fleets[&fid].peers[&me_b].addr.clone().unwrap();
    assert!(pa.starts_with("tls://") && pa.ends_with(&tls::colon(&fb)), "{pa}");
    a.heartbeat_tick(&FixedSource(hb(1)));
    b.heartbeat_tick(&FixedSource(hb(2)));
    let t = |h: &Arc<Hub>| h.status().fleets[0].members.iter().find(|m| !m.me).map(|m| (m.transport.clone(), m.heartbeat.is_some())).unwrap();
    assert_eq!(t(&a), ("tls".to_string(), true));
    assert_eq!(t(&b), ("lan".to_string(), true));
}

#[test]
fn a_member_that_turns_tls_off_is_shown_as_having_left_tls_instead_of_looking_fine() {
    let ((a, fa), (b, _fb)) = (tls_hub("lefta"), tls_hub("leftb"));
    let a_addr = up(&a, 1);
    up(&b, 2);
    pair(&a, &b, &tls_addr(&a_addr, &fa), "bravo");
    a.heartbeat_tick(&FixedSource(hb(1)));
    b.heartbeat_tick(&FixedSource(hb(2)));
    let other = |h: &Arc<Hub>| h.status().fleets[0].members.iter().find(|m| !m.me).cloned().unwrap();
    assert_eq!(other(&a).transport, "tls");
    // bravo turns TLS off for itself: its next heartbeat no longer carries a certificate
    b.set_tls_wanted(false);
    b.heartbeat_tick(&FixedSource(hb(2)));
    assert_eq!(other(&a).transport, "tls-left", "the person is told to set a new address");
}

#[test]
fn the_person_sets_the_heartbeat_interval_between_10_and_30_seconds_and_it_survives_a_restart() {
    let d = crate::testdir::outside_project("fleet-iv", "iv");
    let h = Hub::load(&d).unwrap();
    assert_eq!(h.interval_secs(), 20);
    assert!(h.set_person_interval(5).is_err() && h.set_person_interval(31).is_err());
    h.set_person_interval(12).unwrap();
    assert_eq!(h.interval_secs(), 12);
    drop(h);
    assert_eq!(Hub::load(&d).unwrap().interval_secs(), 12);
}
