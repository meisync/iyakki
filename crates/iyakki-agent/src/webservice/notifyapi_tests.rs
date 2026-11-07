//! The notification routes and the unified approvals list on the web service's fakes: a person sees only their own, another tenant's
//! notes are not reachable, every action works, and the event stream's protocol keeps its rules.

use super::*;
use crate::webservice::{events, tenants};
use crate::notifyhub::service::hub_for;
use crate::notifyhub::sink::{Notification, Recipients, Severity};

fn put(h: &H, to: &[&str], title: &str, at: u64) {
    let hub = hub_for(&h.dir);
    let n = Notification { tenant: hub.tenant.clone(), team: None, recipients: Recipients::People(to.iter().map(|s| s.to_string()).collect()), severity: Severity::Normal, title: title.into(), body: "body text".into(), link: None, dedupe_key: String::new(), source: "flow:t".into() };
    hub.emit_at(&n, at).unwrap();
}

fn list(h: &H, tok: &str, q: &str) -> Value {
    let (s, v, _) = h.call("GET", &format!("/api/v1/notifications{q}"), Some(tok), None);
    assert_eq!(s, 200, "{v}");
    v
}

fn titles(v: &Value) -> Vec<String> {
    v["notifications"].as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_person_sees_only_their_own_notifications_and_cannot_touch_anothers() {
    let (h, _root, ann, bob) = cfg_cloud();
    let now = crate::tenantconfig::now_secs();
    put(&h, &["ann"], "for ann", now);
    put(&h, &["bob"], "for bob", now);
    assert_eq!(titles(&list(&h, &ann, "")), ["for ann"]);
    assert_eq!(titles(&list(&h, &bob, "")), ["for bob"]);
    let id = list(&h, &ann, "")["notifications"][0]["id"].as_str().unwrap().to_string();
    for (m, p) in [("POST", format!("/api/v1/notifications/{id}/read")), ("POST", format!("/api/v1/notifications/{id}/ack")), ("POST", format!("/api/v1/notifications/{id}/snooze")), ("DELETE", format!("/api/v1/notifications/{id}"))] {
        let (s, v, _) = h.call(m, &p, Some(&bob), Some(json!({"minutes": 5})).filter(|_| m == "POST"));
        assert_eq!(s, 404, "{m} {p} {v}");
    }
    assert_eq!(titles(&list(&h, &ann, "?unread=1")), ["for ann"], "still unread, not deleted");
    assert_eq!(h.call("GET", "/api/v1/notifications", None, None).0, 401);
    assert_eq!(h.call("GET", "/api/v1/notifications/count", None, None).0, 401);
    // a name in the query changes nothing: the person is the token's
    let (s, v, _) = h.call("GET", "/api/v1/notifications?user=ann", Some(&bob), None);
    assert_eq!((s, titles(&v)), (200, vec!["for bob".to_string()]));
}

#[test]
fn another_tenants_notifications_are_not_reachable_after_a_switch() {
    let h = harness("cloud", |_| {});
    let (root, ann) = (h.root(), h.ann());
    assert_eq!(h.call("POST", "/api/v1/tenant/bootstrap", Some(&root), None).0, 200);
    assert_eq!(h.call("POST", "/api/v1/tenant/members", Some(&root), Some(json!({"name": "ann", "role": "member"}))).0, 200);
    let beta = h.dir.join("beta-volume");
    let mut r = crate::cloudroles::Roles::new("ann").unwrap();
    r.add_tenant_member("ann", "root", crate::cloudroles::TenantRole::Member).unwrap();
    crate::cloudroles::save(&crate::cloudroles::path(&beta), &r).unwrap();
    let file = tenants::file(&h.dir);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, json!({"home": {"id": "acme", "name": "Acme"}, "tenants": [{"id": "beta", "name": "Beta", "data_dir": beta.to_string_lossy()}]}).to_string()).unwrap();
    put(&h, &["ann"], "acme secret", crate::tenantconfig::now_secs());
    assert_eq!(titles(&list(&h, &ann, "")), ["acme secret"]);
    assert_eq!(h.call("POST", "/api/v1/tenant/switch", Some(&ann), Some(json!({"tenant": "beta"}))).0, 200);
    assert!(titles(&list(&h, &ann, "")).is_empty(), "beta's volume has its own inbox");
    assert_eq!(h.call("GET", "/api/v1/notifications/count", Some(&ann), None).1["unread"], 0);
}

#[test]
fn read_unread_snooze_ack_delete_and_read_all_work_and_the_count_follows_the_store() {
    let (h, _root, ann, _bob) = cfg_cloud();
    let now = crate::tenantconfig::now_secs();
    for (i, t) in ["one", "two", "three", "four"].iter().enumerate() {
        put(&h, &["ann"], t, now - 100 + i as u64);
    }
    let count = || h.call("GET", "/api/v1/notifications/count", Some(&ann), None).1;
    assert_eq!(count()["unread"], 4);
    assert_eq!(count()["red"], false);
    let all = list(&h, &ann, "");
    assert_eq!(titles(&all), ["four", "three", "two", "one"], "newest first");
    let id = |i: usize| all["notifications"][i]["id"].as_str().unwrap().to_string();
    assert_eq!(h.call("POST", &format!("/api/v1/notifications/{}/read", id(0)), Some(&ann), None).0, 200);
    assert_eq!(count()["unread"], 3);
    assert_eq!(h.call("POST", &format!("/api/v1/notifications/{}/unread", id(0)), Some(&ann), None).0, 200);
    assert_eq!(count()["unread"], 4);
    // snooze: hidden from the list and the count, shown with ?snoozed=1, bad values refused
    assert_eq!(h.call("POST", &format!("/api/v1/notifications/{}/snooze", id(1)), Some(&ann), Some(json!({"minutes": 30}))).0, 200);
    assert_eq!(titles(&list(&h, &ann, "")), ["four", "two", "one"]);
    assert_eq!(count()["unread"], 3);
    assert!(titles(&list(&h, &ann, "?snoozed=1")).contains(&"three".to_string()));
    for bad in [json!({}), json!({"minutes": 0}), json!({"minutes": -5}), json!({"minutes": 999999}), json!({"until": 5})] {
        assert_eq!(h.call("POST", &format!("/api/v1/notifications/{}/snooze", id(2)), Some(&ann), Some(bad.clone())).0, 400, "{bad}");
    }
    let (s, v, _) = h.call("POST", &format!("/api/v1/notifications/{}/ack", id(2)), Some(&ann), None);
    assert_eq!((s, v["escalation_stopped"].clone()), (200, json!(false)));
    assert_eq!(h.call("DELETE", &format!("/api/v1/notifications/{}", id(3)), Some(&ann), None).0, 200);
    assert_eq!(h.call("DELETE", &format!("/api/v1/notifications/{}", id(3)), Some(&ann), None).0, 404, "gone");
    let (s, v, _) = h.call("POST", "/api/v1/notifications/read-all", Some(&ann), None);
    assert_eq!((s, v["ok"].clone()), (200, json!(true)));
    assert_eq!(count()["unread"], 0);
    // the count is rebuilt from the file: a new hub on the same volume (a restart) says the same
    let fresh = crate::notifyhub::service::Hub::new(&h.dir, "x", Box::new(crate::notifyhub::service::DirEnv { dir: h.dir.clone() }), "other");
    assert_eq!(fresh.inbox.list("ann").len(), 3);
}

#[test]
fn the_list_pages_with_a_time_cursor_and_clamps_the_limit() {
    let (h, _root, ann, _bob) = cfg_cloud();
    let now = crate::tenantconfig::now_secs();
    for i in 0..5u64 {
        put(&h, &["ann"], &format!("n{i}"), now - 500 + i * 10);
    }
    let p1 = list(&h, &ann, "?limit=2");
    assert_eq!(titles(&p1), ["n4", "n3"]);
    let cur = p1["next_before"].as_i64().unwrap();
    let p2 = list(&h, &ann, &format!("?limit=2&before={cur}"));
    assert_eq!(titles(&p2), ["n2", "n1"]);
    let p3 = list(&h, &ann, &format!("?limit=2&before={}", p2["next_before"].as_i64().unwrap()));
    assert_eq!((titles(&p3), p3["next_before"].is_null()), (vec!["n0".to_string()], true));
    assert_eq!(h.call("GET", "/api/v1/notifications?before=abc", Some(&ann), None).0, 400);
    assert_eq!(list(&h, &ann, "?limit=100000")["notifications"].as_array().unwrap().len(), 5);
}

#[test]
fn the_approvals_list_merges_tool_calls_and_tenant_requests_by_who_may_decide() {
    let (h, root, ann, bob) = cfg_cloud();
    assert_eq!(h.call("POST", "/api/v1/namespaces", Some(&root), Some(json!({"name": "web", "action": "create"}))).0, 200);
    assert_eq!(h.call("POST", "/api/v1/namespaces/web/members", Some(&root), Some(json!({"name": "bob", "role": "admin"}))).0, 200);
    // ann asks for a change in team web: bob (the team's admin) may decide it, she may not
    let dir = h.dir.clone();
    let roles = crate::cloudaccess::Access::new(&dir).roles().unwrap().unwrap();
    let env = crate::tenantconfig::api::Env { dir: &dir, vault: Arc::new(crate::tenantconfig::vault::NoVault), audit: &crate::tenantconfig::api::NoAudit };
    let p = crate::tenantconfig::approvals::request(&env, &roles, "k3s.cluster", "web", "prod", "ann", false, false, crate::tenantconfig::now_secs()).unwrap();
    // and ann has a run waiting for a tool
    let run = h.start_run(&ann, vec![call_turn("web_fetch"), final_turn("ok")]);
    h.wait_event(&ann, &run, |e| e["type"] == "approval");
    let (s, v, _) = h.call("GET", "/api/v1/approvals", Some(&ann), None);
    assert_eq!(s, 200, "{v}");
    let kinds: Vec<&str> = v["approvals"].as_array().unwrap().iter().map(|a| a["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["tool_call"], "her own request is not hers to decide: {v}");
    assert_eq!(v["approvals"][0]["tool"], "web_fetch");
    assert!(v["approvals"][0]["decide"]["path"].as_str().unwrap().starts_with("/api/v1/approvals/"));
    let (_, v, _) = h.call("GET", "/api/v1/approvals", Some(&bob), None);
    let b = v["approvals"].as_array().unwrap();
    assert_eq!(b.len(), 1, "{v}");
    assert_eq!((b[0]["kind"].as_str(), b[0]["id"].as_str()), (Some("tenant"), Some(p.id.as_str())));
    assert!(b[0]["decide"]["approve"].as_str().unwrap().ends_with("/approve"));
    // the bell counts both kinds for the right person
    assert_eq!(h.call("GET", "/api/v1/notifications/count", Some(&bob), None).1["approvals"], 1);
    let c = h.call("GET", "/api/v1/notifications/count", Some(&ann), None).1;
    assert_eq!((c["approvals"].clone(), c["red"].clone()), (json!(1), json!(true)));
    // decisions go through the existing endpoints; a second one is refused clearly
    let path = b[0]["decide"]["approve"].as_str().unwrap().to_string();
    assert_eq!(h.call("POST", &path, Some(&bob), None).0, 200);
    let (s, v, _) = h.call("POST", &path, Some(&bob), None);
    assert_eq!((s, v["error"]["message"].as_str()), (409, Some("already decided")));
    assert_eq!(h.call("GET", "/api/v1/approvals", Some(&bob), None).1["approvals"], json!([]));
    assert_eq!(h.call("GET", "/api/v1/approvals", None, None).0, 401);
}

#[test]
fn the_notification_routes_are_in_the_document_and_the_stream_needs_an_upgrade() {
    let h = harness("cloud", |_| {});
    let ann = h.ann();
    let (_, doc, _) = h.call("GET", "/api/v1/openapi.json", Some(&ann), None);
    for p in ["/api/v1/notifications", "/api/v1/notifications/count", "/api/v1/notifications/{id}/read", "/api/v1/notifications/{id}/ack", "/api/v1/events"] {
        assert!(doc["paths"].get(p).is_some(), "{p}");
    }
    let (s, v, _) = h.call("GET", "/api/v1/events", Some(&ann), None);
    assert_eq!((s, v["error"]["code"].as_str()), (426, Some("invalid_input")), "without an Upgrade it says what it is");
    assert_eq!(h.call("GET", "/api/v1/events", None, None).0, 401, "and a plain request needs the usual sign-in");
    let head = b"GET /api/v1/events HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n";
    assert!(events::is_events_head(head) && !events::is_events_head(b"GET /api/v1/eventsx HTTP/1.1\r\n\r\n") && !events::is_events_head(b"GET /api/v1/events HTTP/1.1\r\nHost: x\r\n\r\n"));
}

// ------------------------------------------------------------------------------------------------ the stream's protocol

use crate::notifyhub::events::{bus_for, Bus};
use crate::wsproto::{self as ws, Parser};
use crate::webservice::events::{Authed, Backing, Session, WsIo};
use std::io::{Read, Write};

struct FakeBacking {
    bus: Arc<Bus>,
    approvals: Mutex<usize>,
}

impl Backing for FakeBacking {
    fn auth(&self, token: &str) -> Option<Authed> {
        let user = match token { "ann-token" => "ann", "bob-token" => "bob", _ => return None };
        Some(Authed { user: user.into(), bus: self.bus.clone() })
    }
    fn approvals(&self, _: &Authed) -> usize {
        *self.approvals.lock().unwrap()
    }
}

fn fb() -> FakeBacking {
    FakeBacking { bus: Bus::new(), approvals: Mutex::new(0) }
}

fn texts(frames: &[Vec<u8>]) -> Vec<String> {
    frames.iter().filter(|f| f[0] & 0x0f == 1).map(|f| String::from_utf8(f[2..].to_vec()).unwrap()).collect()
}

fn is_close(frames: &[Vec<u8>]) -> bool {
    frames.iter().any(|f| f[0] & 0x0f == 8)
}

const AUTH_ANN: &str = r#"{"type":"auth","token":"ann-token"}"#;

#[test]
fn the_stream_needs_the_token_in_the_first_message_and_nothing_before_it() {
    let b = fb();
    let mut s = Session::new(100);
    assert!(s.tick(105, &b).is_empty() && !s.closed(), "waiting for the first message");
    assert!(is_close(&s.tick(111, &b)), "no sign-in within 10 seconds: closed");
    for first in [r#"{"type":"auth","token":"wrong"}"#, r#"{"type":"hello"}"#, "not json", r#"{"type":"auth"}"#, r#"{"type":"auth","token":5}"#] {
        let mut s = Session::new(0);
        let out = s.on_text(first, 1, &b);
        assert!(is_close(&out) && s.closed() && texts(&out).iter().all(|t| !t.contains("ann")), "{first}");
        assert_eq!(b.bus.open(), 0, "no subscription for a refused sign-in");
    }
    let mut s = Session::new(0);
    let out = s.on_text(AUTH_ANN, 1, &b);
    assert!(texts(&out)[0].contains("\"ready\"") && texts(&out)[0].contains("\"ann\""));
    assert_eq!(b.bus.open(), 1);
    // after sign-in the client cannot ask for anything else
    assert!(is_close(&s.on_text(r#"{"type":"subscribe","user":"bob"}"#, 2, &b)));
    s.end();
    assert_eq!(b.bus.open(), 0);
}

#[test]
fn the_stream_carries_only_the_signed_in_persons_events_and_tells_them_when_approvals_change() {
    let b = fb();
    let mut ann = Session::new(0);
    let mut bob = Session::new(0);
    ann.on_text(AUTH_ANN, 1, &b);
    bob.on_text(r#"{"type":"auth","token":"bob-token"}"#, 1, &b);
    b.bus.publish("ann", &crate::notifyhub::events::event("notification", json!({"title": "for ann"})));
    b.bus.publish("bob", &crate::notifyhub::events::event("count", json!({"unread": 9})));
    let a = texts(&ann.tick(2, &b)).join("|");
    let o = texts(&bob.tick(2, &b)).join("|");
    assert!(a.contains("for ann") && !a.contains("\"unread\":9"), "{a}");
    assert!(o.contains("\"unread\":9") && !o.contains("for ann"), "{o}");
    // the approvals count is sent when it changes, not every tick
    *b.approvals.lock().unwrap() = 2;
    assert!(texts(&ann.tick(3, &b)).iter().any(|t| t.contains("\"approval\"") && t.contains("\"count\":2")));
    assert!(texts(&ann.tick(4, &b)).is_empty());
}

#[test]
fn the_stream_pings_every_25_seconds_and_drops_a_silent_client_and_a_slow_one_loses_the_oldest() {
    let b = fb();
    let mut s = Session::new(0);
    s.on_text(AUTH_ANN, 0, &b);
    assert!(s.tick(24, &b).iter().all(|f| f[0] & 0x0f != 9));
    assert!(s.tick(25, &b).iter().any(|f| f[0] & 0x0f == 9), "a ping frame");
    s.pong(26);
    assert!(s.tick(60, &b).iter().any(|f| f[0] & 0x0f == 9), "again after another 25");
    assert!(!s.closed());
    assert!(is_close(&s.tick(26 + events::IDLE_SECS + 1, &b)) && s.closed(), "nothing heard for 90 seconds");
    assert_eq!(b.bus.open(), 1, "still subscribed until end() is called by the loop");
    s.end();
    assert_eq!(b.bus.open(), 0);
    // backpressure: nobody reads, then the newest events are what is there
    let mut slow = Session::new(0);
    slow.on_text(AUTH_ANN, 0, &b);
    for i in 0..300 {
        b.bus.publish("ann", &crate::notifyhub::events::event("notification", json!({"n": i})));
    }
    let got = texts(&slow.tick(1, &b));
    let notes: Vec<&String> = got.iter().filter(|t| t.contains("\"notification\"")).collect();
    assert_eq!(notes.len(), crate::notifyhub::events::QUEUE_CAP);
    assert!(notes.last().unwrap().contains("\"n\":299") && !got.iter().any(|t| t.contains("\"n\":0}")));
}

struct Scripted {
    input: std::collections::VecDeque<Vec<u8>>,
    out: Vec<u8>,
}

impl Read for Scripted {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self.input.pop_front() {
            Some(c) => {
                b[..c.len()].copy_from_slice(&c);
                Ok(c.len())
            }
            None => Ok(0),
        }
    }
}
impl Write for Scripted {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.out.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl WsIo for Scripted {
    fn set_read_timeout(&mut self, _: Duration) {}
}

fn masked(text: &str) -> Vec<u8> {
    let key = [1u8, 2, 3, 4];
    let mut f = vec![0x81, 0x80 | text.len() as u8];
    f.extend_from_slice(&key);
    f.extend(text.bytes().enumerate().map(|(i, b)| b ^ key[i % 4]));
    f
}

#[test]
fn the_loop_reads_masked_frames_answers_and_ends_when_the_client_goes() {
    let b = fb();
    b.bus.publish("ann", "ignored-before-subscribe");
    let mut io = Scripted { input: [masked(AUTH_ANN), vec![0x88, 0x80, 1, 2, 3, 4]].into_iter().collect(), out: vec![] };
    events::run(&mut io, &b, &[]);
    let mut p = Parser::new(1 << 16);
    let _ = &mut p; // server frames are unmasked: read them by hand
    let s = String::from_utf8_lossy(&io.out).to_string();
    assert!(s.contains("\"ready\"") && s.contains("\"approval\""), "{s}");
    assert!(!s.contains("ignored-before-subscribe"));
    assert_eq!(b.bus.open(), 0, "the subscription ended with the connection");
    // an unmasked client frame is a protocol error: closed
    let mut bad = Scripted { input: [vec![0x81, 0x02, b'h', b'i']].into_iter().collect(), out: vec![] };
    events::run(&mut bad, &b, &[]);
    assert_eq!(bad.out, ws::close_frame());
    let _ = bus_for(&std::env::temp_dir());
}
