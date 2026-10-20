// #[cfg(test)] -- this whole file is test code (it is included under #[cfg(test)] from flows/mod.rs); the marker keeps the BOUNDARY check from counting it.
//! Tests of the browser nodes: every node kind against a local fixture web server (login form, select, checkbox, delayed element,
//! console errors, redirect, 404, iframe) with the in-house JavaScript engine as the page engine, host policy, approvals,
//! secrets, budgets, the session's life, the report. Nothing here goes beyond 127.0.0.1. The tests that need a real WebKit helper
//! are in the `live` module at the bottom (ignored by default).

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_flow::host::*;
use iyakki_sec::policy::{BrowserAgent, Policy, Role};
use serde_json::{json, Value};

use iyakki_flow::{Edge, Flow, Node, NodeState, RetryPolicy, Run, RunStatus, StartRequest};

use super::qa_fixture::*;
use super::*;

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

pub(super) fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-qa-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn admin_policy() -> Policy {
    let mut p = Role::Admin.template();
    p.browser_agent = BrowserAgent::Interactive;
    p.net_hosts = vec!["*".into()];
    p.browser_max_actions = 200;
    p.browser_max_minutes = 30;
    p
}

struct Qa {
    env: Arc<FlowEnv>,
    fx: Fixture,
    dir: PathBuf,
}

impl Drop for Qa {
    fn drop(&mut self) {
        self.env.host.browser_sessions.drop_all();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn qa_with(tag: &str, policy: Policy) -> Qa {
    let dir = tmp(tag);
    let svc = Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let user = format!("qatester{}", tag.replace(|c: char| !c.is_ascii_alphanumeric(), ""));
    let env = Arc::new(FlowEnv::build_in(&svc, &user, &policy, false, &dir.join("home")));
    env.host.browser_factory(double_factory((320, 240)));
    Qa { env, fx: Fixture::start(), dir }
}

fn qa(tag: &str) -> Qa {
    qa_with(tag, admin_policy())
}

impl Qa {
    fn host(&self) -> &AgentHost {
        &self.env.host
    }
    fn url(&self, p: &str) -> String {
        self.fx.url(p)
    }
    fn op_in(&self, run: &str, trusted: bool, hosts: &[&str], op: &str, args: Value) -> Result<BrowserOpResult, iyakki_flow::NodeError> {
        let req = BrowserOp { op: op.into(), args, timeout_ms: 8000, flow_id: "qaflow".into(), run_id: run.into(), node_id: format!("n-{op}"), trusted, profile: "isolated".into(), hosts: hosts.iter().map(|s| s.to_string()).collect(), ..Default::default() };
        self.host().browser_op(&req, &Cancel::new())
    }
    fn op(&self, op: &str, args: Value) -> Result<BrowserOpResult, iyakki_flow::NodeError> {
        self.op_in("run1", true, &[], op, args)
    }
    fn ok(&self, op: &str, args: Value) -> BrowserOpResult {
        self.op(op, args).unwrap_or_else(|e| panic!("{op} failed: {e:?}"))
    }
    fn open(&self, path: &str) -> BrowserOpResult {
        self.ok("open", json!({"url": self.url(path), "wait": "idle"}))
    }
}

fn png_size(path: &str) -> (u32, u32) {
    let b = std::fs::read(path).unwrap();
    assert_eq!(&b[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a], "a PNG file");
    assert_eq!(&b[12..16], b"IHDR");
    (u32::from_be_bytes(b[16..20].try_into().unwrap()), u32::from_be_bytes(b[20..24].try_into().unwrap()))
}

// ------------------------------------------------------------------------------------------------ open, dom, screenshot

#[test]
fn open_loads_a_page_and_says_where_it_landed_even_after_a_redirect() {
    let q = qa("open");
    let r = q.open("/login");
    assert_eq!(r.value["title"], "Sign in");
    assert!(r.value["url"].as_str().unwrap().ends_with("/login"));
    let r = q.open("/redirect");
    assert!(r.value["url"].as_str().unwrap().ends_with("/login"), "the redirect was followed: {}", r.value);
    assert!(q.fx.hits.lock().unwrap().iter().any(|l| l.starts_with("GET /redirect ")), "the fixture saw the request");
    // a 404 is a page like any other: the node does not fail, a check on its text does
    let r = q.open("/missing-page");
    assert_eq!(r.value["title"], "Not found");
    let c = q.ok("assert", json!({"check": "text_contains", "selector": "h1", "expected": "Not here"}));
    assert_eq!(c.value["passed"], true);
}

#[test]
fn addresses_with_credentials_other_schemes_and_blocked_hosts_are_refused() {
    let q = qa("addr");
    for bad in ["http://user:pw@127.0.0.1/", "file:///etc/passwd", "javascript:alert(1)", "ftp://127.0.0.1/x"] {
        let e = q.op("open", json!({"url": bad})).unwrap_err();
        assert!(e.config, "{bad}: {e:?}");
    }
    let e = q.op("open", json!({"url": "http://user:pw@127.0.0.1/"})).unwrap_err();
    assert!(e.message.contains("no user name or password"), "{}", e.message);
    // the account's host policy: only one host, so another is refused before anything is opened
    let mut p = admin_policy();
    p.net_hosts = vec!["example.org".into()];
    let q2 = qa_with("addr2", p);
    let e = q2.op("open", json!({"url": "http://example.org/"}));
    assert!(e.is_ok() || e.unwrap_err().message.contains("could not be opened"), "an allowed host passes the policy");
    let e = q2.op_in("run2", true, &[], "open", json!({"url": "http://blocked.test/"})).unwrap_err();
    assert!(e.config && e.message.contains("host policy"), "{}", e.message);
    // loopback addresses are always allowed
    q2.ok("open", json!({"url": q2.url("/login")}));
}

#[test]
fn an_untrusted_run_may_only_use_the_hosts_it_was_approved_for() {
    let q = qa("hosts");
    q.host().browser_sessions.ask_wait.lock().unwrap().replace(Duration::from_millis(1500));
    // the person answers "allow" as soon as the question appears
    let svc = q.env.svc.clone();
    let user = q.env.user.clone();
    let t = std::thread::spawn(move || {
        for _ in 0..200 {
            if let Some(r) = svc.browser.consent.pending(&user).first() {
                assert!(r.text.contains("127.0.0.1"), "the question lists the hosts: {}", r.text);
                assert!(r.text.contains("private browser session"), "{}", r.text);
                svc.browser.consent.resolve(&user, &r.id, crate::browserconsent::Answer::Allow).unwrap();
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    });
    let r = q.op_in("run3", false, &["127.0.0.1"], "open", json!({"url": q.url("/login")}));
    assert!(t.join().unwrap(), "a question was asked");
    r.unwrap();
    // asked once: the second node does not ask again
    q.op_in("run3", false, &["127.0.0.1"], "open", json!({"url": q.url("/form")})).unwrap();
    assert!(q.env.svc.browser.consent.pending(&q.env.user).is_empty());
    // an address the person did not approve (not loopback, not listed) is refused even though the policy allows it
    let e = q.op_in("run3", false, &["127.0.0.1"], "open", json!({"url": "http://other.example/"})).unwrap_err();
    assert!(e.message.contains("not one of the sites this run was approved for"), "{}", e.message);
}

#[test]
fn nobody_answering_or_a_no_means_the_browser_steps_do_not_run_and_a_trusted_flow_never_asks() {
    let q = qa("noask");
    q.host().browser_sessions.ask_wait.lock().unwrap().replace(Duration::from_millis(200));
    let e = q.op_in("runA", false, &["127.0.0.1"], "open", json!({"url": q.url("/login")})).unwrap_err();
    assert!(e.config && e.message.contains("Nobody answered"), "{}", e.message);
    assert!(q.host().browser_sessions.get("runA").is_none(), "no session was made");
    // a no
    let svc = q.env.svc.clone();
    let user = q.env.user.clone();
    q.host().browser_sessions.ask_wait.lock().unwrap().replace(Duration::from_millis(1500));
    let t = std::thread::spawn(move || {
        for _ in 0..200 {
            if let Some(r) = svc.browser.consent.pending(&user).first() {
                svc.browser.consent.resolve(&user, &r.id, crate::browserconsent::Answer::Deny).unwrap();
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let e = q.op_in("runB", false, &["127.0.0.1"], "open", json!({"url": q.url("/login")})).unwrap_err();
    t.join().unwrap();
    assert!(e.config && e.message.contains("did not allow"), "{}", e.message);
    // trusted: no question at all (the pending list stays empty the whole time)
    q.op_in("runC", true, &[], "open", json!({"url": q.url("/login")})).unwrap();
    assert!(q.env.svc.browser.consent.pending(&q.env.user).is_empty());
    // the person's own profile is asked even for a trusted flow
    q.host().browser_sessions.ask_wait.lock().unwrap().replace(Duration::from_millis(200));
    let req = BrowserOp { op: "open".into(), args: json!({"url": q.url("/login")}), timeout_ms: 5000, run_id: "runD".into(), trusted: true, profile: "person".into(), ..Default::default() };
    let e = q.host().browser_op(&req, &Cancel::new()).unwrap_err();
    assert!(e.message.contains("Nobody answered"), "{}", e.message);
}

#[test]
fn get_dom_returns_title_text_and_attributes_with_passwords_and_tokens_removed_and_a_size_cap() {
    let q = qa("dom");
    q.open("/login");
    let r = q.ok("dom", json!({"mode": "page"}));
    let html = r.value["html"].as_str().unwrap();
    assert!(html.contains("<form") && html.contains("Sign in"), "{html}");
    assert!(html.contains(r#"type="password""#));
    assert_eq!(r.value["title"], "Sign in");
    // the password field's value is gone from the dump even when something was typed
    q.ok("type", json!({"selector": "#pw", "text": "hunter2-typed"}));
    let r = q.ok("dom", json!({"mode": "page"}));
    assert!(!r.value.to_string().contains("hunter2-typed"), "no typed password in the dump");
    // tokens in hidden fields and data attributes
    q.open("/table");
    let r = q.ok("dom", json!({"mode": "page"}));
    let html = r.value["html"].as_str().unwrap();
    assert!(!html.contains("TOKEN-ABC-123") && !html.contains("DATA-TOKEN-999"), "{html}");
    assert!(html.contains("[redacted]"));
    // text, outer html, attributes of a selector
    let t = q.ok("dom", json!({"mode": "text", "selector": ".price"}));
    assert_eq!(t.value["text"], "$12.50");
    let o = q.ok("dom", json!({"mode": "outer_html", "selector": "a.buy"}));
    assert!(o.value["outer_html"].as_str().unwrap().contains("href=\"/buy?item=1\""));
    let at = q.ok("dom", json!({"mode": "attributes", "selector": "a.buy"}));
    assert_eq!(at.value["attributes"]["href"], "/buy?item=1");
    // a selector that matches nothing says so
    let e = q.op("dom", json!({"mode": "text", "selector": "#nope"})).unwrap_err();
    assert!(e.message.contains("no element matches \"#nope\""), "{}", e.message);
    // the size cap
    q.open("/login");
    let r = q.ok("dom", json!({"mode": "page", "max_bytes": 1000}));
    assert_eq!(r.value["truncated"], true, "{}", r.value["bytes"]);
    assert!(r.value["html"].as_str().unwrap().len() <= 1000, "{}", r.value["html"].as_str().unwrap().len());
    assert!(r.value["bytes"].as_u64().unwrap() > 1000, "the real size is reported");
    // links
    q.open("/table");
    let l = q.ok("dom", json!({"mode": "summary", "links": true}));
    assert!(l.value["links"][0]["href"].as_str().unwrap().ends_with("/buy?item=1"), "{}", l.value);
}

#[test]
fn console_messages_and_script_errors_are_collected_with_the_page_they_came_from() {
    let q = qa("console");
    q.open("/errors");
    // what the page prints while it loads is printed before the QA script is in the page (the engine has no early hook)
    q.ok("click", json!({"selector": "#boom"}));
    q.ok("wait", json!({"for": "time", "ms": 400}));
    let r = q.ok("dom", json!({"mode": "summary", "console": true}));
    let texts: Vec<String> = r.value["console"].as_array().unwrap().iter().map(|m| format!("{}: {}", m["level"].as_str().unwrap(), m["text"].as_str().unwrap())).collect();
    assert!(texts.iter().any(|t| t.contains("error: boom from click")), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("error: boom from timer")), "a message from a timer after the load: {texts:?}");
    assert!(texts.iter().any(|t| t.contains("warn: careful")), "{texts:?}");
    let c = q.op("assert", json!({"check": "no_console_errors", "wait_ms": 0})).unwrap();
    assert_eq!(c.value["passed"], false, "{}", c.value);
    assert!(c.value["message"].as_str().unwrap().contains("boom from"), "{}", c.value);
    // a clean page has none
    let q2 = qa("console2");
    q2.open("/form");
    assert_eq!(q2.ok("assert", json!({"check": "no_console_errors", "wait_ms": 0})).value["passed"], true);
}

#[test]
fn a_screenshot_is_a_valid_png_of_the_viewport_size_saved_as_a_named_artifact() {
    let q = qa("shot");
    q.open("/login");
    let r = q.ok("screenshot", json!({"name": "login page"}));
    let path = r.value["path"].as_str().unwrap();
    assert_eq!(png_size(path), (320, 240));
    assert_eq!((r.value["width"].as_u64(), r.value["height"].as_u64()), (Some(320), Some(240)));
    assert!(path.contains("login_page"), "the name is in the file name: {path}");
    assert!(path.chars().filter(|c| c.is_ascii_digit()).count() >= 14, "a timestamp is in it: {path}");
    assert_eq!(r.artifacts, vec![path.to_string()], "it is an artifact of the node");
    assert!(!r.value["url"].as_str().unwrap().contains('?'), "no query string in the metadata");
    // the viewport node changes the size of the next picture
    let v = q.ok("viewport", json!({"preset": "phone 375x667"}));
    assert_eq!((v.value["width"].as_u64(), v.value["height"].as_u64()), (Some(375), Some(667)));
    let r2 = q.ok("screenshot", json!({"name": "phone"}));
    assert_eq!(png_size(r2.value["path"].as_str().unwrap()), (375, 667));
    assert_ne!(r.value["path"], r2.value["path"]);
    // too big or too small is refused
    let e = q.op("viewport", json!({"width": 5000, "height": 800})).unwrap_err();
    assert!(e.config, "{e:?}");
    // the preview picture of the run view exists after the actions
    let info = q.host().browser_session("run1").expect("a session");
    assert!(info.preview.is_some_and(|p| std::path::Path::new(&p).is_file()));
}

// ------------------------------------------------------------------------------------------------ checks

#[test]
fn checks_pass_and_fail_with_clear_messages() {
    let q = qa("assert");
    q.open("/table");
    let chk = |check: &str, sel: &str, expected: Value, attr: &str| -> (bool, String) {
        let r = q.ok("assert", json!({"check": check, "selector": sel, "expected": expected, "attribute": attr, "wait_ms": 0}));
        (r.value["passed"].as_bool().unwrap(), r.value["message"].as_str().unwrap().to_string())
    };
    assert!(chk("exists", ".price", json!(null), "").0);
    let (ok, m) = chk("exists", "#nothing", json!(null), "");
    assert!(!ok && m == "expected \"#nothing\" to exist but no element matches it", "{m}");
    assert!(chk("not_exists", "#nothing", json!(null), "").0);
    assert!(chk("count", "tr", json!(4), "").0);
    let (ok, m) = chk("count", "tr", json!(9), "");
    assert!(!ok && m.contains("expected \"tr\" to match 9 element(s) but it matches 4"), "{m}");
    assert!(chk("text_equals", ".price", json!("$12.50"), "").0);
    let (ok, m) = chk("text_equals", ".price", json!("$13"), "");
    assert!(!ok && m.contains("to equal \"$13\" but it is \"$12.50\""), "{m}");
    assert!(chk("text_contains", "h1", json!("Pri"), "").0);
    assert!(chk("text_matches", ".price", json!("^\\$\\d+\\.\\d\\d$"), "").0);
    let (ok, m) = chk("text_matches", ".price", json!("^\\d+$"), "");
    assert!(!ok && m.contains("to match /^\\d+$/"), "{m}");
    assert!(chk("attr_equals", "a.buy", json!("/buy?item=1"), "href").0);
    assert!(chk("attr_contains", "a.buy", json!("item"), "href").0);
    let (ok, m) = chk("attr_equals", "a.buy", json!("/x"), "href");
    assert!(!ok && m.contains("expected href of \"a.buy\" to equal \"/x\" but it is \"/buy?item=1\""), "{m}");
    let (ok, m) = chk("attr_equals", "a.buy", json!("x"), "nope");
    assert!(!ok && m.contains("to have the attribute nope"), "{m}");
    assert!(chk("visible", "h1", json!(null), "").0);
    assert!(chk("enabled", "a.buy", json!(null), "").0);
    // page level checks
    let r = q.ok("assert", json!({"check": "url_contains", "expected": "/table", "wait_ms": 0}));
    assert_eq!(r.value["passed"], true);
    let r = q.ok("assert", json!({"check": "url_equals", "expected": "http://nowhere/", "wait_ms": 0}));
    assert!(!r.value["passed"].as_bool().unwrap() && r.value["message"].as_str().unwrap().contains("expected the address to be http://nowhere/ but it is http://127.0.0.1:"), "{}", r.value);
    assert_eq!(q.ok("assert", json!({"check": "title_equals", "expected": "Prices", "wait_ms": 0})).value["passed"], true);
    assert_eq!(q.ok("assert", json!({"check": "title_contains", "expected": "Price", "wait_ms": 0})).value["passed"], true);
    assert_eq!(q.ok("assert", json!({"check": "page_contains", "expected": "apple", "wait_ms": 0})).value["passed"], true);
    assert_eq!(q.ok("assert", json!({"check": "page_not_contains", "expected": "kiwi", "wait_ms": 0})).value["passed"], true);
    // a wrong pattern is a configuration error, not a failed check
    let e = q.op("assert", json!({"check": "text_matches", "selector": "h1", "expected": "(", "wait_ms": 0})).unwrap_err();
    assert!(e.message.contains("pattern is not valid"), "{}", e.message);
}

#[test]
fn a_check_keeps_looking_until_a_delayed_element_appears() {
    let q = qa("delayed");
    q.open("/delayed");
    let t = std::time::Instant::now();
    let r = q.ok("assert", json!({"check": "exists", "selector": "#late", "wait_ms": 4000}));
    assert_eq!(r.value["passed"], true, "{}", r.value);
    assert!(t.elapsed() < Duration::from_secs(4), "it did not wait the whole time");
    // wait for text, selector gone, time
    q.open("/delayed");
    q.ok("wait", json!({"for": "text", "value": "Loaded late"}));
    q.open("/delayed");
    q.ok("wait", json!({"for": "selector", "value": "#late"}));
    let e = q.op("wait", json!({"for": "selector", "value": "#never"}));
    // the node's time limit (8 s here) is long; shorten it for the failing wait
    drop(e);
    let req = BrowserOp { op: "wait".into(), args: json!({"for": "selector", "value": "#never"}), timeout_ms: 1200, run_id: "run1".into(), trusted: true, profile: "isolated".into(), ..Default::default() };
    let e = q.host().browser_op(&req, &Cancel::new()).unwrap_err();
    assert!(e.timed_out && e.message.contains("timed out") && e.message.contains("#never"), "{e:?}");
    let t = std::time::Instant::now();
    q.ok("wait", json!({"for": "time", "ms": 150}));
    assert!(t.elapsed() >= Duration::from_millis(140));
    q.ok("wait", json!({"for": "url", "value": "/delayed"}));
}

// ------------------------------------------------------------------------------------------------ acting

#[test]
fn click_type_fill_select_submit_and_press_work_on_the_login_form() {
    let q = qa("act");
    q.open("/login");
    // a failing login first
    q.ok("type", json!({"selector": "#email", "text": "qa@example.com"}));
    q.ok("type", json!({"selector": "#pw", "text": "wrong"}));
    q.ok("click", json!({"selector": "#go"}));
    assert_eq!(q.ok("assert", json!({"check": "text_equals", "selector": "#result", "expected": "Invalid login", "wait_ms": 1000})).value["passed"], true);
    // fill form: input, password, checkbox, select; then submit with a click
    let r = q.ok("fill", json!({"fields": {"#email": "qa@example.com", "#pw": "s3cret-pass", "#remember": true, "#role": "QA engineer"}, "submit": "#go"}));
    assert_eq!(r.value["filled"].as_array().unwrap().len(), 4);
    let c = q.ok("assert", json!({"check": "text_equals", "selector": "#result", "expected": "Welcome, qa@example.com (remembered)", "wait_ms": 1000}));
    assert_eq!(c.value["passed"], true, "{}", c.value);
    assert_eq!(q.ok("assert", json!({"check": "checked", "selector": "#remember"})).value["passed"], true);
    assert_eq!(q.ok("assert", json!({"check": "value_equals", "selector": "#role", "expected": "qa"})).value["passed"], true);
    // submit node and Enter in a field
    q.ok("type", json!({"selector": "#email", "text": "again@example.com"}));
    q.ok("submit", json!({"selector": "#login"}));
    assert_eq!(q.ok("assert", json!({"check": "text_contains", "selector": "#result", "expected": "again@example.com", "wait_ms": 1000})).value["passed"], true);
    q.ok("type", json!({"selector": "#email", "text": "enter@example.com"}));
    q.ok("key", json!({"key": "Enter", "selector": "#email"}));
    assert_eq!(q.ok("assert", json!({"check": "text_contains", "selector": "#result", "expected": "enter@example.com", "wait_ms": 1000})).value["passed"], true);
    // hover and scroll run without trouble
    q.ok("hover", json!({"selector": "#go"}));
    q.ok("scroll", json!({"to": "bottom"}));
    q.ok("scroll", json!({"to": "selector", "selector": "#go"}));
    // select by value, by index, by wrong name
    q.open("/form");
    assert_eq!(q.ok("select", json!({"selector": "#color", "option": "Blue"})).value["value"], "b");
    assert_eq!(q.ok("select", json!({"selector": "#color", "option": "g", "by": "value"})).value["selected"], "Green");
    assert_eq!(q.ok("select", json!({"selector": "#color", "option": "0", "by": "index"})).value["selected"], "Red");
    let e = q.op("select", json!({"selector": "#color", "option": "Purple"})).unwrap_err();
    assert!(e.message.contains("no option \"Purple\"") && e.message.contains("\"Red\""), "{}", e.message);
    // checkbox and radio through fill, disabled field refused, then the page reads them back
    q.ok("fill", json!({"fields": {"#news": true, "#b": true, "#note": "hello"}}));
    q.ok("click", json!({"selector": "#save"}));
    assert_eq!(q.ok("assert", json!({"check": "text_equals", "selector": "#saved", "expected": "color=r news=true plan=pro", "wait_ms": 500})).value["passed"], true, "{}", q.ok("assert", json!({"check": "text_equals", "selector": "#saved", "expected": "x", "wait_ms": 0})).value);
    let e = q.op("type", json!({"selector": "#locked", "text": "x"})).unwrap_err();
    assert!(e.message.contains("disabled"), "{}", e.message);
    let e = q.op("click", json!({"selector": "#nothing"})).unwrap_err();
    assert!(e.message.contains("no element matches"), "{}", e.message);
    let e = q.op("fill", json!({"fields": {"#email": "x"}})).unwrap_err();
    assert!(e.message.contains("#email") && e.message.contains("no element"), "{}", e.message);
}

#[test]
fn file_inputs_are_refused_and_a_path_outside_the_read_scope_says_so() {
    let q = qa("file");
    q.open("/login");
    q.ok("dom", json!({"mode": "summary"}));
    // there is no file input on the fixture: the clear message for a missing field
    let e = q.op("fill", json!({"fields": {"input[type=file]": "/etc/passwd"}})).unwrap_err();
    assert!(e.message.contains("no element matches"), "{}", e.message);
}

#[test]
fn extract_reads_fields_lists_and_table_rows_into_json() {
    let q = qa("extract");
    q.open("/table");
    let r = q.ok("extract", json!({"fields": {"price": ".price", "link": {"selector": "a.buy", "attr": "href"}, "tags": {"selector": ".tag", "all": true}, "missing": "#none", "token": {"selector": "input[name=csrf_token]", "attr": "value"}}, "rows": {"selector": "tr", "columns": {"name": "td:nth-child(1)", "qty": "td:nth-child(2)"}}}));
    assert_eq!(r.value["data"]["price"], "$12.50");
    assert_eq!(r.value["price"], "$12.50", "also a top-level field for the next node");
    assert_eq!(r.value["data"]["link"], "/buy?item=1");
    assert_eq!(r.value["data"]["tags"], json!(["a", "b"]));
    assert_eq!(r.value["data"]["missing"], Value::Null);
    assert_eq!(r.value["rows"], json!([{"name": "apple", "qty": "3"}, {"name": "pear", "qty": "5"}, {"name": "plum", "qty": "8"}]));
    assert_eq!(r.value["data"]["token"], "[redacted]", "a token-like hidden field is not extracted");
    let e = q.op("extract", json!({"fields": {"x": "p["}})).unwrap_err();
    assert!(e.message.contains("not a valid selector"), "{}", e.message);
}

#[test]
fn javascript_is_for_administrators_and_returns_json() {
    let q = qa("js");
    q.open("/table");
    let r = q.ok("js", json!({"expression": "document.querySelectorAll('tr').length + args.n", "args": {"n": 10}}));
    assert_eq!(r.value["value"], 14);
    let r = q.ok("js", json!({"expression": "({title: document.title, rows: [1,2,3]})"}));
    assert_eq!(r.value["value"], json!({"title": "Prices", "rows": [1, 2, 3]}));
    let e = q.op("js", json!({"expression": "(function(){ throw new Error('bad thing'); })()"})).unwrap_err();
    assert!(e.message.contains("bad thing"), "{}", e.message);
    // a user who is not an administrator, or whose browser setting is read-only
    let mut p = admin_policy();
    p.manage_settings = false;
    let q2 = qa_with("js2", p);
    q2.open("/table");
    let e = q2.op("js", json!({"expression": "1"})).unwrap_err();
    assert!(e.config && e.message.contains("administrators only"), "{}", e.message);
    let mut p = admin_policy();
    p.browser_agent = BrowserAgent::ReadOnly;
    let q3 = qa_with("js3", p);
    q3.open("/table");
    for (op, args) in [("click", json!({"selector": "a"})), ("type", json!({"selector": "input", "text": "x"})), ("fill", json!({"fields": {"input": "x"}})), ("js", json!({"expression": "1"})), ("storage", json!({"name": "a", "value": "b"}))] {
        let e = q3.op(op, args).unwrap_err();
        assert!(e.config && e.message.contains("read_only"), "{op}: {}", e.message);
    }
    // reading is fine in read-only
    q3.ok("dom", json!({"mode": "summary"}));
    q3.ok("screenshot", json!({}));
    q3.ok("assert", json!({"check": "exists", "selector": "h1"}));
    // off: nothing runs
    let mut p = admin_policy();
    p.browser_agent = BrowserAgent::Off;
    let q4 = qa_with("js4", p);
    let e = q4.op("open", json!({"url": q4.url("/")})).unwrap_err();
    assert!(e.config && e.message.contains("off for your account"), "{}", e.message);
}

#[test]
fn cookies_and_local_storage_are_set_on_the_open_site_and_need_a_page_first() {
    let q = qa("storage");
    let e = q.op("storage", json!({"kind": "cookie", "name": "a", "value": "b"})).unwrap_err();
    assert!(e.message.contains("open a page of the site first") || e.message.contains("could not"), "{}", e.message);
    q.open("/form");
    q.ok("storage", json!({"kind": "local_storage", "name": "theme", "value": "dark"}));
    assert_eq!(q.ok("js", json!({"expression": "localStorage.getItem('theme')"})).value["value"], "dark");
    q.ok("storage", json!({"kind": "session_storage", "name": "tab", "value": "3"}));
    assert_eq!(q.ok("js", json!({"expression": "sessionStorage.getItem('tab')"})).value["value"], "3");
    q.ok("storage", json!({"kind": "cookie", "name": "flag", "value": "on"}));
}

#[test]
fn frames_are_entered_by_selector_and_left_again_and_an_unreadable_one_says_so() {
    let q = qa("frame");
    q.open("/frame");
    let t = q.ok("frame", json!({"action": "tabs"}));
    assert_eq!(t.value["tabs"].as_array().unwrap().len(), 1, "one tab");
    // the in-house engine does not load iframes: the message says that the frame is not readable
    let e = q.op("frame", json!({"action": "frame", "selector": "#inner"}));
    if let Err(e) = e {
        assert!(e.message.contains("#inner"), "{}", e.message);
    }
    let r = q.ok("frame", json!({"action": "main"}));
    assert_eq!(r.value["frame"], json!([]));
    let r = q.ok("frame", json!({"action": "parent"}));
    assert_eq!(r.value["frame"], json!([]));
}

// ------------------------------------------------------------------------------------------------ baselines

#[test]
fn compare_saves_a_baseline_the_first_time_passes_when_same_and_fails_with_a_diff_when_changed() {
    let q = qa("compare");
    q.open("/login");
    let args = json!({"name": "login", "threshold": 0.5});
    let first = q.ok("compare", args.clone());
    assert_eq!(first.value["passed"], true);
    assert_eq!(first.value["baseline_created"], true);
    let base = first.value["baseline"].as_str().unwrap().to_string();
    assert!(std::path::Path::new(&base).is_file() && base.contains("baselines/qaflow/login.png"), "{base}");
    let again = q.ok("compare", args.clone());
    assert_eq!(again.value["passed"], true, "{}", again.value);
    assert_eq!(again.value["changed"], 0);
    assert!(again.value["message"].as_str().unwrap().contains("matches its baseline"));
    // a different page gives a different picture
    q.open("/form");
    let bad = q.ok("compare", args.clone());
    assert_eq!(bad.value["passed"], false);
    assert!(bad.value["percent"].as_f64().unwrap() > 0.5, "{}", bad.value);
    let diff = bad.value["diff"].as_str().unwrap();
    assert_eq!(png_size(diff), (320, 240), "a diff picture of the same size");
    assert!(bad.artifacts.iter().any(|a| a == diff), "the diff is an artifact");
    assert!(bad.value["message"].as_str().unwrap().contains("differs from its baseline"));
    // a different viewport size is said plainly
    q.ok("viewport", json!({"width": 400, "height": 300}));
    let sz = q.ok("compare", args.clone());
    assert_eq!(sz.value["passed"], false);
    assert!(sz.value["message"].as_str().unwrap().contains("is 400x300 but the baseline \"login\" is 320x240"), "{}", sz.value);
    // replacing the baseline
    let up = q.ok("compare", json!({"name": "login", "update": true}));
    assert_eq!(up.value["passed"], true);
    assert!(up.value["message"].as_str().unwrap().contains("replaced"));
    assert_eq!(q.ok("compare", args).value["passed"], true);
}

// ------------------------------------------------------------------------------------------------ the session

#[test]
fn the_session_is_one_per_run_isolated_and_closed_at_the_end() {
    let q = qa("session");
    q.op_in("runX", true, &[], "open", json!({"url": q.url("/login")})).unwrap();
    q.op_in("runY", true, &[], "open", json!({"url": q.url("/form")})).unwrap();
    assert_eq!(q.host().browser_sessions.count(), 2, "one session per run");
    // each run has its own page
    let a = q.op_in("runX", true, &[], "assert", json!({"check": "title_equals", "expected": "Sign in", "wait_ms": 0})).unwrap();
    let b = q.op_in("runY", true, &[], "assert", json!({"check": "title_equals", "expected": "Preferences", "wait_ms": 0})).unwrap();
    assert_eq!((a.value["passed"].as_bool(), b.value["passed"].as_bool()), (Some(true), Some(true)));
    let info = q.host().browser_session("runX").unwrap();
    assert_eq!(info.profile, "isolated");
    assert!(info.actions >= 2 && info.url.ends_with("/login"));
    let profile = q.env.data.join("browser-profiles/runX");
    assert!(profile.is_dir(), "a private profile folder");
    q.host().browser_end("runX");
    assert!(q.host().browser_session("runX").is_none());
    assert!(!profile.exists(), "the isolated profile is thrown away");
    assert_eq!(q.host().browser_sessions.count(), 1);
    q.host().browser_end("runX");
    q.host().browser_sessions.drop_all();
    assert_eq!(q.host().browser_sessions.count(), 0);
}

#[test]
fn the_per_run_action_budget_stops_a_runaway_flow() {
    let mut p = admin_policy();
    p.browser_max_actions = 4;
    let q = qa_with("budget", p);
    q.open("/login");
    for _ in 0..3 {
        q.ok("assert", json!({"check": "exists", "selector": "h1", "wait_ms": 0}));
    }
    let e = q.op("assert", json!({"check": "exists", "selector": "h1", "wait_ms": 0})).unwrap_err();
    assert!(e.config && e.message.contains("all 4 browser actions"), "{}", e.message);
    // the flow can set its own number (up to a hard cap)
    let req = BrowserOp { op: "assert".into(), args: json!({"check": "exists", "selector": "h1", "wait_ms": 0}), timeout_ms: 3000, run_id: "run-big".into(), trusted: true, profile: "isolated".into(), max_actions: 9999, ..Default::default() };
    q.host().browser_op(&BrowserOp { op: "open".into(), args: json!({"url": q.url("/login")}), ..req.clone() }, &Cancel::new()).unwrap();
    q.host().browser_op(&req, &Cancel::new()).unwrap();
    assert_eq!(q.host().browser_sessions.get("run-big").unwrap().actions(), 2);
}

#[test]
fn a_page_that_leaves_the_approved_hosts_is_left_and_the_node_fails() {
    let q = qa("leave");
    q.host().browser_sessions.ask_wait.lock().unwrap().replace(Duration::from_millis(1500));
    // approve the question by hand
    let svc = q.env.svc.clone();
    let user = q.env.user.clone();
    let t = std::thread::spawn(move || {
        for _ in 0..300 {
            if let Some(r) = svc.browser.consent.pending(&user).first() {
                svc.browser.consent.resolve(&user, &r.id, crate::browserconsent::Answer::Allow).unwrap();
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    // untrusted, hosts list names a different host than the one used: the loopback address is always fine, so use a policy limit instead
    let mut p = admin_policy();
    p.net_hosts = vec!["127.0.0.1".into()];
    drop(q);
    let q = qa_with("leave2", p);
    q.host().browser_sessions.ask_wait.lock().unwrap().replace(Duration::from_millis(100));
    let _ = t.join();
    q.op("open", json!({"url": q.url("/login")})).unwrap();
    let e = q.op("open", json!({"url": "http://blocked.test/x"})).unwrap_err();
    assert!(e.config, "{e:?}");
}

#[test]
fn secrets_typed_from_the_vault_are_never_in_a_result_a_dom_dump_or_the_audit_trail() {
    let q = qa("secret");
    q.open("/login");
    let secret = "s3cret-pass";
    let audits = Arc::new(Mutex::new(Vec::<String>::new()));
    let a2 = audits.clone();
    *q.host().audit_sink.lock().unwrap() = Some(Arc::new(move |_, a, d| a2.lock().unwrap().push(format!("{a} {d}"))));
    let mk = |op: &str, args: Value, secrets: &[&str]| BrowserOp { op: op.into(), args, timeout_ms: 8000, run_id: "run1".into(), flow_id: "qaflow".into(), trusted: true, profile: "isolated".into(), secrets: secrets.iter().map(|s| s.to_string()).collect(), ..Default::default() };
    let r = q.host().browser_op(&mk("fill", json!({"fields": {"#email": "qa@example.com", "#pw": secret}, "submit": "#go"}), &[secret]), &Cancel::new()).unwrap();
    assert!(!r.value.to_string().contains(secret) && !r.log.join(" ").contains(secret));
    // the page echoes the password into the DOM ("debug: you typed ...")
    let probe = q.host().browser_op(&mk("assert", json!({"check": "text_contains", "selector": "#echo", "expected": "you typed", "wait_ms": 1000}), &[]), &Cancel::new()).unwrap();
    assert_eq!(probe.value["passed"], true);
    let dump = q.host().browser_op(&mk("dom", json!({"mode": "page"}), &[]), &Cancel::new()).unwrap();
    let text = dump.value.to_string();
    assert!(!text.contains(secret), "the typed secret is replaced even in a later node: {text}");
    assert!(text.contains("debug: you typed ***"), "{text}");
    let t = q.host().browser_op(&mk("dom", json!({"mode": "text", "selector": "#echo"}), &[]), &Cancel::new()).unwrap();
    assert_eq!(t.value["text"], "debug: you typed ***");
    // a failing message that would quote the secret is scrubbed as well
    let c = q.host().browser_op(&mk("assert", json!({"check": "text_equals", "selector": "#echo", "expected": secret, "wait_ms": 0}), &[]), &Cancel::new()).unwrap();
    assert!(!c.value.to_string().contains(secret), "{}", c.value);
    for l in audits.lock().unwrap().iter() {
        assert!(!l.contains(secret), "{l}");
    }
}

// ------------------------------------------------------------------------------------------------ whole flows

fn node(id: &str, kind: &str, cfg: Value) -> Node {
    Node::new(id, kind, cfg)
}

/// start -> the nodes in a line -> end, every node's error going nowhere unless the test adds edges.
fn line_flow(id: &str, trusted: bool, nodes: Vec<Node>) -> Flow {
    let mut f = Flow::new(id, id);
    f.defaults.browser.trusted = trusted;
    f.nodes.push(node("start", "start", json!({})));
    let mut prev = "start".to_string();
    for n in nodes {
        f.edges.push(Edge::new(&prev, &n.id));
        prev = n.id.clone();
        f.nodes.push(n);
    }
    f
}

impl Qa {
    fn run_flow(&self, f: Flow) -> Run {
        let id = f.id.clone();
        self.env.create_draft(f, "user:t").unwrap();
        self.env.store.publish(&id, &iyakki_flow::store::PublishReq { by: "user:t", label: "", changelog: "x", approved_by: None }).unwrap();
        let o = self.env.engine.start_run(StartRequest::new(&id)).unwrap();
        self.env.engine.wait_for(&o.run_id, Duration::from_secs(60), |r| r.status.is_terminal()).unwrap()
    }
}

fn all_text_files(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                all_text_files(&p, out);
            } else if let Ok(t) = std::fs::read_to_string(&p) {
                out.push((p, t));
            }
        }
    }
}

#[test]
fn a_login_smoke_test_flow_runs_end_to_end_and_the_vault_password_reaches_no_file() {
    let q = qa("flow1");
    q.env.svc.on_login(&q.env.user, "a-long-test-passphrase", true);
    q.env.svc.user_set(&q.env.user, "qa-pass", "s3cret-pass").unwrap();
    let f = line_flow(
        "smoke",
        true,
        vec![
            node("open", "br_open", json!({"url": q.url("/login")})),
            node("shot1", "br_screenshot", json!({"name": "before"})),
            node("fill", "br_fill", json!({"fields": {"#email": "qa@example.com", "#pw": "vault:qa-pass", "#remember": true}, "submit": "#go"})),
            node("check", "br_assert", json!({"check": "text_contains", "selector": "#result", "expected": "Welcome, qa@example.com", "wait_ms": 2000})),
            node("echo", "br_dom", json!({"mode": "text", "selector": "#echo", "console": false})),
            node("shot2", "br_screenshot", json!({"name": "after"})),
        ],
    );
    let r = q.run_flow(f);
    assert_eq!(r.status, RunStatus::Succeeded, "{} {:?}", r.error, r.nodes.iter().map(|(k, n)| (k.clone(), n.error.clone())).collect::<Vec<_>>());
    // outputs the later nodes can use
    assert_eq!(r.nodes["check"].output["passed"], true);
    assert_eq!(r.nodes["echo"].output["text"], "debug: you typed ***", "the page echoed the password; the run keeps ***");
    let p1 = r.nodes["shot1"].output["path"].as_str().unwrap();
    assert_eq!(png_size(p1), (320, 240));
    assert_eq!(r.nodes["shot1"].artifacts, vec![p1.to_string()], "the picture is an artifact of its node");
    // the session ended with the run
    assert_eq!(q.host().browser_sessions.count(), 0);
    assert!(q.env.svc.browser.consent.pending(&q.env.user).is_empty(), "a trusted flow asked nothing");
    // the secret is in no file the program wrote (the run, its history, the logs, the flow) nor in the audit trail
    let mut files = Vec::new();
    all_text_files(&q.env.data, &mut files);
    all_text_files(&q.env.dir, &mut files);
    assert!(files.len() >= 3, "{}", files.len());
    for (p, t) in &files {
        assert!(!t.contains("s3cret-pass"), "{} holds the secret", p.display());
    }
    assert!(files.iter().any(|(_, t)| t.contains("vault:qa-pass")), "the flow file names the vault entry");
    // the report
    let rep = iyakki_flow::report::build(&r);
    assert!(rep.passed && rep.summary.checks == 1 && rep.summary.screenshots == 2, "{:?}", rep.summary);
    assert_eq!(rep.nodes.iter().find(|n| n.id == "check").unwrap().assertion.as_ref().unwrap()["passed"], true);
    let html = iyakki_flow::report::to_html(&rep, &|p| std::fs::read(p).ok());
    assert!(html.contains("data:image/png;base64,") && html.contains("PASSED") && !html.contains("s3cret-pass"));
    assert!(html.contains("can show anything"), "the report says screenshots may show secrets");
    let json: Value = serde_json::from_str(&iyakki_flow::report::to_json(&rep)).unwrap();
    assert_eq!(json["summary"]["screenshots"], 2);
}

#[test]
fn a_node_can_retry_until_the_element_appears() {
    let q = qa("flow2");
    let mut wait = node("see", "br_assert", json!({"check": "exists", "selector": "#late-arrival", "wait_ms": 0}));
    wait.retry = Some(RetryPolicy::fixed(10, 200));
    // something else changes the page while the node keeps trying (the in-house engine plays timers out at once, so the
    // element is added from outside after a moment)
    let host = q.env.host.clone();
    let url = q.url("/login");
    let t = std::thread::spawn(move || {
        for _ in 0..400 {
            if let Some(run) = host.browser_sessions.runs().first().cloned().filter(|r| host.browser_session(r).is_some_and(|i| i.actions >= 2)) {
                std::thread::sleep(Duration::from_millis(300));
                let req = BrowserOp { op: "js".into(), args: json!({"expression": "(function(){var d=document.createElement('div');d.id='late-arrival';document.body.appendChild(d);return 1;})()"}), timeout_ms: 5000, run_id: run, trusted: true, profile: "isolated".into(), ..Default::default() };
                return host.browser_op(&req, &Cancel::new()).is_ok();
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    });
    let r = q.run_flow(line_flow("retry", true, vec![node("open", "br_open", json!({"url": url})), wait]));
    assert!(t.join().unwrap(), "the element was added");
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    let n = &r.nodes["see"];
    assert!(n.attempts.len() >= 2, "it looked more than once: {}", n.attempts.len());
    assert!(!n.attempts[0].ok && n.attempts[0].error.contains("expected \"#late-arrival\" to exist"), "{:?}", n.attempts[0]);
    assert!(n.attempts.last().unwrap().ok);
}

#[test]
fn soft_checks_are_recorded_and_hard_checks_fail_the_node_with_a_message_and_use_the_error_port() {
    let q = qa("flow3");
    let mut f = line_flow(
        "checks",
        true,
        vec![
            node("open", "br_open", json!({"url": q.url("/table")})),
            node("soft", "br_assert", json!({"check": "text_equals", "selector": ".price", "expected": "$99", "soft": true, "wait_ms": 0})),
            node("hard", "br_assert", json!({"check": "count", "selector": "tr", "expected": 7, "wait_ms": 0})),
            node("never", "br_screenshot", json!({})),
        ],
    );
    f.nodes.push(node("tell", "notify", json!({"title": "check failed: {{ nodes.hard.error }}"})));
    f.edges.push(Edge::new("hard", "tell").port("error"));
    let r = q.run_flow(f);
    assert_eq!(r.nodes["soft"].state, NodeState::Succeeded);
    assert_eq!(r.nodes["soft"].output["passed"], false);
    assert_eq!(r.nodes["soft"].output["soft"], true);
    assert!(r.nodes["soft"].output["message"].as_str().unwrap().contains("to equal \"$99\" but it is \"$12.50\""));
    assert_eq!(r.nodes["hard"].state, NodeState::Failed);
    assert!(r.nodes["hard"].error.contains("expected \"tr\" to match 7 element(s) but it matches 4"), "{}", r.nodes["hard"].error);
    assert_eq!(r.nodes["tell"].state, NodeState::Succeeded, "the error port was followed");
    assert_eq!(r.nodes["never"].state, NodeState::Skipped);
    let rep = iyakki_flow::report::build(&r);
    assert_eq!((rep.summary.checks, rep.summary.checks_failed, rep.summary.soft_failed), (2, 2, 1));
    assert!(!rep.passed, "a hard check failed");
    let html = iyakki_flow::report::to_html(&rep, &|_| None);
    assert!(html.contains("FAILED") && html.contains("check failed") && html.contains("(soft)"));
}

#[test]
fn the_session_is_closed_when_a_run_fails_and_when_it_is_stopped() {
    let q = qa("flow4");
    // a failing run
    let r = q.run_flow(line_flow("fails", true, vec![node("open", "br_open", json!({"url": q.url("/login")})), node("bad", "br_click", json!({"selector": "#nothing-here"}))]));
    assert_eq!(r.status, RunStatus::Failed);
    assert_eq!(q.host().browser_sessions.count(), 0, "closed after a failure");
    // a stopped run: waiting for something that never comes
    let f = line_flow("stopme", true, vec![node("open", "br_open", json!({"url": q.url("/login")})), node("hang", "br_wait", json!({"for": "selector", "value": "#never"}))]);
    q.env.create_draft(f, "user:t").unwrap();
    q.env.store.publish("stopme", &iyakki_flow::store::PublishReq { by: "user:t", label: "", changelog: "x", approved_by: None }).unwrap();
    let o = q.env.engine.start_run(StartRequest::new("stopme")).unwrap();
    for _ in 0..200 {
        if q.host().browser_session(&o.run_id).is_some_and(|i| i.actions >= 2) {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(q.host().browser_session(&o.run_id).is_some(), "the session is open while the run waits");
    q.env.engine.cancel(&o.run_id).unwrap();
    let r = q.env.engine.wait_for(&o.run_id, Duration::from_secs(20), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Cancelled);
    assert_eq!(q.host().browser_sessions.count(), 0, "closed after a stop");
    assert!(q.host().browser_session(&o.run_id).is_none());
}

#[test]
fn an_untrusted_flow_started_with_nobody_there_does_not_open_a_browser_and_a_trusted_one_does() {
    let q = qa("flow5");
    q.host().browser_sessions.ask_wait.lock().unwrap().replace(Duration::from_millis(300));
    let r = q.run_flow(line_flow("untrusted", false, vec![node("open", "br_open", json!({"url": q.url("/login")}))]));
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.nodes["open"].error.contains("Nobody answered"), "{}", r.nodes["open"].error);
    assert_eq!(q.host().browser_sessions.count(), 0);
    let r = q.run_flow(line_flow("trusted", true, vec![node("open", "br_open", json!({"url": q.url("/login")}))]));
    assert_eq!(r.status, RunStatus::Succeeded);
}

#[test]
fn extracted_data_and_a_screenshot_feed_later_nodes_conditions_and_a_vision_model() {
    let q = qa("flow6");
    struct Seen(Mutex<Vec<crate::chat::Message>>);
    impl crate::chat::ChatBackend for Seen {
        fn complete(&self, h: &[crate::chat::Message]) -> Result<String, String> {
            self.0.lock().unwrap().extend(h.iter().cloned());
            Ok("The page shows a price list.".into())
        }
    }
    let seen = Arc::new(Seen(Mutex::new(Vec::new())));
    *q.host().backend.lock().unwrap() = Some(seen.clone());
    let mut f = line_flow(
        "dataflow",
        true,
        vec![
            node("open", "br_open", json!({"url": q.url("/table")})),
            node("ex", "br_extract", json!({"fields": {"price": ".price"}, "rows": {"selector": "tr", "columns": {"name": "td:nth-child(1)", "qty": "td:nth-child(2)"}}})),
            node("shot", "br_screenshot", json!({"name": "prices"})),
            node("see", "agent", json!({"prompt": "Describe this page in one line. The price is {{ nodes.ex.output.price }}.", "images": ["{{ nodes.shot.output.path }}"]})),
            node("dom", "br_dom", json!({"mode": "text", "selector": "h1", "console": false})),
            node("sum", "agent", json!({"prompt": "Summarise: {{ nodes.dom.output.text }} / {{ nodes.ex.output.rows }}"})),
        ],
    );
    f.nodes.push(node("cond", "condition", json!({"expr": "len(nodes.ex.output.rows) == 3"})));
    f.nodes.push(node("yes", "end", json!({})));
    f.edges.push(Edge::new("sum", "cond"));
    f.edges.push(Edge::new("cond", "yes").port("true"));
    // a model that cannot look at pictures: refused with a plain message
    q.env.svc.settings.lock().unwrap().upsert_provider(crate::providers::ProviderCfg::new("p", crate::providers::Kind::Ollama)).unwrap();
    q.env.svc.settings.lock().unwrap().update(|s| s.chat = Some(crate::settings::ModelChoice { provider: "p".into(), model: "llama3:8b".into() })).unwrap();
    let r = q.run_flow(f.clone());
    assert_eq!(r.status, RunStatus::Failed);
    assert!(r.nodes["see"].error.contains("not one that can look at pictures"), "{}", r.nodes["see"].error);
    // a vision-capable model gets the PNG
    q.env.svc.settings.lock().unwrap().update(|s| s.chat = Some(crate::settings::ModelChoice { provider: "p".into(), model: "gpt-4o".into() })).unwrap();
    f.id = "dataflow2".into();
    let r = q.run_flow(f);
    assert_eq!(r.status, RunStatus::Succeeded, "{} {:?}", r.error, r.nodes.get("see").map(|n| n.error.clone()));
    assert_eq!(r.nodes["ex"].output["price"], "$12.50");
    assert_eq!(r.nodes["cond"].port, "true");
    let msgs = seen.0.lock().unwrap();
    let with_pic = msgs.iter().find(|m| !m.images.is_empty()).expect("the model was shown the picture");
    assert_eq!(&with_pic.images[0][..4], &[0x89, b'P', b'N', b'G']);
    assert!(with_pic.content.contains("The price is $12.50"), "{}", with_pic.content);
    let sum = msgs.iter().find(|m| m.content.starts_with("Summarise: Prices")).expect("the DOM text reached the summary step");
    assert!(sum.content.contains("apple"));
}

#[test]
fn models_that_can_look_at_pictures_are_recognised_by_name() {
    use crate::chat::Message;
    let png = vec![0x89, b'P', b'N', b'G', 1, 2, 3];
    let m = Message::user_with_images("look", vec![png.clone()]);
    assert_eq!(m.images.len(), 1);
    assert!(crate::providers::model_sees_pictures("gpt-4o-mini") && crate::providers::model_sees_pictures("llava:13b") && crate::providers::model_sees_pictures("claude-sonnet-4"));
    assert!(!crate::providers::model_sees_pictures("llama3:8b") && !crate::providers::model_sees_pictures("") && !crate::providers::model_sees_pictures("mistral:7b"));
}

#[test]
fn a_flow_made_by_the_assistant_or_imported_can_use_the_nodes_but_never_arrives_trusted() {
    let q = qa("flow7");
    let mut f = line_flow("imp", true, vec![node("open", "br_open", json!({"url": q.url("/login")}))]);
    f.defaults.browser.profile = "person".into();
    let e = q.env.import_json(&serde_json::to_string(&f).unwrap(), "user:t").unwrap();
    assert!(!e.defaults.browser.trusted && e.defaults.browser.profile.is_empty(), "an imported flow is not trusted and uses the isolated profile");
}

#[test]
fn the_assistant_makes_flows_with_browser_nodes_reads_their_qa_report_and_cannot_make_a_flow_trusted() {
    let q = qa("flow8");
    let call = |name: &str, args: Value| q.env.call(name, &args, None);
    let made = call(
        "flow_create",
        json!({"name": "Shop QA", "nodes": [
            {"id": "open", "kind": "br_open", "config": {"url": q.url("/table")}},
            {"id": "price", "kind": "br_assert", "config": {"check": "text_equals", "selector": ".price", "expected": "$12.50"}},
            {"id": "shot", "kind": "br_screenshot", "config": {"name": "prices"}}
        ], "defaults": {"browser": {"max_actions": 30}}}),
    )
    .unwrap();
    assert!(made.contains("Made the draft `shop-qa`"), "{made}");
    let f = q.env.store.get_draft("shop-qa").unwrap();
    assert_eq!(f.defaults.browser.max_actions, 30);
    assert!(!f.defaults.browser.trusted);
    // the assistant cannot take away the question, in a new flow or an edit
    let e = call("flow_edit", json!({"flow": "shop-qa", "ops": [{"op": "set_default", "browser": {"trusted": true}}]})).unwrap_err();
    assert!(e.contains("only the person can mark a flow trusted"), "{e}");
    let e = call("flow_edit", json!({"flow": "shop-qa", "ops": [{"op": "set_default", "browser": {"profile": "person"}}]})).unwrap_err();
    assert!(e.contains("own browser profile"), "{e}");
    assert!(!q.env.store.get_draft("shop-qa").unwrap().defaults.browser.trusted);
    let e = call("flow_create", json!({"name": "Sneaky", "nodes": [{"id": "o", "kind": "br_open", "config": {"url": q.url("/")}}], "defaults": {"browser": {"trusted": true}}})).unwrap_err();
    assert!(e.contains("only the person"), "{e}");
    // a bad browser node is caught by the checker with a plain message
    let bad = call("flow_edit", json!({"flow": "shop-qa", "ops": [{"op": "add_node", "id": "x", "kind": "br_open", "config": {"url": "http://user:pw@127.0.0.1/"}}]}));
    let shown = call("flow_get", json!({"flow": "shop-qa"})).unwrap();
    assert!(bad.is_ok() && shown.contains("no user name or password"), "{shown}");
    // run it (the person marks it trusted in the tile; here the test does what the tile does) and read the report back
    let mut f = q.env.store.get_draft("shop-qa").unwrap();
    f.nodes.retain(|n| n.id != "x");
    f.edges.retain(|e| e.from != "x" && e.to != "x");
    f.defaults.browser.trusted = true;
    q.env.store.save_draft(&f, None).unwrap();
    q.env.store.publish("shop-qa", &iyakki_flow::store::PublishReq { by: "user:t", label: "", changelog: "x", approved_by: Some("user:t") }).unwrap();
    let o = q.env.engine.start_run(StartRequest::new("shop-qa")).unwrap();
    let r = q.env.engine.wait_for(&o.run_id, Duration::from_secs(60), |r| r.status.is_terminal()).unwrap();
    assert_eq!(r.status, RunStatus::Succeeded, "{}", r.error);
    let st = call("flow_run_status", json!({"run": o.run_id})).unwrap();
    assert!(st.contains("QA report: PASSED (1 checks, 0 failed") && st.contains("check price: passed") && st.contains("picture shot:"), "{st}");
}

#[test]
fn downloads_are_saved_only_where_the_person_may_write_under_the_names_the_page_gave_and_hosts_follow_policy() {
    let q = qa("dl");
    q.open("/downloads");
    let session = q.host().browser_sessions.get("run1").unwrap();
    let req = BrowserOp { op: "download".into(), run_id: "run1".into(), flow_id: "qaflow".into(), ..Default::default() };
    let (mut arts, mut log) = (Vec::new(), Vec::new());
    // the flow's own work folder: always fine; a path in the page's file name cannot climb out
    let r = q.host().save_download(&session, &req, &q.url("/file.txt"), "../../etc/report.txt", "", &Cancel::new(), &mut arts, &mut log).unwrap();
    let path = r["path"].as_str().unwrap();
    assert!(path.contains("/work/qaflow/downloads/report.txt"), "{path}");
    assert_eq!(std::fs::read_to_string(path).unwrap(), "quarterly report\n");
    assert_eq!(arts, vec![path.to_string()]);
    // a folder the account may not write to is refused, and nothing is created there
    let mut p = admin_policy();
    p.fs_write = vec![q.dir.join("home/allowed").display().to_string()];
    let q2 = qa_with("dl2", p);
    q2.open("/downloads");
    let s2 = q2.host().browser_sessions.get("run1").unwrap();
    let outside = q2.dir.join("elsewhere");
    let e = q2.host().save_download(&s2, &req, &q2.url("/file.txt"), "a.txt", &outside.display().to_string(), &Cancel::new(), &mut Vec::new(), &mut Vec::new()).unwrap_err();
    assert!(e.config || e.message.to_lowercase().contains("write") || e.message.contains("not"), "{}", e.message);
    assert!(!outside.join("a.txt").exists(), "nothing was written outside the write scope");
    // a download from a host the policy refuses
    let mut p = admin_policy();
    p.net_hosts = vec!["example.org".into()];
    let q3 = qa_with("dl3", p);
    q3.open("/downloads");
    let s3 = q3.host().browser_sessions.get("run1").unwrap();
    let e = q3.host().save_download(&s3, &req, "http://blocked.test/f.txt", "f.txt", "", &Cancel::new(), &mut Vec::new(), &mut Vec::new()).unwrap_err();
    assert!(e.message.contains("may not download from blocked.test"), "{}", e.message);
}

#[test]
fn sessions_belong_to_their_person_and_their_run() {
    let a = qa("usr-a");
    let b = qa("usr-b");
    a.op_in("shared-run-id", true, &[], "open", json!({"url": a.url("/login")})).unwrap();
    assert!(a.host().browser_session("shared-run-id").is_some());
    assert!(b.host().browser_session("shared-run-id").is_none(), "another person's host knows nothing of it");
    // the same run id for another person makes that person's own, separate session with its own page
    b.op_in("shared-run-id", true, &[], "open", json!({"url": b.url("/form")})).unwrap();
    let ta = a.op_in("shared-run-id", true, &[], "assert", json!({"check": "title_equals", "expected": "Sign in", "wait_ms": 0})).unwrap();
    let tb = b.op_in("shared-run-id", true, &[], "assert", json!({"check": "title_equals", "expected": "Preferences", "wait_ms": 0})).unwrap();
    assert_eq!((ta.value["passed"].as_bool(), tb.value["passed"].as_bool()), (Some(true), Some(true)));
    // their queues are keyed by the person too
    let ja = format!("flowrun:{}:shared-run-id", a.env.user);
    let jb = format!("flowrun:{}:shared-run-id", b.env.user);
    assert_ne!(ja, jb);
}

// ------------------------------------------------------------------------------------------------ live (real WebKit)

/// Needs WebKitGTK (python3-gi, gir1.2-webkit2-4.1) and Xvfb: `cargo test -p iyakki-agent --lib browser_tests::live -- --ignored
/// --test-threads=1`. The fixture server is the same one; the engine is the real helper program.
mod live {
    use super::*;

    fn live(tag: &str) -> Qa {
        let dir = tmp(tag);
        let svc = Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
        let user = format!("livetester{}", tag.replace(|c: char| !c.is_ascii_alphanumeric(), ""));
        crate::webengine::set_host_mode(true);
        let env = Arc::new(FlowEnv::build_in(&svc, &user, &admin_policy(), false, &dir.join("home")));
        Qa { env, fx: Fixture::start(), dir }
    }

    #[test]
    #[ignore = "needs WebKitGTK (python3-gi, gir1.2-webkit2-4.1) and Xvfb"]
    fn live_login_form_screenshots_console_and_downloads_with_real_webkit() {
        let q = live("a");
        let r = q.open("/login");
        assert_eq!(r.value["title"], "Sign in");
        let shot = q.ok("screenshot", json!({"name": "login"}));
        let (w, h) = png_size(shot.value["path"].as_str().unwrap());
        assert_eq!((w, h), (1280, 800), "the viewport");
        assert!(std::fs::metadata(shot.value["path"].as_str().unwrap()).unwrap().len() > 3000, "a real picture, not a blank one");
        q.ok("fill", json!({"fields": {"#email": "qa@example.com", "#pw": "s3cret-pass", "#remember": true, "#role": "QA engineer"}, "submit": "#go"}));
        let c = q.ok("assert", json!({"check": "text_equals", "selector": "#result", "expected": "Welcome, qa@example.com (remembered)", "wait_ms": 3000}));
        assert_eq!(c.value["passed"], true, "{}", c.value);
        let el = q.ok("screenshot", json!({"name": "form", "selector": "#login"}));
        let (ew, eh) = png_size(el.value["path"].as_str().unwrap());
        assert!(ew > 20 && ew < 1280 && eh > 20 && eh < 800, "an element picture: {ew}x{eh}");
        let v = q.ok("viewport", json!({"preset": "phone 375x667"}));
        assert_eq!(v.value["inner_width"], 375, "{}", v.value);
        let p = q.ok("screenshot", json!({"name": "phone"}));
        assert_eq!(png_size(p.value["path"].as_str().unwrap()), (375, 667));
        // console
        q.open("/errors");
        q.ok("click", json!({"selector": "#boom"}));
        q.ok("wait", json!({"for": "time", "ms": 400}));
        let errs = q.ok("dom", json!({"mode": "summary"}));
        let t = errs.value["console"].to_string();
        assert!(t.contains("boom from click") && t.contains("boom from timer") && t.contains("async failure"), "{t}");
        // a key press and hover with real input
        q.open("/login");
        q.ok("type", json!({"selector": "#email", "text": "keys@example.com", "native": true}));
        q.ok("key", json!({"key": "Enter", "selector": "#email"}));
        q.ok("assert", json!({"check": "text_contains", "selector": "#result", "expected": "Invalid login", "wait_ms": 3000}));
        q.ok("click", json!({"selector": "#go", "native": true}));
        q.ok("hover", json!({"selector": "#go"}));
        // the iframe
        q.open("/frame");
        q.ok("frame", json!({"action": "frame", "selector": "#inner"}));
        assert_eq!(q.ok("assert", json!({"check": "text_equals", "selector": "#in", "expected": "Inside the frame", "wait_ms": 3000})).value["passed"], true);
        q.ok("frame", json!({"action": "main"}));
        // a download
        q.open("/downloads");
        q.ok("click", json!({"selector": "#dl"}));
        let d = q.ok("download", json!({"name_contains": "report"}));
        assert_eq!(std::fs::read_to_string(d.value["path"].as_str().unwrap()).unwrap(), "quarterly report\n");
        // a full-page picture of a long page is taller than the viewport
        q.open("/table");
        q.ok("viewport", json!({"width": 800, "height": 300}));
        let full = q.ok("screenshot", json!({"name": "full", "full_page": true}));
        let (fw, fh) = png_size(full.value["path"].as_str().unwrap());
        assert_eq!(fw, 800);
        assert!(fh >= 300, "{fh}");
        // a long page in six coloured blocks: the whole-page picture is as tall as the page and shows them in order
        q.open("/long");
        q.ok("viewport", json!({"width": 800, "height": 400}));
        let full = q.ok("screenshot", json!({"name": "long", "full_page": true}));
        let img = crate::qaimage::decode_png(&std::fs::read(full.value["path"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(img.w, 800);
        assert!((2990..=3100).contains(&img.h), "as tall as the page: {}", img.h);
        let at = |y: u32| -> [u8; 3] {
            let i = ((y * img.w + 400) * 4) as usize;
            [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
        };
        let near = |p: [u8; 3], want: [u8; 3]| p.iter().zip(want.iter()).all(|(a, b)| (*a as i32 - *b as i32).abs() < 20);
        for (y, want) in [(250, [255, 0, 0]), (750, [0, 255, 0]), (1250, [0, 0, 255]), (1750, [255, 255, 0]), (2250, [0, 255, 255]), (2750, [255, 0, 255])] {
            assert!(near(at(y), want), "y={y}: {:?} should be {want:?}", at(y));
        }
        // the viewport and a short element picture of one block
        let blk = q.ok("screenshot", json!({"name": "block", "selector": "div:nth-child(2)"}));
        let (bw, bh) = png_size(blk.value["path"].as_str().unwrap());
        assert!(bw >= 790 && (300..=401).contains(&bh), "the part of the block that fits the window: {bw}x{bh}");
        // script that never ends is stopped by the node's time limit
        let req = BrowserOp { op: "js".into(), args: json!({"expression": "(function(){ while (true) {} })()"}), timeout_ms: 1500, run_id: "run1".into(), trusted: true, profile: "isolated".into(), ..Default::default() };
        let t = std::time::Instant::now();
        let e = q.host().browser_op(&req, &Cancel::new()).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
        assert!(e.message.to_lowercase().contains("script") || e.timed_out, "{}", e.message);
        q.host().browser_end("run1");
    }
}
