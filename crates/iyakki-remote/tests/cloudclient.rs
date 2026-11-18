//! The desktop connector's client (`iyakki_agent::cloudclient`) against the real web server over real TLS on the loopback: the
//! certificate probe, pinned sign-in, pictures, input, resize, reconnecting after a cut link, and closing.
//!
//! The screen behind the server is a fake (a solid picture that changes colour on a key and follows `size`), so these tests prove
//! the client and the server's protocol and TLS code, not the real shell's drawing (`tests/shell.rs` covers that for the browser).

use iyakki_agent::cloudclient::{probe, Client, Input as Cin, Pin, Status, Target};
use iyakki_remote::input::Input;
use iyakki_remote::screen::{Gate, LoginError, Pixels, Screen};
use iyakki_remote::server::{Config, Server};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Pic {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    ver: u64,
    delivered: u64,
    inputs: Vec<String>,
    finish: bool,
}

type Shared = Arc<Mutex<Pic>>;

fn solid(w: u32, h: u32, c: [u8; 3]) -> Vec<u8> {
    (0..w * h).flat_map(|_| [c[0], c[1], c[2], 255]).collect()
}

struct Fake {
    pic: Shared,
    armed: bool,
}

impl Screen for Fake {
    fn user(&self) -> String {
        "alice".into()
    }
    fn input(&mut self, i: Input) {
        let mut p = self.pic.lock().unwrap();
        p.inputs.push(format!("{i:?}"));
        match i {
            Input::Size(w, h) => {
                p.w = w;
                p.h = h;
                p.rgba = solid(w, h, [10, 20, 30]);
                p.ver += 1;
            }
            Input::Event(_) => {
                p.rgba = solid(p.w, p.h, [200, 0, 0]);
                p.ver += 1;
            }
            _ => {}
        }
    }
    fn viewing(&mut self, on: bool) {
        if on {
            self.pic.lock().unwrap().delivered = 0; // whoever looks gets the whole picture
        }
    }
    fn step(&mut self) -> Option<iyakki_remote::screen::Frame> {
        std::thread::sleep(Duration::from_millis(3));
        None
    }
    fn take_copy(&mut self) -> Option<String> {
        None
    }
    fn set_stream(&mut self, _: bool) {}
    fn arm(&mut self) {
        self.armed = true;
    }
    fn take_pixels(&mut self) -> Option<Pixels> {
        let mut p = self.pic.lock().unwrap();
        if self.armed && p.ver != p.delivered {
            self.armed = false;
            p.delivered = p.ver;
            return Some(Pixels { w: p.w, h: p.h, rgba: p.rgba.clone() });
        }
        None
    }
    fn finished(&self) -> bool {
        self.pic.lock().unwrap().finish
    }
}

struct FakeGate {
    pic: Shared,
    logins: Arc<AtomicUsize>,
}

impl Gate for FakeGate {
    fn login(&self, user: &str, password: &str) -> Result<Box<dyn Screen>, LoginError> {
        self.logins.fetch_add(1, Ordering::SeqCst);
        if user == "alice" && password == "right" {
            Ok(Box::new(Fake { pic: self.pic.clone(), armed: false }))
        } else {
            Err(LoginError::Invalid)
        }
    }
}

struct Rig {
    server: Server,
    pic: Shared,
    logins: Arc<AtomicUsize>,
    pin: Pin,
    dir: std::path::PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rig(tag: &str) -> Rig {
    let dir = std::env::temp_dir().join(format!("iyakki-cloudclient-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let id = iyakki_remote::tls::identity(&dir, &["acme.example".into()]).expect("a certificate");
    let pin = Pin::parse(&id.fingerprint).expect("the fingerprint is a pin");
    let pic = Arc::new(Mutex::new(Pic { w: 640, h: 400, rgba: solid(640, 400, [10, 20, 30]), ver: 1, delivered: 0, inputs: vec![], finish: false }));
    let logins = Arc::new(AtomicUsize::new(0));
    let server = Server::start(Config { addr: "127.0.0.1:0".into(), tls: Some(id.config) }, Arc::new(FakeGate { pic: pic.clone(), logins: logins.clone() })).unwrap();
    Rig { server, pic, logins, pin, dir }
}

fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

fn target(r: &Rig) -> Target {
    Target { addr: r.server.local_addr().to_string(), pin: r.pin }
}

fn connect(r: &Rig, size: (u32, u32)) -> Client {
    Client::connect(&target(r), "alice", "right", size).expect("connects")
}

fn live_with_picture(c: &Client) {
    wait_for("a live picture", 10, || c.status() == Status::Live && c.picture().is_some());
}

#[test]
fn the_probe_returns_the_servers_certificate_without_signing_in() {
    let r = rig("probe");
    let p = probe(&r.server.local_addr().to_string(), Duration::from_secs(5)).expect("probed");
    assert_eq!(p.pin, r.pin, "the pin is the SHA-256 of the certificate the server uses");
    assert_eq!(p.subject, "acme.example", "the first name of the certificate");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    assert!(p.not_after > now, "a certificate that is still valid: {}", p.not_after);
    assert_eq!(r.logins.load(Ordering::SeqCst), 0, "nothing secret was sent");
}

#[test]
fn the_right_pin_and_password_go_live_with_a_picture_of_the_requested_size() {
    let r = rig("live");
    let c = connect(&r, (1000, 600));
    live_with_picture(&c);
    let pic = c.picture().unwrap();
    // the size message sent at the start reaches the (fake) screen, which then draws at that size
    wait_for("the picture at the window size", 10, || c.picture().is_some_and(|p| (p.w, p.h) == (1000, 600)));
    let pic2 = c.picture().unwrap();
    assert_eq!(pic2.rgba.len(), 1000 * 600 * 4);
    assert_eq!(&pic2.rgba[..4], &[10, 20, 30, 255], "the pixels are the server's");
    assert!(pic2.rgba.chunks(4).all(|p| p == [10, 20, 30, 255]));
    assert!(pic2.generation >= pic.generation && pic2.generation > 0 && c.generation() >= pic2.generation);
    assert_eq!(r.logins.load(Ordering::SeqCst), 1);
    assert_eq!(r.server.sessions(), 1);
    c.close();
}

#[test]
fn a_wrong_password_is_refused_for_good() {
    let r = rig("refused");
    let c = Client::connect(&target(&r), "alice", "wrong", (800, 600)).expect("a definite no is a client in the Refused state");
    assert!(matches!(c.status(), Status::Refused(m) if m.contains("wrong")), "{:?}", c.status());
    assert!(c.picture().is_none());
    assert!(c.finished());
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(r.logins.load(Ordering::SeqCst), 1, "no retry with a refused password");
    c.send(Cin::Shift(true)); // harmless
}

#[test]
fn a_wrong_pin_is_closed_and_the_password_never_reaches_the_server() {
    let r = rig("pin");
    let mut bad = r.pin;
    bad.0[0] ^= 0xff;
    let c = Client::connect(&Target { addr: r.server.local_addr().to_string(), pin: bad }, "alice", "right", (800, 600)).expect("a pin mismatch is a closed client");
    match c.status() {
        Status::Closed(m) => assert!(m.contains("not the one") && m.contains(&r.pin.display()), "{m}"),
        s => panic!("{s:?}"),
    }
    assert!(c.finished());
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(r.logins.load(Ordering::SeqCst), 0, "the server never saw a sign-in");
    assert_eq!(r.server.sessions(), 0);
}

#[test]
fn a_key_typed_through_send_reaches_the_screen_and_changes_the_picture() {
    let r = rig("key");
    let c = connect(&r, (640, 400));
    live_with_picture(&c);
    let before = c.generation();
    c.send(Cin::Key { name: "a".into(), ctrl: false, shift: false, alt: false });
    wait_for("the key at the screen", 10, || r.pic.lock().unwrap().inputs.iter().any(|i| i.contains("Key(Char('a'))")));
    wait_for("the changed picture", 10, || c.picture().is_some_and(|p| p.generation > before && p.rgba[..4] == [200, 0, 0, 255]));
    c.send(Cin::MouseDown { x: 10, y: 20, button: 0 });
    c.send(Cin::MouseDown { x: 11, y: 21, button: 2 });
    c.send(Cin::Text("é".into()));
    wait_for("mouse and text at the screen", 10, || {
        let p = r.pic.lock().unwrap();
        ["MouseDown(10, 20)", "MouseRight(11, 21)", "Char('é')"].iter().all(|w| p.inputs.iter().any(|i| i.contains(w)))
    });
    c.close();
}

#[test]
fn resizing_the_window_resizes_the_picture() {
    let r = rig("size");
    let c = connect(&r, (800, 500));
    wait_for("the first size", 10, || c.picture().is_some_and(|p| (p.w, p.h) == (800, 500)));
    c.send(Cin::Size { w: 1024, h: 768 });
    wait_for("the new size", 10, || c.picture().is_some_and(|p| (p.w, p.h) == (1024, 768) && p.rgba.len() == 1024 * 768 * 4));
    c.close();
}

/// A relay on the loopback that can cut every link through it at once (the server has no such switch).
struct Relay {
    addr: String,
    links: Arc<Mutex<Vec<TcpStream>>>,
    stop: Arc<AtomicBool>,
    accepted: Arc<AtomicUsize>,
}

impl Relay {
    fn start(to: std::net::SocketAddr) -> Relay {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let (links, stop, accepted) = (Arc::new(Mutex::new(Vec::<TcpStream>::new())), Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        let (lk, st, ac) = (links.clone(), stop.clone(), accepted.clone());
        std::thread::spawn(move || {
            while !st.load(Ordering::Relaxed) {
                match l.accept() {
                    Ok((a, _)) => {
                        let _ = a.set_nonblocking(false);
                        let Ok(b) = TcpStream::connect(to) else { continue };
                        ac.fetch_add(1, Ordering::SeqCst);
                        lk.lock().unwrap().push(a.try_clone().unwrap());
                        lk.lock().unwrap().push(b.try_clone().unwrap());
                        for (mut from, mut into) in [(a.try_clone().unwrap(), b.try_clone().unwrap()), (b, a)] {
                            std::thread::spawn(move || {
                                let mut buf = [0u8; 16384];
                                while let Ok(n) = from.read(&mut buf) {
                                    if n == 0 || into.write_all(&buf[..n]).is_err() {
                                        break;
                                    }
                                }
                                let _ = into.shutdown(Shutdown::Both);
                                let _ = from.shutdown(Shutdown::Both);
                            });
                        }
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        Relay { addr, links, stop, accepted }
    }

    fn cut(&self) {
        for s in self.links.lock().unwrap().drain(..) {
            let _ = s.shutdown(Shutdown::Both);
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.cut();
    }
}

#[test]
fn a_cut_link_is_reconnected_with_the_same_session_and_a_full_picture() {
    let r = rig("reconnect");
    let relay = Relay::start(r.server.local_addr());
    let c = Client::connect(&Target { addr: relay.addr.clone(), pin: r.pin }, "alice", "right", (800, 500)).expect("connects through the relay");
    wait_for("the first picture", 10, || c.picture().is_some_and(|p| (p.w, p.h) == (800, 500)));
    let before = c.generation();
    let fulls = |r: &Rig| r.pic.lock().unwrap().inputs.iter().filter(|i| i.contains("Full")).count();
    assert_eq!(fulls(&r), 0);
    relay.cut();
    wait_for("Reconnecting", 10, || c.status() == Status::Reconnecting || c.generation() > before);
    // back: Live again, a new picture (the server sends the whole screen to a new connection), and a full picture was asked for
    wait_for("Live and a new picture again", 20, || c.status() == Status::Live && c.generation() > before);
    wait_for("the full-picture request", 10, || fulls(&r) >= 1);
    assert!(relay.accepted.load(Ordering::SeqCst) >= 3, "sign-in, first socket, then a new socket");
    assert_eq!(r.logins.load(Ordering::SeqCst), 1, "the server still knew the session, so no second sign-in");
    // and the new link works for input
    c.send(Cin::Key { name: "b".into(), ctrl: false, shift: false, alt: false });
    wait_for("input over the new link", 10, || r.pic.lock().unwrap().inputs.iter().any(|i| i.contains("Char('b')")));
    c.close();
}

#[test]
fn close_signs_out_and_ends_the_thread_and_so_does_drop() {
    let r = rig("close");
    let c = connect(&r, (640, 400));
    live_with_picture(&c);
    assert_eq!(r.server.sessions(), 1);
    c.close();
    assert!(matches!(c.status(), Status::Closed(_)));
    wait_for("the client thread to end", 10, || c.finished());
    wait_for("the server session to end (signed out)", 10, || r.server.sessions() == 0);
    c.send(Cin::Shift(true)); // after close: ignored, no panic
    // dropping without close also signs out
    let d = connect(&r, (640, 400));
    live_with_picture(&d);
    assert_eq!(r.server.sessions(), 1);
    drop(d);
    wait_for("the server session to end after a drop", 10, || r.server.sessions() == 0);
}

#[test]
fn the_person_signing_out_inside_the_desktop_closes_the_client_without_retrying() {
    let r = rig("bye");
    let c = connect(&r, (640, 400));
    live_with_picture(&c);
    r.pic.lock().unwrap().finish = true;
    wait_for("Closed", 10, || matches!(c.status(), Status::Closed(_)));
    match c.status() {
        Status::Closed(m) => assert!(m.contains("signed out"), "{m}"),
        s => panic!("{s:?}"),
    }
    wait_for("the thread to end", 10, || c.finished());
    assert_eq!(r.logins.load(Ordering::SeqCst), 1);
}
