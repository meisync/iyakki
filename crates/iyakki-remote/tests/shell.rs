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
    let d = std::env::temp_dir().join(format!("iyakki-remote-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let mut store = UserStore::open(&d.join("users.json")).unwrap();
    store.create_first_admin("root", "rootpass12").unwrap();
    let up: Arc<dyn Upstream> = Arc::new(Offline);
    ShellEnv {
        store: Arc::new(Mutex::new(store)),
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
    }
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
    let (op, m) = frame(&mut s);
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
    let (op, m) = frame(&mut s);
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
