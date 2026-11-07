//! The web UI and the web service's API on the desktop's browser server, over real TLS on one port: `/ui/` serves the page's files
//! with the strict headers, `/api/v1/` answers behind the same sign-in (the desktop's password sign-in, a device key, or the API's
//! own login), and nothing else is open before sign-in. The screen and the accounts behind the server are fakes.

use iyakki_agent::agent::Tools;
use iyakki_agent::chat::{ChatBackend, Message};
use iyakki_agent::runs::RunStore;
use iyakki_agent::webservice::deps::{Deps, Extras, ToolSource};
use iyakki_agent::webservice::embed::Embedded;
use iyakki_core_api::{ApiError, ApiResult, Principal};
use iyakki_core_fake::FakeAccounts;
use iyakki_link::sigscheme::DeviceKey;
use iyakki_remote::devicekeys::{DeviceKeys, LOGIN_PATH};
use iyakki_remote::input::Input;
use iyakki_remote::screen::{Gate, LoginError, Screen};
use iyakki_remote::server::{Config, Server};
use iyakki_sec::policy::Role;
use rustls::pki_types::{pem::PemObject, CertificateDer, ServerName};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Fake;

impl Screen for Fake {
    fn user(&self) -> String {
        "alice".into()
    }
    fn input(&mut self, _: Input) {}
    fn viewing(&mut self, _: bool) {}
    fn step(&mut self) -> Option<iyakki_remote::screen::Frame> {
        None
    }
    fn take_copy(&mut self) -> Option<String> {
        None
    }
    fn finished(&self) -> bool {
        false
    }
}

struct NoChat;
impl ChatBackend for NoChat {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        Err("no model in this test".into())
    }
}

struct NoTools;
impl ToolSource for NoTools {
    fn tools_for(&self, _: &Principal) -> Arc<dyn Tools> {
        struct T;
        impl Tools for T {
            fn defs(&self) -> Vec<iyakki_agent::agent::ToolDef> {
                vec![]
            }
            fn call(&self, _: &str, _: &Value) -> Result<String, String> {
                Err("none".into())
            }
            fn describe(&self, n: &str, _: &Value) -> String {
                n.into()
            }
        }
        Arc::new(T)
    }
}

struct Small;
impl Extras for Small {
    fn ready(&self) -> Result<(), String> {
        Ok(())
    }
    fn models(&self, _: &Principal) -> Vec<Value> {
        vec![]
    }
    fn backend_for(&self, _: &Principal, _: &str, _: &str) -> ApiResult<Arc<dyn ChatBackend>> {
        Err(ApiError::Unsupported("none".into()))
    }
    fn flows(&self, _: &Principal) -> ApiResult<Vec<Value>> {
        Ok(vec![])
    }
    fn flow_export(&self, _: &Principal, _: &str) -> ApiResult<Value> {
        Err(ApiError::NotFound("none".into()))
    }
    fn data_call(&self, _: &Principal, _: &str, _: &Value) -> ApiResult<String> {
        Err(ApiError::Unsupported("none".into()))
    }
}

struct FakeGate {
    keys: Arc<DeviceKeys>,
    api: Embedded,
}

impl Gate for FakeGate {
    fn login(&self, user: &str, password: &str) -> Result<Box<dyn Screen>, LoginError> {
        if user == "alice" && password == "alice-password-1" {
            Ok(Box::new(Fake))
        } else {
            Err(LoginError::Invalid)
        }
    }
    fn device_keys(&self, user: &str) -> Vec<[u8; 32]> {
        self.keys.keys_of(user)
    }
    fn login_key(&self, user: &str) -> Result<Box<dyn Screen>, LoginError> {
        if user == "alice" {
            Ok(Box::new(Fake))
        } else {
            Err(LoginError::Invalid)
        }
    }
    fn api(&self) -> Option<&Embedded> {
        Some(&self.api)
    }
}

struct Rig {
    server: Server,
    keys: Arc<DeviceKeys>,
    cfg: Arc<rustls::ClientConfig>,
    dir: std::path::PathBuf,
}

fn rig(tag: &str, with_api: bool) -> Rig {
    let dir = std::env::temp_dir().join(format!("iyakki-webui-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let id = iyakki_remote::tls::identity(&dir, &["localhost".into()]).expect("a certificate");
    let accounts = Arc::new(FakeAccounts::new(&[("alice", "alice-password-1", Role::User), ("root", "root-password-1", Role::Admin)]));
    let deps = Arc::new(Deps { auth: accounts.clone(), audit: accounts, store: Arc::new(RunStore::new(&dir.join("runs"))), chat: Arc::new(NoChat), tools: Arc::new(NoTools), extras: Arc::new(Small), system_prompt: "t".into() });
    let keys = Arc::new(DeviceKeys::in_memory());
    let api = Embedded::new(deps).unwrap();
    let gate: Arc<dyn Gate> = if with_api { Arc::new(FakeGate { keys: keys.clone(), api }) } else { Arc::new(NoApiGate) };
    let server = Server::start(Config { addr: "127.0.0.1:0".into(), tls: Some(id.config) }, gate).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for c in CertificateDer::pem_slice_iter(&std::fs::read(dir.join("cert.pem")).unwrap()) {
        roots.add(c.unwrap()).unwrap();
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = Arc::new(rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions().unwrap().with_root_certificates(roots).with_no_client_auth());
    Rig { server, keys, cfg, dir }
}

/// A gate that has no API (what every other fake gate is).
struct NoApiGate;
impl Gate for NoApiGate {
    fn login(&self, _: &str, _: &str) -> Result<Box<dyn Screen>, LoginError> {
        Err(LoginError::Invalid)
    }
}

struct Reply {
    status: u16,
    head: String,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<String> {
        self.head.lines().find_map(|l| l.split_once(": ").filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.to_string()))
    }
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

impl Rig {
    /// One request over TLS, checking the certificate against the one the server made.
    fn send(&self, raw: &str) -> Reply {
        let a: SocketAddr = self.server.local_addr();
        let conn = rustls::ClientConnection::new(self.cfg.clone(), ServerName::try_from("localhost").unwrap()).unwrap();
        let tcp = TcpStream::connect(a).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut t = rustls::StreamOwned::new(conn, tcp);
        t.write_all(raw.as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = t.read_to_end(&mut out);
        let text = String::from_utf8_lossy(&out).into_owned();
        let status = text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        let (head, body) = text.split_once("\r\n\r\n").map(|(h, b)| (h.to_string(), b.to_string())).unwrap_or_default();
        Reply { status, head, body }
    }
    fn req(&self, method: &str, path: &str, extra: &[(&str, &str)], body: &str) -> Reply {
        let mut s = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n");
        for (k, v) in extra {
            s.push_str(&format!("{k}: {v}\r\n"));
        }
        if !body.is_empty() {
            s.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
        }
        self.send(&format!("{s}\r\n{body}"))
    }
    fn desktop_login(&self) -> String {
        let r = self.req("POST", "/api/login", &[], &json!({"user": "alice", "password": "alice-password-1"}).to_string());
        assert_eq!(r.status, 200, "{}", r.body);
        r.header("Set-Cookie").expect("a cookie").split(';').next().unwrap().to_string()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn the_ui_files_are_served_over_tls_with_the_strict_headers_and_without_sign_in() {
    let r = rig("files", true);
    for (path, ctype, body) in iyakki_agent::webservice::ui::ASSETS {
        let a = r.req("GET", path, &[], "");
        assert_eq!(a.status, 200, "{path}");
        assert_eq!(a.header("Content-Type").as_deref(), Some(*ctype), "{path}");
        assert_eq!(a.body, *body, "{path}");
        assert_eq!(a.header("Content-Security-Policy").as_deref(), Some(iyakki_agent::webservice::ui::CSP), "{path}");
        assert_eq!(a.header("X-Content-Type-Options").as_deref(), Some("nosniff"));
        assert_eq!(a.header("Cache-Control").as_deref(), Some("no-store"));
        assert!(a.header("Strict-Transport-Security").is_some());
        assert!(a.header("Set-Cookie").is_none());
    }
    assert_eq!(r.req("GET", "/ui/index.html", &[], "").status, 200);
    for (m, p) in [("GET", "/ui/missing.js"), ("POST", "/ui/"), ("GET", "/ui/../api/session"), ("DELETE", "/ui/app.js")] {
        assert_eq!(r.req(m, p, &[], "").status, 404, "{m} {p}");
    }
    // the shell's own page is where it was
    assert_eq!(r.req("GET", "/", &[], "").status, 200);
}

#[test]
fn before_sign_in_the_api_gives_nothing_but_the_login_and_the_version() {
    let r = rig("closed", true);
    for p in ["/api/v1/whoami", "/api/v1/chat/runs", "/api/v1/models", "/api/v1/webaccess", "/api/v1/openapi.json", "/api/v1/admin/status"] {
        assert_eq!(r.req("GET", p, &[], "").status, 401, "{p}");
    }
    assert_eq!(r.req("GET", "/api/v1/version", &[], "").status, 200);
    assert_eq!(r.req("GET", "/api/v1/files", &[], "").status, 404, "cloud routes are not on a desktop");
    // a bad token and a forged cookie are no sign-in
    assert_eq!(r.req("GET", "/api/v1/whoami", &[("Authorization", "Bearer iyk_nope")], "").status, 401);
    assert_eq!(r.req("GET", "/api/v1/whoami", &[("Cookie", "iyakki_session=forged")], "").status, 401);
    // a gate without an API (every other fake) answers 404 for the whole prefix
    let n = rig("noapi", false);
    assert_eq!(n.req("GET", "/api/v1/version", &[], "").status, 404);
}

#[test]
fn the_desktop_password_sign_in_carries_over_to_the_api() {
    let r = rig("pw", true);
    let cookie = r.desktop_login();
    let w = r.req("GET", "/api/v1/whoami", &[("Cookie", &cookie)], "");
    assert_eq!(w.status, 200, "{}", w.body);
    assert_eq!(w.json()["user"], "alice");
    assert!(!w.body.contains("iyk_"), "the bridged token never reaches the page");
    // the account's role decides, as everywhere
    assert_eq!(r.req("GET", "/api/v1/admin/status", &[("Cookie", &cookie)], "").status, 403);
    // a page on another origin cannot use it
    assert_eq!(r.req("GET", "/api/v1/whoami", &[("Cookie", &cookie), ("Origin", "https://evil.example")], "").status, 403);
    assert_eq!(r.req("GET", "/api/v1/whoami", &[("Cookie", &cookie), ("Sec-Fetch-Site", "cross-site")], "").status, 401);
    // signing out from the page ends the desktop session too
    let out = r.req("POST", "/api/v1/logout", &[("Cookie", &cookie)], "");
    assert_eq!(out.status, 200, "{}", out.body);
    assert_eq!(r.req("GET", "/api/session", &[("Cookie", &cookie)], "").status, 401);
    assert_eq!(r.req("GET", "/api/v1/whoami", &[("Cookie", &cookie)], "").status, 401);
}

#[test]
fn a_device_key_sign_in_carries_over_to_the_api() {
    let r = rig("key", true);
    let k = DeviceKey::from_seed([7; 32]);
    r.keys.enroll("alice", &k.ssh_public_line("laptop")).unwrap();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let h = k.sign_request("alice", "POST", LOGIN_PATH, now, b"").unwrap();
    let l = r.req("POST", LOGIN_PATH, &[("Authorization", &h)], "");
    assert_eq!(l.status, 200, "{}", l.body);
    let cookie = l.header("Set-Cookie").unwrap().split(';').next().unwrap().to_string();
    let w = r.req("GET", "/api/v1/whoami", &[("Cookie", &cookie)], "");
    assert_eq!((w.status, w.json()["user"].clone()), (200, json!("alice")), "{}", w.body);
}

#[test]
fn the_page_can_also_sign_in_with_the_api_login_and_use_its_bearer_token() {
    let r = rig("login", true);
    let bad = r.req("POST", "/api/v1/login", &[], &json!({"name": "root", "password": "wrong"}).to_string());
    assert_eq!(bad.status, 401);
    let ok = r.req("POST", "/api/v1/login", &[], &json!({"name": "root", "password": "root-password-1"}).to_string());
    assert_eq!(ok.status, 200, "{}", ok.body);
    let token = ok.json()["token"].as_str().unwrap().to_string();
    let bearer = format!("Bearer {token}");
    let w = r.req("GET", "/api/v1/whoami", &[("Authorization", &bearer)], "");
    assert_eq!((w.status, w.json()["user"].clone()), (200, json!("root")));
    assert_eq!(r.req("GET", "/api/v1/admin/status", &[("Authorization", &bearer)], "").status, 200);
    // this path made no desktop session: no cookie was set
    assert!(ok.header("Set-Cookie").is_none());
}
