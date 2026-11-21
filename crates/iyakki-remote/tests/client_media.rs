//! The media half of the client endpoint over a real local socket: sound to the client (and the machine's speakers staying quiet), the text
//! clipboard, camera frames of the instance, and the client's own camera and microphone as virtual sources. Fakes: screen, camera feed,
//! card. No browser, phone, speaker or camera was used.

use iyakki_agent::clientlink as link;
use iyakki_agent::clientmedia::{local_session, CamState, CameraFeed, ClientConsent, Lease, MediaHost, Taken};
use iyakki_core_api::appcaps::{record_approval, Approval, CapabilityRequest};
use iyakki_core_api::broker::{client_key, ApprovalLookup, Broker, CardAnswer, CardAsk, Caller, MemoryLog};
use iyakki_core_api::client::media as media;
use iyakki_core_api::client::{self as proto, ClientMsg, ErrorCode, ServerMsg};
use iyakki_link::sigscheme::DeviceKey;
use iyakki_remote::clientsession::{CallHandler, ClientEndpoint, FailLimit};
use iyakki_remote::input::Input;
use iyakki_remote::screen::{Frame, Gate, LoginError, Pixels, Screen};
use iyakki_remote::server::{Config, Server};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ------------------------------------------------------------------------------------------------------------------ the fakes

#[derive(Default)]
struct Seen {
    inputs: Vec<String>,
    copy: Option<String>,
    /// The person's home as the fake screen's files facet has it (name -> bytes), the reads it served and what it saved.
    files: std::collections::BTreeMap<String, Vec<u8>>,
    reads: u32,
}

struct Fake {
    seen: Arc<Mutex<Seen>>,
}

impl Screen for Fake {
    fn user(&self) -> String {
        "ann".into()
    }
    fn input(&mut self, i: Input) {
        self.seen.lock().unwrap().inputs.push(format!("{i:?}"));
    }
    fn viewing(&mut self, _: bool) {}
    fn step(&mut self) -> Option<Frame> {
        None
    }
    fn take_copy(&mut self) -> Option<String> {
        self.seen.lock().unwrap().copy.take()
    }
    fn arm(&mut self) {}
    fn take_pixels(&mut self) -> Option<Pixels> {
        None
    }
    fn files_list(&mut self, dir: &str) -> Result<iyakki_core_api::Listing, iyakki_remote::screen::AppRefusal> {
        let g = self.seen.lock().unwrap();
        let entries = g.files.iter().map(|(n, b)| iyakki_core_api::FileEntry { name: n.clone(), is_dir: false, size: b.len() as u64, modified: None }).collect();
        Ok(iyakki_core_api::Listing { dir: dir.into(), entries, truncated: false })
    }
    fn files_read(&mut self, path: &str) -> Result<Vec<u8>, iyakki_remote::screen::AppRefusal> {
        let mut g = self.seen.lock().unwrap();
        g.reads += 1;
        g.files.get(path).cloned().ok_or_else(|| iyakki_remote::screen::AppRefusal::new("failed", "that file cannot be read"))
    }
    fn files_save(&mut self, path: &str, bytes: Vec<u8>) -> Result<String, iyakki_remote::screen::AppRefusal> {
        self.seen.lock().unwrap().files.insert(path.to_string(), bytes);
        Ok("Saved.".into())
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

/// Which capabilities each client device holds (by device id).
#[derive(Default)]
struct Book(Mutex<HashMap<String, Vec<String>>>);
impl ApprovalLookup for Book {
    fn approval(&self, installation: &str) -> Option<Approval> {
        let m = self.0.lock().unwrap();
        let (_, caps) = m.iter().find(|(id, _)| client_key(id) == installation)?;
        let reqs: Vec<CapabilityRequest> = caps.iter().map(|c| CapabilityRequest { name: c.clone(), why: "test".into(), param: None, extra: Default::default() }).collect();
        Some(record_approval("ann", "client", "1", "", 1, &reqs))
    }
}

/// 0 allow, 1 deny, 2 no card.
struct Card(Arc<AtomicU8>);
impl CardAsk for Card {
    fn ask(&self, _: &Caller, _: &str, _: &str) -> CardAnswer {
        match self.0.load(Ordering::SeqCst) {
            0 => CardAnswer::Allow,
            1 => CardAnswer::Deny,
            _ => CardAnswer::Unavailable,
        }
    }
}

struct Calls;
impl CallHandler for Calls {
    fn role_allows(&self, person: &str, _: &str) -> bool {
        person == "ann"
    }
    fn perform(&self, _: &str, capability: &str, unit: Option<&str>) -> Result<String, String> {
        Ok(format!("{capability}:{}", unit.unwrap_or("-")))
    }
}

struct FakeFeed {
    state: Mutex<CamState>,
    seq: Mutex<u64>,
}
impl CameraFeed for FakeFeed {
    fn list(&self, person: &str) -> Vec<(String, String)> {
        if person == "ann" { vec![("usb:video0".into(), "Desk camera".into())] } else { Vec::new() }
    }
    fn label(&self, _: &str, unit: &str) -> String {
        format!("camera {unit}")
    }
    fn state(&self, _: &str, unit: &str) -> CamState {
        if unit == "usb:nothing" {
            return CamState::Missing;
        }
        self.state.lock().unwrap().clone()
    }
    fn jpeg(&self, _: &str, _: &str, after: u64) -> Result<Option<(u64, Vec<u8>)>, String> {
        // a new picture on every look (a fast camera); the sender must still hold itself to 5 a second
        let mut s = self.seq.lock().unwrap();
        *s += 1;
        let _ = after;
        Ok(Some((*s, vec![0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9])))
    }
}

struct Rig {
    srv: Server,
    ep: Arc<ClientEndpoint>,
    data: PathBuf,
    seen: Arc<Mutex<Seen>>,
    log: Arc<MemoryLog>,
    audit: Arc<Mutex<Vec<String>>>,
    book: Arc<Book>,
    card: Arc<AtomicU8>,
    host: Arc<MediaHost>,
    feed: Arc<FakeFeed>,
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

fn rig(tag: &str, with_media: bool) -> Rig {
    let data = dir(tag);
    link::set_enabled(&data, true).unwrap();
    let log = Arc::new(MemoryLog::new(500));
    let book = Arc::new(Book::default());
    let card = Arc::new(AtomicU8::new(0));
    let feed = Arc::new(FakeFeed { state: Mutex::new(CamState::Ready), seq: Mutex::new(0) });
    // the host has parts of its own (a test must not touch the process-wide ones)
    let consent = Arc::new(ClientConsent::new(Some(Arc::new(Card(card.clone())))));
    let host = Arc::new(MediaHost::new(consent).with_cameras(feed.clone()).with_tenant(|p| format!("tenant-of-{p}")));
    host.audio.set_audible(|p| p == "ann");
    let broker = Arc::new(Broker::new(book.clone(), host.card(), log.clone(), Arc::new(|| iyakki_agent::clock::now_unix())));
    let audit = Arc::new(Mutex::new(Vec::<String>::new()));
    let a2 = audit.clone();
    let mut ep = ClientEndpoint::with_limit(&data, broker, Arc::new(Calls), Arc::new(move |u: &str, a: &str, d: &str| a2.lock().unwrap().push(format!("{u}|{a}|{d}"))), FailLimit::new(50, Duration::from_secs(60)));
    if with_media {
        ep = ep.with_media(host.clone());
    }
    let ep = Arc::new(ep);
    let seen = Arc::new(Mutex::new(Seen::default()));
    let srv = Server::start(Config { addr: "127.0.0.1:0".into(), tls: None }, Arc::new(FakeGate { seen: seen.clone(), ep: ep.clone() })).unwrap();
    Rig { srv, ep, data, seen, log, audit, book, card, host, feed }
}

// ------------------------------------------------------------------------------------------------------------------ the client

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
    /// A file-transfer message (not a `ServerMsg`: its own `file_*` tags).
    File(serde_json::Value),
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
            1 if p.starts_with(br#"{"t":"file_"#) => Got::File(serde_json::from_slice(&p).unwrap()),
            1 => Got::Msg(proto::decode_server(std::str::from_utf8(&p).unwrap()).expect("a message of the protocol")),
            2 => Got::Bin(p),
            _ => Got::Close,
        })
    }

    /// The next text message (binary ones are skipped).
    fn msg(&mut self) -> ServerMsg {
        let t0 = Instant::now();
        loop {
            assert!(t0.elapsed() < Duration::from_secs(8), "no message");
            match self.next(Duration::from_millis(200)) {
                Some(Got::Msg(m)) => return m,
                Some(Got::Close) => panic!("closed"),
                _ => {}
            }
        }
    }

    /// Everything that arrives within `wait`: text messages and the payloads of binary streams (stream tag, payload).
    fn drain(&mut self, wait: Duration) -> (Vec<ServerMsg>, Vec<(u8, Vec<u8>)>) {
        let (mut m, mut b) = (vec![], vec![]);
        let t0 = Instant::now();
        while t0.elapsed() < wait {
            match self.next(Duration::from_millis(50)) {
                Some(Got::Msg(x)) => m.push(x),
                Some(Got::File(_)) => {}
                Some(Got::Bin(x)) => b.push((x[0], x[1..].to_vec())),
                Some(Got::Close) => break,
                None => {}
            }
        }
        (m, b)
    }

    fn closed_within(&mut self, wait: Duration) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < wait {
            let mut b = [0u8; 1];
            self.s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            match self.s.peek(&mut b) {
                Ok(0) => return true,
                Ok(_) => {
                    if let Some(Got::Close) = self.next(Duration::from_millis(200)) {
                        return true;
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(_) => return true,
            }
        }
        false
    }

    fn hello(&mut self, offer_media: bool) -> (String, Vec<String>) {
        let mut offered: Vec<String> = ["picture", "input", "events", "calls"].iter().map(|s| s.to_string()).collect();
        if offer_media {
            offered.extend(proto::cap::SERVER_MEDIA.iter().map(|s| s.to_string()));
            offered.extend(proto::cap::SERVER_FILES.iter().map(|s| s.to_string()));
        }
        self.send(&ClientMsg::Hello { v: 1, min_v: 1, offered, needed: vec![], client: "test".into() });
        match self.msg() {
            ServerMsg::HelloOk { challenge, caps, .. } => (challenge, caps),
            other => panic!("{other:?}"),
        }
    }

    fn media_err(&mut self) -> (String, String) {
        match self.msg() {
            ServerMsg::MediaErr { what, code, .. } => (what, code),
            other => panic!("expected a media refusal, got {other:?}"),
        }
    }
}

fn now() -> u64 {
    iyakki_agent::clock::now_unix() as u64
}

/// Hello (with or without the media capabilities), enrol with a fresh link, prove the device, sign in. The device holds `caps`.
fn connect(r: &Rig, name: &str, seed: u8, offer_media: bool, caps: &[&str]) -> (Client, String, DeviceKey) {
    let token = link::create_token(&r.data, "ann", now() as i64, 300).unwrap();
    let key = DeviceKey::from_seed([seed; 32]);
    let mut c = Client::open(r.srv.local_addr());
    c.hello(offer_media);
    c.send(&ClientMsg::Enrol { token, public: key.public_hex(), name: name.into() });
    let (device, challenge) = match c.msg() {
        ServerMsg::Enrolled { device, challenge } => (device, challenge),
        other => panic!("{other:?}"),
    };
    c.send(&ClientMsg::Auth { authorization: link::sign_answer(&key, &device, &challenge, now()).unwrap() });
    assert!(matches!(c.msg(), ServerMsg::Authed { .. }));
    r.book.0.lock().unwrap().insert(device.clone(), caps.iter().map(|s| s.to_string()).collect());
    c.send(&ClientMsg::SignIn { user: "ann".into(), password: "right".into() });
    assert_eq!(c.msg(), ServerMsg::SignedIn { person: "ann".into() });
    (c, device, key)
}

/// A returning device (second connection of an enrolled one).
fn reconnect(r: &Rig, device: &str, key: &DeviceKey) -> Client {
    let mut c = Client::open(r.srv.local_addr());
    let (ch, _) = c.hello(true);
    c.send(&ClientMsg::Auth { authorization: link::sign_answer(key, device, &ch, now()).unwrap() });
    assert!(matches!(c.msg(), ServerMsg::Authed { .. }));
    c.send(&ClientMsg::SignIn { user: "ann".into(), password: "right".into() });
    assert_eq!(c.msg(), ServerMsg::SignedIn { person: "ann".into() });
    c
}

fn wait_for(what: &str, f: impl Fn() -> bool) {
    let t0 = Instant::now();
    while !f() {
        assert!(t0.elapsed() < Duration::from_secs(60), "timed out: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn period(v: i16) -> Vec<i16> {
    vec![v; 1920]
}

// ------------------------------------------------------------------------------------------------------------------ negotiation

#[test]
fn the_media_capabilities_are_offered_only_when_the_instance_serves_them_and_only_agreed_ones_work() {
    let r = rig("caps", true);
    let mut c = Client::open(r.srv.local_addr());
    let (_, caps) = c.hello(true);
    for want in proto::cap::SERVER_MEDIA {
        assert!(caps.iter().any(|x| x == want), "{want}");
    }
    // an older client that did not offer them does not get them, and its audio_open is refused by name
    let r2 = rig("caps-old", true);
    let (mut old, _, _) = connect(&r2, "Old browser", 5, false, &["speaker.play"]);
    old.send(&ClientMsg::AudioOpen { side: None });
    assert_eq!(old.media_err(), ("audio".into(), "capability_missing".into()));
    // an instance with no media host: nothing offered, every request answers unsupported
    let r3 = rig("caps-none", false);
    let mut c3 = Client::open(r3.srv.local_addr());
    let (_, caps3) = c3.hello(true);
    assert_eq!(caps3, vec!["picture", "input", "events", "calls"]);
}

#[test]
fn without_a_media_host_every_media_request_says_unsupported() {
    let r = rig("nomedia", false);
    let (mut c, _, _) = connect(&r, "Laptop", 6, true, &[]);
    c.send(&ClientMsg::AudioOpen { side: None });
    assert_eq!(c.media_err(), ("audio".into(), "unsupported".into()));
    c.send(&ClientMsg::CameraOpen { unit: "usb:video0".into() });
    assert_eq!(c.media_err().1, "unsupported");
    c.send_bin(proto::STREAM_MIC_IN, &[1, 2]);
    assert_eq!(c.err_code(), ErrorCode::Unsupported, "the old behaviour: binary to the instance is not used");
}

impl Client {
    fn err_code(&mut self) -> ErrorCode {
        match self.msg() {
            ServerMsg::Error { code, .. } => code,
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn a_cloud_instance_has_no_instance_devices_so_a_camera_request_is_unsupported_and_only_client_devices_are_offered() {
    // the media host of a cloud pod has no camera feed: nothing about hardware that is not there is advertised or served
    let r = rig_without_feed(&rig("cloud", true));
    assert!(r.host.cameras.is_none());
    let (mut c, _, _) = connect(&r, "Maya's laptop", 30, true, &["camera.view", "client.camera.offer"]);
    c.send(&ClientMsg::CameraOpen { unit: "usb:video0".into() });
    assert_eq!(c.media_err(), ("camera".into(), "unsupported".into()));
    // the client's own devices still work (card says allow in this rig)
    c.send(&ClientMsg::OfferOpen { kind: "camera".into() });
    assert!(matches!(c.msg(), ServerMsg::OfferOk { .. }));
}

fn rig_without_feed(_like: &Rig) -> Rig {
    let r = rig("cloud-nofeed", true);
    // the same endpoint with a host that has no camera feed
    let data = r.data.clone();
    let log = r.log.clone();
    let consent = Arc::new(ClientConsent::new(Some(Arc::new(Card(r.card.clone())))));
    let host = Arc::new(MediaHost::new(consent));
    let broker = Arc::new(Broker::new(r.book.clone(), host.card(), log, Arc::new(|| iyakki_agent::clock::now_unix())));
    let ep = Arc::new(ClientEndpoint::with_limit(&data, broker, Arc::new(Calls), Arc::new(|_: &str, _: &str, _: &str| {}), FailLimit::new(50, Duration::from_secs(60))).with_media(host.clone()));
    let srv = Server::start(Config { addr: "127.0.0.1:0".into(), tls: None }, Arc::new(FakeGate { seen: r.seen.clone(), ep: ep.clone() })).unwrap();
    Rig { srv, ep, data, seen: r.seen.clone(), log: r.log.clone(), audit: r.audit.clone(), book: r.book.clone(), card: r.card.clone(), host, feed: r.feed.clone() }
}

// ------------------------------------------------------------------------------------------------------------------ sound

#[test]
fn sound_goes_to_the_client_in_20_ms_chunks_and_the_machines_speakers_stay_quiet_until_it_disconnects() {
    let r = rig("audio", true);
    let (mut c, _, _) = connect(&r, "Maya's laptop", 11, true, &["speaker.play"]);
    assert!(!r.host.audio.machine_muted());
    c.send(&ClientMsg::AudioOpen { side: None });
    assert_eq!(c.msg(), ServerMsg::AudioOk { rate: 48_000, channels: 2, chunk_ms: 20, side: "client".into() });
    assert!(r.host.audio.machine_muted(), "the default for a remote client is the client alone");
    // the mixer's output (the tee) offers periods; the session sends them as binary stream 2 with rising numbers
    for i in 0..3 {
        assert!(r.host.audio.tap(48_000, 2, &period(100 + i)));
        std::thread::sleep(Duration::from_millis(30));
    }
    let (_, bins) = c.drain(Duration::from_millis(400));
    let audio: Vec<_> = bins.iter().filter(|(s, _)| *s == proto::STREAM_AUDIO).collect();
    assert_eq!(audio.len(), 3);
    let decoded: Vec<(u32, Vec<i16>)> = audio.iter().map(|(_, p)| media::decode_audio(p, 2).unwrap()).collect();
    assert_eq!(decoded.iter().map(|d| d.0).collect::<Vec<_>>(), vec![0, 1, 2]);
    assert_eq!((decoded[0].1[0], decoded[2].1[1919]), (100, 102));
    // the sound stops (the mixer closed its output): the queue is flushed and the client is told
    r.host.audio.tap(48_000, 2, &period(9));
    r.host.audio.flush_all();
    let (msgs, bins) = c.drain(Duration::from_millis(300));
    assert!(msgs.contains(&ServerMsg::AudioFlush));
    assert!(bins.iter().all(|(s, _)| *s != proto::STREAM_AUDIO), "nothing queued plays on after the flush");
    // the client closes: speakers come back; opening again with both is explicit
    c.send(&ClientMsg::AudioClose);
    assert_eq!(c.msg(), ServerMsg::AudioClosed { reason: "closed".into() });
    assert!(!r.host.audio.machine_muted());
    c.send(&ClientMsg::AudioOpen { side: Some("both".into()) });
    assert!(matches!(c.msg(), ServerMsg::AudioOk { ref side, .. } if side == "both"));
    assert!(!r.host.audio.machine_muted(), "both: the machine plays too");
    c.send(&ClientMsg::AudioOpen { side: Some("client".into()) });
    assert!(matches!(c.msg(), ServerMsg::AudioOk { ref side, .. } if side == "client"));
    assert!(r.host.audio.machine_muted() && r.host.audio.sessions() == 1, "one route, one side");
    c.drain(Duration::from_millis(100)); // the flush that came with the change of side
    c.send(&ClientMsg::AudioOpen { side: Some("everywhere".into()) });
    assert_eq!(c.media_err().1, "bad_request");
    // the connection drops: the machine's speakers are restored without anyone asking
    drop(c);
    wait_for("speakers restored", || !r.host.audio.machine_muted() && r.host.audio.sessions() == 0);
    // the access log shows the device and the person; the audit has side and device but no sound
    let row = r.log.rows().into_iter().find(|e| e.capability == "speaker.play").unwrap();
    assert_eq!((row.device.as_str(), row.person.as_str(), row.allowed), ("Maya's laptop", "ann", true));
    assert!(r.audit.lock().unwrap().iter().any(|l| l.starts_with("ann|client.audio.open|client ")));
}

#[test]
fn a_client_that_was_not_given_sound_hears_nothing_and_the_machine_is_not_silenced() {
    let r = rig("audio-denied", true);
    let (mut c, _, _) = connect(&r, "Laptop", 12, true, &[]);
    c.send(&ClientMsg::AudioOpen { side: None });
    assert_eq!(c.media_err(), ("audio".into(), "not_granted".into()));
    assert!(!r.host.audio.machine_muted() && r.host.audio.sessions() == 0);
}

#[test]
fn one_sessions_route_does_not_silence_the_machine_for_another_persons_sound() {
    let r = rig("audio-person", true);
    r.host.audio.set_audible(|p| p == "bob"); // the machine's sound belongs to bob now
    let (mut c, _, _) = connect(&r, "Laptop", 13, true, &["speaker.play"]);
    c.send(&ClientMsg::AudioOpen { side: None });
    assert!(matches!(c.msg(), ServerMsg::AudioOk { .. }));
    assert!(!r.host.audio.machine_muted(), "ann's client does not mute, or hear, bob's sound");
    assert!(!r.host.audio.tap(48_000, 2, &period(1)));
}

// ------------------------------------------------------------------------------------------------------------------ clipboard

#[test]
fn the_clipboard_is_text_only_needs_a_press_each_time_and_is_capped() {
    let r = rig("clip", true);
    let (mut c, _, _) = connect(&r, "Laptop", 14, true, &["clipboard.client"]);
    // without the press of the button nothing happens
    c.send(&ClientMsg::ClipboardPut { text: "hello".into(), confirmed: false });
    assert_eq!(c.media_err(), ("clipboard".into(), "consent_required".into()));
    c.send(&ClientMsg::ClipboardGet { confirmed: false });
    assert_eq!(c.media_err().1, "consent_required");
    // a plain call cannot get around the button: the hard ask goes to the machine's card, which is not there to answer
    r.card.store(2, Ordering::SeqCst);
    c.send(&ClientMsg::Call { id: 9, capability: "clipboard.client".into(), unit: None });
    assert!(matches!(c.msg(), ServerMsg::CallErr { id: 9, ref code, .. } if code == "no_card"));
    // the press: the text goes to the person's screen as a paste
    c.send(&ClientMsg::ClipboardPut { text: "hello\u{0} world".into(), confirmed: true });
    assert_eq!(c.msg(), ServerMsg::ClipboardOk);
    wait_for("paste", || !r.seen.lock().unwrap().inputs.is_empty());
    let seen = r.seen.lock().unwrap().inputs.clone();
    assert!(seen[0].contains("Paste") && seen[0].contains("hello world"), "{seen:?}");
    // the size cap, both ways of getting it wrong
    c.send(&ClientMsg::ClipboardPut { text: "a".repeat(media::MAX_CLIPBOARD + 1), confirmed: true });
    // (a text message that long is itself refused by the 64 KB message limit)
    let (_, code) = match c.msg() {
        ServerMsg::MediaErr { what, code, .. } => (what, code),
        ServerMsg::Error { code, .. } => ("".into(), code.as_str().to_string()),
        other => panic!("{other:?}"),
    };
    assert!(code == "too_big", "{code}");
    // from the machine: what the person last copied there, only on request, only with the press
    c.send(&ClientMsg::ClipboardGet { confirmed: true });
    assert_eq!(c.media_err().1, "empty");
    r.seen.lock().unwrap().copy = Some("copied on the machine".into());
    std::thread::sleep(Duration::from_millis(150));
    c.send(&ClientMsg::ClipboardGet { confirmed: true });
    assert_eq!(c.msg(), ServerMsg::ClipboardText { text: "copied on the machine".into() });
    // the access log and the audit know it happened, and neither holds the text
    assert!(r.log.rows().iter().any(|e| e.capability == "clipboard.client" && e.allowed && e.device == "Laptop"));
    let audit = r.audit.lock().unwrap().join("\n");
    assert!(audit.contains("client.clipboard.put") && audit.contains("client.clipboard.get"));
    assert!(!audit.contains("hello world") && !audit.contains("copied on the machine"), "the audit holds no content");
    assert!(!format!("{:?}", r.log.rows()).contains("hello"), "nor does the access log");
}

#[test]
fn a_device_that_was_not_given_the_clipboard_cannot_use_it_even_with_a_press() {
    let r = rig("clip-denied", true);
    let (mut c, _, _) = connect(&r, "Laptop", 15, true, &["speaker.play"]);
    c.send(&ClientMsg::ClipboardPut { text: "x".into(), confirmed: true });
    assert_eq!(c.media_err(), ("clipboard".into(), "not_granted".into()));
    assert!(r.seen.lock().unwrap().inputs.is_empty(), "nothing reached the screen");
    // the unused press does not stay around to cover a later call
    r.book.0.lock().unwrap().values_mut().for_each(|v| v.push("clipboard.client".into()));
    r.card.store(2, Ordering::SeqCst);
    c.send(&ClientMsg::Call { id: 1, capability: "clipboard.client".into(), unit: None });
    assert!(matches!(c.msg(), ServerMsg::CallErr { ref code, .. } if code == "no_card"));
}

// ------------------------------------------------------------------------------------------------------------------ camera frames

#[test]
fn an_instance_camera_comes_as_jpeg_frames_at_five_a_second_at_most_and_only_after_the_grant() {
    let r = rig("camera", true);
    let (mut c, _, _) = connect(&r, "Maya's laptop", 16, true, &[]);
    c.send(&ClientMsg::CameraOpen { unit: "usb:video0".into() });
    assert_eq!(c.media_err(), ("camera".into(), "not_granted".into()));
    r.book.0.lock().unwrap().values_mut().for_each(|v| v.push("camera.view".into()));
    c.send(&ClientMsg::CameraOpen { unit: "usb:video0".into() });
    assert_eq!(c.msg(), ServerMsg::CameraOk { unit: "usb:video0".into(), fps: 5 });
    let (_, bins) = c.drain(Duration::from_millis(2000));
    let frames: Vec<_> = bins.iter().filter(|(s, _)| *s == proto::STREAM_CAMERA).collect();
    assert!((8..=11).contains(&frames.len()), "5 a second for 2 seconds, got {}", frames.len());
    let (unit, seq, jpeg) = media::decode_camera(&frames[0].1).unwrap();
    assert_eq!((unit.as_str(), seq), ("usb:video0", 1));
    assert!(media::looks_like_jpeg(jpeg));
    let seqs: Vec<u32> = frames.iter().map(|(_, p)| media::decode_camera(p).unwrap().1).collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1));
    // the lease: the camera is shared, so the machine's own view is not stopped; the access log names the device
    assert!(r.host.leases.holds("usb:video0", 1) || !r.host.leases.holders("usb:video0").is_empty());
    let row = r.log.rows().into_iter().find(|e| e.capability == "camera.view" && e.allowed).unwrap();
    assert_eq!((row.device.as_str(), row.person.as_str(), row.target.as_str()), ("Maya's laptop", "ann", "camera usb:video0"));
    // closing releases the lease
    c.send(&ClientMsg::CameraClose { unit: "usb:video0".into() });
    let (msgs, _) = c.drain(Duration::from_millis(300));
    assert!(msgs.contains(&ServerMsg::CameraClosed { unit: "usb:video0".into(), reason: "closed".into() }));
    assert!(r.host.leases.holders("usb:video0").is_empty());
}

#[test]
fn the_camera_list_has_names_for_the_cameras_the_person_may_view_and_needs_the_grant() {
    let r = rig("camlist", true);
    let (mut c, _, _) = connect(&r, "Maya's laptop", 17, true, &[]);
    c.send(&ClientMsg::CameraList);
    assert_eq!(c.media_err(), ("camera".into(), "not_granted".into()), "a device without camera.view gets no list");
    r.book.0.lock().unwrap().values_mut().for_each(|v| v.push("camera.view".into()));
    c.send(&ClientMsg::CameraList);
    match c.msg() {
        ServerMsg::CameraList { cameras } => assert_eq!(cameras, vec![proto::CameraEntry { unit: "usb:video0".into(), name: "Desk camera".into() }]),
        other => panic!("{other:?}"),
    }
    assert!(r.log.rows().iter().any(|e| e.capability == "camera.view" && e.allowed && e.person == "ann"), "the list is in the access log");
    // an instance without a camera feed (a cloud pod) says unsupported
    let cloud = rig_without_feed(&r);
    let (mut c2, _, _) = connect(&cloud, "Maya's laptop", 18, true, &[]);
    c2.send(&ClientMsg::CameraList);
    assert_eq!(c2.media_err(), ("camera".into(), "unsupported".into()));
}

#[test]
fn a_camera_another_app_holds_exclusively_answers_in_use_and_the_person_at_the_machine_can_take_it() {
    let r = rig("camera-busy", true);
    let (mut c, _, _) = connect(&r, "Maya's laptop", 17, true, &["camera.view"]);
    // the hub says another program has it
    *r.feed.state.lock().unwrap() = CamState::Busy("Recorder".into());
    c.send(&ClientMsg::CameraOpen { unit: "usb:video0".into() });
    assert_eq!(c.media_err(), ("camera".into(), "in_use".into()));
    *r.feed.state.lock().unwrap() = CamState::Ready;
    // a local app holds it exclusively through the lease table
    let app = local_session();
    assert!(matches!(r.host.leases.acquire("usb:video0", Lease { session: app, who: "Recorder".into(), exclusive: true, local: true }), Taken::Granted { .. }));
    c.send(&ClientMsg::CameraOpen { unit: "usb:video0".into() });
    match c.msg() {
        ServerMsg::MediaErr { code, message, .. } => assert!(code == "in_use" && message.contains("Recorder"), "{message}"),
        other => panic!("{other:?}"),
    }
    r.host.leases.release("usb:video0", app);
    // a camera that is not there
    c.send(&ClientMsg::CameraOpen { unit: "usb:nothing".into() });
    assert_eq!(c.media_err().1, "not_found");
    // it opens; then the person at the machine takes it exclusively and the client is told, with the holder's name
    c.send(&ClientMsg::CameraOpen { unit: "usb:video0".into() });
    assert!(matches!(c.msg(), ServerMsg::CameraOk { .. }));
    let here = local_session();
    assert!(matches!(r.host.leases.acquire("usb:video0", Lease { session: here, who: "the person at the machine".into(), exclusive: true, local: true }), Taken::Granted { ref preempted } if preempted.len() == 1));
    let (msgs, _) = c.drain(Duration::from_millis(600));
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::CameraClosed { reason, .. } if reason.contains("the person at the machine"))), "{msgs:?}");
}

#[test]
fn taking_the_grant_away_stops_the_frames_within_a_couple_of_seconds() {
    let r = rig("camera-revoke", true);
    let (mut c, device, _) = connect(&r, "Laptop", 18, true, &["camera.view"]);
    c.send(&ClientMsg::CameraOpen { unit: "usb:video0".into() });
    assert!(matches!(c.msg(), ServerMsg::CameraOk { .. }));
    r.book.0.lock().unwrap().insert(device, vec![]);
    let (msgs, _) = c.drain(Duration::from_millis(2500));
    assert!(msgs.iter().any(|m| matches!(m, ServerMsg::CameraClosed { reason, .. } if reason == "not_granted")), "{msgs:?}");
}

#[test]
fn a_disconnect_releases_the_camera_lease() {
    let r = rig("camera-drop", true);
    let (mut c, _, _) = connect(&r, "Laptop", 19, true, &["camera.view"]);
    c.send(&ClientMsg::CameraOpen { unit: "usb:video0".into() });
    assert!(matches!(c.msg(), ServerMsg::CameraOk { .. }));
    assert_eq!(r.host.leases.holders("usb:video0").len(), 1);
    drop(c);
    wait_for("lease released", || r.host.leases.holders("usb:video0").is_empty());
}

// ------------------------------------------------------------------------------------------------------------------ the client's own devices

#[test]
fn a_clients_camera_and_microphone_become_virtual_sources_named_after_the_device() {
    let r = rig("offer", true);
    let (mut c, _, _) = connect(&r, "Maya's laptop", 20, true, &["client.camera.offer", "client.microphone.offer"]);
    c.send(&ClientMsg::OfferOpen { kind: "camera".into() });
    assert_eq!(c.msg(), ServerMsg::OfferOk { kind: "camera".into(), source: "Maya's laptop camera".into() });
    c.send(&ClientMsg::OfferOpen { kind: "microphone".into() });
    assert_eq!(c.msg(), ServerMsg::OfferOk { kind: "microphone".into(), source: "Maya's laptop microphone".into() });
    // the tenant sees them; another tenant does not
    let names: Vec<String> = r.host.sources.list("tenant-of-ann").into_iter().map(|s| s.name).collect();
    assert_eq!(names, vec!["Maya's laptop camera", "Maya's laptop microphone"]);
    assert!(r.host.sources.list("tenant-of-bob").is_empty());
    // the indicator the shell's red band shows
    assert_eq!(r.host.sources.indicator().text().unwrap(), "Maya's laptop camera and Maya's laptop microphone are in use by this machine");
    // frames and sound are accepted and dropped (nothing consumes them yet)
    c.send_bin(proto::STREAM_CAMERA_IN, &[0xFF, 0xD8, 9, 9, 0xFF, 0xD9]);
    c.send_bin(proto::STREAM_MIC_IN, &[1, 0, 2, 0, 3, 0]);
    let s = r.host.sources.list("tenant-of-ann")[0].session;
    wait_for("frames counted", || r.host.sources.counts(s, media::SourceKind::Camera).is_some_and(|c| c.0 == 1) && r.host.sources.counts(s, media::SourceKind::Microphone).is_some_and(|c| c.0 == 1));
    assert_eq!(r.host.sources.counts(s, media::SourceKind::Camera), Some((1, 1)));
    assert_eq!(r.host.sources.counts(s, media::SourceKind::Microphone), Some((1, 1)));
    // the access log and the audit line name the device and the person
    let row = r.log.rows().into_iter().find(|e| e.capability == "client.camera.offer").unwrap();
    assert_eq!((row.device.as_str(), row.person.as_str(), row.allowed), ("Maya's laptop", "ann", true));
    assert!(r.audit.lock().unwrap().iter().any(|l| l.starts_with("ann|client.offer.camera|Maya's laptop camera")));
    // the client stops one: it goes; the other stays until the client disconnects, and then it vanishes with the indicator
    c.send(&ClientMsg::OfferClose { kind: "camera".into() });
    assert_eq!(c.msg(), ServerMsg::OfferClosed { kind: "camera".into(), reason: "closed".into() });
    assert_eq!(r.host.sources.indicator().text().unwrap(), "Maya's laptop microphone is in use by this machine");
    drop(c);
    wait_for("sources vanish", || r.host.sources.list("tenant-of-ann").is_empty() && !r.host.sources.indicator().active());
    assert!(r.audit.lock().unwrap().iter().any(|l| l.contains("client.offer.end") && l.contains("disconnected")));
}

#[test]
fn an_offer_is_a_hard_ask_on_the_machine_every_time() {
    let r = rig("offer-card", true);
    let (mut c, _, _) = connect(&r, "Laptop", 21, true, &["client.camera.offer", "client.microphone.offer"]);
    r.card.store(1, Ordering::SeqCst); // the person at the machine presses Deny
    c.send(&ClientMsg::OfferOpen { kind: "camera".into() });
    assert_eq!(c.media_err(), ("offer".into(), "card_declined".into()));
    r.card.store(2, Ordering::SeqCst); // nobody at the machine to ask
    c.send(&ClientMsg::OfferOpen { kind: "microphone".into() });
    assert_eq!(c.media_err(), ("offer".into(), "no_card".into()));
    assert!(r.host.sources.list("tenant-of-ann").is_empty(), "nothing was registered");
    // allowed once is allowed once: after the source is closed the card is asked again
    r.card.store(0, Ordering::SeqCst);
    c.send(&ClientMsg::OfferOpen { kind: "camera".into() });
    assert!(matches!(c.msg(), ServerMsg::OfferOk { .. }));
    c.send(&ClientMsg::OfferClose { kind: "camera".into() });
    assert!(matches!(c.msg(), ServerMsg::OfferClosed { .. }));
    r.card.store(1, Ordering::SeqCst);
    c.send(&ClientMsg::OfferOpen { kind: "camera".into() });
    assert_eq!(c.media_err().1, "card_declined");
    // a client the person did not give the offer to cannot even be asked about
    let (mut d, _, _) = connect(&r, "Other", 22, true, &[]);
    d.send(&ClientMsg::OfferOpen { kind: "camera".into() });
    assert_eq!(d.media_err().1, "not_granted");
    d.send(&ClientMsg::OfferOpen { kind: "screen".into() });
    assert_eq!(d.media_err().1, "bad_request");
}

#[test]
fn two_connections_of_one_device_cannot_both_offer_its_camera() {
    let r = rig("offer-two", true);
    let (mut a, device, key) = connect(&r, "Laptop", 23, true, &["client.camera.offer"]);
    a.send(&ClientMsg::OfferOpen { kind: "camera".into() });
    assert!(matches!(a.msg(), ServerMsg::OfferOk { .. }));
    let mut b = reconnect(&r, &device, &key);
    b.send(&ClientMsg::OfferOpen { kind: "camera".into() });
    assert_eq!(b.media_err(), ("offer".into(), "in_use".into()));
    drop(a);
    wait_for("first gone", || r.host.sources.list("tenant-of-ann").is_empty());
    b.send(&ClientMsg::OfferOpen { kind: "camera".into() });
    assert!(matches!(b.msg(), ServerMsg::OfferOk { .. }), "free once the first connection is gone");
}

#[test]
fn frames_without_an_offer_or_malformed_ones_are_refused_once_and_never_kept() {
    let r = rig("offer-frames", true);
    let (mut c, _, _) = connect(&r, "Laptop", 24, true, &["client.microphone.offer"]);
    c.send_bin(proto::STREAM_MIC_IN, &[1, 0]);
    assert_eq!(c.media_err(), ("offer".into(), "not_open".into()));
    c.send_bin(proto::STREAM_MIC_IN, &[1, 0]);
    c.send(&ClientMsg::OfferOpen { kind: "microphone".into() });
    assert!(matches!(c.msg(), ServerMsg::OfferOk { .. }), "the second stray frame was not answered again");
    c.send_bin(proto::STREAM_MIC_IN, &[1, 0, 3]);
    assert_eq!(c.media_err().1, "bad_data");
    c.send_bin(9, &[1, 2, 3]);
    assert_eq!(c.err_code(), ErrorCode::Unsupported);
    // a revoke still closes the session and everything it offered
    let id = link::list_devices(&r.data)[0].id.clone();
    r.ep.revoke(&id, "ann").unwrap();
    assert!(c.closed_within(Duration::from_secs(3)));
    wait_for("vanished", || r.host.sources.list("tenant-of-ann").is_empty());
}


// ------------------------------------------------------------------------------------------------------------------ file transfer (F-CLIENT-01)

impl Client {
    /// The next file-transfer message (other messages are skipped; binary chunks are collected into `chunks`).
    fn file(&mut self, chunks: &mut Vec<(u32, Vec<u8>)>) -> serde_json::Value {
        loop {
            match self.next(Duration::from_secs(8)) {
                Some(Got::File(v)) => return v,
                Some(Got::Bin(b)) if b.first() == Some(&proto::STREAM_FILE_OUT) => {
                    let (id, bytes) = proto::split_file_chunk(&b[1..]).unwrap();
                    chunks.push((id, bytes.to_vec()));
                }
                Some(Got::Close) => panic!("closed"),
                Some(_) => {}
                None => panic!("no file message"),
            }
        }
    }

    fn put_chunk(&mut self, id: u32, bytes: &[u8]) {
        let mut p = id.to_be_bytes().to_vec();
        p.extend_from_slice(bytes);
        self.send_bin(proto::STREAM_FILE_IN, &p);
    }
}

fn kind(v: &serde_json::Value) -> &str {
    v["t"].as_str().unwrap_or("")
}

#[test]
fn files_are_listed_once_asked_per_session_and_a_download_is_a_card_then_chunks_in_order_then_done() {
    let r = rig("files-get", true);
    let body: Vec<u8> = (0..(proto::FILE_CHUNK * 2 + 100)).map(|i| (i % 251) as u8).collect();
    r.seen.lock().unwrap().files.insert("photo.bin".into(), body.clone());
    let (mut c, _, _) = connect(&r, "Maya's phone", 40, true, &["files.client"]);
    let mut chunks = Vec::new();
    c.send(&ClientMsg::FileList { dir: String::new() });
    let l = c.file(&mut chunks);
    assert_eq!((kind(&l), l["entries"][0]["name"].as_str(), l["entries"][0]["size"].as_u64()), ("file_list", Some("photo.bin"), Some(body.len() as u64)));
    // the card was answered yes for browsing; a second listing does not ask again, even with the card set to no
    r.card.store(1, Ordering::SeqCst);
    c.send(&ClientMsg::FileList { dir: String::new() });
    assert_eq!(kind(&c.file(&mut chunks)), "file_list");
    // a file asks every time: the card says no, nothing is read from the home
    c.send(&ClientMsg::FileGet { id: 1, path: "photo.bin".into() });
    let e = c.file(&mut chunks);
    assert_eq!((kind(&e), e["id"].as_u64(), e["code"].as_str()), ("file_err", Some(1), Some("card_declined")), "{e}");
    assert_eq!(r.seen.lock().unwrap().reads, 0, "a refused file is never read");
    // the card says yes: the size, then the chunks of at most 32 KB with the id, then done
    r.card.store(0, Ordering::SeqCst);
    c.send(&ClientMsg::FileGet { id: 2, path: "photo.bin".into() });
    let ok = c.file(&mut chunks);
    assert_eq!((kind(&ok), ok["size"].as_u64()), ("file_get_ok", Some(body.len() as u64)));
    let done = c.file(&mut chunks);
    assert_eq!((kind(&done), done["id"].as_u64()), ("file_done", Some(2)));
    assert!(chunks.iter().all(|(id, b)| *id == 2 && b.len() <= proto::FILE_CHUNK) && chunks.len() == 3);
    assert_eq!(chunks.into_iter().flat_map(|(_, b)| b).collect::<Vec<u8>>(), body);
    let a = r.audit.lock().unwrap().join("\n");
    assert!(a.contains("ann|client.file.get|") && !a.contains("photo.bin"), "audited by size, never by the file's name: {a}");
    // a file that is not there is a sentence, not a crash
    c.send(&ClientMsg::FileGet { id: 3, path: "nope.bin".into() });
    let e = c.file(&mut Vec::new());
    assert_eq!((kind(&e), e["code"].as_str()), ("file_err", Some("failed")));
}

#[test]
fn an_upload_asks_on_the_machine_then_takes_exactly_the_announced_bytes_and_saves_them_through_the_screen() {
    let r = rig("files-put", true);
    let (mut c, _, _) = connect(&r, "Maya's phone", 41, true, &["files.client"]);
    let mut none = Vec::new();
    let body: Vec<u8> = (0..70_000).map(|i| (i % 7) as u8).collect();
    // the card says no: nothing is accepted
    r.card.store(1, Ordering::SeqCst);
    c.send(&ClientMsg::FilePut { id: 1, path: "in/up.bin".into(), size: body.len() as u64 });
    assert_eq!(c.file(&mut none)["code"].as_str(), Some("card_declined"));
    r.card.store(0, Ordering::SeqCst);
    c.send(&ClientMsg::FilePut { id: 2, path: "up.bin".into(), size: body.len() as u64 });
    assert_eq!(kind(&c.file(&mut none)), "file_put_ok");
    for part in body.chunks(proto::FILE_CHUNK) {
        c.put_chunk(2, part);
    }
    c.send(&ClientMsg::FileEnd { id: 2 });
    let saved = c.file(&mut none);
    assert_eq!((kind(&saved), saved["path"].as_str()), ("file_saved", Some("up.bin")));
    assert_eq!(r.seen.lock().unwrap().files.get("up.bin"), Some(&body));
    // fewer bytes than announced, more bytes than announced, a size over the limit and a name outside the home are refused
    c.send(&ClientMsg::FilePut { id: 3, path: "short.bin".into(), size: 10 });
    assert_eq!(kind(&c.file(&mut none)), "file_put_ok");
    c.put_chunk(3, &[1, 2, 3]);
    c.send(&ClientMsg::FileEnd { id: 3 });
    assert_eq!(c.file(&mut none)["code"].as_str(), Some("bad_request"));
    assert!(r.seen.lock().unwrap().files.get("short.bin").is_none());
    c.send(&ClientMsg::FilePut { id: 4, path: "over.bin".into(), size: 2 });
    assert_eq!(kind(&c.file(&mut none)), "file_put_ok");
    c.put_chunk(4, &[1, 2, 3]);
    assert_eq!(c.file(&mut none)["code"].as_str(), Some("bad_request"), "more than announced throws the upload away");
    c.send(&ClientMsg::FilePut { id: 5, path: "big.bin".into(), size: proto::MAX_FILE as u64 + 1 });
    assert_eq!(c.file(&mut none)["code"].as_str(), Some("too_big"));
    for bad in ["../x", "/etc/passwd", ""] {
        c.send(&ClientMsg::FilePut { id: 6, path: bad.into(), size: 1 });
        assert_eq!(c.file(&mut none)["code"].as_str(), Some("bad_request"), "{bad}");
    }
    // a chunk with no upload open is refused, and a cancel ends one
    c.put_chunk(9, &[1]);
    assert_eq!(c.file(&mut none)["code"].as_str(), Some("bad_request"));
    c.send(&ClientMsg::FilePut { id: 7, path: "c.bin".into(), size: 3 });
    assert_eq!(kind(&c.file(&mut none)), "file_put_ok");
    c.send(&ClientMsg::FileCancel { id: 7 });
    c.send(&ClientMsg::FileEnd { id: 7 });
    assert_eq!(c.file(&mut none)["code"].as_str(), Some("bad_request"), "a cancelled upload is gone");
}

#[test]
fn a_device_that_was_not_given_files_or_that_did_not_offer_them_gets_nothing_and_a_machine_without_media_says_unsupported() {
    let r = rig("files-denied", true);
    r.seen.lock().unwrap().files.insert("a.txt".into(), b"secret".to_vec());
    let (mut c, _, _) = connect(&r, "Phone", 42, true, &["speaker.play"]);
    let mut none = Vec::new();
    c.send(&ClientMsg::FileList { dir: String::new() });
    assert_eq!(c.file(&mut none)["code"].as_str(), Some("not_granted"), "an on switch is needed before the card is even asked");
    c.send(&ClientMsg::FileGet { id: 1, path: "a.txt".into() });
    assert_eq!(c.file(&mut none)["code"].as_str(), Some("not_granted"));
    assert_eq!(r.seen.lock().unwrap().reads, 0);
    // a client that did not offer the capability in hello
    let (mut old, _, _) = connect(&r, "Old", 43, false, &["files.client"]);
    old.send(&ClientMsg::FileList { dir: String::new() });
    assert_eq!(old.file(&mut none)["code"].as_str(), Some("capability_missing"));
    // an instance with no media half serves no files
    let r2 = rig("files-nomedia", false);
    let (mut c2, _, _) = connect(&r2, "Phone", 44, true, &[]);
    c2.send(&ClientMsg::FileList { dir: String::new() });
    assert_eq!(c2.file(&mut none)["code"].as_str(), Some("unsupported"));
}

#[test]
fn file_messages_are_not_printed_with_their_names_and_the_old_wire_is_unchanged() {
    let shown = format!("{:?} {:?}", ClientMsg::FileGet { id: 1, path: "tax/2025/return.pdf".into() }, ClientMsg::FileList { dir: "private".into() });
    assert!(!shown.contains("return") && !shown.contains("private"), "{shown}");
    assert!(proto::cap::SERVER_FILES.contains(&"files.transfer") && !proto::cap::SERVER.contains(&"files.transfer") && !proto::cap::SERVER_MEDIA.contains(&"files.transfer"));
}
