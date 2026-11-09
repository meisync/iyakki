//! Tiles in a browser (tiles.rs): the example counter through the real sandbox, over TLS on a loopback port and through the
//! pipeline; budgets and refusals; hostile JSON; sign-in; one person never reaching another's tile; service calls scoped by the tile's
//! manifest and then by the person's tools (a tool that asks first is not run from a tile).

use super::*;
use crate::webservice::tiles::{self, TileDef};
use iyakki_tile_abi::{Color, DisplayList, Op, Rect};

fn texts(v: &Value) -> Vec<String> {
    v["frame"]["ops"].as_array().map(|a| a.iter().filter(|o| o["op"] == "text").map(|o| o["text"].as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
}

fn start(h: &H, tok: &str) -> (u16, Value) {
    let (s, v, _) = h.call("POST", "/api/v1/tiles/counter/events", Some(tok), Some(json!({"width": 320, "height": 200})));
    (s, v)
}

fn click(h: &H, tok: &str, x: i64, y: i64) -> Value {
    let body = json!({"events": [{"type": "pointer_down", "x": x, "y": y, "button": 0}, {"type": "pointer_up", "x": x, "y": y, "button": 0}]});
    let (s, v, _) = h.call("POST", "/api/v1/tiles/counter/events", Some(tok), Some(body));
    assert_eq!(s, 200, "{v}");
    v
}

fn one_rect_list() -> Vec<u8> {
    let mut l = DisplayList::new();
    l.push(Op::Rect { r: Rect::new(0, 0, 50, 50), color: Color(9, 9, 9, 255) });
    l.encode().unwrap()
}

/// A tile that draws one rectangle; `event_body` runs in `iyakki_tile_event` (then it asks for a redraw); the strings are at 1000 + 16*i.
fn wat_tile(abi: u32, event_body: &str, methods: &[&str]) -> String {
    let frame = one_rect_list();
    let mut data = (frame.len() as u32).to_le_bytes().iter().map(|b| format!("\\{b:02x}")).collect::<String>();
    data.push_str(&frame.iter().map(|b| format!("\\{b:02x}")).collect::<String>());
    let names: String = methods.iter().enumerate().map(|(i, m)| format!("(data (i32.const {}) \"{m}\")\n", 1000 + 16 * i)).collect();
    format!(
        r#"(module
  (import "iyakki_tile" "call" (func $call (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 3)
  (data (i32.const 4096) "{data}")
  (data (i32.const 900) "tools") (data (i32.const 920) "{{}}")
  {names}
  (global $h (mut i32) (i32.const 16384))
  (func (export "iyakki_tile_abi") (result i32) (i32.const {abi}))
  (func (export "iyakki_alloc") (param i32) (result i32) (global.get $h))
  (func (export "iyakki_tile_init") (param i32 i32 i32) (result i32) (i32.const 0))
  (func (export "iyakki_tile_event") (param i32 i32) (result i32) {event_body} (i32.const 1))
  (func (export "iyakki_tile_frame") (result i32) (i32.const 4096)))"#
    )
}

/// `event_body` text that calls tools.<method> for each name (the strings are placed by `wat_tile`).
fn calls(methods: &[&str]) -> String {
    methods.iter().enumerate().map(|(i, m)| format!("(drop (call $call (i32.const 900) (i32.const 5) (i32.const {}) (i32.const {}) (i32.const 920) (i32.const 2) (i32.const 8192) (i32.const 512)))", 1000 + 16 * i, m.len())).collect::<Vec<_>>().join("\n")
}

fn def(id: &str, wat: String, caps: &[&str]) -> TileDef {
    TileDef { id: id.into(), title: id.into(), wasm: Arc::new(wat.into_bytes()), caps: caps.iter().map(|s| s.to_string()).collect(), width: 100, height: 100 }
}

fn post(h: &H, tok: &str, tile: &str, body: Value) -> (u16, Value) {
    let (s, v, _) = h.call("POST", &format!("/api/v1/tiles/{tile}/events"), Some(tok), Some(body));
    (s, v)
}

#[test]
fn the_list_names_the_example_tile_and_needs_a_sign_in() {
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    let (s, v, _) = h.call("GET", "/api/v1/tiles", Some(&ann), None);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["tiles"][0]["id"], "counter");
    assert_eq!(v["tiles"][0]["abi"], 1);
    assert_eq!(h.call("GET", "/api/v1/tiles", None, None).0, 401);
    assert_eq!(h.call("GET", "/api/v1/tiles", Some("iyk_nonsense"), None).0, 401);
    // the same list in the cloud profile
    let c = harness("cloud", |_| {});
    assert_eq!(c.call("GET", "/api/v1/tiles", Some(&c.ann()), None).0, 200);
}

#[test]
fn events_in_and_a_display_list_out_over_tls_on_a_loopback_port() {
    let h = harness("agent-api", |_| {});
    let (port, fp) = (h.svc.local_addr().port(), h.svc.fingerprint().to_string());
    assert!(h.svc.local_addr().ip().is_loopback());
    let call = |m: &str, p: &str, tok: Option<&str>, body: Option<Value>| {
        let mut hd = vec![];
        if let Some(t) = tok {
            hd.push(("Authorization".to_string(), format!("Bearer {t}")));
        }
        let b = body.map(|b| b.to_string()).unwrap_or_default();
        if !b.is_empty() {
            hd.push(("Content-Type".to_string(), "application/json".to_string()));
        }
        let r = crate::devmode::client::https("127.0.0.1", port, Some(&fp), m, p, &hd, b.as_bytes(), Duration::from_secs(15)).expect("answer");
        (r.status, r.json())
    };
    let (_, v) = call("POST", "/api/v1/login", None, Some(json!({"name": "ann", "password": "ann-password-1"})));
    let tok = v["token"].as_str().unwrap().to_string();
    // the first call starts the tile and returns its picture
    let (s, v) = call("POST", "/api/v1/tiles/counter/events", Some(&tok), Some(json!({"width": 320, "height": 200, "scale_pct": 200, "dark": true})));
    assert_eq!(s, 200, "{v}");
    assert_eq!((v["frame"]["abi"].clone(), v["frame"]["width"].clone(), v["frame"]["height"].clone()), (json!(1), json!(320), json!(200)));
    assert!(texts(&v).contains(&"TILE DEMO".to_string()) && texts(&v).contains(&"0".to_string()), "{v}");
    assert!(v["frame"]["ops"].as_array().unwrap().iter().any(|o| o["op"] == "hit" && o["id"] == 1));
    // a tap on the button: the host turns the release into a Hit for the topmost hit region, the tile counts
    let tap = json!({"events": [{"type": "pointer_down", "x": 70, "y": 88, "button": 0}, {"type": "pointer_up", "x": 70, "y": 88, "button": 0}]});
    let (s, v) = call("POST", "/api/v1/tiles/counter/events", Some(&tok), Some(tap.clone()));
    assert_eq!((s, v["redraw"].clone()), (200, json!(true)), "{v}");
    assert!(texts(&v).contains(&"1".to_string()), "{:?}", texts(&v));
    call("POST", "/api/v1/tiles/counter/events", Some(&tok), Some(tap));
    // typing: text and a backspace key
    let (_, v) = call("POST", "/api/v1/tiles/counter/events", Some(&tok), Some(json!({"events": [{"type": "text", "text": "hey"}]})));
    assert!(texts(&v).contains(&"2".to_string()) && texts(&v).contains(&"hey".to_string()), "{:?}", texts(&v));
    let (_, v) = call("POST", "/api/v1/tiles/counter/events", Some(&tok), Some(json!({"events": [{"type": "key", "code": 8, "mods": 0, "down": true}, {"type": "key", "code": 8, "mods": 0, "down": false}]})));
    assert!(texts(&v).contains(&"he".to_string()), "{:?}", texts(&v));
    // a resize changes the size the picture is for
    let (_, v) = call("POST", "/api/v1/tiles/counter/events", Some(&tok), Some(json!({"events": [{"type": "resize", "w": 400, "h": 300, "scale_pct": 100}]})));
    assert_eq!((v["frame"]["width"].clone(), v["frame"]["height"].clone()), (json!(400), json!(300)));
    // a click that misses every hit region changes nothing
    let (_, v) = call("POST", "/api/v1/tiles/counter/events", Some(&tok), Some(json!({"events": [{"type": "pointer_down", "x": 300, "y": 280, "button": 0}, {"type": "pointer_up", "x": 300, "y": 280, "button": 0}]})));
    assert_eq!(v["redraw"], false);
    // reset starts again: the state is gone
    let (_, v) = call("POST", "/api/v1/tiles/counter/events", Some(&tok), Some(json!({"reset": true, "width": 320, "height": 200})));
    assert!(texts(&v).contains(&"0".to_string()) && !texts(&v).contains(&"hey".to_string()), "{:?}", texts(&v));
}

#[test]
fn the_same_events_give_the_same_picture_as_the_native_counter() {
    // the service's picture is the Rust twin's: this ties the sandbox path to the reference app
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    start(&h, &ann);
    click(&h, &ann, 70, 88);
    let v = click(&h, &ann, 70, 88);
    let mut native = iyakki_tile_example::Counter::new();
    native.event(&iyakki_tile_abi::Event::Hit { id: 1 });
    native.event(&iyakki_tile_abi::Event::Hit { id: 1 });
    let want: Value = serde_json::from_str(&iyakki_tile_host::export::to_json(&native.frame(), 320, 200)).unwrap();
    assert_eq!(v["frame"], want);
}

#[test]
fn a_second_person_cannot_reach_the_first_persons_tile_state() {
    let h = harness("agent-api", |_| {});
    let (ann, bob) = (h.ann(), h.login("bob", "bob-password-1"));
    start(&h, &ann);
    click(&h, &ann, 70, 88);
    click(&h, &ann, 70, 88);
    click(&h, &ann, 70, 88);
    // bob starts his own copy: zero, not three
    let (s, v) = start(&h, &bob);
    assert_eq!(s, 200);
    assert!(texts(&v).contains(&"0".to_string()) && !texts(&v).contains(&"3".to_string()), "{:?}", texts(&v));
    // events carry no name: bob's click counts for bob only, and ann still has three
    let b = click(&h, &bob, 70, 88);
    assert!(texts(&b).contains(&"1".to_string()));
    let (_, a) = post(&h, &ann, "counter", json!({"events": []}));
    assert!(texts(&a).contains(&"3".to_string()), "{:?}", texts(&a));
    assert_eq!(h.svc.shared().tiles.open_sessions(), 2);
    // a name in the body or the query changes nothing
    let (_, a2) = post(&h, &bob, "counter", json!({"events": [], "user": "ann"}));
    assert!(texts(&a2).contains(&"1".to_string()));
    let (s, _, _) = h.call("GET", "/api/v1/tiles?user=ann", Some(&bob), None);
    assert_eq!(s, 200);
}

#[test]
fn another_tenants_volume_gets_its_own_state() {
    // the slot key holds the data directory the request works on, so a tenant switch never shows the earlier tenant's tile state
    let (h, _root, ann, _bob) = cfg_cloud();
    start(&h, &ann);
    click(&h, &ann, 70, 88);
    let (s, v) = post(&h, &ann, "counter", json!({"events": []}));
    assert_eq!(s, 200);
    assert!(texts(&v).contains(&"1".to_string()));
}

#[test]
fn sign_in_and_origin_are_checked_like_every_other_route() {
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    let body = json!({"width": 320, "height": 200});
    assert_eq!(h.call("POST", "/api/v1/tiles/counter/events", None, Some(body.clone())).0, 401);
    assert_eq!(h.call("POST", "/api/v1/tiles/counter/events", Some("iyk_0000"), Some(body.clone())).0, 401);
    // a request from another site is refused even with a good token, and nothing starts
    let mut r = req("POST", "/api/v1/tiles/counter/events", Some(&ann), Some(body.clone()));
    r.headers.insert("origin".into(), "https://evil.example".into());
    let (s, v, _) = h.call_req(r);
    assert_eq!((s, v["error"]["code"].as_str()), (403, Some("denied")));
    assert_eq!(h.svc.shared().tiles.open_sessions(), 0);
    // a cookie is not a credential on this route: only the bearer token is
    let mut r = req("POST", "/api/v1/tiles/counter/events", None, Some(body.clone()));
    r.headers.insert("cookie".into(), "iyakki_session=anything".into());
    assert_eq!(h.call_req(r).0, 401);
    // a body that is not JSON is refused before the tile is touched
    let mut r = req("POST", "/api/v1/tiles/counter/events", Some(&ann), Some(body));
    r.headers.insert("content-type".into(), "text/plain".into());
    assert_eq!(h.call_req(r).0, 400);
    // the route is audited
    start(&h, &ann);
    assert!(h.accounts.lines().iter().any(|l| l.contains("api.tile_events")), "{:?}", h.accounts.lines());
}

#[test]
fn hostile_json_is_refused_without_starting_or_hurting_anything() {
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    let ok_ev = json!({"type": "pointer_move", "x": 1, "y": 2});
    let cases: Vec<(&str, Value, u16)> = vec![
        ("not an object", json!([1, 2, 3]), 400),
        ("a string", json!("hello"), 400),
        ("events not a list", json!({"width": 10, "height": 10, "events": "x"}), 400),
        ("events an object", json!({"width": 10, "height": 10, "events": {"a": 1}}), 400),
        ("too many events", json!({"width": 10, "height": 10, "events": vec![ok_ev.clone(); 129]}), 413),
        ("an event that is not an object", json!({"width": 10, "height": 10, "events": [7]}), 400),
        ("no type", json!({"width": 10, "height": 10, "events": [{"x": 1}]}), 400),
        ("type not a string", json!({"width": 10, "height": 10, "events": [{"type": 5}]}), 400),
        ("unknown type", json!({"width": 10, "height": 10, "events": [{"type": "format_disk"}]}), 400),
        ("hit from the page", json!({"width": 10, "height": 10, "events": [{"type": "hit", "id": 1}]}), 400),
        ("x a string", json!({"width": 10, "height": 10, "events": [{"type": "pointer_move", "x": "1", "y": 2}]}), 400),
        ("x a float", json!({"width": 10, "height": 10, "events": [{"type": "pointer_move", "x": 1.5, "y": 2}]}), 400),
        ("x huge", json!({"width": 10, "height": 10, "events": [{"type": "pointer_move", "x": 99999999999i64, "y": 2}]}), 400),
        ("x beyond the ABI", json!({"width": 10, "height": 10, "events": [{"type": "pointer_move", "x": 40000, "y": 2}]}), 400),
        ("x missing", json!({"width": 10, "height": 10, "events": [{"type": "pointer_move", "y": 2}]}), 400),
        ("x null", json!({"width": 10, "height": 10, "events": [{"type": "pointer_move", "x": null, "y": 2}]}), 400),
        ("button too big", json!({"width": 10, "height": 10, "events": [{"type": "pointer_down", "x": 1, "y": 1, "button": 256}]}), 400),
        ("key down not a bool", json!({"width": 10, "height": 10, "events": [{"type": "key", "code": 8, "mods": 0, "down": 1}]}), 400),
        ("key code negative", json!({"width": 10, "height": 10, "events": [{"type": "key", "code": -1, "mods": 0, "down": true}]}), 400),
        ("mods too big", json!({"width": 10, "height": 10, "events": [{"type": "key", "code": 8, "mods": 16, "down": true}]}), 400),
        ("text too long", json!({"width": 10, "height": 10, "events": [{"type": "text", "text": "a".repeat(1025)}]}), 400),
        ("text a number", json!({"width": 10, "height": 10, "events": [{"type": "text", "text": 5}]}), 400),
        ("tick too long", json!({"width": 10, "height": 10, "events": [{"type": "tick", "dt_ms": 4000000000u64}]}), 400),
        ("resize zero", json!({"width": 10, "height": 10, "events": [{"type": "resize", "w": 0, "h": 5, "scale_pct": 100}]}), 400),
        ("resize huge", json!({"width": 10, "height": 10, "events": [{"type": "resize", "w": 100000, "h": 5, "scale_pct": 100}]}), 400),
        ("scale zero", json!({"width": 10, "height": 10, "events": [{"type": "resize", "w": 5, "h": 5, "scale_pct": 0}]}), 400),
        ("no width", json!({"events": []}), 400),
        ("width a string", json!({"width": "10", "height": 10}), 400),
        ("width zero", json!({"width": 0, "height": 10}), 400),
        ("width huge", json!({"width": 5000, "height": 10}), 400),
        ("height negative", json!({"width": 10, "height": -4}), 400),
        ("scale absurd", json!({"width": 10, "height": 10, "scale_pct": 100000}), 400),
        ("dark not a bool", json!({"width": 10, "height": 10, "dark": "yes"}), 400),
        ("accent not a colour", json!({"width": 10, "height": 10, "accent": "red"}), 400),
    ];
    for (name, body, want) in cases {
        let (s, v) = post(&h, &ann, "counter", body);
        assert_eq!(s, want, "{name}: {v}");
        assert_eq!(v["error"].is_object(), true, "{name}");
    }
    assert_eq!(h.svc.shared().tiles.open_sessions(), 0, "a refused first call leaves nothing behind");
    // deep nesting does not crash the parser; a big array of small events within the caps is fine
    let deep = format!("{}1{}", "[".repeat(5000), "]".repeat(5000));
    let mut r = req("POST", "/api/v1/tiles/counter/events", Some(&ann), None);
    r.body = deep.into_bytes();
    r.headers.insert("content-type".into(), "application/json".into());
    let (s, _, _) = h.call_req(r);
    assert_eq!(s, 400);
    // a body over 64 KiB is refused
    let mut r = req("POST", "/api/v1/tiles/counter/events", Some(&ann), None);
    r.body = format!("{{\"width\":10,\"height\":10,\"pad\":\"{}\"}}", "x".repeat(70_000)).into_bytes();
    r.headers.insert("content-type".into(), "application/json".into());
    let (s, v, _) = h.call_req(r);
    assert_eq!((s, v["error"]["code"].as_str()), (413, Some("too_large")));
    // nothing in the machinery was damaged: a good call still works
    assert_eq!(start(&h, &ann).0, 200);
    let many = vec![ok_ev; 128];
    assert_eq!(post(&h, &ann, "counter", json!({"events": many})).0, 200);
}

#[test]
fn unknown_tiles_and_odd_names_are_not_found() {
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    for id in ["nope", "Counter", "..", "a%2fb", "counter%00", "co unter"] {
        let (s, _, _) = h.call("POST", &format!("/api/v1/tiles/{id}/events"), Some(&ann), Some(json!({"width": 10, "height": 10})));
        assert_eq!(s, 404, "{id}");
    }
    assert_eq!(h.call("GET", "/api/v1/tiles/counter/events", Some(&ann), None).0, 405);
    assert_eq!(h.call("POST", "/api/v1/tiles", Some(&ann), Some(json!({}))).0, 405);
}

#[test]
fn a_tile_that_runs_too_long_is_stopped_and_the_next_start_is_clean() {
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    h.svc.shared().tiles.register(def("spin", wat_tile(1, "(loop $l (br $l))", &[]), &[]));
    let (s, v) = post(&h, &ann, "spin", json!({"width": 50, "height": 50}));
    assert_eq!(s, 200, "{v}");
    let t = Instant::now();
    let (s, v) = post(&h, &ann, "spin", json!({"events": [{"type": "tick", "dt_ms": 16}]}));
    assert!(t.elapsed() < Duration::from_secs(5), "stopped by its budget");
    assert_eq!(s, 502, "{v}");
    let m = v["error"]["message"].as_str().unwrap();
    assert!(m.contains("stopped") && m.contains("too much"), "{m}");
    assert_eq!(h.svc.shared().tiles.open_sessions(), 0, "the crashed tile's state is gone");
    // a new start works (the same picture), and the other tiles were never affected
    assert_eq!(post(&h, &ann, "spin", json!({"width": 50, "height": 50})).0, 200);
    assert_eq!(start(&h, &ann).0, 200);
}

#[test]
fn a_tile_for_another_interface_version_or_a_broken_module_is_refused_in_plain_words() {
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    h.svc.shared().tiles.register(def("future", wat_tile(2, "", &[]), &[]));
    h.svc.shared().tiles.register(def("junk", "this is not wasm".into(), &[]));
    let (s, v) = post(&h, &ann, "future", json!({"width": 50, "height": 50}));
    assert_eq!(s, 501, "{v}");
    let m = v["error"]["message"].as_str().unwrap();
    assert!(m.contains("version 2") && m.contains("version 1") && m.contains("Update"), "{m}");
    let (s, v) = post(&h, &ann, "junk", json!({"width": 50, "height": 50}));
    assert_eq!(s, 501, "{v}");
    assert_eq!(h.svc.shared().tiles.open_sessions(), 0);
}

#[test]
fn service_calls_follow_the_manifest_then_the_persons_tools_and_a_tool_that_asks_is_not_run() {
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    // now: safe and allowed; web_fetch: the manifest allows it but it asks first; run_command: in the manifest, but not in this profile;
    // radio_play: safe, but the manifest does not list it
    let names = ["now", "web_fetch", "run_command", "radio_play"];
    h.svc.shared().tiles.register(def("caller", wat_tile(1, &calls(&names), &names), &["tools.now", "tools.web_fetch", "tools.run_command"]));
    assert_eq!(post(&h, &ann, "caller", json!({"width": 50, "height": 50})).0, 200);
    let (s, v) = post(&h, &ann, "caller", json!({"events": [{"type": "tick", "dt_ms": 1}]}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(*h.executed.lock().unwrap(), vec!["now".to_string()], "only the safe, allowed, declared tool ran");
    let lines = h.accounts.lines().join("\n");
    assert!(lines.contains("tile.call") && lines.contains("tool=now"), "{lines}");
    assert!(lines.contains("tile.refused") && lines.contains("tool=web_fetch") && lines.contains("tool=run_command"), "{lines}");
    assert!(lines.contains("tile.denied") && lines.contains("radio_play"), "{lines}");
    // no secret or argument text is written to the audit line
    assert!(!lines.contains("example.org"));
}

#[test]
fn a_tile_with_no_capabilities_reaches_nothing() {
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    h.svc.shared().tiles.register(def("bare", wat_tile(1, &calls(&["now"]), &["now"]), &[]));
    post(&h, &ann, "bare", json!({"width": 50, "height": 50}));
    assert_eq!(post(&h, &ann, "bare", json!({"events": [{"type": "tick", "dt_ms": 1}]})).0, 200);
    assert!(h.executed.lock().unwrap().is_empty());
}

#[test]
fn too_many_open_tiles_and_too_many_requests_are_refused() {
    let h = harness("agent-api", |_| {});
    let (ann, bob) = (h.ann(), h.login("bob", "bob-password-1"));
    h.svc.shared().tiles.set_max_sessions(2);
    assert_eq!(start(&h, &ann).0, 200);
    assert_eq!(start(&h, &bob).0, 200);
    h.svc.shared().tiles.register(def("other", wat_tile(1, "", &[]), &[]));
    let (s, v) = post(&h, &ann, "other", json!({"width": 50, "height": 50}));
    assert_eq!((s, v["error"]["code"].as_str()), (429, Some("busy")), "{v}");
    // the ones that are open keep working
    assert_eq!(post(&h, &ann, "counter", json!({"events": []})).0, 200);
    // the per-person rate limit of this route
    assert_eq!(tiles::EVENTS_PER_MINUTE, 600, "the documented limit");
    h.svc.shared().tiles.set_rate(20);
    let mut got_429 = None;
    for _ in 0..30 {
        let (s, v) = post(&h, &bob, "counter", json!({"events": []}));
        if s == 429 {
            got_429 = Some(v);
            break;
        }
    }
    let v = got_429.expect("the route has a limit of its own");
    assert!(v["error"]["message"].as_str().unwrap().contains("tile events"), "{v}");
    // and it is per person: ann is not held up by bob
    assert_eq!(post(&h, &ann, "counter", json!({"events": []})).0, 200);
}

#[test]
fn the_page_files_for_tiles_are_served_and_listed() {
    let h = harness("agent-api", |_| {});
    for p in ["/ui/tilerender.js", "/ui/tiles.js"] {
        let (s, _, r) = h.call("GET", p, None, None);
        assert_eq!(s, 200, "{p}");
        assert!(String::from_utf8_lossy(&r.body).contains("SPDX-License-Identifier"));
    }
}
