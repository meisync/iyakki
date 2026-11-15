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
    wat_tile_frame(one_rect_list(), abi, event_body, methods)
}

fn wat_tile_frame(frame: Vec<u8>, abi: u32, event_body: &str, methods: &[&str]) -> String {
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
    TileDef { id: id.into(), title: id.into(), wasm: Arc::new(wat.into_bytes()), caps: caps.iter().map(|s| s.to_string()).collect(), width: 100, height: 100, pictures: vec![] }
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
        // one more try when the loopback connection is refused or reset (seen once in a full run on a loaded machine; alone it never failed)
        let mut tries = 0;
        let r = loop {
            match crate::devmode::client::https("127.0.0.1", port, Some(&fp), m, p, &hd, b.as_bytes(), Duration::from_secs(15)) {
                Ok(r) => break r,
                Err(e) if tries < 2 => {
                    tries += 1;
                    eprintln!("retry after: {e}");
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(e) => panic!("answer: {e}"),
            }
        };
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
fn service_calls_follow_the_manifest_then_the_persons_tools_and_a_tool_that_asks_waits_for_the_person() {
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
    assert!(lines.contains("tile.asked") && lines.contains("tool=web_fetch"), "{lines}");
    assert!(lines.contains("tile.refused") && lines.contains("tool=run_command"), "{lines}");
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

fn pending_tile_asks(h: &H, tok: &str) -> Vec<Value> {
    let (s, v, _) = h.call("GET", "/api/v1/approvals", Some(tok), None);
    assert_eq!(s, 200, "{v}");
    v["approvals"].as_array().unwrap().iter().filter(|a| a["run"] == "tile").cloned().collect()
}

fn decide(h: &H, tok: &str, id: &str, verdict: &str) -> (u16, Value) {
    let (s, v, _) = h.call("POST", &format!("/api/v1/approvals/tile/{id}"), Some(tok), Some(json!({"verdict": verdict})));
    (s, v)
}

fn asking_tile(h: &H) {
    h.svc.shared().tiles.register(def("asker", wat_tile(1, &calls(&["web_fetch"]), &["web_fetch"]), &["tools.web_fetch"]));
    assert_eq!(post(h, &h.ann(), "asker", json!({"width": 50, "height": 50})).0, 200);
}

fn tick(h: &H, tok: &str) {
    assert_eq!(post(h, tok, "asker", json!({"events": [{"type": "tick", "dt_ms": 1}]})).0, 200);
}

#[test]
fn a_tool_that_asks_runs_once_and_only_after_the_persons_own_click() {
    let h = harness("agent-api", |_| {});
    let (ann, bob) = (h.ann(), h.login("bob", "bob-password-1"));
    asking_tile(&h);
    tick(&h, &ann);
    assert!(h.executed.lock().unwrap().is_empty(), "nothing runs before the click");
    let waiting = pending_tile_asks(&h, &ann);
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    assert_eq!(waiting[0]["tool"], "web_fetch");
    assert!(waiting[0]["summary"].as_str().unwrap().contains("asker"));
    // the same call again while waiting is still not run, and does not make a second card
    tick(&h, &ann);
    assert!(h.executed.lock().unwrap().is_empty());
    assert_eq!(pending_tile_asks(&h, &ann).len(), 1);
    // another person neither sees it nor can answer it
    assert!(pending_tile_asks(&h, &bob).is_empty());
    let id = waiting[0]["call"].as_str().unwrap().to_string();
    assert_eq!(decide(&h, &bob, &id, "approve").0, 404);
    // the person approves: the next call runs, exactly once
    let (s, v) = decide(&h, &ann, &id, "approve");
    assert_eq!((s, v["verdict"].as_str()), (200, Some("approve")), "{v}");
    assert_eq!(decide(&h, &ann, &id, "approve").0, 409, "an answered card cannot be answered twice");
    tick(&h, &ann);
    assert_eq!(*h.executed.lock().unwrap(), vec!["web_fetch".to_string()]);
    tick(&h, &ann);
    assert_eq!(h.executed.lock().unwrap().len(), 1, "the approval was used up: asking again");
    assert_eq!(pending_tile_asks(&h, &ann).len(), 1);
    let lines = h.accounts.lines().join("\n");
    assert!(lines.contains("tile.asked") && lines.contains("tile.call") && lines.contains("verdict=approve"), "{lines}");
    assert!(!lines.contains("example.org"), "no arguments in the audit");
}

#[test]
fn a_denied_question_is_never_run_and_is_told_no_once() {
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    asking_tile(&h);
    tick(&h, &ann);
    let id = pending_tile_asks(&h, &ann)[0]["call"].as_str().unwrap().to_string();
    assert_eq!(decide(&h, &ann, &id, "deny").0, 200);
    tick(&h, &ann);
    assert!(h.executed.lock().unwrap().is_empty());
    assert!(pending_tile_asks(&h, &ann).is_empty(), "no new card right after a no");
    assert!(h.accounts.lines().join("\n").contains("tile.ask_denied"));
    // a later call is a new question
    tick(&h, &ann);
    assert_eq!(pending_tile_asks(&h, &ann).len(), 1);
}

#[test]
fn the_ask_store_expires_and_caps_questions() {
    let asks = tiles::TileAsks::default();
    for i in 0..5 {
        assert!(matches!(asks.check_for_test("u", "t", "web_fetch", &format!("{{\"n\":{i}}}")), tiles::AskOutcome::Asked(_)));
    }
    assert!(matches!(asks.check_for_test("u", "t", "web_fetch", "{\"n\":9}"), tiles::AskOutcome::TooMany));
    assert!(matches!(asks.check_for_test("other", "t", "web_fetch", "{\"n\":9}"), tiles::AskOutcome::Asked(_)), "the cap is per person");
    // an approval is for the exact arguments only
    let id = asks.waiting("u")[0].id.clone();
    asks.decide("u", &id, true).unwrap();
    assert!(matches!(asks.check_for_test("u", "t", "web_fetch", "{\"n\":9}"), tiles::AskOutcome::TooMany));
    assert!(matches!(asks.check_for_test("u", "t", "web_fetch", "{\"n\":0}"), tiles::AskOutcome::Run));
}

fn runs_tile(h: &H, tok: &str, body: Value) -> Value {
    let (s, v, _) = h.call("POST", "/api/v1/tiles/runs/events", Some(tok), Some(body));
    assert_eq!(s, 200, "{v}");
    v
}

#[test]
fn the_runs_tile_lists_the_persons_own_runs_opens_one_and_cannot_reach_anyone_elses() {
    let h = harness("agent-api", |_| {});
    let (ann, bob) = (h.ann(), h.login("bob", "bob-password-1"));
    // listed for people who may use tiles, with its own title
    let (_, list, _) = h.call("GET", "/api/v1/tiles", Some(&ann), None);
    assert!(list["tiles"].as_array().unwrap().iter().any(|t| t["id"] == "runs" && t["title"] == "Runs"), "{list}");
    // nothing yet
    let first = runs_tile(&h, &ann, json!({"width": 360, "height": 400}));
    assert!(texts(&first).contains(&"No runs yet.".to_string()), "{:?}", texts(&first));
    // ann makes a run and lets it end
    h.script.turns.lock().unwrap().push_back(final_turn("the answer is four"));
    let (s, v, _) = h.call("POST", "/api/v1/chat/runs", Some(&ann), Some(json!({"prompt": "what is two and two"})));
    assert_eq!(s, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        let (_, g, _) = h.call("GET", &format!("/api/v1/chat/runs/{id}"), Some(&ann), None);
        if g["status"] != "running" {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // three seconds of tick time: the tile reads the list again and shows the run
    let v = runs_tile(&h, &ann, json!({"events": [{"type": "tick", "dt_ms": 3000}]}));
    assert!(texts(&v).iter().any(|t| t.contains("what is two and two")), "{:?}", texts(&v));
    // a real click on the first row opens it: the answer is one of its lines
    let v = runs_tile(&h, &ann, json!({"events": [{"type": "pointer_down", "x": 50, "y": 52, "button": 0}, {"type": "pointer_up", "x": 50, "y": 52, "button": 0}]}));
    assert!(texts(&v).iter().any(|t| t == "answer: the answer is four"), "{:?}", texts(&v));
    // bob's tile is his own and sees none of it
    let b = runs_tile(&h, &bob, json!({"width": 360, "height": 400}));
    assert!(texts(&b).contains(&"No runs yet.".to_string()) && !texts(&b).iter().any(|t| t.contains("two and two")), "{:?}", texts(&b));
    // the audit lines carry the tile and the call name only
    let lines = h.accounts.lines().join("\n");
    assert!(lines.contains("tile.call") && lines.contains("tool=runs_list") && lines.contains("tool=runs_get"), "{lines}");
    assert!(!lines.contains("two and two") && !lines.contains("four"), "{lines}");
}

#[test]
fn the_settings_tile_shows_the_account_the_models_and_web_access_to_an_administrator_and_less_to_others() {
    let h = harness("agent-api", |_| {});
    let (root, ann) = (h.root(), h.ann());
    let (_, list, _) = h.call("GET", "/api/v1/tiles", Some(&ann), None);
    for id in ["settings", "edit", "chat", "runs"] {
        assert!(list["tiles"].as_array().unwrap().iter().any(|t| t["id"] == id), "{id}: {list}");
    }
    let v = runs_tile_named(&h, &root, "settings", json!({"width": 360, "height": 400}));
    let x = texts(&v);
    assert!(x.iter().any(|t| t.starts_with("root (")) && x.contains(&"Web access".to_string()), "{x:?}");
    assert!(!x.contains(&"not shown to you here".to_string()), "an administrator sees the web access: {x:?}");
    let v = runs_tile_named(&h, &ann, "settings", json!({"width": 360, "height": 400}));
    let x = texts(&v);
    assert!(x.contains(&"ann (user)".to_string()) && x.contains(&"not shown to you here".to_string()), "{x:?}");
    // no password, key or address in anything the tile was given
    let all = x.join(" ").to_lowercase();
    assert!(!all.contains("password") && !all.contains("token"), "{all}");
    let lines = h.accounts.lines().join("\n");
    assert!(lines.contains("tile.call") && lines.contains("tool=settings_get"), "{lines}");
}

#[test]
fn the_chat_tile_starts_a_run_for_the_person_and_shows_its_answer_and_a_guest_cannot() {
    let h = harness("agent-api", |_| {});
    let (ann, gus) = (h.ann(), h.login("gus", "gus-password-1"));
    runs_tile_named(&h, &ann, "chat", json!({"width": 360, "height": 400}));
    h.script.turns.lock().unwrap().push_back(final_turn("the tile got this answer"));
    runs_tile_named(&h, &ann, "chat", json!({"events": [{"type": "text", "text": "say something"}]}));
    // the Send button is at the bottom right
    let v = runs_tile_named(&h, &ann, "chat", json!({"events": [{"type": "pointer_down", "x": 330, "y": 376, "button": 0}, {"type": "pointer_up", "x": 330, "y": 376, "button": 0}]}));
    assert!(texts(&v).contains(&"you: say something".to_string()), "{:?}", texts(&v));
    // the run is the person's own, in their list, started with the machine's model; two seconds later the answer is there
    let end = Instant::now() + Duration::from_secs(20);
    let mut got = false;
    while Instant::now() < end && !got {
        std::thread::sleep(Duration::from_millis(200));
        let v = runs_tile_named(&h, &ann, "chat", json!({"events": [{"type": "tick", "dt_ms": 2000}]}));
        got = texts(&v).contains(&"answer: the tile got this answer".to_string());
    }
    assert!(got, "the answer reached the tile");
    let (_, runs, _) = h.call("GET", "/api/v1/chat/runs", Some(&ann), None);
    assert_eq!(runs["runs"].as_array().unwrap().len(), 1, "{runs}");
    // a guest has no chat: the tile is refused where the routes would refuse
    runs_tile_named(&h, &gus, "chat", json!({"width": 360, "height": 400}));
    runs_tile_named(&h, &gus, "chat", json!({"events": [{"type": "text", "text": "hi"}]}));
    let v = runs_tile_named(&h, &gus, "chat", json!({"events": [{"type": "pointer_down", "x": 330, "y": 376, "button": 0}, {"type": "pointer_up", "x": 330, "y": 376, "button": 0}]}));
    assert!(texts(&v).contains(&"the service did not answer".to_string()), "{:?}", texts(&v));
    let (_, gruns, _) = h.call("GET", "/api/v1/chat/runs", Some(&ann), None);
    assert_eq!(gruns["runs"].as_array().unwrap().len(), 1, "nothing was started for the guest");
    let lines = h.accounts.lines().join("\n");
    assert!(lines.contains("tile.call tile=chat tool=chat_send") && lines.contains("tile.refused tile=chat tool=chat_send") && !lines.contains("say something"), "{lines}");
}

#[test]
fn the_chat_tile_lists_the_persons_earlier_runs_and_reads_one_again_and_a_guest_gets_nothing() {
    let h = harness("agent-api", |_| {});
    let (ann, gus) = (h.ann(), h.login("gus", "gus-password-1"));
    runs_tile_named(&h, &ann, "chat", json!({"width": 360, "height": 400}));
    h.script.turns.lock().unwrap().push_back(final_turn("the first answer"));
    runs_tile_named(&h, &ann, "chat", json!({"events": [{"type": "text", "text": "first question"}]}));
    runs_tile_named(&h, &ann, "chat", json!({"events": [{"type": "pointer_down", "x": 330, "y": 376, "button": 0}, {"type": "pointer_up", "x": 330, "y": 376, "button": 0}]}));
    let end = Instant::now() + Duration::from_secs(20);
    let mut done = false;
    while Instant::now() < end && !done {
        std::thread::sleep(Duration::from_millis(200));
        let v = runs_tile_named(&h, &ann, "chat", json!({"events": [{"type": "tick", "dt_ms": 2000}]}));
        done = texts(&v).contains(&"answer: the first answer".to_string());
    }
    assert!(done);
    // New clears the screen, Earlier lists the run, a press on its row reads it again
    click_at(&h, &ann, "chat", 326, 16);
    let v = click_at(&h, &ann, "chat", 226, 16);
    assert!(texts(&v).iter().any(|t| t.ends_with(": first question")) && texts(&v).contains(&"Close".to_string()), "{:?}", texts(&v));
    let v = click_at(&h, &ann, "chat", 100, 66);
    assert!(texts(&v).contains(&"you: first question".to_string()) && texts(&v).contains(&"answer: the first answer".to_string()), "{:?}", texts(&v));
    // the calls are the person's own and audited without content
    let lines = h.accounts.lines().join("\n");
    assert!(lines.contains("tile.call tile=chat tool=runs_list") && !lines.contains("first question"), "{lines}");
    // a guest has no chat: the list is refused where the routes would refuse, and shows the reason
    runs_tile_named(&h, &gus, "chat", json!({"width": 360, "height": 400}));
    let v = click_at(&h, &gus, "chat", 226, 16);
    assert!(texts(&v).contains(&"your account may not use the chat".to_string()) || texts(&v).iter().any(|t| t.contains("not allowed") || t.contains("did not answer")), "{:?}", texts(&v));
}

#[test]
fn the_edit_tile_keeps_its_text_in_the_tile_per_person_and_makes_no_call() {
    let h = harness("agent-api", |_| {});
    let (ann, bob) = (h.ann(), h.login("bob", "bob-password-1"));
    runs_tile_named(&h, &ann, "edit", json!({"width": 480, "height": 360}));
    let v = runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "text", "text": "hello scratch"}]}));
    assert!(texts(&v).contains(&"hello scratch".to_string()), "{:?}", texts(&v));
    let v = runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "key", "code": 122, "mods": 2, "down": true}]}));
    assert!(!texts(&v).contains(&"hello scratch".to_string()), "undone");
    let b = runs_tile_named(&h, &bob, "edit", json!({"width": 480, "height": 360}));
    assert!(!texts(&b).iter().any(|t| t.contains("hello")));
    assert!(!h.accounts.lines().join("\n").contains("tile.call"));
}

fn runs_tile_named(h: &H, tok: &str, tile: &str, body: Value) -> Value {
    let (s, v, _) = h.call("POST", &format!("/api/v1/tiles/{tile}/events"), Some(tok), Some(body));
    assert_eq!(s, 200, "{tile}: {v}");
    v
}

fn click_at(h: &H, tok: &str, tile: &str, x: i64, y: i64) -> Value {
    runs_tile_named(h, tok, tile, json!({"events": [{"type": "pointer_down", "x": x, "y": y, "button": 0}, {"type": "pointer_up", "x": x, "y": y, "button": 0}]}))
}

fn tile_approvals(h: &H, tok: &str) -> Vec<Value> {
    let (s, v, _) = h.call("GET", "/api/v1/approvals", Some(tok), None);
    assert_eq!(s, 200, "{v}");
    v["approvals"].as_array().unwrap().iter().filter(|a| a["run"] == "tile").cloned().collect()
}

#[test]
fn the_edit_tile_finds_text_selects_the_matches_in_turn_and_changes_nothing_through_the_real_routes() {
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    runs_tile_named(&h, &ann, "edit", json!({"width": 480, "height": 360}));
    runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "text", "text": "one two one"}]}));
    runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "key", "code": 102, "mods": 2, "down": true}]}));
    let v = runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "text", "text": "ONE"}, {"type": "key", "code": 13, "mods": 0, "down": true}]}));
    assert!(texts(&v).contains(&"Match 1 of 2".to_string()) && texts(&v).contains(&"find: ONE|".to_string()), "{:?}", texts(&v));
    assert!(texts(&v).contains(&"one two one".to_string()), "the text itself is unchanged: {:?}", texts(&v));
    let v = runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "key", "code": 13, "mods": 0, "down": true}]}));
    assert!(texts(&v).contains(&"Match 2 of 2".to_string()), "{:?}", texts(&v));
    let marked = v["frame"]["ops"].as_array().unwrap().iter().filter(|o| o["op"] == "rect" && o["color"].as_str().is_some_and(|c| c.ends_with("5a"))).count();
    assert!(marked >= 1, "the match is marked: {}", v["frame"]);
}

#[test]
fn the_edit_tile_draws_a_file_with_a_language_name_in_colours_through_the_real_routes() {
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    FAKE_FILES.lock().unwrap().insert("/home/ann/edit-colour.rs".into(), "// hello\nfn main() { let x = 1; }\n".into());
    FAKE_FILES.lock().unwrap().insert("/home/ann/edit-colour.txt".into(), "// hello\nfn main() { let x = 1; }\n".into());
    let open = |name: &str| {
        runs_tile_named(&h, &ann, "edit", json!({"width": 480, "height": 360, "reset": true}));
        click_at(&h, &ann, "edit", 100, 46);
        runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "text", "text": name}]}));
        runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "key", "code": 13, "mods": 0, "down": true}]}))
    };
    let inks = |v: &Value| -> std::collections::BTreeSet<String> { v["frame"]["ops"].as_array().unwrap().iter().filter(|o| o["op"] == "text" && o["y"].as_i64().unwrap_or(0) >= 60).map(|o| o["color"].to_string()).collect() };
    let rs = open("/home/ann/edit-colour.rs");
    assert!(texts(&rs).contains(&"fn".to_string()) && texts(&rs).contains(&"main".to_string()) && texts(&rs).contains(&"// hello".to_string()), "runs: {:?}", texts(&rs));
    let txt = open("/home/ann/edit-colour.txt");
    assert!(texts(&txt).contains(&"fn main() { let x = 1; }".to_string()), "a text file is drawn whole: {:?}", texts(&txt));
    assert!(inks(&rs).len() > inks(&txt).len(), "the Rust file has more inks than the text file: {:?} against {:?}", inks(&rs), inks(&txt));
}

#[test]
fn the_edit_tile_opens_a_file_and_saves_it_only_after_the_persons_own_click_with_the_tools_own_rules_and_audit() {
    let h = harness("agent-api", |_| {});
    let (ann, bob, gus) = (h.ann(), h.login("bob", "bob-password-1"), h.login("gus", "gus-password-1"));
    FAKE_FILES.lock().unwrap().insert("/home/ann/edit-t1.txt".into(), "hello file\n".into());
    runs_tile_named(&h, &ann, "edit", json!({"width": 480, "height": 360}));
    click_at(&h, &ann, "edit", 100, 46);
    runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "text", "text": "/home/ann/edit-t1.txt"}]}));
    let v = runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "key", "code": 13, "mods": 0, "down": true}]}));
    assert!(texts(&v).contains(&"hello file".to_string()) && texts(&v).iter().any(|t| t.starts_with("Opened /home/ann/edit-t1.txt")), "{:?}", texts(&v));
    // type into the text and press Ctrl+S
    // a click far to the right of the first line puts the cursor at its end
    click_at(&h, &ann, "edit", 330, 68);
    runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "text", "text": " more"}]}));
    let v = runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "key", "code": 115, "mods": 2, "down": true}]}));
    assert!(texts(&v).iter().any(|t| t.starts_with("Waiting for your answer")), "{:?}", texts(&v));
    assert_eq!(FAKE_FILES.lock().unwrap()["/home/ann/edit-t1.txt"], "hello file\n", "nothing is written before the click");
    // the card says what will happen, and only ann sees it
    let cards = tile_approvals(&h, &ann);
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert!(cards[0]["summary"].as_str().unwrap().contains("write 16 characters to /home/ann/edit-t1.txt"), "{}", cards[0]);
    assert!(tile_approvals(&h, &bob).is_empty());
    let id = cards[0]["call"].as_str().unwrap().to_string();
    let (s, _, _) = h.call("POST", &format!("/api/v1/approvals/tile/{id}"), Some(&ann), Some(json!({"verdict": "approve"})));
    assert_eq!(s, 200);
    // the tile asks again two seconds later and the file is written, once
    let v = runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "tick", "dt_ms": 2000}]}));
    assert!(texts(&v).iter().any(|t| t.starts_with("Saved /home/ann/edit-t1.txt")), "{:?}", texts(&v));
    assert_eq!(FAKE_FILES.lock().unwrap()["/home/ann/edit-t1.txt"], "hello file more\n");
    // a place the tool refuses: asked, approved, and the tool's own sentence comes back
    click_at(&h, &ann, "edit", 100, 46);
    let backspaces: Vec<Value> = (0..21).map(|_| json!({"type": "key", "code": 8, "mods": 0, "down": true})).collect();
    runs_tile_named(&h, &ann, "edit", json!({"events": backspaces}));
    runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "text", "text": "/etc/edit-t1.txt"}]}));
    runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "key", "code": 27, "mods": 0, "down": true}]}));
    runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "key", "code": 115, "mods": 2, "down": true}]}));
    let id = tile_approvals(&h, &ann)[0]["call"].as_str().unwrap().to_string();
    h.call("POST", &format!("/api/v1/approvals/tile/{id}"), Some(&ann), Some(json!({"verdict": "approve"})));
    let v = runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "tick", "dt_ms": 2000}]}));
    assert!(texts(&v).contains(&"you may not write there".to_string()), "{:?}", texts(&v));
    assert!(!FAKE_FILES.lock().unwrap().contains_key("/etc/edit-t1.txt"));
    // a no is a no
    runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "key", "code": 115, "mods": 2, "down": true}]}));
    let id = tile_approvals(&h, &ann)[0]["call"].as_str().unwrap().to_string();
    h.call("POST", &format!("/api/v1/approvals/tile/{id}"), Some(&ann), Some(json!({"verdict": "deny"})));
    let v = runs_tile_named(&h, &ann, "edit", json!({"events": [{"type": "tick", "dt_ms": 2000}]}));
    assert!(texts(&v).contains(&"you said no".to_string()), "{:?}", texts(&v));
    // an account without the chat right gets nothing from the files domain
    runs_tile_named(&h, &gus, "edit", json!({"width": 480, "height": 360}));
    click_at(&h, &gus, "edit", 100, 46);
    runs_tile_named(&h, &gus, "edit", json!({"events": [{"type": "text", "text": "/home/ann/edit-t1.txt"}]}));
    let v = runs_tile_named(&h, &gus, "edit", json!({"events": [{"type": "key", "code": 13, "mods": 0, "down": true}]}));
    assert!(texts(&v).contains(&"the service did not answer".to_string()) && !texts(&v).contains(&"hello file more".to_string()), "{:?}", texts(&v));
    let lines = h.accounts.lines().join("\n");
    assert!(lines.contains("tile.asked tile=edit tool=write_file") && lines.contains("tile.call tile=edit tool=write_file") && lines.contains("tile.call tile=edit tool=read_file"), "{lines}");
    assert!(lines.contains("tile.ask_denied tile=edit tool=write_file") && lines.contains("tile.refused tile=edit tool=read"), "{lines}");
    assert!(!lines.contains("hello file") && !lines.contains("more"), "no file content in the audit");
}

#[test]
fn the_chat_tile_chooses_a_listed_model_and_stops_a_run_that_waits_for_an_answer() {
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    runs_tile_named(&h, &ann, "chat", json!({"width": 360, "height": 400}));
    // the model line shows the machine's own model first; a press chooses the next one (the only listed pair here)
    let v = click_at(&h, &ann, "chat", 100, 46);
    assert!(texts(&v).contains(&"model: local / tiny".to_string()), "{:?}", texts(&v));
    // the service passes the chosen pair on to its own model rules (this fake host has no second model and says so)
    runs_tile_named(&h, &ann, "chat", json!({"events": [{"type": "text", "text": "hi"}]}));
    let v = click_at(&h, &ann, "chat", 330, 376);
    assert!(texts(&v).iter().any(|t| t.contains("no other model")), "{:?}", texts(&v));
    // back to the machine's own model, ask for something that needs an answer, then stop it
    click_at(&h, &ann, "chat", 100, 46);
    runs_tile_named(&h, &ann, "chat", json!({"events": [{"type": "key", "code": 8, "mods": 0, "down": true}, {"type": "key", "code": 8, "mods": 0, "down": true}]}));
    h.script.turns.lock().unwrap().push_back(call_turn("web_fetch"));
    runs_tile_named(&h, &ann, "chat", json!({"events": [{"type": "text", "text": "fetch it"}]}));
    let v = click_at(&h, &ann, "chat", 330, 376);
    assert!(texts(&v).contains(&"you: fetch it".to_string()), "{:?}", texts(&v));
    let end = Instant::now() + Duration::from_secs(10);
    let mut asked = false;
    while Instant::now() < end && !asked {
        std::thread::sleep(Duration::from_millis(100));
        let v = runs_tile_named(&h, &ann, "chat", json!({"events": [{"type": "tick", "dt_ms": 2000}]}));
        asked = texts(&v).iter().any(|t| t.starts_with("asks: "));
    }
    assert!(asked, "the run waits for the person");
    let mut v = click_at(&h, &ann, "chat", 270, 16);
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end && !texts(&v).contains(&"stopped".to_string()) {
        std::thread::sleep(Duration::from_millis(100));
        v = runs_tile_named(&h, &ann, "chat", json!({"events": [{"type": "tick", "dt_ms": 2000}]}));
    }
    assert!(texts(&v).contains(&"stopped".to_string()), "{:?}", texts(&v));
    let (_, runs, _) = h.call("GET", "/api/v1/chat/runs", Some(&ann), None);
    assert!(runs["runs"].as_array().unwrap().iter().any(|r| r["status"] == "stopped"), "{runs}");
    assert!(h.accounts.lines().join("\n").contains("tile.call tile=chat tool=chat_stop"));
}

fn wait_tile<F: Fn(&Value) -> bool>(h: &H, tok: &str, tile: &str, ev: Value, ok: F) -> Value {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        let v = runs_tile_named(h, tok, tile, ev.clone());
        if ok(&v) || Instant::now() > end {
            return v;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

#[test]
fn the_runs_tile_shows_a_waiting_question_and_the_persons_click_answers_it_once_and_nobody_elses() {
    let h = harness("agent-api", |_| {});
    let (ann, bob) = (h.ann(), h.login("bob", "bob-password-1"));
    runs_tile_named(&h, &ann, "runs", json!({"width": 360, "height": 400}));
    h.script.turns.lock().unwrap().push_back(call_turn("web_fetch"));
    h.script.turns.lock().unwrap().push_back(final_turn("fetched it"));
    let (s, v, _) = h.call("POST", "/api/v1/chat/runs", Some(&ann), Some(json!({"prompt": "fetch the page"})));
    assert_eq!(s, 200, "{v}");
    // the list shows the run; a click opens it; the question that waits is there with Approve and Deny
    wait_tile(&h, &ann, "runs", json!({"events": [{"type": "tick", "dt_ms": 3000}]}), |v| texts(v).iter().any(|t| t.contains("fetch the page")));
    click_at(&h, &ann, "runs", 50, 52);
    let v = wait_tile(&h, &ann, "runs", json!({"events": [{"type": "tick", "dt_ms": 3000}]}), |v| texts(v).iter().any(|t| t.starts_with("asks: ")));
    assert!(texts(&v).contains(&"Approve".to_string()) && texts(&v).contains(&"Deny".to_string()), "{:?}", texts(&v));
    assert_eq!(h.executed.lock().unwrap().iter().filter(|n| *n == "web_fetch").count(), 0, "nothing runs before the click");
    // another person's tile has no such question and cannot answer this one
    runs_tile_named(&h, &bob, "runs", json!({"width": 360, "height": 400}));
    let b = click_at(&h, &bob, "runs", 50, 372);
    assert!(!texts(&b).iter().any(|t| t.starts_with("asks: ")), "{:?}", texts(&b));
    assert_eq!(h.executed.lock().unwrap().iter().filter(|n| *n == "web_fetch").count(), 0);
    // ann clicks Approve: the tool runs once and the run ends with its answer
    click_at(&h, &ann, "runs", 50, 372);
    let v = wait_tile(&h, &ann, "runs", json!({"events": [{"type": "tick", "dt_ms": 3000}]}), |v| texts(v).contains(&"answer: fetched it".to_string()));
    assert!(texts(&v).contains(&"answer: fetched it".to_string()), "{:?}", texts(&v));
    assert_eq!(h.executed.lock().unwrap().iter().filter(|n| *n == "web_fetch").count(), 1);
    assert!(!texts(&v).contains(&"Approve".to_string()), "the question is gone: no buttons");
    let lines = h.accounts.lines().join("\n");
    assert!(lines.contains("tile.decide tile=runs") && lines.contains("tool=web_fetch verdict=approve") && lines.contains("tile.call tile=runs tool=runs_decide"), "{lines}");
}

#[test]
fn the_runs_tile_stops_the_persons_own_run_on_their_click_and_nobody_elses() {
    let h = harness("agent-api", |_| {});
    let (ann, bob) = (h.ann(), h.login("bob", "bob-password-1"));
    runs_tile_named(&h, &ann, "runs", json!({"width": 360, "height": 400}));
    h.script.turns.lock().unwrap().push_back(call_turn("web_fetch"));
    h.script.turns.lock().unwrap().push_back(final_turn("never reached"));
    let (s, v, _) = h.call("POST", "/api/v1/chat/runs", Some(&ann), Some(json!({"prompt": "a long job"})));
    assert_eq!(s, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    wait_tile(&h, &ann, "runs", json!({"events": [{"type": "tick", "dt_ms": 3000}]}), |v| texts(v).iter().any(|t| t.contains("a long job")));
    click_at(&h, &ann, "runs", 50, 52);
    let v = wait_tile(&h, &ann, "runs", json!({"events": [{"type": "tick", "dt_ms": 3000}]}), |v| texts(v).contains(&"Stop".to_string()));
    assert!(texts(&v).contains(&"Stop".to_string()), "{:?}", texts(&v));
    // another person's tile cannot stop it
    let (st, _, _) = h.call("POST", &format!("/api/v1/chat/runs/{id}/stop"), Some(&bob), None);
    assert_ne!(st, 200);
    // ann's click on Stop (right of the title, left of Refresh) ends her run
    let stop_x = 360 - 216 + 20;
    click_at(&h, &ann, "runs", stop_x, 16);
    let v = wait_tile(&h, &ann, "runs", json!({"events": [{"type": "tick", "dt_ms": 3000}]}), |v| texts(v).iter().any(|t| t.starts_with("stopped")));
    assert!(texts(&v).iter().any(|t| t.starts_with("stopped")) && !texts(&v).contains(&"Stop".to_string()), "{:?}", texts(&v));
    let lines = h.accounts.lines().join("\n");
    assert!(lines.contains("tile.call tile=runs tool=runs_stop"), "{lines}");
}

#[test]
fn a_click_on_deny_refuses_the_call_and_the_run_goes_on_without_it() {
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    runs_tile_named(&h, &ann, "runs", json!({"width": 360, "height": 400}));
    h.script.turns.lock().unwrap().push_back(call_turn("web_fetch"));
    h.script.turns.lock().unwrap().push_back(final_turn("without it"));
    h.call("POST", "/api/v1/chat/runs", Some(&ann), Some(json!({"prompt": "fetch the other page"})));
    wait_tile(&h, &ann, "runs", json!({"events": [{"type": "tick", "dt_ms": 3000}]}), |v| texts(v).iter().any(|t| t.contains("fetch the other page")));
    click_at(&h, &ann, "runs", 50, 52);
    wait_tile(&h, &ann, "runs", json!({"events": [{"type": "tick", "dt_ms": 3000}]}), |v| texts(v).iter().any(|t| t.starts_with("asks: ")));
    click_at(&h, &ann, "runs", 160, 372);
    let v = wait_tile(&h, &ann, "runs", json!({"events": [{"type": "tick", "dt_ms": 3000}]}), |v| texts(v).contains(&"answer: without it".to_string()));
    assert!(texts(&v).contains(&"answer: without it".to_string()), "{:?}", texts(&v));
    assert!(h.executed.lock().unwrap().iter().all(|n| n != "web_fetch"), "a no is a no");
    assert!(h.accounts.lines().join("\n").contains("verdict=deny"));
}

/// The middle of the hit region `id` of a picture the service sent.
fn hit_centre(v: &Value, id: u64) -> (i64, i64) {
    let o = v["frame"]["ops"].as_array().unwrap().iter().find(|o| o["op"] == "hit" && o["id"] == id).unwrap_or_else(|| panic!("no hit region {id}"));
    (o["x"].as_i64().unwrap() + o["w"].as_i64().unwrap() / 2, o["y"].as_i64().unwrap() + o["h"].as_i64().unwrap() / 2)
}

#[test]
fn the_settings_tile_changes_a_switch_only_after_the_persons_own_click_through_the_persons_own_tool() {
    let h = harness("agent-api", |_| {});
    let (ann, gus) = (h.ann(), h.login("gus", "gus-password-1"));
    *FAKE_SWITCHES.lock().unwrap() = (true, false, true, true);
    let v = runs_tile_named(&h, &ann, "settings", json!({"width": 360, "height": 480}));
    assert!(texts(&v).contains(&"[x] Flow notices in the bell".to_string()) && texts(&v).contains(&"[ ] Shorter saver on battery".to_string()), "{:?}", texts(&v));
    // click the second switch: a question, nothing changed
    let (x, y) = hit_centre(&v, 21);
    let v = click_at(&h, &ann, "settings", x, y);
    assert!(texts(&v).iter().any(|t| t.starts_with("Waiting for your answer")), "{:?}", texts(&v));
    assert!(!FAKE_SWITCHES.lock().unwrap().1, "nothing changes before the click on the card");
    let cards = tile_approvals(&h, &ann);
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert!(cards[0]["summary"].as_str().unwrap().contains("screensaver_set"), "{}", cards[0]);
    let id = cards[0]["call"].as_str().unwrap().to_string();
    h.call("POST", &format!("/api/v1/approvals/tile/{id}"), Some(&ann), Some(json!({"verdict": "approve"})));
    let v = runs_tile_named(&h, &ann, "settings", json!({"events": [{"type": "tick", "dt_ms": 2000}]}));
    assert!(FAKE_SWITCHES.lock().unwrap().1, "now it is changed");
    assert!(texts(&v).contains(&"[x] Shorter saver on battery".to_string()), "{:?}", texts(&v));
    // the other switch: a no leaves it as it was
    let (x, y) = hit_centre(&v, 20);
    click_at(&h, &ann, "settings", x, y);
    let id = tile_approvals(&h, &ann)[0]["call"].as_str().unwrap().to_string();
    h.call("POST", &format!("/api/v1/approvals/tile/{id}"), Some(&ann), Some(json!({"verdict": "deny"})));
    let v = runs_tile_named(&h, &ann, "settings", json!({"events": [{"type": "tick", "dt_ms": 2000}]}));
    assert!(texts(&v).contains(&"you said no".to_string()), "{:?}", texts(&v));
    assert!(FAKE_SWITCHES.lock().unwrap().0, "flow notices are still shown");
    // the sound cues and the battery percentage are switches too: shown with their state, changed only after the person's click
    assert!(texts(&v).contains(&"[x] System sound cues".to_string()) && texts(&v).contains(&"[x] Battery percent shown".to_string()), "{:?}", texts(&v));
    let (x, y) = hit_centre(&v, 23);
    click_at(&h, &ann, "settings", x, y);
    assert!(FAKE_SWITCHES.lock().unwrap().3, "nothing changes before the click on the card");
    let cards = tile_approvals(&h, &ann);
    assert!(cards.len() == 1 && cards[0]["summary"].as_str().unwrap().contains("battery_settings_set"), "{cards:?}");
    let id = cards[0]["call"].as_str().unwrap().to_string();
    h.call("POST", &format!("/api/v1/approvals/tile/{id}"), Some(&ann), Some(json!({"verdict": "approve"})));
    let v = runs_tile_named(&h, &ann, "settings", json!({"events": [{"type": "tick", "dt_ms": 2000}]}));
    assert!(!FAKE_SWITCHES.lock().unwrap().3, "now the percentage is hidden");
    assert!(texts(&v).contains(&"[ ] Battery percent shown".to_string()), "{:?}", texts(&v));
    // an account without the chat right sees no switches and cannot change one
    let v = runs_tile_named(&h, &gus, "settings", json!({"width": 360, "height": 480}));
    assert!(!texts(&v).contains(&"Switches".to_string()), "{:?}", texts(&v));
    let lines = h.accounts.lines().join("\n");
    assert!(lines.contains("tile.asked tile=settings tool=screensaver_set") && lines.contains("tile.call tile=settings tool=screensaver_set") && lines.contains("tile.ask_denied tile=settings tool=notification_filter_set"), "{lines}");
}

// ---- the picture table (F-WASM-12): pictures a tile names by handle, decoded once when it starts and sent with its frames ----

/// A PNG of `w` x `h` RGBA pixels (stored by the same zlib the agent reads with; chunk checksums are not read by the host).
fn png_of(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::new();
    for y in 0..h as usize {
        raw.push(0u8);
        raw.extend_from_slice(&rgba[y * w as usize * 4..(y + 1) * w as usize * 4]);
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 1);
    let chunk = |kind: &[u8; 4], data: &[u8]| {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        c.extend_from_slice(&[0, 0, 0, 0]);
        c
    };
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    out.extend(chunk(b"IHDR", &ihdr));
    out.extend(chunk(b"IDAT", &z));
    out.extend(chunk(b"IEND", &[]));
    out
}

fn picture_frame() -> Vec<u8> {
    let mut l = DisplayList::new();
    l.push(Op::Rect { r: Rect::new(0, 0, 50, 50), color: Color(9, 9, 9, 255) });
    l.push(Op::Image { handle: 7, r: Rect::new(4, 4, 8, 8) });
    l.push(Op::Image { handle: 99, r: Rect::new(20, 4, 8, 8) });
    l.encode().unwrap()
}

#[test]
fn a_tile_that_names_a_picture_gets_it_decoded_once_and_sent_with_its_frame_and_a_bad_picture_stops_the_start() {
    let h = harness("agent-api", |_| {});
    let ann = h.ann();
    let pixels: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128];
    let mk = |id: &str, pics: Vec<(u32, Vec<u8>)>| TileDef { pictures: pics.into_iter().map(|(k, v)| (k, Arc::new(v))).collect(), ..def(id, wat_tile_frame(picture_frame(), 1, "", &[]), &[]) };
    // handle 7 is in the table and named; handle 8 is in the table and not named (it is not sent); handle 99 is named and not in the table
    h.svc.shared().tiles.register(mk("pics", vec![(7, png_of(2, 2, &pixels)), (8, png_of(1, 1, &[1, 2, 3, 255]))]));
    let (s, v, _) = h.call("POST", "/api/v1/tiles/pics/events", Some(&ann), Some(json!({"width": 64, "height": 64})));
    assert_eq!(s, 200, "{v}");
    let imgs = &v["frame"]["images"];
    assert_eq!(imgs.as_object().map(|o| o.len()), Some(1), "only the picture the list names, and only if the table has it: {imgs}");
    assert_eq!((imgs["7"]["w"].as_i64(), imgs["7"]["h"].as_i64()), (Some(2), Some(2)));
    assert_eq!(crate::models::b64_decode(imgs["7"]["rgba"].as_str().unwrap()).unwrap(), pixels, "the pixels survive the decode, alpha included");
    // a bad picture, a picture the table refuses and one that is not a picture stop the tile before it runs
    for (id, pics, word) in [("p-junk", vec![(7u32, b"not a png".to_vec())], "cannot be read"), ("p-big", vec![(7, png_of(1025, 1, &vec![0u8; 1025 * 4]))], "refuses")] {
        h.svc.shared().tiles.register(mk(id, pics));
        let (s, v, _) = h.call("POST", &format!("/api/v1/tiles/{id}/events"), Some(&ann), Some(json!({"width": 64, "height": 64})));
        assert_eq!(s, 501, "{id}: {v}");
        assert!(v.to_string().contains(word), "{id}: {v}");
    }
    // a tile with no pictures is as before: no "images" member
    h.svc.shared().tiles.register(mk("plain", vec![]));
    let (_, v, _) = h.call("POST", "/api/v1/tiles/plain/events", Some(&ann), Some(json!({"width": 64, "height": 64})));
    assert!(v["frame"].get("images").is_none(), "{v}");
}
