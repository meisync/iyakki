use super::*;

// the relay is one process-wide slot: these tests take turns
static TURN: Mutex<()> = Mutex::new(());
fn turn() -> std::sync::MutexGuard<'static, ()> {
    TURN.lock().unwrap_or_else(|e| e.into_inner())
}

fn post(token: &str, body: &str, peer: &str) -> Reply {
    handle("POST", &format!("/login/{token}"), Some("application/x-www-form-urlencoded"), body.as_bytes(), peer).unwrap()
}

#[test]
fn a_waiting_login_serves_its_page_and_takes_one_code_once() {
    let _t = turn();
    let t = open("Claude Code");
    assert_eq!(t.len(), 32, "128 bits as hex");
    assert!(waiting());
    let (c, ct, body) = handle("GET", &format!("/login/{t}"), None, b"", "10.0.0.5").unwrap();
    let html = String::from_utf8(body).unwrap();
    assert!(c == 200 && ct.starts_with("text/html") && html.contains("<textarea") && html.contains("Send") && html.contains("Claude Code") && html.contains("certificate"), "{html}");
    let code = "abc_DEF-123+xyz.~/=#state-0987654321";
    assert_eq!(post(&t, &format!("code={}", code.replace('#', "%23").replace('+', "%2B")), "10.0.0.5").0, 200);
    assert_eq!(take_submitted().as_deref(), Some(code), "the form is decoded");
    assert_eq!(take_submitted(), None, "taken once");
    assert_eq!(post(&t, "code=second", "10.0.0.5").0, 410, "single use");
    assert_eq!(handle("GET", &format!("/login/{t}"), None, b"", "10.0.0.5").unwrap().0, 410);
    // the vendor refused it: the same page takes another
    rearm();
    assert_eq!(post(&t, "code=again", "10.0.0.5").0, 200);
    assert_eq!(take_submitted().as_deref(), Some("again"));
    finish();
    assert!(!waiting());
}

#[test]
fn a_wrong_token_no_login_and_foreign_paths_are_refused_alike() {
    let _t = turn();
    finish();
    assert_eq!(handle("GET", "/login/00000000000000000000000000000000", None, b"", "10.0.0.5").unwrap().0, 404, "nothing waiting");
    assert_eq!(handle("GET", "/other", None, b"", "10.0.0.5"), None, "not ours");
    let t = open("Claude Code");
    assert_eq!(post("0".repeat(32).as_str(), "code=x", "10.0.0.5").0, 404);
    assert_eq!(post(&t[..31], "code=x", "10.0.0.5").0, 404);
    assert_eq!(post(&format!("{t}0"), "code=x", "10.0.0.5").0, 404);
    assert_eq!(take_submitted(), None);
    finish();
    assert_eq!(post(&t, "code=x", "10.0.0.5").0, 404, "the dialog ended: the token is dead");
}

#[test]
fn an_expired_relay_is_dead() {
    let _t = turn();
    let t = open("Claude Code");
    lock().as_mut().unwrap().until = Instant::now() - Duration::from_secs(1);
    assert!(!waiting());
    assert_eq!(post(&t, "code=x", "10.0.0.5").0, 404);
    assert_eq!(take_submitted(), None);
    finish();
}

#[test]
fn only_a_small_text_body_is_read_and_tries_are_limited() {
    let _t = turn();
    let t = open("Claude Code");
    let big = format!("code={}", "a".repeat(MAX_BODY));
    assert_eq!(post(&t, &big, "10.0.0.6").0, 413);
    assert_eq!(handle("POST", &format!("/login/{t}"), Some("application/json"), b"{}", "10.0.0.6").unwrap().0, 415);
    assert_eq!(post(&t, "code=", "10.0.0.6").0, 200, "an empty paste shows the page again");
    assert_eq!(take_submitted(), None);
    assert_eq!(handle("PUT", &format!("/login/{t}"), None, b"", "10.0.0.6").unwrap().0, 405);
    for _ in 0..PER_MINUTE {
        handle("GET", &format!("/login/{t}"), None, b"", "10.0.0.7");
    }
    assert_eq!(handle("GET", &format!("/login/{t}"), None, b"", "10.0.0.7").unwrap().0, 429, "slowed down");
    assert_eq!(handle("GET", &format!("/login/{t}"), None, b"", "10.0.0.8").unwrap().0, 200, "another address is not");
    finish();
}

#[test]
fn only_the_local_network_may_ask() {
    use std::net::IpAddr;
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    for ok in ["192.168.1.9", "10.1.2.3", "172.16.0.4", "169.254.1.1", "fd00::1", "fe80::1"] {
        assert!(lan_peer(ip(ok)), "{ok}");
    }
    for no in ["127.0.0.1", "::1", "8.8.8.8", "93.184.216.34", "2606:4700::1"] {
        assert!(!lan_peer(ip(no)), "{no}");
    }
}

#[test]
fn the_form_decoder_and_the_listener_hook() {
    assert_eq!(form_value("a=1&code=x%23y+z&b=2", "code"), "x#y z");
    assert_eq!(form_value("a=1", "code"), "");
    struct L(std::sync::atomic::AtomicUsize);
    impl Listener for L {
        fn start(&self) -> Option<(u16, bool)> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some((9443, true))
        }
        fn stop(&self) {
            self.0.fetch_add(100, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let _t = turn();
    let l = std::sync::Arc::new(L(Default::default()));
    set_listener(l.clone());
    if WEB.lock().unwrap().is_none() {
        assert_eq!(port(), Some((9443, true)));
        finish();
        assert_eq!(l.0.load(std::sync::atomic::Ordering::SeqCst), 101, "started once, stopped with the login");
    }
}
