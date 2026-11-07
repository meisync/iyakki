//! The API inside another server (`embed.rs`): the desktop's sign-in carried over to `/api/v1/`, and every refusal the pipeline gives.

use super::*;
use crate::webservice::embed::{self, DesktopSession, Embedded};

fn embedded() -> (Embedded, std::path::PathBuf) {
    let dir = crate::testdir::unique("webservice-embed", &format!("{}", std::thread::current().name().unwrap_or("t")));
    std::fs::create_dir_all(&dir).unwrap();
    let accounts = accounts();
    let deps = Arc::new(Deps {
        auth: accounts.clone(),
        audit: accounts,
        store: Arc::new(RunStore::new(&dir.join("runs"))),
        chat: Arc::new(Script { turns: Mutex::new(VecDeque::new()) }),
        tools: Arc::new(FakeSource { executed: Arc::new(Mutex::new(vec![])) }),
        extras: Arc::new(FakeExtras { ready: AtomicBool::new(true), web: Mutex::new((false, 8443)), switched: Default::default() }),
        system_prompt: "test".into(),
    });
    (Embedded::new(deps).expect("embedded"), dir)
}

const PEER: &str = "10.1.1.1";

fn go(e: &Embedded, r: Request, s: Option<DesktopSession>) -> (u16, Value) {
    let resp = e.handle(&r, PEER.parse().unwrap(), s);
    (resp.status, serde_json::from_slice(&resp.body).unwrap_or(Value::Null))
}

#[test]
fn only_the_api_prefix_is_the_apis() {
    for p in ["/api/v1", "/api/v1/whoami", "/api/v1/chat/runs/x"] {
        assert!(embed::owns(p), "{p}");
    }
    for p in ["/api/login", "/api/session", "/api/v10/x", "/ui/", "/", "/api/v1x"] {
        assert!(!embed::owns(p), "{p}");
    }
}

#[test]
fn a_desktop_session_signs_the_page_in_once_and_the_token_is_reused() {
    let (e, dir) = embedded();
    let s = DesktopSession { cookie: "cookie-of-ann", user: "ann" };
    let (st, v) = go(&e, req("GET", "/api/v1/whoami", None, None), Some(s));
    assert_eq!((st, v["user"].as_str()), (200, Some("ann")), "{v}");
    assert_eq!(e.tokens(), 1);
    for _ in 0..3 {
        assert_eq!(go(&e, req("GET", "/api/v1/whoami", None, None), Some(s)).0, 200);
    }
    assert_eq!(e.tokens(), 1, "one token for the session, not one per request");
    // another session is another token, and the role of the account decides as everywhere else
    let root = DesktopSession { cookie: "cookie-of-root", user: "root" };
    assert_eq!(go(&e, req("GET", "/api/v1/admin/status", None, None), Some(root)).0, 200);
    assert_eq!(go(&e, req("GET", "/api/v1/admin/status", None, None), Some(s)).0, 403);
    assert_eq!(e.tokens(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn without_a_session_or_token_nothing_but_the_public_routes_answers() {
    let (e, dir) = embedded();
    assert_eq!(go(&e, req("GET", "/api/v1/whoami", None, None), None).0, 401);
    assert_eq!(go(&e, req("GET", "/api/v1/chat/runs", None, None), None).0, 401);
    assert_eq!(go(&e, req("GET", "/api/v1/version", None, None), None).0, 200);
    assert_eq!(go(&e, req("GET", "/api/v1/nothing", None, None), None).0, 404);
    // a cloud-only route does not exist on the desktop profile, session or not
    let s = DesktopSession { cookie: "c", user: "ann" };
    assert_eq!(go(&e, req("GET", "/api/v1/files?scope=person", None, None), Some(s)).0, 404);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_page_can_still_sign_in_by_itself_and_its_own_token_wins() {
    let (e, dir) = embedded();
    let s = DesktopSession { cookie: "cookie-of-ann", user: "ann" };
    // password sign-in over the API, with a desktop session present: the login is the login, no token is made for the session
    let (st, v) = go(&e, req("POST", "/api/v1/login", None, Some(json!({"name": "bob", "password": "bob-password-1"}))), Some(s));
    assert_eq!(st, 200, "{v}");
    assert_eq!(e.tokens(), 1);
    let bob = v["token"].as_str().unwrap().to_string();
    let (st, v) = go(&e, req("GET", "/api/v1/whoami", Some(&bob), None), Some(s));
    assert_eq!((st, v["user"].as_str()), (200, Some("bob")), "the request's own token is used, not the session's");
    // a wrong token is refused, not replaced by the session's
    assert_eq!(go(&e, req("GET", "/api/v1/whoami", Some("iyk_nope"), None), Some(s)).0, 401);
    // a wrong password is refused
    assert_eq!(go(&e, req("POST", "/api/v1/login", None, Some(json!({"name": "bob", "password": "no"}))), None).0, 401);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_cross_site_request_does_not_ride_the_session() {
    let (e, dir) = embedded();
    let s = DesktopSession { cookie: "cookie-of-ann", user: "ann" };
    for site in ["cross-site", "same-site"] {
        let mut r = req("GET", "/api/v1/whoami", None, None);
        r.headers.insert("sec-fetch-site".into(), site.into());
        assert_eq!(go(&e, r, Some(s)).0, 401, "{site}");
    }
    let mut r = req("GET", "/api/v1/whoami", None, None);
    r.headers.insert("sec-fetch-site".into(), "same-origin".into());
    assert_eq!(go(&e, r, Some(s)).0, 200);
    // another origin is refused by the pipeline itself
    let mut r = req("GET", "/api/v1/whoami", None, None);
    r.headers.insert("origin".into(), "https://evil.example".into());
    assert_eq!(go(&e, r, Some(s)).0, 403);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn forgetting_a_session_ends_its_token() {
    let (e, dir) = embedded();
    let s = DesktopSession { cookie: "cookie-of-ann", user: "ann" };
    assert_eq!(go(&e, req("GET", "/api/v1/whoami", None, None), Some(s)).0, 200);
    assert_eq!(e.tokens(), 1);
    e.forget("cookie-of-ann");
    assert_eq!(e.tokens(), 0);
    // no session any more: nothing is added
    assert_eq!(go(&e, req("GET", "/api/v1/whoami", None, None), None).0, 401);
    // an account that does not exist gets no token that works
    let ghost = DesktopSession { cookie: "c2", user: "ghost" };
    assert_eq!(go(&e, req("GET", "/api/v1/whoami", None, None), Some(ghost)).0, 401);
    let _ = std::fs::remove_dir_all(dir);
}
