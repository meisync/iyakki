//! Tests of SDK 1.2 (docs/development/SDK-BRIDGE-V2.md): discovery and the compatibility rules (an old-style call, a future-style call, an older host),
//! the capability and approval model of every new namespace, the arguments and limits of each call, the money limits, and the page API on the
//! real engine through the headless harness. The backends are fakes; the bridge side (arguments, scope, approval, limits, errors, audit) is the real one.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use iyakki_core_api::sdkbridge::{self as bridge, Version};
use iyakki_js::modules::Bridge;
use serde_json::{json, Value};

use super::sdkv2_backends::{Backends, FakeBackend};
use super::sdkv2_money;
use super::tile::*;
use super::tilesdk::*;
use crate::usage::UsageLog;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-sdkv2-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn info() -> AppInfo {
    AppInfo { id: "t".into(), name: "T".into(), version: "1.0.0".into(), publisher: "acme".into(), trust: "Community".into(), dev: false }
}

struct Rig {
    host: TileHost,
    sdk: Arc<TileSdk>,
    out: Arc<Mutex<Outbox>>,
    be: Arc<FakeBackend>,
    dir: PathBuf,
}

/// Fake backends and a state file in a folder, like the running system (cards wait for the person; a test answers with [`Rig::answer`]).
fn rig(tag: &str, caps: Caps) -> Rig {
    rig_level(tag, caps, Version::CURRENT)
}

fn rig_level(tag: &str, caps: Caps, level: Version) -> Rig {
    let dir = tmp(tag);
    let out = Arc::new(Mutex::new(Outbox::default()));
    let (b, be) = Backends::fake();
    let file = dir.join("appdata").join("tile-sdk").join("tester").join("t.json");
    let sdk = Arc::new(
        TileSdk::new("t", "tester", Arc::new(FakeLlm::default()), Arc::new(FakeSpeech::default()), Arc::new(LogMeter(UsageLog::open(&dir.join("usage.jsonl")))), out.clone(), Some(&file)).with_backends(b).with_level(level),
    );
    let host = TileHost::new(info(), caps, TileStorage::memory(), out.clone()).with_sdk(sdk.clone());
    Rig { host, sdk, out, be, dir }
}

impl Rig {
    fn call(&mut self, op: &str, a: Value) -> Result<Value, (String, String)> {
        self.host.call(op, &a).map_err(|e| SdkError::decode(&e).unwrap_or_else(|| ("plain".into(), e)))
    }

    fn ok(&mut self, op: &str, a: Value) -> Value {
        self.call(op, a).unwrap_or_else(|e| panic!("{op}: {e:?}"))
    }

    fn code(&mut self, op: &str, a: Value) -> String {
        match self.call(op, a) {
            Err((c, _)) => c,
            Ok(v) => panic!("{op} should fail, gave {v}"),
        }
    }

    /// The call returned `{pending}`: the card id.
    fn card(&mut self, op: &str, a: Value) -> u32 {
        let v = self.ok(op, a);
        v["pending"].as_u64().unwrap_or_else(|| panic!("{op} should ask the person: {v}")) as u32
    }

    /// The person answers; what the promise gets.
    fn answer(&mut self, id: u32, allow: bool) -> Result<Value, (String, String)> {
        assert!(self.sdk.answer(id, allow), "no card {id}");
        let d = {
            let mut o = self.out.lock().unwrap();
            let i = o.done.iter().position(|d| d.id == id).expect("the promise settles");
            o.done.remove(i)
        };
        if d.ok {
            Ok(serde_json::from_str(&d.value).unwrap())
        } else {
            Err(SdkError::decode(&d.value).unwrap())
        }
    }

    fn approved(&mut self, op: &str, a: Value) -> Result<Value, (String, String)> {
        let id = self.card(op, a);
        self.answer(id, true)
    }

    fn requests(&self) -> Vec<Request> {
        self.out.lock().unwrap().requests.clone()
    }

    fn audits(&self) -> Vec<String> {
        self.out.lock().unwrap().audit.iter().map(|(a, d)| format!("{a} {d}")).collect()
    }
}

fn all() -> Caps {
    Caps::all_v2()
}

// ------------------------------------------------------------------------------------------------ discovery and compatibility

#[test]
fn discovery_tells_what_the_host_knows_and_what_the_app_may_use_even_on_a_build_without_the_sdk_part() {
    let out = Arc::new(Mutex::new(Outbox::default()));
    let mut bare = TileHost::new(info(), Caps::default().with("markdown", json!(true)), TileStorage::memory(), out);
    let l = bare.call("sdk.capabilities", &json!({})).unwrap();
    assert_eq!(l["sdk"], bridge::SDK_VERSION);
    assert!(bare.call("sdk.has", &json!({"name": "markdown.render"})).unwrap() == json!(true));
    assert!(bare.call("sdk.has", &json!({"name": "money"})).unwrap() == json!(false), "known, but not declared");
    assert!(bare.call("sdk.has", &json!({"name": "hologram"})).unwrap() == json!(false));
    assert!(bare.call("sdk.has", &json!({"name": 5})).is_err());
    // markdown.render needs no backend
    let r = bare.call("markdown.render", &json!({"text": "# Hi\n[a](https://x.example.com)"})).unwrap();
    assert_eq!(r["links"][0]["url"], "https://x.example.com");
    // anything that needs the machine says so with a code
    let e = bare.call("markdown.view", &json!({"text": "x"})).unwrap_err();
    assert_eq!(SdkError::decode(&e).unwrap().0, "Unavailable");
}

#[test]
fn the_page_version_and_the_manifest_agree_and_the_install_text_names_every_capability() {
    assert_eq!(bridge::SDK_VERSION, "1.2");
    let c = all();
    let perms = c.permissions();
    for k in ["markdown", "embed=markdown,browser,media,dash", "embed_hosts=docs.example.com", "dash=prom", "flows=backup", "k3s=manage", "media", "audio", "microphone", "money=USD"] {
        assert!(perms.contains(&format!("tile:{k}")), "{k}: {perms:?}");
    }
    assert_eq!(Caps::from_permissions(&perms).v2, c.v2, "the permission strings carry the parameters back");
    for p in &perms {
        let parsed = super::manifest::Permission::parse(p).unwrap_or_else(|e| panic!("{p}: {e}"));
        assert!(!parsed.words("u").is_empty(), "{p}");
        assert_eq!(parsed.canonical(), *p);
    }
    for p in ["tile:money=USD", "tile:microphone", "tile:k3s=read"] {
        assert!(super::manifest::Permission::parse(p).unwrap().sensitive(), "{p}");
    }
    assert!(!super::manifest::Permission::parse("tile:markdown").unwrap().sensitive());
    assert!(super::manifest::Permission::parse("tile:money").is_err(), "money needs its currency");
    assert!(super::manifest::Permission::parse("tile:hologram").is_err());
}

#[test]
fn app_json_declares_the_new_capabilities_with_their_parameters_and_refuses_what_it_cannot_honour() {
    let ok = |caps: Value, extra: Value| {
        let mut m = json!({"id": "demo", "name": "Demo", "capabilities": caps});
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            m[k] = v;
        }
        TileManifest::parse(&m.to_string())
    };
    let m = ok(json!({"markdown": true, "flows": ["backup", "report"], "k3s": "read", "money": "EUR", "embed": ["markdown"], "audio": false}), json!({"sdk": "1.2"})).unwrap();
    assert_eq!(m.caps.v2["flows"], json!(["backup", "report"]));
    assert!(!m.caps.v2.contains_key("audio"), "false switches it off");
    let back = TileManifest::parse(&m.to_json().to_string()).unwrap();
    assert_eq!(back.caps, m.caps, "written back as read");
    for (caps, why) in [(json!({"money": "usd"}), "currency"), (json!({"k3s": "root"}), "one of"), (json!({"flows": "all"}), "list"), (json!({"hologram": true}), "unknown capability"), (json!({"embed": ["flash"]}), "kind")] {
        let e = ok(caps.clone(), json!({})).unwrap_err();
        assert!(e.contains(why), "{caps}: {e}");
    }
    // an app written for a newer SDK says so, and says what it can do without
    let e = ok(json!({}), json!({"sdk": "1.9"})).unwrap_err();
    assert!(e.contains("update IYAKKI"), "{e}");
    assert!(ok(json!({}), json!({"sdk": "soon"})).is_err());
    let m = ok(json!({"hologram": {"depth": 3}, "markdown": true}), json!({"requires": {"capabilities_optional": ["hologram"]}})).unwrap();
    assert!(!m.caps.holds("hologram") && m.caps.holds("markdown"), "an optional capability of a newer IYAKKI is ignored, not granted");
    assert_eq!(m.to_json()["capabilities"]["hologram"], json!({"depth": 3}), "and kept when the manifest is written back");
    assert!(!m.caps.permissions().iter().any(|p| p.contains("hologram")));
}

fn replay(r: &mut Rig, steps: &Value) {
    for (i, s) in steps.as_array().unwrap().iter().enumerate() {
        let (op, args) = (s["op"].as_str().unwrap(), s["args"].clone());
        let got = r.call(op, args.clone());
        let what = format!("step {i}: {op} {args}");
        if s["ok"] == true {
            let v = got.unwrap_or_else(|e| panic!("{what}: {e:?}"));
            if let Some(eq) = s.get("equals") {
                assert_eq!(&v, eq, "{what}");
            }
            for k in s["has_keys"].as_array().into_iter().flatten() {
                assert!(v.get(k.as_str().unwrap()).is_some(), "{what}: missing {k}");
            }
            if s["is_array"] == true {
                assert!(v.is_array(), "{what}");
            }
        } else {
            let (c, m) = got.err().unwrap_or_else(|| panic!("{what} should be refused"));
            if let Some(code) = s["error_code"].as_str() {
                assert_eq!(c, code, "{what}: {m}");
            }
            if let Some(w) = s["error_contains"].as_str() {
                assert!(m.contains(w), "{what}: {m}");
            }
        }
    }
}

#[test]
fn a_recorded_old_style_call_still_answers_as_it_did_and_a_future_style_call_degrades_with_a_structured_answer() {
    let t: Value = serde_json::from_str(include_str!("../../../../tests/compat/alpha3/sdk-bridge.json")).unwrap();
    let mut caps = Caps::default();
    caps.storage = t["caps"]["storage"] == true;
    caps.notifications = t["caps"]["notifications"] == true;
    caps.llm = t["caps"]["llm"] == true;
    caps.speech = t["caps"]["speech"] == true;
    let mut r = rig("compat", caps);
    replay(&mut r, &t["old"]);
    replay(&mut r, &t["future"]);
    // the unknown answer carries the facts an app can test
    let e = r.host.call("hologram.show", &json!({})).unwrap_err();
    let body: Value = serde_json::from_str(e.strip_prefix(ERROR_PREFIX).unwrap()).unwrap();
    assert_eq!((body["code"].as_str(), body["namespace"].as_str(), body["sdk"].as_str(), body["op"].as_str()), (Some("Unsupported"), Some("hologram"), Some("1.2"), Some("hologram.show")));
    assert!(body["message"].as_str().unwrap().contains("not available in this version"));
}

#[test]
fn an_older_host_keeps_what_it_had_and_answers_unsupported_for_the_newer_calls_so_a_newer_app_can_degrade() {
    let mut r = rig_level("older", all(), Version { major: 1, minor: 1 });
    // what SDK 1.1 had works
    r.ok("storage.get", json!({"k": "a"}));
    r.ok("models.list", json!({}));
    assert_eq!(r.ok("sdk.has", json!({"name": "speech.say"})), json!(true));
    // what came later is not there, whatever the app declared
    for (op, a) in [("markdown.render", json!({"text": "x"})), ("money.limits", json!({})), ("speech.listen", json!({})), ("embed.list", json!({})), ("k3s.status", json!({}))] {
        assert_eq!(r.code(op, a), "Unsupported", "{op}");
    }
    assert_eq!(r.ok("sdk.has", json!({"name": "markdown"})), json!(false));
    assert_eq!(r.ok("sdk.has", json!({"name": "speech.listen"})), json!(false));
    let l = r.ok("sdk.capabilities", json!({}));
    assert_eq!(l["sdk"], "1.1");
    assert!(l["namespaces"].as_array().unwrap().iter().all(|n| n["name"] != "money"));
    // and the newer host with the same app: all there
    let mut now = rig("newer", all());
    assert_eq!(now.ok("sdk.has", json!({"name": "markdown"})), json!(true));
    assert_eq!(now.ok("sdk.call", json!({"op": "markdown.render", "args": {"text": "x"}}))["lines"], 1, "the generic door reaches the same call");
}

#[test]
fn every_new_call_refuses_an_app_that_did_not_declare_its_capability_and_says_nothing_about_other_apps() {
    let mut r = rig("nocap", Caps::default());
    for n in bridge::NAMESPACES.iter().filter(|n| !n.legacy) {
        for o in n.ops {
            let full = format!("{}.{}", n.name, o.name);
            let Some(cap) = bridge::capability_of(n, o) else { continue };
            if full == "speech.say" || full == "speech.stop" {
                continue; // the original arms, tested in tilesdk_tests
            }
            let (c, m) = r.call(&full, json!({})).expect_err(&full);
            assert_eq!(c, "NoCapability", "{full}: {m}");
            assert!(m.contains("did not ask for") && m.contains(cap), "{full}: {m}");
        }
    }
    assert!(!r.out.lock().unwrap().refused.is_empty(), "refusals are kept for the info panel");
    // the microphone is not the voice, the voice is not the microphone
    let mut voice_only = rig("voice", Caps { speech: true, ..Caps::default() });
    assert_eq!(voice_only.code("speech.listen", json!({})), "NoCapability");
    let mut mic_only = rig("mic", Caps::default().with("microphone", json!(true)));
    assert_ne!(mic_only.code("speech.say", json!({"text": "x"})), "Unavailable");
}

trait CapsWith {
    fn with(self, k: &str, v: Value) -> Caps;
}

impl CapsWith for Caps {
    fn with(mut self, k: &str, v: Value) -> Caps {
        self.v2.insert(k.into(), v);
        self
    }
}

#[test]
fn unknown_arguments_of_the_new_calls_are_ignored_and_reported_and_the_wrong_shapes_are_refused_by_name() {
    let mut r = rig("lenient", all());
    let v = r.ok("markdown.render", json!({"text": "hi", "colour": "red", "future": 1}));
    assert_eq!(v["ignored"], json!(["colour", "future"]));
    assert!(r.ok("markdown.render", json!({"text": "hi"})).get("ignored").is_none(), "nothing to report: no field");
    for (a, field) in [(json!({}), "text"), (json!({"text": 5}), "text"), (json!({"text": "x", "width": "wide"}), "width"), (json!({"text": "x", "width": 99999}), "width")] {
        let (c, m) = r.call("markdown.render", a.clone()).unwrap_err();
        assert_eq!(c, "BadArgument", "{a}");
        assert!(m.starts_with(&format!("{field}:")), "{a}: {m}");
    }
    assert_eq!(r.code("markdown.render", json!("not an object")), "BadArgument");
    assert_eq!(r.code("markdown.render", json!({"text": "x".repeat(bridge::markdown::MAX_TEXT + 1)})), "BadArgument");
    // the generic door cannot be turned on itself or on the test hooks
    assert_eq!(r.code("sdk.call", json!({"op": "sdk.call", "args": {}})), "Unsupported");
    assert_eq!(r.code("sdk.call", json!({"op": "__test.llm.reply", "args": {}})), "Unsupported");
    assert_eq!(r.code("sdk.call", json!({"op": "markdown.render", "args": "x"})), "BadArgument");
    // a name with no dot is not a call of any SDK
    assert!(r.host.call("nothing", &json!({})).unwrap_err().contains("no such bridge call"));
}

// ------------------------------------------------------------------------------------------------ markdown

#[test]
fn markdown_view_and_open_file_go_through_the_viewer_and_open_file_asks_every_time() {
    let mut r = rig("md", all());
    r.be.reply("markdown.view", json!({"id": 7}));
    let v = r.ok("markdown.view", json!({"text": "# Hi", "title": "Notes\u{7}"}));
    assert_eq!(v["id"], 7);
    assert_eq!(r.be.calls_of("markdown.view")[0]["title"], "Notes", "control characters are dropped");
    assert_eq!(r.code("markdown.view", json!({"text": "   "})), "BadArgument");
    assert_eq!(r.code("markdown.view", json!({"text": "x", "title": "t".repeat(61)})), "BadArgument");
    for _ in 0..19 {
        r.ok("markdown.view", json!({"text": "x"}));
    }
    assert_eq!(r.code("markdown.view", json!({"text": "x"})), "QuotaExceeded", "20 viewers an hour");
    // openFile: a card each time, and the viewer is not asked until the person allows
    let id = r.card("markdown.openFile", json!({}));
    assert!(r.be.calls_of("markdown.openFile").is_empty());
    let req = r.requests();
    assert_eq!(req[0].kind, "sdk.card");
    assert_eq!(req[0].args["everyTime"], true);
    assert!(req[0].args["lines"][0].as_str().unwrap().contains("never sees where it is"));
    assert_eq!(r.answer(id, false).unwrap_err().0, "Denied");
    assert!(r.be.calls_of("markdown.openFile").is_empty());
    let id = r.card("markdown.openFile", json!({}));
    assert!(r.answer(id, true).is_ok());
    assert_eq!(r.be.calls_of("markdown.openFile").len(), 1);
    let id = r.card("markdown.openFile", json!({}));
    assert!(r.answer(id, true).is_ok(), "and asked again: it is every time");
}

#[test]
fn a_backend_that_is_not_wired_answers_unavailable_with_a_fix_line_for_every_namespace_that_needs_one() {
    let out = Arc::new(Mutex::new(Outbox::default()));
    let file = tmp("missing").join("t.json");
    let sdk = Arc::new(TileSdk::new("t", "tester", Arc::new(FakeLlm::default()), Arc::new(FakeSpeech::default()), Arc::new(LogMeter(UsageLog::open(&tmp("missing2").join("u.jsonl")))), out.clone(), Some(&file)));
    let mut h = TileHost::new(info(), all(), TileStorage::memory(), out).with_sdk(sdk);
    for (op, a) in [
        ("markdown.view", json!({"text": "x"})),
        ("embed.mount", json!({"target": {"x": 0, "y": 0, "w": 300, "h": 200}, "kind": "markdown", "options": {"text": "x"}})),
        ("dash.sources", json!({})),
        ("flows.list", json!({})),
        ("k3s.status", json!({})),
        ("media.play", json!({"id": "m1"})),
        ("audio.outputs", json!({})),
        ("speech.listen", json!({})),
    ] {
        let e = h.call(op, &a).unwrap_err();
        let body: Value = serde_json::from_str(e.strip_prefix(ERROR_PREFIX).unwrap_or_else(|| panic!("{op}: {e}"))).unwrap();
        assert_eq!(body["code"], "Unavailable", "{op}");
        assert!(body["fix"].as_str().is_some_and(|f| f.len() > 20), "{op}: {body}");
    }
}

// ------------------------------------------------------------------------------------------------ embed

fn rect() -> Value {
    json!({"x": 0, "y": 10, "w": 400, "h": 200})
}

#[test]
fn embed_checks_the_kind_the_rectangle_the_options_and_the_limits_before_the_surface_is_asked() {
    let mut r = rig("embed", all().with("embed", json!(["markdown", "browser"])));
    assert_eq!(r.code("embed.mount", json!({"target": rect(), "kind": "media", "options": {"file": "a.mp4"}})), "NoCapability", "media was not declared");
    assert_eq!(r.code("embed.mount", json!({"target": rect(), "kind": "flash"})), "BadArgument");
    for t in [json!("1bad"), json!("has space"), json!(7), json!({"x": 0, "y": 0, "w": 100, "h": 100}), json!({"x": 0, "y": 0, "w": 901, "h": 100}), json!({"x": 0, "y": 0, "w": 300, "h": 2881}), json!({"x": -1, "y": 0, "w": 300, "h": 200}), json!({"x": 0})] {
        assert_eq!(r.code("embed.mount", json!({"target": t.clone(), "kind": "markdown", "options": {"text": "x"}})), "BadArgument", "{t}");
    }
    for o in [json!({}), json!({"text": "x", "file": "a.md"}), json!({"file": "../a.md"}), json!({"file": "/etc/passwd"}), json!({"text": 5})] {
        assert_eq!(r.code("embed.mount", json!({"target": rect(), "kind": "markdown", "options": o.clone()})), "BadArgument", "{o}");
    }
    assert!(r.be.calls_of("embed.mount").is_empty(), "nothing reached the surface");
    let m = r.ok("embed.mount", json!({"target": "doc", "kind": "markdown", "options": {"text": "# x"}, "colour": 1}));
    assert_eq!(m["ignored"], json!(["colour"]));
    let id = m["id"].as_u64().unwrap();
    let sent = &r.be.calls_of("embed.mount")[0];
    assert_eq!((sent["kind"].as_str(), sent["target"].as_str()), (Some("markdown"), Some("doc")));
    assert_eq!(r.ok("embed.list", json!({}))["items"], json!([{"id": id, "kind": "markdown"}]));
    r.ok("embed.mount", json!({"target": "doc2", "kind": "markdown", "options": {"text": "y"}}));
    assert_eq!(r.code("embed.mount", json!({"target": "doc3", "kind": "markdown", "options": {"text": "y"}})), "QuotaExceeded", "two embeds at most");
}

#[test]
fn a_browser_embed_may_only_visit_the_listed_hosts_asks_once_and_is_one_per_app() {
    let mut r = rig("embedweb", all());
    let mount = |url: &str| json!({"target": "w", "kind": "browser", "options": {"url": url}});
    for bad in ["file:///etc/passwd", "javascript:alert(1)", "https://evil.example.com/", "https://docs.example.com@evil.example.com/", "//docs.example.com", "ftp://docs.example.com/", "docs.example.com"] {
        let c = r.code("embed.mount", mount(bad));
        assert!(c == "Blocked" || c == "BadArgument", "{bad}: {c}");
    }
    assert_eq!(r.code("embed.mount", mount("https://evil.example.com/")), "Blocked");
    let id = r.card("embed.mount", mount("https://Docs.Example.com:8443/guide?x=1"));
    assert!(r.be.calls_of("embed.mount").is_empty(), "the person is asked first");
    let card = r.requests().into_iter().find(|q| q.id == id).unwrap();
    assert_eq!(card.args["everyTime"], false);
    assert!(card.args["lines"].to_string().contains("docs.example.com"));
    let m = r.answer(id, true).unwrap();
    assert!(m["id"].as_u64().is_some());
    assert_eq!(r.be.calls_of("embed.mount")[0]["hosts"], json!(["docs.example.com"]), "the surface gets the list to intersect with the person's host policy");
    assert_eq!(r.code("embed.mount", mount("https://docs.example.com/")), "QuotaExceeded", "one browser at most");
    // the answer was kept: a later app run does not ask again
    let r2 = r.sdk.v2().call("embed.list", &json!({}), &all()).unwrap();
    assert_eq!(r2["items"].as_array().unwrap().len(), 1);
    let id = m["id"].as_u64().unwrap();
    assert_eq!(r.code("embed.update", json!({"id": id, "options": {"url": "https://docs.example.com/other"}})), "BadArgument", "a browser embed is not sent elsewhere");
    r.ok("embed.unmount", json!({"id": id}));
    let again = r.ok("embed.mount", mount("https://docs.example.com/"));
    assert!(again["id"].as_u64().is_some(), "first-use was kept: no second card");
}

#[test]
fn embed_calls_are_limited_to_the_methods_of_the_kind_and_to_this_apps_own_embeds() {
    let mut r = rig("embedcall", all());
    let id = r.ok("embed.mount", json!({"target": "d", "kind": "markdown", "options": {"text": "x"}}))["id"].as_u64().unwrap();
    r.be.reply("embed.call", json!({"y": 40}));
    assert_eq!(r.ok("embed.call", json!({"id": id, "method": "scrollTo", "args": {"y": 40}}))["result"], json!({"y": 40}));
    assert_eq!(r.code("embed.call", json!({"id": id, "method": "reload"})), "BadArgument", "a markdown viewer has no reload");
    assert_eq!(r.code("embed.call", json!({"id": id + 100, "method": "scrollTo"})), "BadArgument");
    assert_eq!(r.code("embed.update", json!({"id": id + 100, "options": {}})), "BadArgument");
    r.ok("embed.update", json!({"id": id, "options": {"text": "new"}}));
    r.ok("embed.unmount", json!({"id": id}));
    assert_eq!(r.code("embed.unmount", json!({"id": id})), "BadArgument", "already gone");
    // events reach the page through the handle mechanism, at most 50 waiting
    for i in 0..60 {
        r.sdk.v2().event(id as u32, "scroll", json!({"y": i}));
    }
    let o = r.out.lock().unwrap();
    assert!(o.emits.iter().filter(|e| e.handle == id as u32).count() <= 51, "50 events and a dropped marker at most");
    assert!(o.emits.iter().any(|e| e.event == "dropped"));
}

#[test]
fn closing_the_tile_unmounts_embeds_stops_sound_and_listening() {
    let mut r = rig("close", all());
    r.ok("embed.mount", json!({"target": "d", "kind": "markdown", "options": {"text": "x"}}));
    r.approved("audio.play", json!({"file": "a.wav"})).unwrap();
    r.sdk.close();
    assert_eq!(r.be.calls_of("embed.unmount").len(), 1);
    assert_eq!(r.be.calls_of("audio.stop").len(), 1);
}

// ------------------------------------------------------------------------------------------------ dash

#[test]
fn dash_lists_only_the_granted_sources_asks_once_per_source_and_cuts_rows_at_the_limit() {
    let mut r = rig("dash", all().with("dash", json!(["prom", "logs"])));
    r.be.reply("dash.sources", json!([{"name": "prom", "kind": "prometheus", "language": "promql"}, {"name": "secret-db", "kind": "sql", "language": "sql"}]));
    let s = r.ok("dash.sources", json!({}));
    assert_eq!(s["items"].as_array().unwrap().len(), 1, "a source the app may not read is not even listed: {s}");
    assert_eq!(r.code("dash.query", json!({"source": "secret-db", "query": "select 1"})), "NoCapability");
    for (a, f) in [(json!({"source": "prom"}), "query"), (json!({"source": "prom", "query": "  "}), "query"), (json!({"source": "prom", "query": "up", "limit": 0}), "limit"), (json!({"source": "prom", "query": "up", "limit": 1001}), "limit"), (json!({"source": "prom", "query": "up", "range": {"from": "now-1h"}}), "range"), (json!({"source": "prom", "query": "x".repeat(8001)}), "query")] {
        let (c, m) = r.call("dash.query", a.clone()).unwrap_err();
        assert_eq!(c, "BadArgument", "{a}");
        assert!(m.starts_with(f), "{a}: {m}");
    }
    let rows: Vec<Value> = (0..1500).map(|i| json!([i, 1.0])).collect();
    r.be.reply("dash.query", json!({"columns": [{"name": "t", "type": "time"}, {"name": "v", "type": "number"}], "rows": rows, "truncated": false}));
    let id = r.card("dash.query", json!({"source": "prom", "query": "up", "limit": 100, "range": {"from": "now-1h", "to": "now"}}));
    assert!(r.be.calls_of("dash.query").is_empty());
    let q = r.answer(id, true).unwrap();
    assert_eq!(q["rows"].as_array().unwrap().len(), 100);
    assert_eq!(q["truncated"], true);
    let sent = &r.be.calls_of("dash.query")[0];
    assert_eq!((sent["limit"].as_u64(), sent["range"]["from"].as_str()), (Some(100), Some("now-1h")));
    // the same source does not ask again; another source does
    assert!(r.ok("dash.query", json!({"source": "prom", "query": "up"}))["rows"].is_array());
    assert!(r.card("dash.query", json!({"source": "logs", "query": "error"})) > 0);
}

#[test]
fn dash_queries_are_rate_limited_and_a_panel_is_added_only_after_a_card_every_time() {
    let mut r = rig("dashrate", all());
    let id = r.card("dash.query", json!({"source": "prom", "query": "up"}));
    r.answer(id, true).unwrap();
    for _ in 0..29 {
        r.ok("dash.query", json!({"source": "prom", "query": "up"}));
    }
    assert_eq!(r.code("dash.query", json!({"source": "prom", "query": "up"})), "QuotaExceeded", "30 a minute");
    r.sdk.v2().skew_ms.fetch_add(61_000, std::sync::atomic::Ordering::SeqCst);
    r.ok("dash.query", json!({"source": "prom", "query": "up"}));
    let panel = json!({"dashboard": "main", "panel": {"title": "CPU", "source": "prom", "query": "rate(cpu[5m])"}});
    let id = r.card("dash.panel.add", panel.clone());
    assert!(r.be.calls_of("dash.panel.add").is_empty());
    assert!(r.requests()[0].args["lines"].to_string().contains("rate(cpu[5m])"), "the card shows the query");
    assert_eq!(r.answer(id, true).unwrap()["id"], "p1");
    assert_eq!(r.be.calls_of("dash.panel.add")[0]["panel"]["visual"], "timeseries");
    let id = r.card("dash.panel.add", panel);
    assert_eq!(r.answer(id, false).unwrap_err().0, "Denied");
    assert_eq!(r.be.calls_of("dash.panel.add").len(), 1);
    assert!(r.audits().iter().any(|a| a.starts_with("app.dash.panel")));
}

// ------------------------------------------------------------------------------------------------ flows

#[test]
fn flows_list_only_the_granted_published_flows_and_run_after_a_card_that_names_the_flow_version_and_inputs() {
    let mut r = rig("flows", all().with("flows", json!(["backup"])));
    r.be.reply("flows.list", json!([{"id": "f1", "name": "backup", "version": 3, "inputs": []}, {"id": "f2", "name": "wipe", "version": 1, "inputs": []}]));
    assert_eq!(r.ok("flows.list", json!({}))["items"].as_array().unwrap().len(), 1);
    assert_eq!(r.code("flows.run", json!({"flow": "wipe"})), "NoCapability");
    assert_eq!(r.code("flows.run", json!({"flow": "backup", "inputs": "x"})), "BadArgument");
    assert_eq!(r.code("flows.run", json!({"flow": "backup", "inputs": {"a": "x".repeat(17000)}})), "BadArgument");
    assert_eq!(r.code("flows.run", json!({"flow": "backup", "key": "bad key!"})), "BadArgument");
    let id = r.card("flows.run", json!({"flow": "backup", "inputs": {"target": "nas", "secret_token": "abc"}, "key": "nightly-1"}));
    let text = r.requests()[0].args["lines"].to_string();
    assert!(text.contains("backup") && text.contains("version 3") && text.contains("target: \\\"nas\\\""), "{text}");
    assert!(r.be.calls_of("flows.run").is_empty());
    r.be.reply("flows.run", json!({"run": "run-9"}));
    assert_eq!(r.answer(id, true).unwrap()["run"], "run-9");
    assert_eq!(r.be.calls_of("flows.run")[0]["version"], 3);
    // the same key gives the same run and asks nothing
    let again = r.ok("flows.run", json!({"flow": "backup", "key": "nightly-1"}));
    assert_eq!((again["run"].as_str(), again["repeated"].as_bool()), (Some("run-9"), Some(true)));
    // status and cancel: only this app's runs
    r.be.reply("flows.status", json!({"state": "running", "steps": [{"node": "a", "state": "done"}]}));
    assert_eq!(r.ok("flows.status", json!({"run": "run-9"}))["state"], "running");
    assert_eq!(r.code("flows.status", json!({"run": "someone-elses"})), "BadArgument");
    assert_eq!(r.code("flows.cancel", json!({"run": "someone-elses"})), "BadArgument");
    let id = r.card("flows.cancel", json!({"run": "run-9"}));
    assert_eq!(r.answer(id, false).unwrap_err().0, "Denied");
    assert!(r.be.calls_of("flows.cancel").is_empty());
    let id = r.card("flows.cancel", json!({"run": "run-9"}));
    assert_eq!(r.answer(id, true).unwrap()["state"], "cancelled");
    assert!(r.audits().iter().any(|a| a.starts_with("app.flow.run") && a.contains("backup v3")));
}

#[test]
fn flows_refuse_an_unpublished_flow_and_the_eleventh_run_in_an_hour() {
    let mut r = rig("flowrate", all());
    r.be.reply("flows.list", json!([]));
    assert_eq!(r.code("flows.run", json!({"flow": "backup"})), "BadArgument", "not a published flow");
    for i in 0..10 {
        let id = r.card("flows.run", json!({"flow": "backup", "inputs": {"i": i}}));
        r.be.reply("flows.run", json!({"run": format!("r{i}")}));
        r.answer(id, true).unwrap();
    }
    assert_eq!(r.code("flows.run", json!({"flow": "backup"})), "QuotaExceeded");
    for _ in 0..2 {
        r.sdk.v2().skew_ms.fetch_add(1_800_000, std::sync::atomic::Ordering::SeqCst);
    }
    assert!(r.card("flows.run", json!({"flow": "backup"})) > 0, "an hour later it may run again");
}

// ------------------------------------------------------------------------------------------------ k3s

#[test]
fn k3s_reads_only_with_read_changes_only_with_manage_and_never_shows_secret_values() {
    let mut r = rig("k3s", all().with("k3s", json!("read")));
    assert_eq!(r.ok("k3s.status", json!({}))["writable"], false);
    assert_eq!(r.code("k3s.get", json!({"kind": "nodes"})), "BadArgument", "cluster-scoped kinds are not offered");
    assert_eq!(r.code("k3s.get", json!({"kind": "pods", "name": "Bad Name"})), "BadArgument");
    assert_eq!(r.code("k3s.get", json!({"kind": "pods", "selector": "app=web;rm -rf"})), "BadArgument");
    r.be.reply("k3s.get", json!({"items": [{"kind": "Secret", "metadata": {"name": "db", "annotations": {"kubectl.kubernetes.io/last-applied-configuration": "{\"data\":{\"p\":\"c2VjcmV0\"}}"}}, "data": {"password": "c2VjcmV0"}, "stringData": {"token": "plain"}}]}));
    let g = r.ok("k3s.get", json!({"kind": "Secrets"}));
    let text = g.to_string();
    assert!(!text.contains("c2VjcmV0") && !text.contains("plain") && !text.contains("last-applied"), "{text}");
    assert_eq!(g["items"][0]["keys"], json!(["password", "token"]), "names only");
    assert_eq!(r.be.calls_of("k3s.get")[0]["kind"], "secrets");
    r.be.reply("k3s.logs", json!({"lines": (0..800).map(|i| format!("line {i}")).collect::<Vec<_>>()}));
    let l = r.ok("k3s.logs", json!({"pod": "web-1", "tail": 500}));
    assert_eq!(l["lines"].as_array().unwrap().len(), 500);
    assert_eq!(l["lines"][499], "line 799", "the last lines");
    assert_eq!(r.code("k3s.logs", json!({"pod": "web-1", "tail": 501})), "BadArgument");
    assert_eq!(r.code("k3s.apply", json!({"yaml": "kind: Pod"})), "NoCapability");
    assert_eq!(r.code("k3s.restart", json!({"name": "web"})), "NoCapability");
    assert!(r.be.calls_of("k3s.plan").is_empty() && r.be.calls_of("k3s.apply").is_empty() && r.be.calls_of("k3s.restart").is_empty());
}

#[test]
fn k3s_apply_shows_the_plan_obeys_the_cluster_rules_and_runs_only_after_the_person_allows() {
    let mut r = rig("k3sm", all());
    assert_eq!(r.ok("k3s.status", json!({}))["writable"], true);
    r.be.reply("k3s.plan", json!({"plan": "would create deployment web", "refused": ["privileged containers are not allowed"]}));
    let (c, m) = r.call("k3s.apply", json!({"yaml": "kind: Deployment"})).unwrap_err();
    assert_eq!(c, "Blocked");
    assert!(m.contains("privileged"), "{m}");
    assert!(r.be.calls_of("k3s.apply").is_empty());
    r.be.reply("k3s.plan", json!({"plan": "would create deployment web\nwould create service web"}));
    let id = r.card("k3s.apply", json!({"yaml": "kind: Deployment"}));
    assert!(r.requests()[0].args["lines"].to_string().contains("would create service web"));
    assert!(r.be.calls_of("k3s.apply").is_empty());
    assert_eq!(r.answer(id, false).unwrap_err().0, "Denied");
    r.be.reply("k3s.plan", json!({"plan": "would create deployment web"}));
    let id = r.card("k3s.apply", json!({"yaml": "kind: Deployment"}));
    assert!(r.answer(id, true).unwrap()["plan"].as_str().unwrap().contains("deployment web"));
    assert_eq!(r.be.calls_of("k3s.apply").len(), 1);
    assert_eq!(r.code("k3s.apply", json!({"yaml": ""})), "BadArgument");
    assert_eq!(r.code("k3s.apply", json!({"yaml": "x".repeat(65537)})), "BadArgument");
    let id = r.card("k3s.restart", json!({"name": "web"}));
    assert_eq!(r.answer(id, true).unwrap()["restarted"], "web");
    assert!(r.audits().iter().any(|a| a.starts_with("app.k3s.apply")) && r.audits().iter().any(|a| a.starts_with("app.k3s.restart")));
}

// ------------------------------------------------------------------------------------------------ media and audio

#[test]
fn media_lists_after_a_first_use_card_plays_by_id_and_asks_every_time_for_a_file_outside_the_media_folders() {
    let mut r = rig("media", all());
    let id = r.card("media.library", json!({"kind": "audio"}));
    assert!(r.be.calls_of("media.library").is_empty());
    assert_eq!(r.answer(id, true).unwrap()["items"][0]["title"], "Song");
    assert!(r.ok("media.library", json!({}))["items"].is_array(), "asked once");
    assert_eq!(r.code("media.library", json!({"kind": "text"})), "BadArgument");
    assert_eq!(r.code("media.play", json!({})), "BadArgument");
    assert_eq!(r.code("media.play", json!({"id": "m1", "file": "/x.mp3"})), "BadArgument", "one or the other");
    assert_eq!(r.code("media.play", json!({"id": "m1", "start": 100000})), "BadArgument");
    r.be.reply("media.play", json!({"id": 5}));
    let p = r.ok("media.play", json!({"id": "m1", "start": 30}));
    assert_eq!(p["id"], 5);
    assert_eq!(r.be.calls_of("media.play")[0]["start"], 30);
    // a file inside the media folders needs no card, one outside does
    r.be.reply("media.in_folders", json!({"inside": true}));
    r.be.reply("media.play", json!({"id": 6}));
    assert_eq!(r.ok("media.play", json!({"file": "Music/a.mp3"}))["id"], 6);
    r.be.reply("media.in_folders", json!({"inside": false}));
    let id = r.card("media.play", json!({"file": "/media/other/private.mp4"}));
    assert_eq!(r.answer(id, false).unwrap_err().0, "Denied");
    r.be.reply("media.in_folders", json!({"inside": false}));
    let id = r.card("media.play", json!({"file": "/media/other/private.mp4"}));
    r.be.reply("media.play", json!({"id": 7}));
    assert_eq!(r.answer(id, true).unwrap()["id"], 7);
    // control only of what this app started
    assert_eq!(r.code("media.pause", json!({"id": 99})), "BadArgument");
    assert_eq!(r.ok("media.pause", json!({"id": 5}))["state"], "pause");
    assert_eq!(r.ok("media.state", json!({"id": 5}))["state"], "playing");
    r.ok("media.stop", json!({"id": 5}));
    assert_eq!(r.code("media.resume", json!({"id": 5})), "BadArgument", "stopped playbacks are forgotten");
    let id = r.card("media.queue.add", json!({"id": "m2"}));
    assert_eq!(r.answer(id, true).unwrap()["queued"], 1);
    assert!(r.ok("media.queue.list", json!({}))["items"].is_array());
}

#[test]
fn audio_plays_one_stream_at_a_time_after_one_card_and_has_no_internet_source() {
    let mut r = rig("audio", all());
    for (a, f) in [
        (json!({}), "file"),
        (json!({"file": "a.wav", "wav": "AAAA"}), "file"),
        (json!({"file": "../a.wav"}), "file"),
        (json!({"file": "a.exe"}), "file"),
        (json!({"wav": "not base64!"}), "wav"),
        (json!({"file": "a.wav", "volume": 2}), "volume"),
        (json!({"file": "a.wav", "loop": "yes"}), "loop"),
        (json!({"file": "a.wav", "channel": "radio"}), "channel"),
    ] {
        let (c, m) = r.call("audio.play", a.clone()).unwrap_err();
        assert_eq!(c, "BadArgument", "{a}");
        assert!(m.starts_with(f), "{a}: {m}");
    }
    let (c, m) = r.call("audio.play", json!({"url": "https://example.com/a.mp3"})).unwrap_err();
    assert_eq!(c, "Unsupported", "{m}");
    assert!(r.be.calls_of("audio.play").is_empty());
    r.be.reply("audio.play", json!({"id": 11}));
    let id = r.card("audio.play", json!({"file": "sounds/ding.wav", "volume": 0.5, "loop": true, "channel": "cue"}));
    assert_eq!(r.answer(id, true).unwrap()["id"], 11);
    let sent = &r.be.calls_of("audio.play")[0];
    assert_eq!((sent["volume"].as_f64(), sent["loop"].as_bool(), sent["channel"].as_str()), (Some(0.5), Some(true), Some("cue")));
    // a second play replaces the first; the person is not asked again
    r.be.reply("audio.play", json!({"id": 12}));
    assert_eq!(r.ok("audio.play", json!({"wav": "UklGRg=="}))["id"], 12);
    assert_eq!(r.be.calls_of("audio.stop")[0]["id"], 11);
    assert_eq!(r.code("audio.pause", json!({"id": 11})), "BadArgument", "the replaced stream is gone");
    r.ok("audio.pause", json!({"id": 12}));
    r.ok("audio.setVolume", json!({"id": 12, "volume": 0.2}));
    assert_eq!(r.code("audio.setVolume", json!({"id": 12, "volume": -1})), "BadArgument");
    r.ok("audio.stop", json!({"id": 12}));
    assert_eq!(r.ok("audio.outputs", json!({}))["items"][0]["name"], "Speakers");
    // the speaker held by another program is Busy with who holds it, not a failure
    r.be.fail("audio.play", "Busy", "the speaker is in use by radio");
    assert_eq!(r.code("audio.play", json!({"file": "a.wav"})), "Busy");
}

// ------------------------------------------------------------------------------------------------ speech.listen

#[test]
fn listening_asks_every_time_names_the_microphone_gives_text_only_and_audits_each_use() {
    let mut r = rig("listen", all());
    assert_eq!(r.code("speech.listen", json!({"lang": "en us"})), "BadArgument");
    assert_eq!(r.code("speech.listen", json!({"maxSeconds": 61})), "BadArgument");
    assert_eq!(r.code("speech.listen", json!({"maxSeconds": 0})), "BadArgument");
    // no microphone: nobody is asked to allow what cannot work
    r.be.fail("speech.available", "Unavailable", "no microphone");
    assert_eq!(r.code("speech.listen", json!({})), "Unavailable");
    assert!(r.requests().is_empty());
    r.be.reply("speech.available", json!({"microphone": "USB Mic"}));
    let id = r.card("speech.listen", json!({"lang": "ta-IN", "maxSeconds": 20, "future": 1}));
    let card = r.requests().remove(0);
    assert_eq!(card.args["everyTime"], true);
    let lines = card.args["lines"].to_string();
    assert!(lines.contains("USB Mic") && lines.contains("20 seconds") && lines.contains("never the sound"), "{lines}");
    assert!(r.be.calls_of("speech.listen").is_empty());
    let h = r.answer(id, true).unwrap();
    let handle = h["handle"].as_u64().unwrap() as u32;
    assert_eq!(h["ignored"], json!(["future"]));
    assert_eq!(r.be.calls_of("speech.listen")[0]["max_seconds"], 20);
    assert_eq!(r.code("speech.listen", json!({})), "QuotaExceeded", "one at a time");
    // the engine's text arrives as events; the sound never does
    r.sdk.v2().heard(handle, Ok("turn on the light\u{7}".into()));
    {
        let o = r.out.lock().unwrap();
        let evs: Vec<(&str, &Value)> = o.emits.iter().filter(|e| e.handle == handle).map(|e| (e.event.as_str(), &e.payload)).collect();
        assert_eq!(evs[0], ("result", &json!({"text": "turn on the light"})));
        assert_eq!(evs[1].0, "done");
    }
    assert!(r.audits().iter().any(|a| a.starts_with("speech.listen") && a.contains("USB Mic")));
    // the person says no
    let id = r.card("speech.listen", json!({}));
    assert_eq!(r.answer(id, false).unwrap_err().0, "Denied");
    assert_eq!(r.be.calls_of("speech.listen").len(), 1);
}

#[test]
fn listening_is_limited_to_twenty_an_hour_times_out_and_can_be_cancelled() {
    let mut r = rig("listenrate", all());
    for _ in 0..20 {
        let id = r.card("speech.listen", json!({"maxSeconds": 1}));
        let h = r.answer(id, true).unwrap()["handle"].as_u64().unwrap() as u32;
        r.ok("speech.cancel", json!({"handle": h}));
    }
    assert_eq!(r.code("speech.listen", json!({})), "QuotaExceeded");
    // a listen that never ends is a Timeout event
    let mut r = rig("listentime", all());
    let id = r.card("speech.listen", json!({"maxSeconds": 5}));
    let h = r.answer(id, true).unwrap()["handle"].as_u64().unwrap() as u32;
    r.sdk.v2().skew_ms.fetch_add(40_000, std::sync::atomic::Ordering::SeqCst);
    r.sdk.tick();
    let o = r.out.lock().unwrap();
    let e = o.emits.iter().find(|e| e.handle == h && e.event == "error").expect("a timeout error event");
    assert_eq!(e.payload["code"], "Timeout");
    assert_eq!(r.be.calls_of("speech.cancel").len(), 1);
}

// ------------------------------------------------------------------------------------------------ money

fn spend(amount: u64, why: &str) -> Value {
    json!({"amountMinor": amount, "currency": "USD", "purpose": why})
}

#[test]
fn money_starts_at_nothing_so_every_spend_is_a_budget_stop_before_anyone_is_asked_and_no_provider_is_called() {
    let mut r = rig("money0", all());
    let l = r.ok("money.limits", json!({}));
    assert_eq!((l["currency"].as_str(), l["perDayMinor"].as_u64(), l["remainingMinor"].as_u64(), l["confirmEach"].as_bool()), (Some("USD"), Some(0), Some(0), Some(true)));
    let (c, m) = r.call("money.spend", spend(100, "a coffee")).unwrap_err();
    assert_eq!(c, "BudgetStop");
    assert!(m.contains("nothing until you raise it"), "{m}");
    assert!(r.requests().is_empty(), "no card for something that is refused");
    assert!(r.be.calls_of("money.available").is_empty() && r.be.calls_of("money.charge").is_empty(), "the provider is never reached");
    assert!(r.audits().iter().any(|a| a.starts_with("app.money.stop")));
    for (a, f) in [
        (json!({"amountMinor": 0, "currency": "USD", "purpose": "x"}), "amountMinor"),
        (json!({"amountMinor": 1.5, "currency": "USD", "purpose": "x"}), "amountMinor"),
        (json!({"amountMinor": -5, "currency": "USD", "purpose": "x"}), "amountMinor"),
        (json!({"amountMinor": 5, "currency": "EUR", "purpose": "x"}), "currency"),
        (json!({"amountMinor": 5, "currency": "usd", "purpose": "x"}), "currency"),
        (json!({"amountMinor": 5, "currency": "USD"}), "purpose"),
        (json!({"amountMinor": 5, "currency": "USD", "purpose": " "}), "purpose"),
        (json!({"amountMinor": 5, "currency": "USD", "purpose": "x", "ref": "bad ref"}), "ref"),
    ] {
        let (c, m) = r.call("money.spend", a.clone()).unwrap_err();
        assert_eq!(c, "BadArgument", "{a}");
        assert!(m.starts_with(f), "{a}: {m}");
    }
}

#[test]
fn only_the_person_raises_a_limit_and_it_applies_before_each_spend_and_again_when_the_card_is_answered() {
    let mut r = rig("money1", all());
    sdkv2_money::set_limit(&r.dir, "tester", "t", "USD", 1000, None).unwrap();
    let l = r.ok("money.limits", json!({}));
    assert_eq!((l["perDayMinor"].as_u64(), l["remainingMinor"].as_u64()), (Some(1000), Some(1000)));
    // a payment shows a card by default and the provider is asked only after the person allows
    let id = r.card("money.spend", spend(350, "a month of hosting"));
    let card = r.requests().remove(0);
    assert_eq!(card.args["everyTime"], true);
    assert!(card.args["lines"].to_string().contains("3.50 USD") && card.args["lines"].to_string().contains("10.00 USD"));
    assert!(r.be.calls_of("money.charge").is_empty());
    r.be.reply("money.charge", json!({"receipt": "rcpt-1"}));
    let ok = r.answer(id, true).unwrap();
    assert_eq!((ok["receipt"].as_str(), ok["remainingMinor"].as_u64()), (Some("rcpt-1"), Some(650)));
    assert_eq!(r.be.calls_of("money.charge")[0]["amountMinor"], 350);
    assert!(r.audits().iter().any(|a| a.starts_with("app.money.spend") && a.contains("3.50 USD") && a.contains("hosting")));
    // the next one that would pass the limit is refused before a card
    let (c, m) = r.call("money.spend", spend(651, "too much")).unwrap_err();
    assert_eq!(c, "BudgetStop");
    assert!(m.contains("6.50 USD"), "{m}");
    // lowered while the card waits: refused when the person answers, nothing charged
    let id = r.card("money.spend", spend(600, "later"));
    sdkv2_money::set_limit(&r.dir, "tester", "t", "USD", 500, None).unwrap();
    assert_eq!(r.answer(id, true).unwrap_err().0, "BudgetStop");
    assert_eq!(r.be.calls_of("money.charge").len(), 1);
    // the person says no
    let id = r.card("money.spend", spend(100, "x"));
    assert_eq!(r.answer(id, false).unwrap_err().0, "Denied");
    assert_eq!(r.be.calls_of("money.charge").len(), 1);
    assert_eq!(sdkv2_money::status(&r.dir, "tester", "t", &crate::clock::Zone::utc(), crate::clock::now_unix()).spent_today_minor, 350);
}

#[test]
fn a_day_ends_at_midnight_a_failed_charge_gives_the_amount_back_and_a_repeated_ref_is_not_charged_twice() {
    let mut r = rig("money2", all());
    sdkv2_money::set_limit(&r.dir, "tester", "t", "USD", 1000, Some(false)).unwrap();
    // inside the limit and with the card switched off by the person: no card
    let s = r.ok("money.spend", json!({"amountMinor": 900, "currency": "USD", "purpose": "a", "ref": "order-1"}));
    assert_eq!(s["remainingMinor"], 100);
    let again = r.ok("money.spend", json!({"amountMinor": 900, "currency": "USD", "purpose": "a", "ref": "order-1"}));
    assert_eq!((again["repeated"].as_bool(), again["receipt"].as_str()), (Some(true), s["receipt"].as_str()));
    assert_eq!(r.be.calls_of("money.charge").len(), 1);
    assert_eq!(r.code("money.spend", spend(200, "b")), "BudgetStop");
    // a failing provider: the reservation is returned
    r.be.fail("money.charge", "Failed", "the bank said no");
    assert_eq!(r.code("money.spend", spend(100, "c")), "Failed");
    assert_eq!(r.ok("money.limits", json!({}))["spentTodayMinor"], 900);
    assert!(r.audits().iter().any(|a| a.starts_with("app.money.failed")));
    // tomorrow
    for _ in 0..2 {
        r.sdk.v2().skew_ms.fetch_add(12 * 3_600_000 + 1, std::sync::atomic::Ordering::SeqCst);
    }
    let l = r.ok("money.limits", json!({}));
    assert_eq!((l["spentTodayMinor"].as_u64(), l["remainingMinor"].as_u64()), (Some(0), Some(1000)));
    r.ok("money.spend", spend(1000, "d"));
}

#[test]
fn a_ref_whose_charge_is_still_running_is_a_repeat_and_is_released_when_the_provider_fails() {
    let mut r = rig("money-pending", all());
    sdkv2_money::set_limit(&r.dir, "tester", "t", "USD", 1000, Some(false)).unwrap();
    // another call with this ref is between its reservation and its receipt
    sdkv2_money::AppFile::edit(&sdkv2_money::state_path(&r.dir, "tester", "t"), true, |f| {
        f.money.refs.insert("order-9".into(), "pending".into());
    });
    let again = r.ok("money.spend", json!({"amountMinor": 400, "currency": "USD", "purpose": "a", "ref": "order-9"}));
    assert_eq!(again["repeated"].as_bool(), Some(true));
    assert_eq!(r.be.calls_of("money.charge").len(), 0, "nothing is charged for a ref that is taken");
    assert_eq!(r.ok("money.limits", json!({}))["spentTodayMinor"], 0);
    // a provider that fails frees the ref for a new try
    r.be.fail("money.charge", "Failed", "the bank said no");
    assert_eq!(r.code("money.spend", json!({"amountMinor": 400, "currency": "USD", "purpose": "b", "ref": "order-8"})), "Failed");
    let held = sdkv2_money::AppFile::edit(&sdkv2_money::state_path(&r.dir, "tester", "t"), false, |f| f.money.refs.contains_key("order-8"));
    assert!(!held, "a failed charge does not keep its ref");
}

#[test]
fn a_ref_stays_a_repeat_after_many_newer_payments() {
    let mut r = rig("money-refs", all());
    sdkv2_money::set_limit(&r.dir, "tester", "t", "USD", 1_000_000, Some(false)).unwrap();
    for i in 0..120 {
        r.ok("money.spend", json!({"amountMinor": 1, "currency": "USD", "purpose": "p", "ref": format!("order-{i:03}")}));
    }
    assert_eq!(r.be.calls_of("money.charge").len(), 120);
    // the oldest ref (the old limit kept 50) is still a repeat
    let again = r.ok("money.spend", json!({"amountMinor": 1, "currency": "USD", "purpose": "p", "ref": "order-000"}));
    assert_eq!(again["repeated"].as_bool(), Some(true));
    assert_eq!(r.be.calls_of("money.charge").len(), 120);
}

#[test]
fn a_provider_that_is_not_set_up_is_found_before_the_person_is_asked_and_nothing_is_recorded() {
    let out = Arc::new(Mutex::new(Outbox::default()));
    let dir = tmp("money3");
    let file = dir.join("appdata").join("tile-sdk").join("tester").join("t.json");
    let sdk = Arc::new(TileSdk::new("t", "tester", Arc::new(FakeLlm::default()), Arc::new(FakeSpeech::default()), Arc::new(LogMeter(UsageLog::open(&dir.join("u.jsonl")))), out.clone(), Some(&file)));
    let mut h = TileHost::new(info(), all(), TileStorage::memory(), out.clone()).with_sdk(sdk);
    sdkv2_money::set_limit(&dir, "tester", "t", "USD", 1000, None).unwrap();
    let e = h.call("money.spend", &spend(100, "x")).unwrap_err();
    let body: Value = serde_json::from_str(e.strip_prefix(ERROR_PREFIX).unwrap()).unwrap();
    assert_eq!(body["code"], "Unavailable");
    assert!(body["message"].as_str().unwrap().contains("payment provider"));
    assert!(out.lock().unwrap().requests.is_empty(), "no card for a payment that cannot happen");
    assert_eq!(sdkv2_money::status(&dir, "tester", "t", &crate::clock::Zone::utc(), crate::clock::now_unix()).spent_today_minor, 0);
}

#[test]
fn raising_the_limit_is_a_card_every_time_that_no_setting_and_no_fake_switch_answers() {
    let mut r = rig("money4", all());
    assert_eq!(r.code("money.requestLimit", json!({"perDayMinor": 0, "why": "x"})), "BadArgument");
    assert_eq!(r.code("money.requestLimit", json!({"perDayMinor": 500})), "BadArgument", "a reason is needed");
    let id = r.card("money.requestLimit", json!({"perDayMinor": 2500, "why": "to buy credits for the report"}));
    let lines = r.requests()[0].args["lines"].to_string();
    assert!(lines.contains("0.00 USD") && lines.contains("25.00 USD") && lines.contains("buy credits"), "{lines}");
    assert_eq!(r.ok("money.limits", json!({}))["perDayMinor"], 0, "nothing changed while the card waits");
    assert_eq!(r.answer(id, false).unwrap_err().0, "Denied");
    assert_eq!(r.ok("money.limits", json!({}))["perDayMinor"], 0);
    let id = r.card("money.requestLimit", json!({"perDayMinor": 2500, "why": "again"}));
    assert_eq!(r.answer(id, true).unwrap()["perDayMinor"], 2500);
    assert_eq!(r.ok("money.limits", json!({}))["perDayMinor"], 2500);
    assert!(r.audits().iter().any(|a| a.starts_with("app.money.limit") && a.contains("0.00 USD -> 25.00 USD")));
    assert_eq!(r.code("money.requestLimit", json!({"perDayMinor": 2500, "why": "x"})), "BadArgument", "already that high");
    // the headless harness's switch for first-use cards does not answer this one
    let out = Arc::new(Mutex::new(Outbox::default()));
    let sdk = Arc::new(TileSdk::fake("t", out.clone(), &tmp("money5b")));
    let mut host = TileHost::new(info(), all(), TileStorage::memory(), out).with_sdk(sdk.clone());
    sdk.fake_op("approve.set", &json!({"mode": "allow"})).unwrap();
    let v = host.call("money.requestLimit", &json!({"perDayMinor": 100, "why": "x"})).unwrap();
    assert!(v["pending"].is_number(), "still a card: {v}");
}

#[test]
fn a_limits_file_this_release_cannot_write_keeps_the_limit_but_lets_nothing_be_spent() {
    let mut r = rig("money6", all());
    sdkv2_money::set_limit(&r.dir, "tester", "t", "USD", 1000, Some(false)).unwrap();
    let p = sdkv2_money::state_path(&r.dir, "tester", "t");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    v["v"] = json!(99);
    v["future_field"] = json!({"keep": "me"});
    std::fs::write(&p, v.to_string()).unwrap();
    assert_eq!(r.ok("money.limits", json!({}))["perDayMinor"], 1000);
    assert_eq!(r.code("money.spend", spend(10, "x")), "Unavailable", "the spent amount could not be kept");
    assert!(sdkv2_money::set_limit(&r.dir, "tester", "t", "USD", 5, None).is_err());
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!((after["v"].as_u64(), after["future_field"].clone()), (Some(99), json!({"keep": "me"})), "a newer file is never rewritten");
    // damaged: the same
    std::fs::write(&p, "{ not json").unwrap();
    assert_eq!(r.ok("money.limits", json!({}))["perDayMinor"], 0);
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "{ not json");
    // an older file without the fields reads with the defaults, and unknown fields are kept when it is saved
    std::fs::write(&p, r#"{"v":1,"unknown_top":[1,2],"money":{"currency":"USD","per_day_minor":300,"new_money_field":true}}"#).unwrap();
    sdkv2_money::set_limit(&r.dir, "tester", "t", "USD", 400, None).unwrap();
    let kept: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!((kept["unknown_top"].clone(), kept["money"]["new_money_field"].clone(), kept["money"]["per_day_minor"].clone(), kept["money"]["confirm_each"].clone()), (json!([1, 2]), json!(true), json!(400), json!(true)));
}

// ------------------------------------------------------------------------------------------------ the page

#[test]
fn the_page_api_discovers_degrades_and_drives_the_new_namespaces_on_the_real_engine() {
    let dir = tmp("page");
    let write = |p: &str, t: &str| {
        let f = dir.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    };
    write("app.json", r#"{"id":"v2app","name":"V2","icon":"🧩","sdk":"1.2","capabilities":{"markdown":true,"embed":["markdown"],"microphone":true,"money":"USD","flows":["backup"],"audio":true}}"#);
    write("index.html", "<h1>x</h1>");
    write(
        "tests/v2.js",
        r#"
T.test('discovery', () => {
  T.eq(iyakki.version, '1.2');
  T.ok(iyakki.has('markdown.render') && iyakki.has('embed') && iyakki.has('money'));
  T.ok(!iyakki.has('k3s') && !iyakki.has('hologram'), 'not declared and not known');
  const c = iyakki.capabilities();
  T.eq(c.sdk, '1.2');
  T.ok(c.namespaces.some((n) => n.name === 'markdown' && n.ready));
  T.ok(c.namespaces.some((n) => n.name === 'k3s' && !n.ready));
});
T.test('a name this host does not know rejects with Unsupported', async () => {
  let err = null;
  try { await iyakki.call('hologram.show', { depth: 3 }); } catch (e) { err = e; }
  T.ok(err && err.code === 'Unsupported' && err.namespace === 'hologram' && err.sdk === '1.2', 'structured: ' + (err && err.message));
  const r = await iyakki.call('markdown.render', { text: '# Title', future: 1 });
  T.eq(r.plain, 'Title');
  T.eq(r.ignored[0], 'future');
});
T.test('markdown, an undeclared namespace and a bad argument', async () => {
  const r = await iyakki.markdown.render('Some **bold** [x](https://e.example.com)', { width: 400 });
  T.eq(r.links[0].url, 'https://e.example.com');
  let e1 = null; try { await iyakki.k3s.status(); } catch (e) { e1 = e; }
  T.eq(e1.code, 'NoCapability');
  let e2 = null; try { await iyakki.markdown.render(5); } catch (e) { e2 = e; }
  T.ok(e2 instanceof TypeError, 'the page checks the type before the call leaves');
});
T.test('an embed handle gets events from the surface and unmounts', async () => {
  const h = await iyakki.embed.mount('doc', 'markdown', { text: '# hi' });
  T.ok(typeof h.id === 'number' && typeof h.update === 'function' && typeof h.unmount === 'function');
  const first = new Promise((resolve) => h.on('scroll', (e) => resolve(e.y)));
  T.fake.v2.event(h.id, 'scroll', { y: 12 });
  T.eq(await first, 12);
  await h.unmount();
  T.eq((await iyakki.embed.list()).items.length, 0);
});
T.test('listening gives text through a handle', async () => {
  T.fake.v2.answer(true);
  const h = await iyakki.speech.listen({ maxSeconds: 5 });
  let text = null;
  const ended = new Promise((resolve) => h.on('result', (e) => { text = e.text; }).on('done', resolve));
  T.fake.v2.heard('turn on the light');
  await ended;
  T.eq(text, 'turn on the light');
});
T.test('money: nothing by default, then the limit the person set', async () => {
  let e = null;
  try { await iyakki.money.spend({ amountMinor: 100, currency: 'USD', purpose: 'test' }); } catch (x) { e = x; }
  T.eq(e.code, 'BudgetStop');
  T.fake.v2.limit('USD', 1000, true);
  T.fake.v2.answer(true);
  const s = await iyakki.money.spend({ amountMinor: 350, currency: 'USD', purpose: 'test' });
  T.eq(s.remainingMinor, 650);
  T.eq((await iyakki.money.limits()).remainingMinor, 650);
  e = null;
  try { await iyakki.money.spend({ amountMinor: 651, currency: 'USD', purpose: 'again' }); } catch (x) { e = x; }
  T.eq(e.code, 'BudgetStop'); T.ok(e.resets_at > 0 && e.remaining === 650, 'the extra fields reach the page');
});
"#,
    );
    let rep = run_tests(&dir).unwrap();
    assert!(rep.ok(), "{}", rep.text());
    assert_eq!(rep.passed, 6, "{}", rep.text());
}

#[test]
fn the_info_panel_lines_name_what_was_allowed_once_and_the_money_limit_and_revoke_takes_the_answers_back() {
    let mut r = rig("info-lines", all());
    assert!(r.sdk.info_lines().is_empty(), "nothing asked, no limit: nothing to say");
    let id = r.card("media.library", json!({}));
    r.answer(id, true).unwrap();
    let lines = r.sdk.info_lines();
    assert!(lines.len() == 1 && lines[0].contains("see the titles of your music and videos") && lines[0].contains("Revoke"), "{lines:?}");
    sdkv2_money::set_limit(&r.dir, "tester", "t", "USD", 1000, None).unwrap();
    let lines = r.sdk.info_lines();
    assert!(lines.iter().any(|l| l.starts_with("Money:") && l.contains("10.00 USD") && l.contains("asks you first")), "{lines:?}");
    r.sdk.revoke();
    let lines = r.sdk.info_lines();
    assert!(!lines.iter().any(|l| l.contains("You said yes")), "{lines:?}");
    // the answer is asked for again
    assert!(r.card("media.library", json!({})) > 0);
}
