//! The real shell behind the web server: sign in with a real account, receive the real picture, click and type.

use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_agent::HostApi;
use iyakki_remote::screen::ShellGate;
use iyakki_remote::server::{Config, Server};
use iyakki_sec::policy::Policy;
use iyakki_shell::accounts::{AuditLog, UserStore};
use iyakki_shell::shell::ShellEnv;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct Offline;
impl Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct Host;
impl HostApi for Host {
    fn detect_env(&self) -> String {
        "test".into()
    }
    fn net_status(&self) -> String {
        "none".into()
    }
    fn run_wasm(&self, _: &str, _: &Policy, _: &[u8], _: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn power_off(&self) {}
    fn reboot(&self) {}
}

fn env(tag: &str) -> ShellEnv {
    env_store(tag).0
}

/// The environment and the account store itself, for a test that adds an account from the command line's side or poisons the lock.
fn env_store(tag: &str) -> (ShellEnv, Arc<Mutex<UserStore>>) {
    let d = std::env::temp_dir().join(format!("iyakki-remote-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    // the UI reaches the agent only through the handles the composition root installs (the `testing` default of iyakki-apps is the unwired one)
    iyakki_compose::wire();
    let mut store = UserStore::open(&d.join("users.json")).unwrap();
    store.create_first_admin("root", "rootpass12").unwrap();
    let up: Arc<dyn Upstream> = Arc::new(Offline);
    let raw = Arc::new(Mutex::new(store));
    let env = ShellEnv {
        store: iyakki_shell::accounts::UsersRef::of_store(&raw),
        audit: Arc::new(AuditLog::new(&d.join("audit.log"))),
        host: Arc::new(Host),
        cfg: iyakki_agent::config::AiConfig::parse("").unwrap(),
        chat: Arc::new(iyakki_agent::chat::Unconfigured),
        mcp_servers: vec![],
        services: iyakki_agent::services::Services::open(&d.join("svc"), up.clone(), up).unwrap(),
        web: Arc::new(iyakki_web::fetch::NoNetwork),
        hostname: "iyakki-test".into(),
        data_volatile: false,
        hwcheck: false,
    };
    (env, raw)
}

fn http(addr: std::net::SocketAddr, raw: &str) -> (u16, String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).into_owned();
    let code = text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let (h, b) = text.split_once("\r\n\r\n").map(|(h, b)| (h.to_string(), b.to_string())).unwrap_or_default();
    (code, h, b)
}

fn login(a: std::net::SocketAddr, pass: &str) -> (u16, String, String) {
    let body = format!(r#"{{"user":"root","password":"{pass}"}}"#);
    http(a, &format!("POST /api/login HTTP/1.1\r\nHost: {a}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()))
}

fn masked(s: &str) -> Vec<u8> {
    let mask = [5u8, 6, 7, 8];
    let mut f = vec![0x81, 0x80 | s.len() as u8];
    f.extend_from_slice(&mask);
    f.extend(s.bytes().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

fn frame(s: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut h = [0u8; 2];
    s.read_exact(&mut h).unwrap();
    let len = match h[1] & 0x7f {
        126 => {
            let mut l = [0u8; 2];
            s.read_exact(&mut l).unwrap();
            u16::from_be_bytes(l) as usize
        }
        127 => {
            let mut l = [0u8; 8];
            s.read_exact(&mut l).unwrap();
            u64::from_be_bytes(l) as usize
        }
        n => n as usize,
    };
    let mut p = vec![0u8; len];
    s.read_exact(&mut p).unwrap();
    (h[0] & 0x0f, p)
}

fn png_size(png: &[u8]) -> (u32, u32) {
    (u32::from_be_bytes(png[16..20].try_into().unwrap()), u32::from_be_bytes(png[20..24].try_into().unwrap()))
}

#[test]
fn a_person_signs_in_and_sees_and_drives_the_real_desktop_from_the_browser() {
    // the shell has no composition root of its own (iyakki-shell wiring.rs): a program wires it first
    iyakki_compose::wire();
    let server = Server::start(Config { addr: "127.0.0.1:0".into(), tls: None }, Arc::new(ShellGate::new(env("shell")))).unwrap();
    let a = server.local_addr();
    assert_eq!(login(a, "not the password").0, 401);
    let (code, head, body) = login(a, "rootpass12");
    assert_eq!(code, 200, "{body}");
    let cookie = head.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_string();
    let csrf = serde_json::from_str::<serde_json::Value>(&body).unwrap()["csrf"].as_str().unwrap().to_string();

    let mut s = TcpStream::connect(a).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(format!("GET /ws?csrf={csrf} HTTP/1.1\r\nHost: {a}\r\nOrigin: http://{a}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nCookie: {cookie}\r\n\r\n").as_bytes()).unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut b).unwrap();
        head.push(b[0]);
    }
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101"));

    // the first picture is the whole desktop at the default size, already signed in
    // (the page is also asked for its speech voices on connecting: a text frame, in either order with the picture)
    let (op, m) = loop {
        let f = frame(&mut s);
        if f.0 == 2 {
            break f;
        }
        assert_eq!((f.0, String::from_utf8_lossy(&f.1).as_ref()), (1, r#"{"t":"speak_voices"}"#));
    };
    assert_eq!(op, 2);
    assert_eq!(&m[..8], &[0, 0, 0, 0, 0, 5, 208, 2], "x 0, y 0, 1280 wide, 720 high");
    assert_eq!(png_size(&m[8..]), (1280, 720));

    // a smaller window: the next full picture has that size
    s.write_all(&masked(r#"{"t":"size","w":800,"h":600}"#)).unwrap();
    let mut got = None;
    for _ in 0..40 {
        let (op, m) = frame(&mut s);
        if op == 2 && png_size(&m[8..]) == (800, 600) {
            got = Some(m);
            break;
        }
    }
    let m = got.expect("a picture at the new size");
    assert_eq!(&m[..8], &[0, 0, 0, 0, 32, 3, 88, 2]);

    // a key press changes the picture (the help popup opens on F1)
    s.write_all(&masked(r#"{"t":"key","key":"F1"}"#)).unwrap();
    // (a one-line strip, such as the status line or clock, can repaint on its own while the key is in flight: it is not the answer to the key)
    let (op, m) = loop {
        let (op, m) = frame(&mut s);
        if op == 2 && png_size(&m[8..]).1 <= 30 {
            continue;
        }
        break (op, m);
    };
    assert_eq!(op, 2);
    let (w, h) = png_size(&m[8..]);
    assert!(w > 50 && h > 50 && w <= 800 && h <= 600, "a part of the screen changed: {w}x{h}");

    // closing and coming back shows the same signed-in session
    drop(s);
    std::thread::sleep(Duration::from_millis(200));
    let (code, _, body) = http(a, &format!("GET /api/session HTTP/1.1\r\nHost: {a}\r\nCookie: {cookie}\r\n\r\n"));
    assert_eq!(code, 200);
    assert!(body.contains("root"));
}

fn login_as(a: std::net::SocketAddr, user: &str, pass: &str) -> (String, String) {
    let body = format!(r#"{{"user":"{user}","password":"{pass}"}}"#);
    let (code, head, body) = http(a, &format!("POST /api/login HTTP/1.1\r\nHost: {a}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()));
    assert_eq!(code, 200, "{user}: {body}");
    let cookie = head.lines().find_map(|l| l.strip_prefix("Set-Cookie: ")).unwrap().split(';').next().unwrap().to_string();
    let csrf = serde_json::from_str::<serde_json::Value>(&body).unwrap()["csrf"].as_str().unwrap().to_string();
    (cookie, csrf)
}

fn open_screen(a: std::net::SocketAddr, cookie: &str, csrf: &str) -> TcpStream {
    let mut s = TcpStream::connect(a).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(format!("GET /ws?csrf={csrf} HTTP/1.1\r\nHost: {a}\r\nOrigin: http://{a}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nCookie: {cookie}\r\n\r\n").as_bytes()).unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        s.read_exact(&mut b).unwrap();
        head.push(b[0]);
    }
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101"), "{}", String::from_utf8_lossy(&head));
    s
}

fn first_picture(s: &mut TcpStream) -> Vec<u8> {
    loop {
        let (op, m) = frame(s);
        if op == 2 {
            return m;
        }
    }
}

#[test]
fn two_people_signed_in_at_once_each_have_their_own_shell_session_and_one_signing_out_leaves_the_other() {
    iyakki_compose::wire();
    let (e, raw) = env_store("two-people");
    raw.lock().unwrap().add_local("ann", "annpass1234", iyakki_sec::policy::Role::Guest).unwrap();
    let audit = e.audit.clone();
    let server = Server::start(Config { addr: "127.0.0.1:0".into(), tls: None }, Arc::new(ShellGate::new(e))).unwrap();
    let a = server.local_addr();
    let (root_cookie, root_csrf) = login_as(a, "root", "rootpass12");
    let (ann_cookie, ann_csrf) = login_as(a, "ann", "annpass1234");
    assert_ne!(root_cookie, ann_cookie);
    assert_eq!(server.sessions(), 2);
    // each cookie is its own person
    let who = |cookie: &str| {
        let (code, _, body) = http(a, &format!("GET /api/session HTTP/1.1\r\nHost: {a}\r\nCookie: {cookie}\r\n\r\n"));
        (code, serde_json::from_str::<serde_json::Value>(&body).map(|v| v["user"].as_str().unwrap_or("").to_string()).unwrap_or_default())
    };
    assert_eq!(who(&root_cookie), (200, "root".to_string()));
    assert_eq!(who(&ann_cookie), (200, "ann".to_string()));
    // both watch their own desktop at the same time (no "one window at a time" refusal across people)
    let mut root_screen = open_screen(a, &root_cookie, &root_csrf);
    let mut ann_screen = open_screen(a, &ann_cookie, &ann_csrf);
    let (rp, ap) = (first_picture(&mut root_screen), first_picture(&mut ann_screen));
    assert_eq!(&rp[..8], &[0, 0, 0, 0, 0, 5, 208, 2]);
    assert_eq!(&ap[..8], &[0, 0, 0, 0, 0, 5, 208, 2]);
    // the audit log names each person with their own role
    let lines = audit.recent(50).join("\n");
    assert!(lines.contains("root") && lines.contains("login.web") && lines.contains("admin"), "{lines}");
    assert!(lines.contains("ann") && lines.contains("guest"), "{lines}");
    // ann signs out: her session ends and her cookie is dead; root's session and picture keep going
    let (code, _, _) = http(a, &format!("POST /api/logout HTTP/1.1\r\nHost: {a}\r\nCookie: {ann_cookie}\r\nX-CSRF: {ann_csrf}\r\nContent-Length: 0\r\n\r\n"));
    assert_eq!(code, 200);
    assert_eq!(who(&ann_cookie).0, 401);
    assert_eq!(who(&root_cookie), (200, "root".to_string()));
    assert_eq!(server.sessions(), 1);
    root_screen.write_all(&masked(r#"{"t":"key","key":"F1"}"#)).unwrap();
    let (op, m) = loop {
        let (op, m) = frame(&mut root_screen);
        if op == 2 && png_size(&m[8..]).1 > 30 {
            break (op, m);
        }
    };
    assert_eq!(op, 2);
    assert!(png_size(&m[8..]).0 > 50, "root's own desktop still answers its keys");
}

#[test]
fn ki121_a_poisoned_account_lock_does_not_block_a_web_or_key_sign_in() {
    use iyakki_remote::screen::{Gate, LoginError};
    let (e, store) = env_store("poison");
    let raw = store.clone();
    let _ = std::thread::spawn(move || {
        let _g = store.lock().unwrap();
        panic!("a thread dies while it holds the account lock");
    })
    .join();
    assert!(raw.is_poisoned(), "the lock is poisoned");
    let gate = ShellGate::new(e);
    // the real answers, never "accounts are busy"
    assert!(matches!(gate.login("root", "not-the-password"), Err(LoginError::Invalid)));
    assert!(matches!(gate.login_key("nobody"), Err(LoginError::Invalid)));
    assert!(gate.login("root", "rootpass12").is_ok(), "the right password still signs in");
}

#[test]
fn the_real_shell_screen_opens_an_sdk_app_for_a_client_and_answers_input_lines() {
    use iyakki_remote::screen::Gate;
    let gate = ShellGate::new(env("listapp"));
    let mut screen = gate.login("root", "rootpass12").ok().expect("signed in");
    let first = screen.app_open("sample", 800, 600).expect("the first frame");
    assert!(first.starts_with("iyakki-display-list"), "{}", &first[..first.len().min(40)]);
    let (next, left) = screen.app_input("key tab").expect("the next frame");
    assert!(next.starts_with("iyakki-display-list") && !left);
    assert_eq!(screen.app_input("not an input line").unwrap_err().code, "bad_request");
    assert_eq!(screen.app_open("chat", 800, 600).unwrap_err().code, "not_found", "an app that is not an SDK app is never made for a client");
    screen.app_close();
    assert_eq!(screen.app_input("key tab").unwrap_err().code, "not_open");
}

#[test]
fn the_real_shell_screen_saves_lists_and_reads_a_clients_file_through_the_core_and_keeps_to_the_home() {
    use iyakki_remote::screen::Gate;
    let gate = ShellGate::new(env("clientfiles"));
    let mut screen = gate.login("root", "rootpass12").ok().expect("signed in");
    let body: Vec<u8> = (0..3000u32).map(|i| (i % 253) as u8).collect();
    assert!(screen.files_save("from-phone.bin", body.clone()).is_ok(), "saved through the core's change book");
    let l = screen.files_list("").expect("listing");
    assert!(l.entries.iter().any(|e| e.name == "from-phone.bin" && e.size == 3000), "{l:?}");
    assert_eq!(screen.files_read("from-phone.bin").unwrap(), body);
    // nothing outside the home, nothing hidden
    for bad in ["../x", "/etc/passwd", ".hidden", ""] {
        assert!(screen.files_read(bad).is_err(), "{bad}");
        assert!(screen.files_save(bad, vec![1]).is_err(), "{bad}");
    }
    assert!(screen.files_list("../..").is_err());
    assert!(screen.files_save("big.bin", vec![0; iyakki_core_api::files::MAX_UPLOAD + 1]).is_err());
}
