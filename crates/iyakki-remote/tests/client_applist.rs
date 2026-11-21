//! Display-list mode of the client endpoint over a real local socket (F-SDK-29, D-CLIENT-PROTO-DISPLAYLIST): the capability is advertised in
//! hello and only agreed when the client offers it, an app opens and answers frames to input lines, refusals have stable words, a frame
//! over the bound is refused, leaving the app is told, ending the session closes the app, and an older client is unaffected. Fake screen
//! and gate; the real shell's side is tested in `iyakki-shell` and in `real_shell_screen` below. No browser was used.

#![allow(dead_code)]

use iyakki_agent::clientlink as link;
use iyakki_core_api::appcaps::{record_approval, Approval, CapabilityRequest};
use iyakki_core_api::broker::{client_key, ApprovalLookup, Broker, CardAnswer, CardAsk, Caller, MemoryLog};
use iyakki_core_api::client::{self as proto, AppMsg, ClientMsg, ServerMsg};
use iyakki_link::sigscheme::DeviceKey;
use iyakki_remote::clientsession::{CallHandler, ClientEndpoint, FailLimit};
use iyakki_remote::input::Input;
use iyakki_remote::screen::{AppRefusal, Frame, Gate, LoginError, Pixels, Screen};
use iyakki_remote::server::{Config, Server};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Seen {
    opened: Vec<String>,
    inputs: Vec<String>,
    closed: u32,
    big: bool,
    closed_tx: Option<std::sync::mpsc::Sender<()>>,
}

struct Fake {
    seen: Arc<Mutex<Seen>>,
}

impl Screen for Fake {
    fn user(&self) -> String {
        "ann".into()
    }
    fn input(&mut self, _: Input) {}
    fn viewing(&mut self, _: bool) {}
    fn step(&mut self) -> Option<Frame> {
        None
    }
    fn take_copy(&mut self) -> Option<String> {
        None
    }
    fn arm(&mut self) {}
    fn take_pixels(&mut self) -> Option<Pixels> {
        None
    }
    fn app_open(&mut self, app: &str, w: u32, h: u32) -> Result<String, AppRefusal> {
        let mut s = self.seen.lock().unwrap();
        match app {
            "sample" => {
                s.opened.push(format!("{app} {w}x{h}"));
                if s.big {
                    Ok("x".repeat(proto::MAX_APP_LIST + 1))
                } else {
                    Ok(format!("iyakki-display-list 1 {w} {h}\n"))
                }
            }
            "private" => Err(AppRefusal::new("denied", "your role cannot open that")),
            _ => Err(AppRefusal::new("not_found", "that app cannot be shown in a client")),
        }
    }
    fn app_input(&mut self, lines: &str) -> Result<(String, bool), AppRefusal> {
        self.seen.lock().unwrap().inputs.push(lines.to_string());
        if lines.contains("bad") {
            return Err(AppRefusal::new("bad_request", "line 1: unknown or malformed input"));
        }
        Ok((format!("iyakki-display-list 1 800 600\n# after {}\n", lines.replace('\n', ";")), lines.contains("esc")))
    }
    fn app_close(&mut self) {
        let mut s = self.seen.lock().unwrap();
        s.closed += 1;
        if let Some(t) = &s.closed_tx {
            let _ = t.send(());
        }
    }
    fn finished(&self) -> bool {
        false
    }
}

struct FakeGate {
    seen: Arc<Mutex<Seen>>,
    ep: Arc<ClientEndpoint>,
}

impl Gate for FakeGate {
    fn login(&self, user: &str, password: &str) -> Result<Box<dyn Screen>, LoginError> {
        if user == "ann" && password == "right" {
            Ok(Box::new(Fake { seen: self.seen.clone() }))
        } else {
            Err(LoginError::Invalid)
        }
    }
    fn client_endpoint(&self) -> Option<&ClientEndpoint> {
        Some(&self.ep)
    }
}

struct Book;
impl ApprovalLookup for Book {
    fn approval(&self, installation: &str) -> Option<Approval> {
        let _ = client_key("x");
        let reqs: Vec<CapabilityRequest> = Vec::new();
        let _ = installation;
        Some(record_approval("ann", "client", "1", "", 1, &reqs))
    }
}

struct Card;
impl CardAsk for Card {
    fn ask(&self, _: &Caller, _: &str, _: &str) -> CardAnswer {
        CardAnswer::Deny
    }
}

struct Calls;
impl CallHandler for Calls {
    fn role_allows(&self, person: &str, _: &str) -> bool {
        person == "ann"
    }
    fn perform(&self, _: &str, _: &str, _: Option<&str>) -> Result<String, String> {
        Ok(String::new())
    }
}

struct Rig {
    closed_rx: std::sync::mpsc::Receiver<()>,
    srv: Server,
    data: PathBuf,
    seen: Arc<Mutex<Seen>>,
    audit: Arc<Mutex<Vec<String>>>,
}

/// A folder that is this test's alone. The name carries the process id, the time and a counter, and the folder is made with `create_dir`,
/// which fails when it exists already: a folder is never taken over from, or cleared under, another run (two runs that share `TMPDIR`
/// and happen to have the same process id, as in a container or a gate that runs jobs side by side, used to share a name, and one
/// run's clearing of it took the other's token and device files away: "no such file or directory", "that link is not known").
fn dir(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let base = std::env::temp_dir();
    loop {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = base.join(format!("iyakki-clientmedia-{}-{nanos:x}-{n}-{tag}", std::process::id()));
        match std::fs::create_dir(&d) {
            Ok(()) => return d,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("{}: {e}", d.display()),
        }
    }
}

fn rig(tag: &str) -> Rig {
    rig_in(tag, None)
}

fn rig_in(tag: &str, tenant: Option<&str>) -> Rig {
    let data = dir(tag);
    link::set_enabled(&data, true).unwrap();
    let log = Arc::new(MemoryLog::new(100));
    let broker = Arc::new(Broker::new(Arc::new(Book), Arc::new(Card), log, Arc::new(|| iyakki_agent::clock::now_unix())));
    let audit = Arc::new(Mutex::new(Vec::<String>::new()));
    let a2 = audit.clone();
    let mut ep = ClientEndpoint::with_limit(&data, broker, Arc::new(Calls), Arc::new(move |u: &str, a: &str, d: &str| a2.lock().unwrap().push(format!("{u}|{a}|{d}"))), FailLimit::new(50, Duration::from_secs(60)));
    if let Some(t) = tenant {
        ep = ep.with_tenant(t);
    }
    let ep = Arc::new(ep);
    let (closed_tx, closed_rx) = std::sync::mpsc::channel();
    let seen = Arc::new(Mutex::new(Seen { closed_tx: Some(closed_tx), ..Default::default() }));
    let srv = Server::start(Config { addr: "127.0.0.1:0".into(), tls: None }, Arc::new(FakeGate { seen: seen.clone(), ep })).unwrap();
    Rig { closed_rx, srv, data, seen, audit }
}

fn masked(op: u8, p: &[u8]) -> Vec<u8> {
    let mask = [7u8, 1, 4, 2];
    let mut f = vec![0x80 | op];
    if p.len() < 126 {
        f.push(0x80 | p.len() as u8);
    } else if p.len() < 65536 {
        f.push(0x80 | 126);
        f.extend_from_slice(&(p.len() as u16).to_be_bytes());
    } else {
        f.push(0x80 | 127);
        f.extend_from_slice(&(p.len() as u64).to_be_bytes());
    }
    f.extend_from_slice(&mask);
    f.extend(p.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

enum Got {
    Msg(ServerMsg),
    App(AppMsg),
    Bin(Vec<u8>),
    Close,
}

struct Client {
    s: TcpStream,
}

impl Client {
    fn open(addr: SocketAddr) -> Client {
        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let req = format!("GET /client/ws HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n");
        s.write_all(req.as_bytes()).unwrap();
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            s.read_exact(&mut b).unwrap();
            head.push(b[0]);
        }
        assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101"));
        Client { s }
    }

    fn send(&mut self, m: &ClientMsg) {
        self.s.write_all(&masked(1, proto::encode_client(m).as_bytes())).unwrap();
    }

    fn send_bin(&mut self, stream: u8, payload: &[u8]) {
        self.s.write_all(&masked(2, &proto::binary(stream, payload))).unwrap();
    }

    fn next(&mut self, wait: Duration) -> Option<Got> {
        self.s.set_read_timeout(Some(wait)).unwrap();
        let mut h = [0u8; 2];
        if self.s.read_exact(&mut h).is_err() {
            return None;
        }
        self.s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let len = match h[1] & 0x7f {
            126 => {
                let mut l = [0u8; 2];
                self.s.read_exact(&mut l).ok()?;
                u16::from_be_bytes(l) as usize
            }
            127 => {
                let mut l = [0u8; 8];
                self.s.read_exact(&mut l).ok()?;
                u64::from_be_bytes(l) as usize
            }
            n => n as usize,
        };
        let mut p = vec![0u8; len];
        self.s.read_exact(&mut p).ok()?;
        Some(match h[0] & 0x0f {
            1 => {
                let t = std::str::from_utf8(&p).unwrap();
                match proto::decode_app(t).expect("a message of the protocol") {
                    Some(a) => Got::App(a),
                    None => Got::Msg(proto::decode_server(t).expect("a message of the protocol")),
                }
            }
            2 => Got::Bin(p),
            _ => Got::Close,
        })
    }

    /// The next text message (binary ones are skipped).
    fn msg(&mut self) -> ServerMsg {
        loop {
            match self.next(Duration::from_secs(8)) {
                Some(Got::Msg(m)) => return m,
                Some(Got::App(a)) => panic!("expected a message of the old protocol, got {a:?}"),
                Some(Got::Close) => panic!("closed"),
                Some(Got::Bin(_)) => {}
                None => panic!("no message"),
            }
        }
    }

    /// The next message of the display-list mode.
    fn app(&mut self) -> AppMsg {
        loop {
            match self.next(Duration::from_secs(8)) {
                Some(Got::App(a)) => return a,
                Some(Got::Msg(m)) => panic!("expected a display-list message, got {m:?}"),
                Some(Got::Close) => panic!("closed"),
                Some(Got::Bin(_)) => {}
                None => panic!("no message"),
            }
        }
    }

    fn hello(&mut self, offer_list: bool) -> (String, Vec<String>) {
        let mut offered: Vec<String> = ["picture", "input", "events", "calls"].iter().map(|s| s.to_string()).collect();
        if offer_list {
            offered.push("display.list".into());
        }
        self.send(&ClientMsg::Hello { v: 1, min_v: 1, offered, needed: vec![], client: "test".into() });
        match self.msg() {
            ServerMsg::HelloOk { challenge, caps, .. } => (challenge, caps),
            other => panic!("{other:?}"),
        }
    }

    }

fn now() -> u64 {
    iyakki_agent::clock::now_unix() as u64
}

fn connect(r: &Rig, seed: u8, offer_list: bool) -> (Client, Vec<String>) {
    let token = link::create_token(&r.data, "ann", now() as i64, 300).unwrap();
    let key = DeviceKey::from_seed([seed; 32]);
    let mut c = Client::open(r.srv.local_addr());
    let (_, caps) = c.hello(offer_list);
    c.send(&ClientMsg::Enrol { token, public: key.public_hex(), name: "Phone".into() });
    let (device, challenge) = match c.msg() {
        ServerMsg::Enrolled { device, challenge } => (device, challenge),
        other => panic!("{other:?}"),
    };
    c.send(&ClientMsg::Auth { authorization: link::sign_answer(&key, &device, &challenge, now()).unwrap() });
    assert!(matches!(c.msg(), ServerMsg::Authed { .. }));
    c.send(&ClientMsg::SignIn { user: "ann".into(), password: "right".into() });
    assert_eq!(c.msg(), ServerMsg::SignedIn { person: "ann".into() });
    (c, caps)
}

fn app_err(c: &mut Client) -> (String, String) {
    match c.app() {
        AppMsg::AppErr { code, message } => (code, message),
        other => panic!("expected an app refusal, got {other:?}"),
    }
}

fn app_list(c: &mut Client) -> (String, String) {
    match c.app() {
        AppMsg::AppList { app, text } => (app, text),
        other => panic!("expected a frame, got {other:?}"),
    }
}

#[test]
fn the_display_list_is_advertised_and_agreed_only_when_the_client_offers_it() {
    let r = rig("caps");
    let (_, caps) = connect(&r, 1, true);
    assert_eq!(caps, vec!["picture", "input", "events", "calls", "display.list"]);
    // an older client that does not know the capability agrees on exactly what it did before, and its request is refused by name
    let (mut old, caps) = connect(&r, 2, false);
    assert_eq!(caps, vec!["picture", "input", "events", "calls"]);
    old.send(&ClientMsg::AppOpen { app: "sample".into(), w: 800, h: 600 });
    assert_eq!(app_err(&mut old).0, "unsupported");
    assert!(r.seen.lock().unwrap().opened.is_empty(), "nothing reached the screen");
}

#[test]
fn an_app_opens_answers_frames_to_input_lines_and_closes_and_each_step_is_audited() {
    let r = rig("flow");
    let (mut c, _) = connect(&r, 3, true);
    c.send(&ClientMsg::AppOpen { app: "sample".into(), w: 800, h: 600 });
    let (app, text) = app_list(&mut c);
    assert_eq!((app.as_str(), text.as_str()), ("sample", "iyakki-display-list 1 800 600\n"));
    c.send(&ClientMsg::AppInput { lines: "key enter\naction 3".into() });
    let (_, text) = app_list(&mut c);
    assert!(text.contains("# after key enter;action 3"), "{text}");
    // the picture of the screen is not stopped by the app: a ping still answers
    c.send(&ClientMsg::Ping { ts: 7 });
    assert_eq!(c.msg(), ServerMsg::Pong { ts: 7 });
    c.send(&ClientMsg::AppClose);
    assert_eq!(c.app(), AppMsg::AppClosed { app: "sample".into(), reason: "closed".into() });
    c.send(&ClientMsg::AppInput { lines: "key enter".into() });
    assert_eq!(app_err(&mut c).0, "not_open");
    c.send(&ClientMsg::AppClose);
    assert_eq!(app_err(&mut c).0, "not_open");
    let a = r.audit.lock().unwrap().join("\n");
    assert!(a.contains("ann|client.app.open|sample") && a.contains("ann|client.app.close|sample"), "{a}");
    assert!(!a.contains("key enter"), "the input lines are never audited");
    assert_eq!(r.seen.lock().unwrap().closed, 1);
}

#[test]
fn refusals_have_stable_words_and_the_session_goes_on() {
    let r = rig("refuse");
    let (mut c, _) = connect(&r, 4, true);
    c.send(&ClientMsg::AppOpen { app: "private".into(), w: 800, h: 600 });
    assert_eq!(app_err(&mut c), ("denied".into(), "your role cannot open that".into()));
    c.send(&ClientMsg::AppOpen { app: "nothing".into(), w: 800, h: 600 });
    assert_eq!(app_err(&mut c).0, "not_found");
    c.send(&ClientMsg::AppOpen { app: "../../etc".into(), w: 800, h: 600 });
    assert_eq!(app_err(&mut c).0, "bad_request", "a name that is not a plain id never reaches the screen");
    c.send(&ClientMsg::AppOpen { app: "sample".into(), w: 800, h: 600 });
    app_list(&mut c);
    c.send(&ClientMsg::AppInput { lines: "bad line".into() });
    assert_eq!(app_err(&mut c).0, "bad_request");
    c.send(&ClientMsg::AppInput { lines: "key tab".into() });
    app_list(&mut c);
    c.send(&ClientMsg::Ping { ts: 1 });
    assert_eq!(c.msg(), ServerMsg::Pong { ts: 1 });
    assert_eq!(r.seen.lock().unwrap().opened, vec!["sample 800x600"], "only the allowed open reached the screen");
}

#[test]
fn leaving_the_app_is_told_after_its_last_frame_and_a_list_over_the_bound_is_refused() {
    let r = rig("leave");
    let (mut c, _) = connect(&r, 5, true);
    c.send(&ClientMsg::AppOpen { app: "sample".into(), w: 800, h: 600 });
    app_list(&mut c);
    c.send(&ClientMsg::AppInput { lines: "key esc".into() });
    app_list(&mut c);
    assert_eq!(c.app(), AppMsg::AppClosed { app: "sample".into(), reason: "left".into() });
    c.send(&ClientMsg::AppInput { lines: "key tab".into() });
    assert_eq!(app_err(&mut c).0, "not_open");
    r.seen.lock().unwrap().big = true;
    c.send(&ClientMsg::AppOpen { app: "sample".into(), w: 800, h: 600 });
    assert_eq!(c.app(), AppMsg::AppClosed { app: "sample".into(), reason: "too_big".into() });
    assert_eq!(app_err(&mut c).0, "too_big");
}

#[test]
fn ending_the_session_closes_the_app_on_the_screen() {
    let r = rig("end");
    let (mut c, _) = connect(&r, 6, true);
    c.send(&ClientMsg::AppOpen { app: "sample".into(), w: 800, h: 600 });
    app_list(&mut c);
    c.send(&ClientMsg::Bye);
    // the screen says so on a channel when its app is closed: no polling and no sleeping
    r.closed_rx.recv_timeout(Duration::from_secs(30)).expect("the app was not closed");
    assert_eq!(r.seen.lock().unwrap().closed, 1);
}

#[test]
fn nobody_signed_in_cannot_open_an_app() {
    let r = rig("nosign");
    let mut c = Client::open(r.srv.local_addr());
    c.hello(true);
    c.send(&ClientMsg::AppOpen { app: "sample".into(), w: 800, h: 600 });
    // a connection that has not proven its device is refused before the app is looked at
    match c.msg() {
        ServerMsg::Error { .. } => {}
        other => panic!("{other:?}"),
    }
    assert!(r.seen.lock().unwrap().opened.is_empty());
}

// ------------------------------------------------------------------------------------------------------------------ cloud tenants (D-CLIENT-TENANT)

/// A second person of the fake machine, to try a device on.
fn sign_in_as(c: &mut Client, user: &str) -> ServerMsg {
    c.send(&ClientMsg::SignIn { user: user.into(), password: "right".into() });
    c.msg()
}

fn enrol_only(r: &Rig, seed: u8) -> Client {
    let token = link::create_token(&r.data, "ann", now() as i64, 300).unwrap();
    let key = DeviceKey::from_seed([seed; 32]);
    let mut c = Client::open(r.srv.local_addr());
    c.hello(true);
    c.send(&ClientMsg::Enrol { token, public: key.public_hex(), name: "Phone".into() });
    let (device, challenge) = match c.msg() {
        ServerMsg::Enrolled { device, challenge } => (device, challenge),
        other => panic!("{other:?}"),
    };
    c.send(&ClientMsg::Auth { authorization: link::sign_answer(&key, &device, &challenge, now()).unwrap() });
    assert!(matches!(c.msg(), ServerMsg::Authed { .. }));
    c
}

#[test]
fn in_a_cloud_tenant_a_device_belongs_to_the_person_who_made_its_link_and_every_audit_line_names_the_tenant() {
    let r = rig_in("tenant", Some("acme"));
    // the person who made the link signs in on the device and opens an app, as on a machine
    let (mut c, _) = connect(&r, 7, true);
    c.send(&ClientMsg::AppOpen { app: "sample".into(), w: 800, h: 600 });
    app_list(&mut c);
    let a = r.audit.lock().unwrap().join("\n");
    assert!(a.contains("client.enrol|") && a.contains("tenant=acme"), "{a}");
    assert!(a.lines().filter(|l| !l.is_empty()).all(|l| l.ends_with("tenant=acme")), "every audit line names the tenant: {a}");
    // another person of the tenant cannot sign in on that person's device
    let mut other = enrol_only(&r, 8);
    let refused = sign_in_as(&mut other, "bob");
    assert!(matches!(refused, ServerMsg::Error { code: iyakki_core_api::client::ErrorCode::RoleDenied, .. }), "{refused:?}");
    assert!(r.audit.lock().unwrap().join("\n").contains("client.signin.refused|other_person"), "the refusal is audited");
}

#[test]
fn a_machine_that_is_not_a_cloud_tenant_keeps_its_old_rule_and_two_tenants_never_share_links_or_devices() {
    // no tenant: the person is not matched against the device's creator (as before)
    let m = rig_in("machine", None);
    let mut c = enrol_only(&m, 9);
    let r = sign_in_as(&mut c, "ann");
    assert_eq!(r, ServerMsg::SignedIn { person: "ann".into() });
    // two tenants, two data folders: a token made in one is not known in the other, and a device enrolled in one is unknown in the other
    let (a, b) = (rig_in("tenant-a", Some("acme")), rig_in("tenant-b", Some("globex")));
    assert_ne!(a.data, b.data);
    let token = link::create_token(&a.data, "ann", now() as i64, 300).unwrap();
    let key = DeviceKey::from_seed([10; 32]);
    let mut cb = Client::open(b.srv.local_addr());
    cb.hello(true);
    cb.send(&ClientMsg::Enrol { token, public: key.public_hex(), name: "Phone".into() });
    assert!(matches!(cb.msg(), ServerMsg::Error { code: iyakki_core_api::client::ErrorCode::TokenUnknown, .. }), "tenant A's link means nothing to tenant B");
    let ca = enrol_only(&a, 11);
    drop(ca);
    assert!(b.audit.lock().unwrap().join("\n").contains("tenant=globex") && !b.audit.lock().unwrap().join("\n").contains("acme"));
}
