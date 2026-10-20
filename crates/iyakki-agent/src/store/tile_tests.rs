//! Tests of tile apps on the real engine: the manifest rules, the bundle and its lint, the sample apps (with real clicks), the bridge
//! (permissions, quotas, rate limit, requests that wait for the person), the network rules, the strict code policy and the
//! scaffold with its own headless tests.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_js::modules::Bridge;
use iyakki_sec::policy::Role;
use iyakki_web::fetch::Response;
use iyakki_web::{Fetch, Url};
use serde_json::json;

use super::tile::*;

fn dist(p: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist").join(p)
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-tile-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn info() -> AppInfo {
    AppInfo { id: "t".into(), name: "T".into(), version: "1.0.0".into(), publisher: "acme".into(), trust: "Community".into(), dev: false }
}

fn bundle(index: &str, extra: &[(&str, &str)], manifest: &str) -> Bundle {
    let mut files: Vec<(&str, &str)> = vec![("index.html", index), ("app.json", manifest)];
    files.extend_from_slice(extra);
    Bundle::from_files(&files, "t").unwrap()
}

struct Web(std::sync::Mutex<Vec<String>>);
impl Fetch for Web {
    fn get(&self, u: &Url) -> Result<Response, String> {
        self.0.lock().unwrap().push(u.host.clone());
        Ok(Response { status: 200, content_type: "application/json".into(), body: b"{\"ok\":1}".to_vec(), ..Default::default() })
    }
}

fn open(b: Bundle, hosts: &[&str]) -> (TileRuntime, Arc<Web>) {
    let web = Arc::new(Web(Default::default()));
    let rt = TileRuntime::open_with(b.clone(), info(), TileStorage::memory(), web.clone(), hosts.iter().map(|h| h.to_string()).collect(), None).unwrap();
    (rt, web)
}

#[test]
fn the_manifest_is_validated_in_words_and_defaults_are_closed() {
    let m = TileManifest::parse(r#"{"id":"calc","name":"Calc","version":"1.2.0","icon":"🧮"}"#).unwrap();
    assert_eq!((m.group.as_str(), m.caps.clone()), ("My apps", Caps::default()), "no group means My apps and nothing is allowed");
    assert_eq!(m.origin_host(), "tile-calc.iyakki.invalid");
    let ok = TileManifest::parse(r#"{"id":"x","name":"X","capabilities":{"storage":true,"network":["API.Example.com"],"clipboard":true,"files":true,"agent":true,"notifications":true}}"#).unwrap();
    assert_eq!(ok.caps.permissions(), ["tile:storage", "tile:clipboard", "tile:notifications", "tile:files", "tile:agent", "net:api.example.com"]);
    assert_eq!(Caps::from_permissions(&ok.caps.permissions()), ok.caps);
    for (bad, want) in [
        (r#"{"id":"Bad Name","name":"X"}"#, "id:"),
        (r#"{"id":"x","name":""}"#, "name:"),
        (r#"{"id":"x","name":"X","version":"one"}"#, "version:"),
        (r#"{"id":"x","name":"X","icon":"calc.exe"}"#, "icon:"),
        (r#"{"id":"x","name":"X","capabilities":{"vault":true}}"#, "may not use the vault"),
        (r#"{"id":"x","name":"X","capabilities":{"js-eval":true}}"#, "code from text"),
        (r#"{"id":"x","name":"X","capabilities":{"network":["*"]}}"#, "plain host name"),
        (r#"{"id":"x","name":"X","capabilities":{"network":["a.com/path"]}}"#, "plain host name"),
        (r#"{"id":"x","name":"X","capabilities":{"root":true}}"#, "unknown capability"),
        (r#"{"id":"x","name":"X","colour":"red"}"#, "unknown field"),
        ("[]", "JSON object"),
        ("{", "not valid JSON"),
    ] {
        let e = TileManifest::parse(bad).unwrap_err();
        assert!(e.contains(want), "{bad} -> {e}");
    }
}

#[test]
fn a_bundle_round_trips_through_its_payload_and_refuses_what_is_not_an_app() {
    let b = Bundle::from_dir(&dist("samples/tiles/notes")).unwrap();
    assert_eq!((b.manifest.id.as_str(), b.manifest.caps.storage), ("notes", true));
    assert!(!b.files.keys().any(|p| p.starts_with("tests/")), "tests are not shipped");
    assert_eq!(Bundle::from_payload(&b.to_payload()).unwrap(), b);
    assert!(Bundle::from_payload("{}").is_err() && Bundle::from_payload("nope").is_err());
    // no index.html, too many files, a file too large, a bad name
    let d = tmp("shape");
    std::fs::write(d.join("app.js"), "1").unwrap();
    assert!(Bundle::from_dir(&d).unwrap_err().contains("index.html"));
    std::fs::write(d.join("index.html"), "<p>x</p>").unwrap();
    assert!(Bundle::from_dir(&d).is_ok(), "no app.json: the folder name and defaults are used");
    std::fs::write(d.join("big.js"), "x".repeat(MAX_FILE_BYTES + 1)).unwrap();
    assert!(Bundle::from_dir(&d).unwrap_err().contains("larger than"));
    std::fs::remove_file(d.join("big.js")).unwrap();
    std::fs::write(d.join("run.exe"), "MZ").unwrap();
    assert!(Bundle::from_dir(&d).unwrap_err().contains("not allowed"));
    std::fs::remove_file(d.join("run.exe")).unwrap();
    std::fs::write(d.join("my file.js"), "1").unwrap();
    assert!(Bundle::from_dir(&d).unwrap_err().contains("file names"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn lint_finds_remote_scripts_eval_missing_icon_undeclared_capabilities_and_secrets() {
    let m = r#"{"id":"t","name":"T","icon":"🧩"}"#;
    let good = bundle("<script src=\"app.js\"></script>", &[("app.js", "var x = 1;")], m);
    assert_eq!(lint(&good).0, Vec::<String>::new());
    let remote = bundle("<script src=\"https://cdn.example.com/lib.js\"></script>", &[], m);
    assert!(lint(&remote).0.iter().any(|p| p.contains("cdn.example.com") && p.contains("capabilities.network")), "{:?}", lint(&remote));
    let declared = bundle("<script src=\"https://cdn.example.com/lib.js\"></script>", &[], r#"{"id":"t","name":"T","icon":"🧩","capabilities":{"network":["cdn.example.com"]}}"#);
    assert!(lint(&declared).0.is_empty(), "a declared host is fine");
    let svg = bundle("<p>x</p>", &[("a.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"/>")], m);
    assert!(lint(&svg).0.is_empty(), "an XML namespace is not a request");
    let noicon = bundle("<p>x</p>", &[], r#"{"id":"t","name":"T"}"#);
    assert!(lint(&noicon).0.iter().any(|p| p.contains("no icon")));
    let badicon = bundle("<p>x</p>", &[], r#"{"id":"t","name":"T","icon":"icon.png"}"#);
    assert!(lint(&badicon).0.iter().any(|p| p.contains("icon file icon.png")));
    let ev = bundle("<p>x</p>", &[("app.js", "var a = eval('1');\nvar f = new Function('return 1');")], m);
    assert_eq!(lint(&ev).0.iter().filter(|p| p.contains("eval and new Function")).count(), 2);
    let fl = bundle("<img src=\"file:///etc/passwd\">", &[], m);
    assert!(lint(&fl).0.iter().any(|p| p.contains("file:")));
    let undeclared = bundle("<p>x</p>", &[("app.js", "iyakki.storage.set('a','b'); iyakki.agent.ask('q');")], m);
    let p = lint(&undeclared).0;
    assert!(p.iter().any(|x| x.contains("capabilities.storage")) && p.iter().any(|x| x.contains("capabilities.agent")), "{p:?}");
    let sec = bundle("<p>x</p>", &[("app.js", "var token = 'ghp_abcdefghijklmnopqrstuvwxyz0123456789';")], m);
    assert!(lint(&sec).0.iter().any(|x| x.contains("access token")), "{:?}", lint(&sec));
    assert!(lint(&bundle("<p>x</p>", &[("app.js", "var v = iyakki.vault.get('x');")], m)).0.iter().any(|x| x.contains("no vault")));
}

#[test]
fn the_calculator_adds_seven_and_eight_with_real_clicks_through_the_drawn_page() {
    let b = Bundle::from_dir(&dist("samples/tiles/calculator")).unwrap();
    let (mut rt, _) = open(b, &[]);
    assert!(rt.page.report.failed == 0, "{:?}", rt.page.report.notes);
    let doc = rt.doc();
    let labels: Vec<&str> = doc.controls.iter().map(|c| c.label.as_str()).collect();
    assert!(["7", "8", "+", "=", "C", "÷", "×"].iter().all(|k| labels.contains(k)), "{labels:?}");
    assert!(rt.page_text().contains("0"), "{}", rt.page_text());
    for k in ["7", "+", "8", "="] {
        rt.press(k).unwrap();
    }
    assert!(rt.page_text().contains("15"), "{}", rt.page_text());
    rt.press("C").unwrap();
    rt.press("9").unwrap();
    rt.press("÷").unwrap();
    rt.press("0").unwrap();
    rt.press("=").unwrap();
    assert!(rt.page_text().contains("Error"), "{}", rt.page_text());
    assert!(rt.press("nope").is_err());
}

#[test]
fn every_sample_app_passes_its_own_headless_tests() {
    for name in ["calculator", "notes", "converter"] {
        let r = run_tests(&dist(&format!("samples/tiles/{name}"))).unwrap();
        assert!(r.ok(), "{name}: {}", r.text());
    }
}

#[test]
fn the_scaffold_passes_validate_and_its_headless_test_and_a_broken_app_fails() {
    let d = tmp("scaffold");
    let said = super::author::scaffold("tile", "my-app", &d, "").unwrap();
    assert!(said.contains("tile_test"), "{said}");
    let dir = d.join("my-app");
    for f in ["index.html", "app.js", "style.css", "app.json", "iyakki-app.json", "README.md", "tests/app.test.js"] {
        assert!(dir.join(f).is_file(), "{f}");
    }
    let rep = super::vet::vet_dir(&dir);
    assert!(rep.ok(), "{}", rep.text());
    assert!(rep.text().contains("tile app my-app"), "{}", rep.text());
    let t = run_tests(&dir).unwrap();
    assert!(t.ok(), "{}", t.text());
    // break it: the test names what failed
    std::fs::write(dir.join("app.js"), "document.getElementById('message').textContent = 'wrong';").unwrap();
    let t = run_tests(&dir).unwrap();
    assert!(!t.ok() && t.text().contains("FAIL"), "{}", t.text());
    // lint problems stop packing
    std::fs::write(dir.join("app.js"), "var a = eval('1');").unwrap();
    assert!(!super::vet::vet_dir(&dir).ok());
    assert!(super::author::pack_dir(&dir, None).is_err());
    std::fs::write(dir.join("app.js"), "var a = 1;").unwrap();
    let (text, _) = super::author::pack_dir(&dir, None).unwrap();
    let p = crate::pkg::parse(&text).unwrap();
    assert_eq!((p.manifest.kind.as_str(), p.manifest.name.as_str()), ("tile", "my-app"));
    assert_eq!(Bundle::from_payload(&p.payload).unwrap().manifest.version, "0.1.0");
    let _ = std::fs::remove_dir_all(&d);
}

fn host(caps: Caps) -> (TileHost, Arc<std::sync::Mutex<Outbox>>) {
    let out = Arc::new(std::sync::Mutex::new(Outbox::default()));
    (TileHost::new(info(), caps, TileStorage::memory(), out.clone()), out)
}

#[test]
fn the_bridge_refuses_what_was_not_declared_and_validates_every_argument() {
    let (mut h, out) = host(Caps::default());
    assert_eq!(h.call("app.info", &json!({})).unwrap()["capabilities"], json!([]));
    for op in ["storage.get", "storage.set", "storage.list", "host.openFile", "clipboard.read", "notify.show", "agent.ask"] {
        let e = h.call(op, &json!({"k": "a", "v": "b", "q": "?", "title": "t", "name": "n", "text": "x"})).unwrap_err();
        assert!(e.contains("did not ask for"), "{op}: {e}");
    }
    assert!(!out.lock().unwrap().refused.is_empty(), "refusals are kept for the info panel");
    assert!(h.call("vault.get", &json!({})).unwrap_err().contains("no such bridge call"));
    assert!(h.call("storage", &json!({})).is_err());
    let (mut h, _) = host(Caps::all());
    for (op, a) in [("storage.get", json!({"k": "bad key"})), ("storage.get", json!({"k": 5})), ("storage.set", json!({"k": "a"})), ("storage.set", json!({"k": "a", "v": "x".repeat(STORAGE_VALUE + 1)})), ("host.saveFile", json!({"name": "../x", "text": "t"})), ("host.saveFile", json!({"name": ".hidden", "text": "t"})), ("ui.toast", json!({"text": "x".repeat(201)}))] {
        assert!(h.call(op, &a).is_err(), "{op} {a}");
    }
}

#[test]
fn storage_has_quotas_and_lives_in_a_private_file_the_lifecycle_layer_can_offer_to_delete() {
    let (mut h, _) = host(Caps { storage: true, ..Caps::default() });
    h.call("storage.set", &json!({"k": "a", "v": "1"})).unwrap();
    assert_eq!(h.call("storage.get", &json!({"k": "a"})).unwrap(), json!("1"));
    assert_eq!(h.call("storage.get", &json!({"k": "zzz"})).unwrap(), serde_json::Value::Null);
    assert_eq!(h.call("storage.list", &json!({})).unwrap(), json!(["a"]));
    h.call("storage.remove", &json!({"k": "a"})).unwrap();
    // the total quota
    for i in 0..4 {
        h.call("storage.set", &json!({"k": format!("big{i}"), "v": "x".repeat(60_000)})).unwrap();
    }
    let e = h.call("storage.set", &json!({"k": "big4", "v": "x".repeat(60_000)})).unwrap_err();
    assert!(e.contains("QuotaExceeded") && e.contains("256 KB"), "{e}");
    h.call("storage.set", &json!({"k": "big0", "v": "y"})).unwrap();
    // the key count
    let mut s = TileStorage::memory();
    for i in 0..STORAGE_KEYS {
        s.set(&format!("k{i}"), "v").unwrap();
    }
    assert!(s.set("one-more", "v").unwrap_err().contains("keys"));
    // persisted, mode 0600
    let d = tmp("store");
    let f = d.join("appdata/tile/user_alice/t/alice.json");
    let mut p = TileStorage::open(&f);
    p.set("a", "b").unwrap();
    assert_eq!(TileStorage::open(&f).get("a").as_deref(), Some("b"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn bridge_calls_are_rate_limited_per_second() {
    let out = Arc::new(std::sync::Mutex::new(Outbox::default()));
    let now = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let n2 = now.clone();
    let mut h = TileHost::new(info(), Caps::default(), TileStorage::memory(), out).with_clock(move || n2.load(std::sync::atomic::Ordering::SeqCst));
    for _ in 0..RATE_PER_SEC {
        h.call("app.info", &json!({})).unwrap();
    }
    assert!(h.call("app.info", &json!({})).unwrap_err().contains("too many calls"));
    now.store(1500, std::sync::atomic::Ordering::SeqCst);
    assert!(h.call("app.info", &json!({})).is_ok(), "a new second, a new allowance");
}

#[test]
fn agent_ask_and_file_dialogs_wait_for_the_person_and_only_the_host_can_settle_them() {
    let m = r#"{"id":"t","name":"T","icon":"🧩","capabilities":{"agent":true,"files":true}}"#;
    let b = bundle("<script>globalThis.got = []; iyakki.agent.ask('what is 2+2?').then(function (a) { got.push('yes ' + a); }, function (e) { got.push('no ' + e.message); });</script>", &[], m);
    let (mut rt, _) = open(b, &[]);
    let reqs = rt.take_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!((reqs[0].kind.as_str(), reqs[0].args["q"].as_str()), ("agent.ask", Some("what is 2+2?")));
    assert_eq!(rt.page.eval("t", "got.join()").unwrap(), "", "nothing happened without the person's answer");
    rt.resolve(reqs[0].id, false, "the person said no");
    assert_eq!(rt.page.eval("t", "got.join()").unwrap(), "no the person said no");
    assert!(rt.take_requests().is_empty());
    // at most MAX_PENDING requests wait at once
    let (mut h, _) = host(Caps::all());
    for _ in 0..MAX_PENDING {
        assert!(h.call("agent.ask", &json!({"q": "x"})).unwrap()["pending"].is_number());
    }
    assert!(h.call("agent.ask", &json!({"q": "x"})).unwrap_err().contains("too many requests"));
}

#[test]
fn the_network_is_closed_unless_declared_and_the_persons_policy_allows_the_host() {
    let page = |m: &str| bundle("<script>globalThis.r = []; fetch('https://api.example.com/x').then(function (x) { return x.text(); }).then(function (t) { r.push('ok ' + t); }, function (e) { r.push('blocked'); });</script>", &[], m);
    let settle = |rt: &mut TileRuntime| {
        rt.page.advance(1000);
        rt.page.eval("r", "r.join()").unwrap()
    };
    // nothing declared: blocked, and the real network was never asked
    let (mut rt, web) = open(page(r#"{"id":"t","name":"T","icon":"🧩"}"#), &[]);
    assert_eq!(settle(&mut rt), "blocked");
    assert!(web.0.lock().unwrap().is_empty());
    // declared but the policy does not allow it: the runtime is given no hosts
    let m = bundle("x", &[], r#"{"id":"t","name":"T","icon":"🧩","capabilities":{"network":["api.example.com"]}}"#).manifest;
    let mut policy = Role::User.template();
    policy.net_hosts = vec!["other.org".into()];
    assert!(reachable_hosts(&m, &policy).is_empty());
    policy.net_hosts = vec!["api.example.com".into()];
    assert_eq!(reachable_hosts(&m, &policy), ["api.example.com"]);
    policy.net_hosts = vec!["*".into()];
    assert_eq!(reachable_hosts(&m, &policy), ["api.example.com"], "declared hosts only, even for an account that may reach everything");
    // declared and allowed: it goes through, and another host still does not
    let decl = page(r#"{"id":"t","name":"T","icon":"🧩","capabilities":{"network":["api.example.com"]}}"#);
    let (mut rt, web) = open(decl, &["api.example.com"]);
    assert!(settle(&mut rt).starts_with("ok"), "{}", settle(&mut rt));
    assert_eq!(web.0.lock().unwrap().as_slice(), ["api.example.com"]);
    let other = bundle("<script>globalThis.r = []; fetch('https://evil.example.org/x').then(function () { r.push('ok'); }, function () { r.push('blocked'); });</script>", &[], r#"{"id":"t","name":"T","icon":"🧩","capabilities":{"network":["api.example.com"]}}"#);
    let (mut rt, web) = open(other, &["api.example.com"]);
    assert_eq!(settle(&mut rt), "blocked");
    assert!(web.0.lock().unwrap().is_empty());
}

#[test]
fn the_app_has_its_own_origin_and_cannot_make_code_from_text_or_reach_the_bridge_internals() {
    let m = r#"{"id":"t","name":"T","icon":"🧩"}"#;
    let b = bundle("<p>hi</p>", &[], m);
    let (mut rt, _) = open(b, &[]);
    assert_eq!(rt.page.eval("t", "location.origin").unwrap(), "https://tile-t.iyakki.invalid");
    assert!(rt.page.eval("t", "eval('1')").is_err());
    assert!(rt.page.eval("t", "new Function('return 1')").is_err());
    // the bridge is the only door: calling the host directly gets the same checks
    assert!(rt.page.eval("t", "__host('iyakki', 'storage.get', '{\"k\":\"a\"}')").is_err(), "storage was not declared");
    assert!(rt.page.eval("t", "__host('iyakki', 'vault.get', '{}')").is_err());
    assert_eq!(rt.page.eval("t", "typeof window.localStorage").unwrap(), "object", "the standard web storage is the page's own, in memory");
    // a second app shares nothing: another origin, another storage
    let (mut other, _) = open(bundle("<p>hi</p>", &[], r#"{"id":"u","name":"U","icon":"🧩"}"#), &[]);
    assert_eq!(other.page.eval("t", "location.host").unwrap(), "tile-u.iyakki.invalid");
}

#[test]
fn ui_calls_reach_the_shell_and_a_page_with_a_stylesheet_is_laid_out_with_its_colours() {
    let m = r#"{"id":"t","name":"T","icon":"🧩"}"#;
    let b = bundle("<link rel=\"stylesheet\" href=\"s.css\"><body><p id=\"p\">hello</p><script>iyakki.ui.toast('saved'); iyakki.ui.title('Hello app');</script></body>", &[("s.css", "body { background: #102030; } p { color: #ff0000; }")], m);
    let (mut rt, _) = open(b, &[]);
    {
        let o = rt.out.lock().unwrap();
        assert_eq!((o.toasts.as_slice(), o.title.as_deref()), (["saved".to_string()].as_slice(), Some("Hello app")));
    }
    let doc = rt.doc();
    assert_eq!(doc.canvas, Some([0x10, 0x20, 0x30, 255]), "the page's own background reaches the view");
    assert!(rt.page_text().contains("hello"));
}
