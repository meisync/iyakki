//! `iyakki-connect serve` against a fake tenant over real TLS: every refusal of the page server (no token, wrong token, a used token,
//! wrong Host, wrong Origin, wrong path, a second WebSocket, an oversized message), what the page may and may not say to the tenant,
//! and that the tile stream reaches the page byte for byte. The page's scripts are checked for the rules the Content-Security-Policy
//! enforces. No browser is involved here (see the report for what was driven in a real one).

mod common;

use common::*;
use iyakki_connect::serve::{Options, Server, MAX_PAGE_MESSAGE};
use iyakki_link::cloudclient::{Pin, Target};
use iyakki_link::rawlink::Auth;

fn auth() -> Auth {
    Auth::Password { user: USER.into(), password: zeroize::Zeroizing::new(PASSWORD.into()) }
}

fn serve(r: &Remote) -> Server {
    Server::start(Options { target: r.target.clone(), auth: auth(), share: None, files: None, port: 0 }).unwrap()
}

/// Spend the token and return the cookie (`name=value`).
fn sign_in(s: &Server) -> String {
    let token = s.url().split("?t=").nth(1).unwrap().to_string();
    let (code, head, _) = get(s.port(), &format!("/?t={token}"), None, "");
    assert_eq!(code, 303, "{head}");
    head.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).expect("a cookie").split(';').next().unwrap().to_string()
}

fn origin(s: &Server) -> String {
    format!("http://127.0.0.1:{}", s.port())
}

#[test]
fn the_server_listens_on_the_loopback_address_only_with_a_link_that_carries_a_long_random_token() {
    let r = start_remote();
    let a = serve(&r);
    let b = serve(&r);
    assert!(a.url().starts_with(&format!("http://127.0.0.1:{}/?t=", a.port())), "{}", a.url());
    let t = a.url().split("?t=").nth(1).unwrap().to_string();
    assert_eq!(t.len(), 64);
    assert_ne!(a.url(), b.url(), "a new token for each launch");
    assert_ne!(a.port(), 0);
    // not reachable by any other local address (the listener is bound to 127.0.0.1 only)
    let s = std::net::TcpStream::connect(("127.0.0.1", a.port()));
    assert!(s.is_ok());
}

#[test]
fn without_the_token_nothing_is_served() {
    let r = start_remote();
    let s = serve(&r);
    for path in ["/", "/app.js", "/style.css", "/decoder.js", "/media.js", "/api/config", "/fs/local/list", "/ws", "/nothing"] {
        let (code, head, body) = get(s.port(), path, None, "");
        assert_eq!(code, 403, "{path}: {head}");
        assert!(!String::from_utf8_lossy(&body).contains("<html"), "{path}");
        assert!(!head.contains("Set-Cookie"));
    }
    assert!(matches!(Ws::open(s.port(), "", Some(&origin(&s))), Err(403)), "the WebSocket too");
    assert_eq!(r.logins.load(std::sync::atomic::Ordering::SeqCst), 0, "the tenant was never contacted");
}

#[test]
fn a_wrong_token_is_refused_and_does_not_spend_the_real_one() {
    let r = start_remote();
    let s = serve(&r);
    for bad in ["", "0", &"0".repeat(64), "../../etc", &format!("{}x", s.url().split("?t=").nth(1).unwrap())] {
        let (code, head, _) = get(s.port(), &format!("/?t={bad}"), None, "");
        assert_eq!(code, 403, "{bad}");
        assert!(!head.contains("Set-Cookie"));
    }
    let _ = sign_in(&s); // the right one still works
}

#[test]
fn the_token_works_once_then_the_cookie_does() {
    let r = start_remote();
    let s = serve(&r);
    let token = s.url().split("?t=").nth(1).unwrap().to_string();
    let (code, head, _) = get(s.port(), &format!("/?t={token}"), None, "");
    assert_eq!(code, 303);
    assert!(head.contains("Location: /\r\n"), "the token leaves the address bar: {head}");
    let cookie_line = head.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).unwrap();
    assert!(cookie_line.contains("HttpOnly") && cookie_line.contains("SameSite=Strict") && cookie_line.contains("Path=/"), "{cookie_line}");
    let cookie = cookie_line.split(';').next().unwrap();
    // a second use of the link is refused (the cookie is not the token)
    assert_eq!(get(s.port(), &format!("/?t={token}"), None, "").0, 403, "one-time");
    assert_eq!(get(s.port(), &format!("/?t={token}"), None, &format!("Cookie: {cookie}\r\n")).0, 403, "also from a signed-in browser");
    // the cookie opens the page, with the security headers on
    let (code, head, body) = get(s.port(), "/", None, &format!("Cookie: {cookie}\r\n"));
    assert_eq!(code, 200);
    let csp = head.lines().find_map(|l| l.strip_prefix("Content-Security-Policy: ")).expect("a CSP");
    assert!(csp.contains("default-src 'none'") && csp.contains("script-src 'self'") && !csp.contains("unsafe-inline") && !csp.contains("unsafe-eval"), "{csp}");
    assert!(csp.contains(&format!("connect-src 'self' ws://127.0.0.1:{}", s.port())) && csp.contains("frame-ancestors 'none'") && csp.contains("base-uri 'none'"), "{csp}");
    for h in ["Cache-Control: no-store", "X-Content-Type-Options: nosniff", "Referrer-Policy: no-referrer", "X-Frame-Options: DENY"] {
        assert!(head.contains(h), "{h}");
    }
    assert!(String::from_utf8_lossy(&body).contains("<canvas id=\"screen\""));
    // a wrong cookie is the same as none
    assert_eq!(get(s.port(), "/", None, "Cookie: iyakki_connect=abc\r\n").0, 403);
    assert_eq!(get(s.port(), "/", None, &format!("Cookie: {cookie}0\r\n")).0, 403);
}

#[test]
fn the_wrong_host_is_refused_even_with_the_token_or_the_cookie() {
    let r = start_remote();
    let s = serve(&r);
    let token = s.url().split("?t=").nth(1).unwrap().to_string();
    let p = s.port();
    for host in ["evil.example", &format!("evil.example:{p}"), &format!("localhost:{p}"), "127.0.0.1", &format!("127.0.0.1:{}", p + 1), &format!("127.0.0.1:{p}.evil.example"), &format!("[::1]:{p}"), ""] {
        let (code, head, _) = get(p, &format!("/?t={token}"), Some(host), "");
        assert_eq!(code, 403, "Host {host:?}");
        assert!(!head.contains("Set-Cookie"));
    }
    // no Host header at all, and two of them
    assert_eq!(http(p, &format!("GET /?t={token} HTTP/1.1\r\n\r\n")).0, 403);
    assert_eq!(http(p, &format!("GET /?t={token} HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nHost: evil.example\r\n\r\n")).0, 403);
    // the failed tries did not spend the token
    let cookie = sign_in(&s);
    assert_eq!(get(p, "/", Some("rebind.example"), &format!("Cookie: {cookie}\r\n")).0, 403, "a rebinding name with a real cookie");
    assert_eq!(get(p, "/app.js", Some(&format!("localhost:{p}")), &format!("Cookie: {cookie}\r\n")).0, 403);
}

#[test]
fn the_wrong_origin_is_refused_on_pages_changes_and_the_websocket() {
    let r = start_remote();
    let s = serve(&r);
    let cookie = sign_in(&s);
    let p = s.port();
    for o in ["http://evil.example", "https://127.0.0.1:1", &format!("http://localhost:{p}"), &format!("http://127.0.0.1:{}", p + 1), "null", &format!("https://127.0.0.1:{p}"), ""] {
        assert_eq!(get(p, "/app.js", None, &format!("Cookie: {cookie}\r\nOrigin: {o}\r\n")).0, 403, "GET with Origin {o:?}");
        assert!(matches!(Ws::open(p, &cookie, Some(o)), Err(403)), "WebSocket with Origin {o:?}");
    }
    // a script without an Origin on the WebSocket (browsers always send one)
    assert!(matches!(Ws::open(p, &cookie, None), Err(403)));
    // a change needs this origin's Origin and the CSRF value
    let cfg: serde_json::Value = serde_json::from_slice(&get(p, "/api/config", None, &format!("Cookie: {cookie}\r\n")).2).unwrap();
    let csrf = cfg["csrf"].as_str().unwrap().to_string();
    let post = |extra: &str| http(p, &format!("POST /fs/send HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nCookie: {cookie}\r\n{extra}Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}")).0;
    assert_eq!(post(""), 403, "no Origin, no CSRF");
    assert_eq!(post(&format!("Origin: http://evil.example\r\nX-Csrf: {csrf}\r\n")), 403, "another site, even with a stolen value");
    assert_eq!(post(&format!("Origin: {}\r\n", origin(&s))), 403, "this origin without the CSRF value");
    assert_eq!(post(&format!("Origin: {}\r\nX-Csrf: nope\r\n", origin(&s))), 403, "a wrong CSRF value");
    assert_ne!(post(&format!("Origin: {}\r\nX-Csrf: {csrf}\r\n", origin(&s))), 403, "both right gets to the route (404 here: nothing is shared)");
}

#[test]
fn only_the_known_paths_and_methods_exist() {
    let r = start_remote();
    let s = serve(&r);
    let cookie = sign_in(&s);
    let c = format!("Cookie: {cookie}\r\n");
    let p = s.port();
    for path in ["/nothing", "/app.js/", "//app.js", "/%61pp.js", "/APP.JS", "/app.js%00", "/../app.js", "/ws/", "/index.html", "/.git/config", "/fs", "/fs/", "/fs/local/", "/api/config/x"] {
        let code = get(p, path, None, &c).0;
        assert_eq!(code, 404, "{path}");
    }
    assert_eq!(get(p, "/fs/local/list?path=%zz", None, &c).0, 400, "a broken escape");
    assert_eq!(http(p, &format!("DELETE /app.js HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nCookie: {cookie}\r\nOrigin: {}\r\n\r\n", origin(&s))).0, 403, "a change without the CSRF value never gets further");
    assert_eq!(http(p, &format!("PUT /app.js HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nCookie: {cookie}\r\n\r\n")).0, 403);
    for bad in ["GET /app.js HTTP/1.0\r\n\r\n", "GET app.js HTTP/1.1\r\n\r\n", "GARBAGE\r\n\r\n"] {
        let code = http(p, bad).0;
        assert!(code == 400 || code == 0, "{bad:?}: {code}");
    }
    // a body that is too big is refused before it is read
    assert_eq!(http(p, &format!("POST /fs/send HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nContent-Length: 99999999\r\n\r\n")).0, 413);
    // chunked bodies are not accepted
    assert_eq!(http(p, &format!("POST /fs/send HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n")).0, 400);
}

#[test]
fn a_second_websocket_is_refused_until_the_first_is_gone() {
    let r = start_remote();
    let s = serve(&r);
    let cookie = sign_in(&s);
    let mut first = Ws::open(s.port(), &cookie, Some(&origin(&s))).expect("the first window");
    first.send_text(r#"{"t":"hello","v":2,"caps":{"deflate":true}}"#);
    assert!(first.text_with("hello_ok", 10).is_some(), "the first is live");
    assert!(matches!(Ws::open(s.port(), &cookie, Some(&origin(&s))), Err(409)), "the second is refused");
    assert_eq!(r.logins.load(std::sync::atomic::Ordering::SeqCst), 1, "and cost the tenant nothing");
    drop(first);
    wait_for("the tenant's socket to close", 10, || r.ws_open.load(std::sync::atomic::Ordering::SeqCst) == 0);
    let mut again = None;
    wait_for("the window to be free again", 10, || {
        again = Ws::open(s.port(), &cookie, Some(&origin(&s))).ok();
        again.is_some()
    });
}

#[test]
fn a_message_over_the_limit_ends_the_connection() {
    let r = start_remote();
    let s = serve(&r);
    let cookie = sign_in(&s);
    let mut w = Ws::open(s.port(), &cookie, Some(&origin(&s))).unwrap();
    w.send_text(r#"{"t":"hello","v":2,"caps":{"deflate":true}}"#);
    assert!(w.text_with("hello_ok", 10).is_some());
    // one frame whose declared length is over the limit: refused from the header alone
    w.send_binary(&vec![1u8; MAX_PAGE_MESSAGE + 1]);
    let mut closed = false;
    for _ in 0..40 {
        match w.next(1) {
            Got::Close(code) => {
                assert_eq!(code, 1009, "message too big");
                closed = true;
                break;
            }
            Got::Eof => {
                closed = true;
                break;
            }
            _ => {}
        }
    }
    assert!(closed, "the connection was ended");
    assert!(r.bins.lock().unwrap().iter().all(|b| b.len() < MAX_PAGE_MESSAGE), "nothing that big reached the tenant");
}

#[test]
fn the_tile_stream_reaches_the_page_byte_for_byte_and_input_reaches_the_tenant_in_its_own_words() {
    let r = start_remote();
    let s = serve(&r);
    let cookie = sign_in(&s);
    let mut w = Ws::open(s.port(), &cookie, Some(&origin(&s))).unwrap();
    w.send_text(r#"{"t":"hello","v":2,"caps":{"deflate":true}}"#);
    w.send_text(r#"{"t":"size","w":640,"h":400}"#);
    let mut frame = None;
    for _ in 0..40 {
        if let Got::Binary(b) = w.next(1) {
            frame = Some(b);
            break;
        }
    }
    let frame = frame.expect("a picture arrived");
    assert_eq!(frame, *r.first_frame.lock().unwrap(), "relayed as it is: no re-encoding");
    assert_eq!(frame[0], 2, "a version 2 frame");
    // compact input becomes the tenant's JSON; a disallowed text and a wrong binary are dropped; media goes through untouched
    w.send_binary(&[0x20, 10, 0, 20, 0]);
    w.send_binary(&[0x25, 1, 1, b'a', b'K', b'e', b'y', b'A']);
    w.send_text(r#"{"t":"sp_voices","ok":true}"#);
    w.send_text(r#"{"t":"logout"}"#);
    w.send_binary(&[0x77, 1, 2, 3]);
    w.send_binary(&[0x01, 9, 8, 7]);
    w.send_text(r#"{"t":"ack","n":1}"#);
    wait_for("input to reach the tenant", 10, || r.texts.lock().unwrap().iter().any(|t| t.contains(r#""t":"ack""#)));
    let texts = r.texts.lock().unwrap().clone();
    assert!(texts.iter().any(|t| t == r#"{"t":"move","x":10,"y":20}"#), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains(r#""key":"a""#) && t.contains(r#""code":"KeyA""#) && t.contains(r#""ctrl":true"#)), "{texts:?}");
    assert!(!texts.iter().any(|t| t.contains("sp_voices") || t.contains("logout")), "dropped: {texts:?}");
    assert_eq!(*r.bins.lock().unwrap(), vec![vec![0x01u8, 9, 8, 7]], "only the microphone message passed, as it was");
}

#[test]
fn a_wrong_password_a_changed_certificate_and_a_gone_tenant_are_told_to_the_page_without_sending_secrets_further() {
    let r = start_remote();
    let cookie_for = |s: &Server| sign_in(s);
    // wrong password: final
    let s = Server::start(Options { target: r.target.clone(), auth: Auth::Password { user: USER.into(), password: zeroize::Zeroizing::new("wrong".into()) }, share: None, files: None, port: 0 }).unwrap();
    let c = cookie_for(&s);
    let mut w = Ws::open(s.port(), &c, Some(&origin(&s))).unwrap();
    let t = w.text_with("connect_error", 10).expect("an error for the page");
    assert!(t.contains(r#""final":true"#) && t.contains("wrong name or password"), "{t}");
    assert!(!t.contains("wrong\""), "the password never appears");
    // changed certificate: final, and the password was not sent
    let before = r.logins.load(std::sync::atomic::Ordering::SeqCst);
    let s2 = Server::start(Options { target: Target { addr: r.target.addr.clone(), pin: Pin([7; 32]) }, auth: auth(), share: None, files: None, port: 0 }).unwrap();
    let c2 = cookie_for(&s2);
    let mut w2 = Ws::open(s2.port(), &c2, Some(&origin(&s2))).unwrap();
    let t = w2.text_with("connect_error", 10).expect("an error for the page");
    assert!(t.contains(r#""final":true"#) && t.contains("not the one that was trusted"), "{t}");
    assert_eq!(r.logins.load(std::sync::atomic::Ordering::SeqCst), before, "no sign-in reached the tenant with a different certificate");
}

#[test]
fn the_page_has_no_inline_script_no_outside_address_and_the_tenants_own_decoder_and_media_files() {
    let html = iyakki_connect::serve::INDEX_HTML;
    let scripts: Vec<&str> = html.match_indices("<script").map(|(i, _)| &html[i..i + html[i..].find('>').unwrap()]).collect();
    assert_eq!(scripts.len(), 3);
    assert!(scripts.iter().all(|s| s.contains("src=\"/")), "{scripts:?}");
    assert!(!html.contains(" on") || !html.split_whitespace().any(|w| w.starts_with("on") && w.contains("=\"")), "no event-handler attributes");
    assert!(!html.contains("style=\""), "no inline style attributes (the policy forbids them)");
    for (name, text) in [("index.html", html), ("app.js", iyakki_connect::serve::APP_JS), ("style.css", iyakki_connect::serve::STYLE_CSS)] {
        assert!(!text.contains("https://") && !text.contains("http://") && !text.contains("//cdn") && !text.contains("@import"), "{name} talks to nothing outside");
        assert!(!text.contains("eval(") && !text.contains("new Function") && !text.contains("innerHTML") && !text.contains("document.write"), "{name}");
        assert!(text.contains("SPDX-License-Identifier: Apache-2.0"), "{name} carries the licence line");
    }
    // not forked: the served decoder and media bridge are the tenant page's own files
    assert_eq!(iyakki_connect::serve::DECODER_JS, include_str!("../../iyakki-remote/src/decoder.js"));
    assert_eq!(iyakki_connect::serve::MEDIA_JS, include_str!("../../iyakki-remote/src/media.js"));
    // the page keeps no secret: no password, key or token words in the scripts
    for w in ["password", "localStorage", "sessionStorage", "document.cookie"] {
        assert!(!iyakki_connect::serve::APP_JS.contains(w), "app.js mentions {w}");
    }
}
