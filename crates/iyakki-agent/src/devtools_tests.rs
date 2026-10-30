// #[cfg(test)] -- this whole file is test code (it is included under #[cfg(test)] from lib.rs); the marker keeps the BOUNDARY check from counting it.
use super::devtools::*;
use crate::services::Services;
use iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_sec::policy::{App, BrowserAgent, Role};
use serde_json::json;
use std::sync::Arc;

struct Dead;
impl Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
        Err("no internet".into())
    }
}

fn svc(tag: &str) -> Arc<Services> {
    let d = std::env::temp_dir().join(format!("iyakki-devtools-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap()
}

fn policy() -> iyakki_sec::policy::Policy {
    let mut p = Role::User.template();
    p.apps.insert(App::Browser);
    p.browser_agent = BrowserAgent::ReadOnly;
    p
}

fn l(parts: &[&str]) -> String {
    parts.join("\u{1f}")
}

#[test]
fn addresses_lose_query_fragment_user_info_and_token_like_path_pieces() {
    assert_eq!(scrub_url("https://User:pw@Example.COM/a/b.js?token=abc123&x=1#frag"), "https://example.com/a/b.js");
    assert_eq!(scrub_url("https://a.org/reset/3f2a9c8e7d6b5a4938271605f4e3d2c1/confirm"), "https://a.org/reset/<redacted>/confirm");
    assert_eq!(scrub_url("https://a.org/app;jsessionid=XYZ/page"), "https://a.org/app/page");
    assert_eq!(scrub_url("https://a.org/eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abc/x"), "https://a.org/<redacted>/x");
    assert_eq!(scrub_url("data:text/html;base64,AAAA"), "data: URL");
    assert_eq!(scrub_url("blob:https://a.org/uuid"), "blob: URL");
    assert_eq!(scrub_url("file:///etc/passwd"), "file:");
    assert_eq!(scrub_url("not an address"), "(not an address)");
    assert_eq!(scrub_url("https://a.org/static/main.chunk.js"), "https://a.org/static/main.chunk.js", "ordinary paths stay");
    let long = format!("https://a.org/{}", "p/".repeat(400));
    assert!(scrub_url(&long).chars().count() <= MAX_URL);
}

#[test]
fn texts_are_cut_and_anything_credential_like_is_replaced() {
    let t = scrub_text("Failed to load https://a.org/x.js?token=SECRET123 with Bearer abcdefghijklmnop12345 and password=hunter2 and key=AKIAIOSFODNN7EXAMPLE1");
    assert!(!t.contains("SECRET123") && !t.contains("abcdefghijklmnop12345") && !t.contains("hunter2") && !t.contains("AKIAIOSFODNN7EXAMPLE1"), "{t}");
    assert!(t.contains("https://a.org/x.js") && t.contains("password=<redacted>"), "{t}");
    assert_eq!(scrub_text("TypeError: x is undefined"), "TypeError: x is undefined");
    assert!(scrub_text("id 0123456789abcdef0123456789abcdef seen").contains("<redacted>"));
    assert!(scrub_text("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig").contains("<redacted>"));
    let long = scrub_text(&"word ".repeat(500));
    assert!(long.chars().count() <= MAX_TEXT && long.ends_with('…'));
    assert!(!scrub_text("a\u{0}b\u{7}c\nd").chars().any(|c| c.is_control()));
}

#[test]
fn helper_lines_are_parsed_scrubbed_and_bad_ones_refused() {
    let f = parse_line(&l(&["F", "get", "404", "120", "https://a.org/missing.png?sig=zzz", "Not Found"])).unwrap();
    assert_eq!(f, DevEvent::Failed { method: "GET".into(), status: 404, ms: 120, url: "https://a.org/missing.png".into(), error: "Not Found".into() });
    let c = parse_line(&l(&["C", "error", "12", "3", "https://a.org/app.js?v=1", "TypeError: x is undefined"])).unwrap();
    assert_eq!(c, DevEvent::Console { level: Level::Error, line: 12, col: 3, source: "https://a.org/app.js".into(), text: "TypeError: x is undefined".into() });
    assert!(matches!(parse_line(&l(&["R", "img", "5000", "900", "https://a.org/big.jpg", ""])), Some(DevEvent::Slow { ms: 900, bytes: 5000, .. })));
    assert!(matches!(parse_line(&l(&["P", "80", "300", "900", "https://a.org/?q=1", ""])), Some(DevEvent::Page { response_ms: 80, dcl_ms: 300, load_ms: 900, ref url }) if url == "https://a.org/"));
    for bad in ["", "F", "X\u{1f}a\u{1f}b\u{1f}c\u{1f}d\u{1f}e", "C\u{1f}info\u{1f}0\u{1f}0\u{1f}x\u{1f}y", "F\u{1f}GET\u{1f}1\u{1f}1\u{1f}\u{1f}x"] {
        assert!(parse_line(bad).is_none(), "{bad:?}");
    }
    assert!(parse_line(&format!("C\u{1f}error\u{1f}0\u{1f}0\u{1f}\u{1f}{}", "x".repeat(5000))).is_none(), "an over-long line is refused");
    // garbage numbers never panic
    assert!(matches!(parse_line(&l(&["F", "GET", "99999999999999999999", "-5", "https://a.org/", ""])), Some(DevEvent::Failed { status: 0, ms: 0, .. })));
}

#[test]
fn the_ring_buffers_are_bounded_and_drop_the_oldest() {
    let mut r = Ring::new(3);
    for i in 0..10 {
        r.push(i);
    }
    assert_eq!(r.iter().copied().collect::<Vec<_>>(), vec![7, 8, 9]);
    let s = svc("ring");
    s.devtools.set_enabled("alice", true);
    s.devtools.set_page("alice", "https://a.org/p?x=1");
    for i in 0..(MAX_FAILED + 50) {
        s.devtools.record_line("alice", &l(&["F", "GET", "500", "10", &format!("https://a.org/r{i}"), ""]));
        s.devtools.record_line("alice", &l(&["C", "error", "1", "1", "", &format!("e{i}")]));
        s.devtools.record_line("alice", &l(&["R", "img", "1", "400", &format!("https://a.org/s{i}"), ""]));
    }
    let sum = s.devtools.summary("alice");
    assert_eq!((sum.failed, sum.console_errors), (MAX_FAILED, MAX_CONSOLE));
    let out = s.devtools.failed_report("alice", &Scope::Current, 1000);
    assert!(out.contains("r249") && !out.contains("r0 ") && !out.contains("/r49 "), "newest kept, oldest dropped: {}", &out[..200.min(out.len())]);
}

#[test]
fn nothing_is_recorded_while_it_is_off_and_turning_it_off_forgets_and_the_choice_is_per_person_and_kept() {
    let s = svc("off");
    let line = l(&["F", "GET", "404", "5", "https://a.org/x", ""]);
    assert!(!s.devtools.enabled("alice"), "off by default");
    assert!(!s.devtools.record_line("alice", &line));
    assert_eq!(s.devtools.summary("alice"), Summary::default());
    s.devtools.set_enabled("alice", true);
    assert!(s.devtools.record_line("alice", &line));
    assert!(!s.devtools.enabled("bob"), "another person's switch is their own");
    assert!(!s.devtools.record_line("bob", &line));
    assert_eq!(s.devtools.summary("alice").failed, 1);
    // kept on disk: a fresh object over the same folder sees it
    let again = DevTools::new(s.dir());
    assert!(again.enabled("alice") && !again.enabled("bob"));
    s.devtools.set_enabled("alice", false);
    assert_eq!(s.devtools.summary("alice"), Summary::default(), "turning it off empties the buffer");
    assert!(!DevTools::new(s.dir()).enabled("alice"));
}

#[test]
fn the_tools_say_turned_off_without_looking_and_are_offered_only_with_the_browser_assistant() {
    let s = svc("tools");
    let p = policy();
    let names: Vec<String> = defs(&p).into_iter().map(|d| d.spec.name).collect();
    assert_eq!(names, vec!["browser_failed_requests", "browser_timings", "browser_console", "browser_headers"]);
    for n in TOOLS {
        let out = call(n, &json!({}), &s, &p, "alice").unwrap();
        assert!(out.contains(OFF_MESSAGE) && out.contains("Page: (no page is open"), "{n}: {out}");
        assert!(out.contains("turned off") && out.contains("Settings -> Browser"));
    }
    let mut no_app = p.clone();
    no_app.apps.remove(&App::Browser);
    assert!(defs(&no_app).is_empty());
    let mut off = p.clone();
    off.browser_agent = BrowserAgent::Off;
    assert!(defs(&off).is_empty() && call("browser_console", &json!({}), &s, &off, "alice").is_err());
    assert!(defs(&p).iter().all(|d| d.untrusted && d.risk == crate::agent::Risk::Safe), "read-only tools whose output is page data");
    assert!(describe("browser_console", &json!({})).unwrap().contains("current page") && describe("nope", &json!({})).is_none());
    // the use is logged without page data
    let log = s.toollog.list(7, Some("alice"));
    assert!(log.iter().any(|e| e.tool == "browser.devtools.console" && e.what == "off"));
}

fn fill(s: &Services) {
    let d = &s.devtools;
    d.set_enabled("alice", true);
    d.set_page("alice", "https://shop.example/cart?session=abc");
    for line in [
        l(&["F", "GET", "404", "120", "https://shop.example/img/missing.png?t=SECRETVALUE1234567890", "Not Found"]),
        l(&["F", "GET", "404", "300", "https://shop.example/img/missing.png", "Not Found"]),
        l(&["F", "POST", "0", "2000", "https://api.example/pay?card=4111", "network error"]),
        l(&["R", "script", "90000", "1500", "https://cdn.example/big.js?v=3", ""]),
        l(&["R", "img", "1000", "450", "https://shop.example/hero.jpg", ""]),
        l(&["C", "error", "42", "7", "https://shop.example/app.js", "Uncaught TypeError: cart is undefined"]),
        l(&["C", "warn", "0", "0", "https://shop.example/", "Deprecated API used"]),
        l(&["P", "80", "300", "2600", "https://shop.example/cart?session=abc", ""]),
    ] {
        assert!(d.record_line("alice", &line), "{line:?}");
    }
    // an event of another page
    d.set_page("alice", "https://other.example/");
    d.record_line("alice", &l(&["F", "GET", "500", "9", "https://other.example/api", ""]));
    d.set_page("alice", "https://shop.example/cart?session=abc");
}

#[test]
fn the_tools_report_failed_requests_timings_and_console_messages_with_a_stable_shape() {
    let s = svc("shape");
    let p = policy();
    fill(&s);
    let out = call("browser_failed_requests", &json!({}), &s, &p, "alice").unwrap();
    assert!(out.starts_with(crate::browsertools::UNTRUSTED_OPEN) && out.trim_end().ends_with(crate::browsertools::UNTRUSTED_CLOSE), "{out}");
    assert!(out.contains("Page: https://shop.example/cart"), "{out}");
    assert!(out.contains("2 failed requests") && out.contains("1. POST https://api.example/pay - network error, 2000 ms"), "{out}");
    assert!(out.contains("GET https://shop.example/img/missing.png - status 404 Not Found, 300 ms, seen 2 times"), "grouped with a count and the slowest time: {out}");
    assert!(!out.contains("other.example"), "only the current page by default");
    let all = call("browser_failed_requests", &json!({"all_pages": true}), &s, &p, "alice").unwrap();
    assert!(all.contains("other.example/api") && all.contains("3 failed requests"), "{all}");
    let one = call("browser_failed_requests", &json!({"page": "OTHER.example"}), &s, &p, "alice").unwrap();
    assert!(one.contains("other.example/api") && !one.contains("missing.png"), "{one}");
    let lim = call("browser_failed_requests", &json!({"limit": 1}), &s, &p, "alice").unwrap();
    assert!(lim.contains("... and 1 more."), "{lim}");

    let t = call("browser_timings", &json!({}), &s, &p, "alice").unwrap();
    assert!(t.contains("first response 80 ms, DOMContentLoaded 300 ms, load 2600 ms"), "{t}");
    let (a, b, c) = (t.find("2000 ms").unwrap(), t.find("1500 ms").unwrap(), t.find("450 ms").unwrap());
    assert!(a < b && b < c, "slowest first: {t}");
    assert!(t.contains("https://cdn.example/big.js  (script, 90000 bytes)"), "{t}");

    let c = call("browser_console", &json!({}), &s, &p, "alice").unwrap();
    assert!(c.contains("2 console messages") && c.contains("error: Uncaught TypeError: cart is undefined (https://shop.example/app.js:42:7)") && c.contains("warning: Deprecated API used"), "{c}");
    let e = call("browser_console", &json!({"level": "error"}), &s, &p, "alice").unwrap();
    assert!(e.contains("1 console message ") && !e.contains("Deprecated"), "{e}");
    // nothing stored carries the secrets of the raw lines
    let everything = format!("{out}{all}{t}{c}");
    for secret in ["SECRETVALUE", "card=4111", "session=abc", "v=3"] {
        assert!(!everything.contains(secret), "{secret} leaked");
    }
    let sum = s.devtools.summary("alice");
    assert_eq!((sum.failed, sum.console_errors), (3, 1));
}

fn open_page(s: &Services, url: &str, full: bool) {
    s.set_page_snapshot("alice", Some(crate::webtools::PageSnapshot { url: url.into(), title: "t".into(), text: String::new(), selection: String::new(), time: 0, full_engine: full }));
}

#[test]
fn without_a_probe_the_tools_say_unavailable_with_the_page_never_a_clean_page() {
    let s = svc("noprobe");
    let p = policy();
    s.devtools.set_enabled("alice", true);
    open_page(&s, "https://en.wikipedia.org/wiki/Main_Page?token=SECRETQ#x", true);
    for n in ["browser_failed_requests", "browser_timings", "browser_console"] {
        let out = call(n, &json!({}), &s, &p, "alice").unwrap();
        assert!(out.contains("Page: https://en.wikipedia.org/wiki/Main_Page\n"), "{n}: {out}");
        assert!(out.contains("The page probe is not available in this build") && out.contains("This is not the same as a clean page."), "{n}: {out}");
        assert!(!out.contains("No failed requests") && !out.contains("No console") && !out.contains("SECRETQ"), "{n}: {out}");
    }
    open_page(&s, "https://example.org/", false);
    assert!(call("browser_console", &json!({}), &s, &p, "alice").unwrap().contains("built-in reader"));
    // the probe's hello makes an empty answer a real one
    assert!(s.devtools.record_line("alice", &l(&["H", "", "", "", "", ""])));
    assert!(s.devtools.probe_alive("alice"));
    assert!(call("browser_failed_requests", &json!({}), &s, &p, "alice").unwrap().contains("No failed requests were seen for this page."));
    assert!(call("browser_console", &json!({}), &s, &p, "alice").unwrap().contains("No console errors or warnings were seen for this page."));
    assert!(call("browser_timings", &json!({}), &s, &p, "alice").unwrap().contains("No page load timing was reported yet"));
    // off: the page is named, nothing else
    s.devtools.set_enabled("alice", false);
    let out = call("browser_console", &json!({}), &s, &p, "alice").unwrap();
    assert!(out.contains("Page: https://example.org/") && out.contains("turned off"), "{out}");
}

#[test]
fn the_assistants_instructions_describe_the_tools() {
    for n in TOOLS {
        assert!(super::agent::AGENT_SYSTEM_PROMPT.contains(n), "{n}");
    }
    assert!(super::agent::AGENT_SYSTEM_PROMPT.contains("Developer tools for the chat agent"));
    assert!(PROMPT_RULES.contains("turned on") && PROMPT_RULES.contains("browser_headers") && PROMPT_RULES.contains("NOT done"));
    assert!(super::agent::AGENT_SYSTEM_PROMPT.contains("browser_headers") && super::agent::AGENT_SYSTEM_PROMPT.contains("page probe is not available"));
}

#[test]
fn the_probe_script_reports_only_the_documented_kinds_and_reads_no_bodies() {
    let js = include_str!("../../../dist/webview/wpe-helper/dev-probe.js");
    for want in ["iyakkiDev", "'F'", "'R'", "'C'", "'P'", "'H'", "unhandledrejection", "PerformanceObserver", "console", "XMLHttpRequest", "fetch"] {
        assert!(js.contains(want), "dev-probe.js lacks {want}");
    }
    for never in [".text()", ".json()", "responseText", "getAllResponseHeaders", "setRequestHeader", "document.cookie", "localStorage", ".body"] {
        assert!(!js.contains(never), "dev-probe.js must not touch {never}");
    }
}

#[test]
fn the_panel_collects_without_the_chat_switch_and_closing_it_forgets_everything() {
    let s = svc("panel");
    let d = &s.devtools;
    let p = policy();
    let line = l(&["F", "GET", "404", "5", "https://a.org/x.js?token=ABCDEF0123456789ABCDEF", ""]);
    assert!(!d.collecting("alice") && !d.record_line("alice", &line), "nothing before anything is open");
    d.set_panel("alice", true);
    assert!(d.collecting("alice") && d.panel_open("alice") && !d.enabled("alice"), "the panel does not touch the saved switch");
    assert!(!DevTools::new(s.dir()).enabled("alice"), "and writes nothing to disk");
    d.set_page("alice", "https://a.org/p?x=1");
    assert!(d.record_line("alice", &line));
    assert_eq!(d.summary("alice").failed, 1);
    // the chat tools still follow their own switch
    assert!(call("browser_failed_requests", &json!({}), &s, &p, "alice").unwrap().contains(OFF_MESSAGE));
    // another person is not collecting
    assert!(!d.record_line("bob", &line));
    d.set_panel("alice", false);
    assert!(!d.collecting("alice") && !d.record_line("alice", &line), "closing stops collecting");
    assert_eq!(d.seq("alice"), 0);
    assert!(d.snapshot("alice").failed.is_empty(), "and forgets what was collected");
    // with the switch on, the panel shares the buffers and closing it keeps them
    d.set_enabled("alice", true);
    d.set_panel("alice", true);
    d.set_page("alice", "https://a.org/p");
    d.record_line("alice", &line);
    d.set_panel("alice", false);
    assert!(d.collecting("alice") && d.record_line("alice", &line));
    assert_eq!(d.snapshot("alice").failed.len(), 2);
    assert!(call("browser_failed_requests", &json!({}), &s, &p, "alice").unwrap().contains("a.org/x.js"));
}

#[test]
fn the_sequence_number_moves_on_every_change_and_snapshots_are_scrubbed_copies_of_the_current_page() {
    let s = svc("seq");
    let d = &s.devtools;
    d.set_panel("alice", true);
    assert_eq!(d.seq("alice"), 0);
    d.set_page("alice", "https://shop.example/cart?session=abc");
    let s1 = d.seq("alice");
    assert!(s1 > 0);
    d.set_page("alice", "https://shop.example/cart?session=zzz");
    assert_eq!(d.seq("alice"), s1, "the same page again is no change");
    d.record_line("alice", &l(&["C", "error", "4", "2", "https://shop.example/a.js?k=SECRETVALUE1234567890", "boom at https://shop.example/a.js?token=SECRETVALUE1234567890"]));
    let s2 = d.seq("alice");
    assert!(s2 > s1);
    assert!(!d.record_line("alice", "garbage") && d.seq("alice") == s2, "a bad line changes nothing");
    d.set_page("alice", "https://other.example/");
    d.record_line("alice", &l(&["F", "GET", "500", "9", "https://other.example/api", ""]));
    d.record_line("alice", &l(&["P", "10", "20", "30", "https://other.example/", ""]));
    d.set_page("alice", "https://shop.example/cart");
    let snap = d.snapshot("alice");
    assert_eq!(snap.page, "https://shop.example/cart");
    assert!(snap.probe_seen && snap.seq == d.seq("alice"));
    assert_eq!(snap.console.len(), 1);
    assert!(snap.failed.is_empty(), "the other page's failure is not part of this page");
    assert_eq!(snap.pages.len(), 1, "a timing that names another page is still offered when this page has none");
    let dump = format!("{:?}", snap);
    assert!(!dump.contains("SECRETVALUE") && !dump.contains("session="), "{dump}");
    assert!(snap.console[0].at > 0 && snap.console[0].seq > s1);
}
