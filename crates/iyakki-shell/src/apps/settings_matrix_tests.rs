//! Settings -> Notifications -> Matrix account: the form, the sign-in in the background, the token in the vault and nowhere else, the
//! test message, removal, the network policy, and Store's "add the account" opening the form at the cluster address. A scripted
//! homeserver stands in for the network: no real account.

use std::sync::Mutex;

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};

const TOKEN: &str = "tok_SHELL_SECRET_5521";
const PASSWORD: &str = "pw_SHELL_SECRET_8830";

#[derive(Default)]
struct Hs {
    seen: Mutex<Vec<(String, String)>>,
}

impl Upstream for Hs {
    fn request(&self, m: &str, url: &str, _h: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
        self.seen.lock().unwrap().push((m.to_string(), url.to_string()));
        let ok = |s: &str| Ok(UpstreamResponse { status: 200, body: s.as_bytes().to_vec() });
        if url.ends_with("/login") {
            return if String::from_utf8_lossy(body).contains(PASSWORD) { ok(&format!(r#"{{"access_token":"{TOKEN}","user_id":"@alice:test","device_id":"D1"}}"#)) } else { Ok(UpstreamResponse { status: 403, body: br#"{"errcode":"M_FORBIDDEN"}"#.to_vec() }) };
        }
        if url.ends_with("/joined_rooms") {
            return ok(r#"{"joined_rooms":["!room:test"]}"#);
        }
        if url.ends_with("/account/whoami") {
            return ok(r#"{"user_id":"@alice:test"}"#);
        }
        if url.contains("/state/m.room.encryption") {
            return Ok(UpstreamResponse { status: 404, body: br#"{"errcode":"M_NOT_FOUND"}"#.to_vec() });
        }
        if url.contains("/send/m.room.message/") {
            return ok(r#"{"event_id":"$e1"}"#);
        }
        Ok(UpstreamResponse { status: 404, body: b"{}".to_vec() })
    }
}

fn admin(tag: &str) -> (AppCx, Arc<Hs>) {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    let hs = Arc::new(Hs::default());
    cx.services = services_in(&d, hs.clone());
    cx.services.on_login("root", "rootpass12", true);
    (cx, hs)
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}
fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    for c in s.chars() {
        key(a, cx, Key::Char(c));
    }
}
fn wait_status(a: &mut SettingsApp, cx: &AppCx, needle: &str) -> String {
    for _ in 0..400 {
        a.handle(&Event::Tick, cx);
        if let Some((t, _)) = a.status() {
            if t.contains(needle) {
                return t.clone();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("no status with `{needle}`, last: {:?}", a.status());
}

fn fill(a: &mut SettingsApp, cx: &AppCx, name: &str, server: &str, user: &str, secret: &str) {
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::Matrix, _))), "the Matrix form is open");
    let clear = |a: &mut SettingsApp| (0..80).for_each(|_| key(a, cx, Key::Backspace));
    clear(a);
    typ(a, cx, name);
    key(a, cx, Key::Enter);
    clear(a);
    typ(a, cx, server);
    key(a, cx, Key::Enter);
    key(a, cx, Key::Enter); // sign in with: user name and password
    typ(a, cx, user);
    key(a, cx, Key::Enter);
    typ(a, cx, secret);
    key(a, cx, Key::Enter);
    key(a, cx, Key::Enter); // the room stays empty: the only joined room is used
}

fn page(tag: &str) -> (AppCx, SettingsApp, Arc<Hs>) {
    let (cx, hs) = admin(tag);
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Notifications);
    (cx, a, hs)
}

#[test]
fn the_account_is_added_in_the_background_and_the_token_lives_only_in_the_vault() {
    let (cx, mut a, hs) = page("mx1");
    key(&mut a, &cx, Key::Char('a'));
    fill(&mut a, &cx, "Home", "http://192.168.1.9:8008", "alice", PASSWORD);
    let t = wait_status(&mut a, &cx, "Signed in as @alice:test");
    assert!(t.contains("vault") && !t.contains(TOKEN) && !t.contains(PASSWORD), "{t}");
    assert!(a.modal.is_none(), "the form closed");
    assert_eq!(cx.services.user_get("root", "msg.acct.home.token").unwrap().as_str(), TOKEN);
    let names = cx.services.user_names("root").unwrap();
    assert!(names.iter().filter(|n| n.starts_with("msg.")).count() == 1, "only the token: {names:?}");
    // no file under the data folder holds the token or the password; the audit log has neither
    fn walk(d: &std::path::Path, hits: &mut Vec<String>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, hits);
            } else if let Ok(t) = std::fs::read_to_string(&p) {
                if t.contains(TOKEN) || t.contains(PASSWORD) {
                    hits.push(p.display().to_string());
                }
            }
        }
    }
    let mut hits = vec![];
    walk(cx.services.dir(), &mut hits);
    assert!(hits.is_empty(), "{hits:?}");
    let audit = std::fs::read_to_string(cx.audit.path()).unwrap_or_default();
    assert!(audit.contains("msg.account.add") && !audit.contains(TOKEN) && !audit.contains(PASSWORD), "{audit}");
    assert!(hs.seen.lock().unwrap().iter().all(|(_, u)| u.starts_with("http://192.168.1.9:8008/_matrix/")), "only the configured server");
    // the test message goes to the one room
    key(&mut a, &cx, Key::Char('t'));
    let t = wait_status(&mut a, &cx, "Test message");
    assert!(t.contains("accepted"), "{t}");
    assert!(hs.seen.lock().unwrap().iter().any(|(m, u)| m == "PUT" && u.contains("/rooms/%21room%3Atest/send/m.room.message/")));
    // and the page says where the person can be reached
    let g = {
        let mut g = Gfx::new(1280, 720);
        g.clear(rgb(0, 0, 0));
        a.render(&mut g, R::new(0.0, 72.0, 1280.0, 648.0), 1.0, &cx, 0);
        g
    };
    assert!(g.text_log.iter().any(|t| t.2.contains("Where to reach me: Matrix \"Home\" (@alice:test)")), "{:?}", g.text_log.iter().map(|t| t.2.clone()).filter(|t| t.contains("reach")).collect::<Vec<_>>());
    // removal takes the token with it
    key(&mut a, &cx, Key::Char('x'));
    assert!(a.status().unwrap().0.contains("Removed the Matrix account"));
    assert!(cx.services.user_get("root", "msg.acct.home.token").is_err());
}

#[test]
fn a_wrong_password_says_so_without_the_password_and_nothing_is_kept() {
    let (cx, mut a, _hs) = page("mx2");
    key(&mut a, &cx, Key::Char('a'));
    fill(&mut a, &cx, "Home", "http://192.168.1.9:8008", "alice", "not-the-password");
    let t = wait_status(&mut a, &cx, "not right");
    assert!(!t.contains("not-the-password"));
    assert!(cx.services.user_names("root").unwrap().iter().all(|n| !n.starts_with("msg.")));
}

#[test]
fn the_form_refuses_plain_http_to_the_internet_and_a_host_the_account_may_not_reach() {
    let (mut cx, mut a, hs) = page("mx3");
    key(&mut a, &cx, Key::Char('a'));
    fill(&mut a, &cx, "Home", "http://matrix.example.org", "alice", PASSWORD);
    assert!(matches!(&a.modal, Some(Modal::Form(FormKind::Matrix, f)) if f.error.as_ref().is_some_and(|e| e.contains("https"))), "the form stays open with the reason");
    assert!(hs.seen.lock().unwrap().is_empty(), "nothing was sent");
    // https is fine for the address but the account's network policy decides
    cx.session.policy.net_hosts = vec!["only.example.org".into()];
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Notifications);
    key(&mut a, &cx, Key::Char('a'));
    fill(&mut a, &cx, "Home", "https://matrix.example.org", "alice", PASSWORD);
    assert!(matches!(&a.modal, Some(Modal::Form(FormKind::Matrix, f)) if f.error.as_ref().is_some_and(|e| e.contains("may not reach"))));
    assert!(hs.seen.lock().unwrap().is_empty());
}

#[test]
fn the_test_message_without_an_account_says_what_to_do() {
    let (cx, mut a, _hs) = page("mx4");
    key(&mut a, &cx, Key::Char('t'));
    assert!(a.status().unwrap().0.contains("Connect a Matrix account first"));
    key(&mut a, &cx, Key::Char('x'));
    assert!(a.status().unwrap().0.contains("no Matrix account"));
}

#[test]
fn store_opens_the_form_at_the_cluster_address_of_the_server_it_installed() {
    let (cx, hs) = admin("mx5");
    let mut a = SettingsApp::new(&cx);
    a.open_item("matrix:add", &cx);
    assert_eq!(a.tab(), Tab::Notifications);
    let Some(Modal::Form(FormKind::Matrix, f)) = &a.modal else { panic!("the form is open") };
    assert_eq!(f.value(1), "cluster://iyakki-matrix/matrix:8008");
    assert!(hs.seen.lock().unwrap().is_empty());
    assert_eq!(crate::apps::store::setup::open_target(iyakki_agent::quickinstall::Item::Matrix), Some(("settings", "matrix:add")));
}
