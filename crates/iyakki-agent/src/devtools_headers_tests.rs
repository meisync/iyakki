// #[cfg(test)] -- this whole file is test code (it is included under #[cfg(test)] from lib.rs); the marker keeps the BOUNDARY check from counting it.
use super::devtools::*;
use super::devtools_headers::*;
use crate::services::Services;
use crate::webtools::PageSnapshot;
use iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_sec::policy::{App, BrowserAgent, Policy, Role};
use iyakki_web::fetch::{Fetch, Request, Response};
use iyakki_web::Url;
use serde_json::json;
use std::sync::{Arc, Mutex};

struct Dead;
impl Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
        Err("no internet".into())
    }
}

/// Scripted web: the first route whose text is in the address answers; HEAD of `nohead` hosts gets 405.
struct Web {
    routes: Vec<(String, Response)>,
    seen: Mutex<Vec<(String, String, String, String)>>,
}
impl Fetch for Web {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.send(&Request::get(url))
    }
    fn send(&self, req: &Request) -> Result<Response, String> {
        let u = req.url.to_string();
        self.seen.lock().unwrap().push((req.method.clone(), u.clone(), req.cookie.clone(), req.range.clone()));
        if req.method == "HEAD" && u.contains("nohead") {
            return Ok(Response { status: 405, ..Response::default() });
        }
        self.routes.iter().find(|(k, _)| u.contains(k.as_str())).map(|(_, r)| r.clone()).ok_or_else(|| format!("no route for {u}"))
    }
}

fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

fn world(tag: &str, routes: Vec<(&str, Response)>) -> (Arc<Services>, Arc<Web>, Policy) {
    let d = std::env::temp_dir().join(format!("iyakki-devhdr-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let svc = Services::open(&d.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let web = Arc::new(Web { routes: routes.into_iter().map(|(k, v)| (k.to_string(), v)).collect(), seen: Mutex::new(vec![]) });
    svc.web.set_fetch(web.clone());
    svc.web.set_resolver(Arc::new(|_: &str| vec!["93.184.216.34".parse().unwrap()]));
    let mut p = Role::User.template();
    p.apps.insert(App::Browser);
    p.browser_agent = BrowserAgent::ReadOnly;
    p.net_hosts = vec!["*".into()];
    (svc, web, p)
}

fn open_page(svc: &Services, url: &str) {
    svc.set_page_snapshot("alice", Some(PageSnapshot { url: url.into(), title: "t".into(), text: String::new(), selection: String::new(), time: 0, full_engine: true }));
}

fn ok(headers: Vec<(String, String)>) -> Response {
    Response { status: 200, content_type: "text/html".into(), headers, ..Response::default() }
}

#[test]
fn headers_are_fetched_without_cookies_scrubbed_and_summarised() {
    let (s, web, p) = world("basic", vec![(
        "wiki.example",
        ok(h(&[
            ("Server", "mw1"),
            ("Strict-Transport-Security", "max-age=106384710; includeSubDomains; preload"),
            ("Set-Cookie", "session=SECRETCOOKIEVALUE123456; Path=/"),
            ("X-Request-Id", "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8"),
            ("Authorization", "Bearer abc"),
            ("Cache-Control", "private, s-maxage=0"),
            ("X-Frame-Options", "SAMEORIGIN"),
            ("Content-Security-Policy", "default-src 'self'"),
        ])),
    )]);
    open_page(&s, "https://wiki.example/wiki/Main?token=SECRETQUERY#frag");
    s.devtools.set_enabled("alice", true);
    let out = call("browser_headers", &json!({}), &s, &p, "alice").unwrap();
    assert!(out.starts_with(crate::browsertools::UNTRUSTED_OPEN), "{out}");
    assert!(out.contains("Page: https://wiki.example/wiki/Main\n"), "page without query: {out}");
    assert!(out.contains("fetched these headers itself") && out.contains("no cookies or credentials") && out.contains("may differ"), "{out}");
    assert!(out.contains("Request 1: HEAD https://wiki.example/wiki/Main -> status 200"), "{out}");
    assert!(out.contains("Server: mw1") && out.contains("set-cookie: <redacted>") || out.contains("Set-Cookie: <redacted>"), "{out}");
    for secret in ["SECRETCOOKIEVALUE", "a1b2c3d4e5f6", "Bearer abc", "SECRETQUERY"] {
        assert!(!out.contains(secret), "{secret} leaked: {out}");
    }
    assert!(out.contains("Authorization: <redacted>") && out.contains("X-Request-Id: <redacted>"), "{out}");
    assert!(out.contains("HSTS present (max-age=106384710") && out.contains("CSP present") && out.contains("X-Frame-Options SAMEORIGIN") && out.contains("Server mw1") && out.contains("Cache-Control private"), "{out}");
    let seen = web.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].2.is_empty(), "no cookie is sent");
    let log = s.toollog.list(7, Some("alice"));
    assert!(log.iter().any(|e| e.tool == "browser.devtools.headers" && e.what == "on"));
}

#[test]
fn redirects_are_listed_per_hop_and_head_falls_back_to_a_one_byte_get() {
    let mut r301 = Response { status: 301, location: Some("https://www.wiki.example/nohead/x?s=1".into()), ..Response::default() };
    r301.headers = h(&[("Location", "https://www.wiki.example/nohead/x?s=1")]);
    let (s, web, p) = world("hops", vec![("https://wiki.example/", r301), ("www.wiki.example/nohead", ok(h(&[("Server", "edge")])))]);
    open_page(&s, "https://wiki.example/");
    s.devtools.set_enabled("alice", true);
    let out = call("browser_headers", &json!({}), &s, &p, "alice").unwrap();
    assert!(out.contains("Request 1: HEAD https://wiki.example/ -> status 301, Location: https://www.wiki.example/nohead/x"), "{out}");
    assert!(!out.contains("s=1"), "{out}");
    assert!(out.contains("Request 2: GET with Range: bytes=0-0 https://www.wiki.example/nohead/x -> status 200"), "{out}");
    let seen = web.seen.lock().unwrap();
    assert!(seen.iter().any(|x| x.0 == "GET" && x.3 == "bytes=0-0"), "{seen:?}");
}

#[test]
fn a_redirect_loop_is_cut_and_a_blocked_host_is_refused_with_the_hops_so_far() {
    let (s, _, p) = world("loop", vec![("wiki.example/loop", Response { status: 302, location: Some("https://wiki.example/loop".into()), ..Response::default() })]);
    open_page(&s, "https://wiki.example/loop");
    s.devtools.set_enabled("alice", true);
    let out = call("browser_headers", &json!({}), &s, &p, "alice").unwrap();
    assert!(out.contains("too many redirects") && out.contains("Request 6:"), "{out}");

    let (s, web, mut p) = world("policy", vec![("wiki.example", Response { status: 302, location: Some("https://evil.example/x".into()), ..Response::default() })]);
    p.net_hosts = vec!["wiki.example".into()];
    open_page(&s, "https://wiki.example/");
    s.devtools.set_enabled("alice", true);
    let out = call("browser_headers", &json!({}), &s, &p, "alice").unwrap();
    assert!(out.contains("Request 1:") && out.contains("Stopped: your account may not reach evil.example"), "{out}");
    assert_eq!(web.seen.lock().unwrap().len(), 1, "the refused host is never asked");

    // another site than the open page
    let out = call("browser_headers", &json!({"url": "https://other.example/"}), &s, &p, "alice").unwrap();
    assert!(out.contains("same site"), "{out}");
}

#[test]
fn switch_off_no_page_and_header_caps() {
    let (s, web, p) = world("off", vec![("wiki.example", ok(vec![]))]);
    open_page(&s, "https://wiki.example/a?x=1");
    let out = call("browser_headers", &json!({}), &s, &p, "alice").unwrap();
    assert!(out.contains("turned off") && out.contains("Page: https://wiki.example/a\n") && !out.contains("x=1"), "{out}");
    assert!(web.seen.lock().unwrap().is_empty(), "nothing is fetched while off");
    s.devtools.set_enabled("alice", true);
    s.set_page_snapshot("alice", None);
    assert!(call("browser_headers", &json!({}), &s, &p, "alice").unwrap().contains("no page is open"));

    let many: Vec<(String, String)> = (0..70).map(|i| (format!("X-H{i}"), "v".repeat(500))).collect();
    let (s, _, p) = world("caps", vec![("wiki.example", ok(many))]);
    open_page(&s, "https://wiki.example/");
    s.devtools.set_enabled("alice", true);
    let out = call("browser_headers", &json!({}), &s, &p, "alice").unwrap();
    assert!(out.contains("70 headers, first 40 shown") && out.contains("X-H39:") && !out.contains("X-H40:"), "{out}");
    assert!(!out.contains(&"v".repeat(400)), "values are cut");
}

#[test]
fn scrub_header_hides_cookies_and_tokens_only() {
    assert_eq!(scrub_header("Set-Cookie", "a=b"), "<redacted>");
    assert_eq!(scrub_header("cookie", "a=b"), "<redacted>");
    assert_eq!(scrub_header("Proxy-Authenticate", "Basic realm=x"), "<redacted>");
    assert_eq!(scrub_header("ETag", "W/\"33a64df551425fcc55e4d42a148795d9f25f89d4\""), "W/\"33a64df551425fcc55e4d42a148795d9f25f89d4\"".to_string().replace("33a64df551425fcc55e4d42a148795d9f25f89d4", "33a64df551425fcc55e4d42a148795d9f25f89d4"));
    assert_eq!(scrub_header("Content-Type", "text/html; charset=UTF-8"), "text/html; charset=UTF-8");
    assert_eq!(scrub_header("X-Trace", "eyJhbGciOiJIUzI1NiJ9.e30.abcdefghijk"), "<redacted>");
}
