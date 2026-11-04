//! The real desktop connector (`iyakki_agent::cloudclient`, behind the shell's default `RealConnector`) with the real shell, together:
//! the shell's sign-in screen in its Cloud mode is driven with the events a person's mouse and keyboard make, against real HTTPS servers
//! on the loopback (the web server of this crate, real TLS, real certificates). Before this file the shell side was tested against a fake
//! network and the client against a fake screen, never together.
//!
//! What is real: the shell (`iyakki_shell::shell::Shell`, drawn into real frames), the shell's connector, the client, the pixel-stream
//! protocol, TLS, the certificates and their fingerprints, the remembered-tenants file on disk. What is a stand-in: the screen behind each
//! server is a solid picture that turns red on a key (the same stand-in as `tests/cloudclient.rs`); the tenant's own desktop is not
//! the thing under test here, `tests/shell.rs` covers the real shell behind the web server.

use iyakki_agent::cloudclient::Pin;
use iyakki_agent::cloudtenants::{self as ct, Tenants};
use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_agent::HostApi;
use iyakki_remote::input::Input;
use iyakki_remote::screen::{Gate, LoginError, Pixels, Screen};
use iyakki_remote::server::{Config, Server};
use iyakki_sec::policy::Policy;
use iyakki_shell::accounts::{AuditLog, UserStore};
use iyakki_shell::gfx::Gfx;
use iyakki_shell::shell::{Shell, ShellEnv};
use iyakki_shell::ui::{Event, Key};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------------------------------- the tenant's server

struct Pic {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    ver: u64,
    delivered: u64,
    inputs: Vec<String>,
    base: [u8; 3],
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
                p.rgba = solid(w, h, p.base);
                p.ver += 1;
            }
            Input::Event(_) => {
                p.rgba = solid(p.w, p.h, RED);
                p.ver += 1;
            }
            _ => {}
        }
    }
    fn viewing(&mut self, on: bool) {
        if on {
            self.pic.lock().unwrap().delivered = 0;
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
        false
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

const RED: [u8; 3] = [200, 0, 0];
const ACME: [u8; 3] = [10, 120, 30];
const BETA: [u8; 3] = [20, 40, 170];

struct Tenant {
    server: Option<Server>,
    pic: Shared,
    logins: Arc<AtomicUsize>,
    pin: Pin,
    addr: String,
    dir: std::path::PathBuf,
}

impl Tenant {
    /// A server on a free port (or on `addr` again, with a certificate of its own) showing a picture of colour `base`.
    fn start(root: &std::path::Path, name: &str, base: [u8; 3], addr: &str) -> Tenant {
        let dir = root.join(format!("tls-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id = iyakki_remote::tls::identity(&dir, &[format!("{name}.example")]).expect("a certificate");
        let pin = Pin::parse(&id.fingerprint).expect("the fingerprint is a pin");
        let pic = Arc::new(Mutex::new(Pic { w: 640, h: 400, rgba: solid(640, 400, base), ver: 1, delivered: 0, inputs: vec![], base }));
        let logins = Arc::new(AtomicUsize::new(0));
        let server = Server::start(Config { addr: addr.into(), tls: Some(id.config) }, Arc::new(FakeGate { pic: pic.clone(), logins: logins.clone() })).expect("the server starts");
        let addr = server.local_addr().to_string();
        Tenant { server: Some(server), pic, logins, pin, addr, dir }
    }

    fn stop(&mut self) {
        self.server = None;
    }

    fn inputs(&self) -> usize {
        self.pic.lock().unwrap().inputs.len()
    }
}

impl Drop for Tenant {
    fn drop(&mut self) {
        self.server = None;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------------------------------------------------------------------------------------------------- the local shell

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

fn shell_in(d: &std::path::Path) -> Shell {
    std::fs::create_dir_all(d).unwrap();
    let mut store = UserStore::open(&d.join("users.json")).unwrap();
    store.create_first_admin("root", "rootpass12").unwrap();
    let up: Arc<dyn Upstream> = Arc::new(Offline);
    Shell::new(ShellEnv {
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
    })
}

// ---------------------------------------------------------------------------------------------------- the person's hands

struct Person {
    sh: Shell,
    g: Gfx,
}

impl Person {
    fn new(sh: Shell, w: u32, h: u32) -> Person {
        Person { sh, g: Gfx::new(w, h) }
    }

    fn render(&mut self) {
        self.sh.render(&mut self.g);
    }

    fn labels(&mut self) -> Vec<String> {
        self.render();
        self.sh.hits().iter().map(|h| h.label.clone()).collect()
    }

    /// Click the control with this label (drawn in the last frame), with the mouse events a pointer makes.
    fn click(&mut self, label: &str) {
        self.render();
        let all: Vec<String> = self.sh.hits().iter().map(|h| h.label.clone()).collect();
        let r = self.sh.hits().iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("nothing clickable is labelled `{label}`; on screen: {all:?}")).rect;
        let (x, y) = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
        self.sh.handle(&Event::MouseMove(x, y));
        self.sh.handle(&Event::MouseDown(x, y));
        self.sh.handle(&Event::MouseUp(x, y));
    }

    fn typ(&mut self, s: &str) {
        for c in s.chars() {
            self.sh.handle(&Event::Key(Key::Char(c)));
        }
    }

    fn key(&mut self, k: Key) {
        self.sh.handle(&Event::Key(k));
    }

    /// Beat the shell's heart (and draw) until `ok` holds: the connector works on threads.
    fn until(&mut self, what: &str, ok: impl Fn(&Shell) -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(20) {
            self.sh.handle(&Event::Tick);
            self.render();
            if ok(&self.sh) {
                return;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        panic!("timed out waiting for: {what} (dialog: {}, error: {:?}, open: {:?})", self.sh.tenant_dialog(), self.sh.tenant_dialog_error(), self.sh.open_tenants());
    }

    /// How many pixels of the last frame have this colour.
    fn pixels(&mut self, c: [u8; 3]) -> usize {
        self.render();
        self.g.pm.data().chunks_exact(4).filter(|p| p[..3] == c).count()
    }

    fn until_picture(&mut self, what: &str, c: [u8; 3]) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(20) {
            self.sh.handle(&Event::Tick);
            if self.pixels(c) > 20_000 {
                return;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        panic!("timed out waiting for the picture of {what}");
    }

    /// Add a tenant through the dialogs up to (and including) the trust decision; stops at the sign-in dialog.
    fn add_tenant(&mut self, addr: &str, name: &str, pin: Pin) {
        self.click("cloud");
        self.click("Add a tenant");
        assert_eq!(self.sh.tenant_dialog(), "add");
        self.render();
        self.typ(addr);
        self.key(Key::Tab);
        self.typ(name);
        self.key(Key::Enter);
        self.until("the trust dialog (a real TLS probe of the server)", |s| s.tenant_dialog() == "trust");
        assert_eq!(self.sh.tenant_dialog_fingerprints(), vec![pin.display()], "the fingerprint shown is the SHA-256 of the certificate the server really presented");
        assert!(self.sh.remembered_tenants().iter().all(|(n, _)| n != name), "nothing is remembered before the person accepts");
        self.click("Trust this certificate");
        assert_eq!(self.sh.tenant_dialog(), "signin");
    }

    fn sign_in_to_tenant(&mut self, name: &str, password: &str) {
        self.typ("alice");
        self.key(Key::Tab);
        self.typ(password);
        self.key(Key::Enter);
        self.until(&format!("{name} to open"), |s| s.open_tenants().iter().any(|n| n == name));
    }
}

fn root(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-cloudshell-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// ---------------------------------------------------------------------------------------------------- the tests

#[test]
fn trust_on_first_use_then_connected_then_switch_and_input_reach_only_the_shown_tenant() {
    let dir = root("tofu");
    let acme = Tenant::start(&dir, "acme", ACME, "127.0.0.1:0");
    let beta = Tenant::start(&dir, "beta", BETA, "127.0.0.1:0");
    let mut p = Person::new(shell_in(&dir.join("shell")), 1280, 720);
    p.render();
    assert_eq!(p.sh.tenant_dialog(), "closed");

    // add Acme: the real probe shows the real fingerprint; nothing was sent to the login route of the server
    p.add_tenant(&acme.addr, "Acme", acme.pin);
    assert_eq!(acme.logins.load(Ordering::SeqCst), 0, "no account or password reached the server before the certificate was trusted");
    let saved = Tenants::load(&ct::path_in(&dir.join("shell").join("svc")));
    assert_eq!(saved.get("Acme").unwrap().pin, acme.pin, "the pin on disk is the one the person trusted");

    // wrong password first: refused, nothing opens, the dialog stays
    p.typ("alice");
    p.key(Key::Tab);
    p.typ("wrong");
    p.key(Key::Enter);
    p.until("the refusal", |s| s.tenant_dialog_error().is_some());
    assert!(p.sh.open_tenants().is_empty());
    assert_eq!(acme.logins.load(Ordering::SeqCst), 1);

    // the right one: connected, and the server's picture is on the shell's screen
    p.typ("right"); // the wrong password was cleared and the cursor is back in the password field
    p.key(Key::Enter);
    p.until("Acme to open", |s| s.open_tenants() == vec!["Acme".to_string()]);
    assert_eq!(p.sh.shown_tenant().as_deref(), Some("Acme"));
    p.until_picture("Acme", ACME);

    // a key typed on the shell reaches Acme's screen and turns its picture red, in the shell's frame
    let before = acme.inputs();
    p.key(Key::Char('k'));
    p.until_picture("Acme after the key", RED);
    assert!(acme.inputs() > before);
    assert_eq!(p.pixels(ACME), 0, "the old picture is gone");

    // add Beta while Acme is open: both stay open; Beta is shown
    p.add_tenant(&beta.addr, "Beta", beta.pin);
    p.sign_in_to_tenant("Beta", "right");
    assert_eq!(p.sh.open_tenants(), vec!["Acme".to_string(), "Beta".to_string()]);
    assert_eq!(p.sh.shown_tenant().as_deref(), Some("Beta"));
    p.until_picture("Beta", BETA);
    assert_eq!(p.pixels(RED), 0, "Acme's picture is not drawn while Beta is shown");

    // input goes to the shown tenant only
    let (a0, b0) = (acme.inputs(), beta.inputs());
    p.key(Key::Char('z'));
    p.until_picture("Beta after the key", RED);
    assert_eq!(acme.inputs(), a0, "Acme got nothing while Beta was shown");
    assert!(beta.inputs() > b0);

    // switch back with the chip: Acme's picture (still red) returns, Beta's is no longer drawn
    p.click("tenant Acme (live)");
    assert_eq!(p.sh.shown_tenant().as_deref(), Some("Acme"));
    p.until_picture("Acme again", RED);
    assert_eq!(p.pixels(BETA), 0);
    p.click("this machine");
    assert_eq!(p.sh.shown_tenant(), None);
    assert_eq!(p.pixels(RED), 0, "Personal shows no pixel of a tenant");

    // closing signs out of the tenant's server connection and leaves the other one
    p.click("tenant Beta (live)");
    p.click("close tenant");
    assert_eq!(p.sh.open_tenants(), vec!["Acme".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn after_sign_in_the_iyakki_menu_lists_the_open_tenants_and_switches_by_keyboard_and_mouse() {
    let dir = root("menu");
    let acme = Tenant::start(&dir, "acme", ACME, "127.0.0.1:0");
    let mut p = Person::new(shell_in(&dir.join("shell")), 1280, 960);
    p.render();
    p.add_tenant(&acme.addr, "Acme", acme.pin);
    p.sign_in_to_tenant("Acme", "right");
    p.click("this machine");
    // sign in on this machine; the tenant stays open in the background
    p.render();
    p.typ("root");
    p.key(Key::Tab);
    p.typ("rootpass12");
    p.key(Key::Enter);
    p.until("the machine sign-in", |s| s.session().is_some());
    assert_eq!(p.sh.open_tenants(), vec!["Acme".to_string()]);
    p.sh.handle(&Event::Tick);
    // Personal after sign-in: nothing at the bottom edge covers the launcher
    let labels = p.labels();
    assert!(!labels.iter().any(|l| l.starts_with("tenant ")), "{labels:?}");
    // the menu entry, by mouse
    p.click("IYAKKI menu");
    p.click("Cloud");
    assert_eq!(p.sh.tenant_dialog(), "pick");
    let labels = p.labels();
    assert!(labels.iter().any(|l| l == "switch to Acme (live)"), "{labels:?}");
    p.click("switch to Acme (live)");
    assert_eq!(p.sh.shown_tenant().as_deref(), Some("Acme"));
    p.until_picture("Acme", ACME);
    // the menu still opens over a shown tenant, by keyboard: F10, c, Esc
    let n = acme.inputs();
    p.key(Key::F(10));
    p.key(Key::Char('c'));
    assert_eq!(p.sh.tenant_dialog(), "pick");
    p.key(Key::Esc);
    assert_eq!(p.sh.tenant_dialog(), "closed");
    assert_eq!(acme.inputs(), n, "the menu keys went to the shell, not to the tenant");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_changed_fingerprint_is_a_hard_stop_and_no_password_reaches_the_server() {
    let dir = root("changed");
    let mut acme = Tenant::start(&dir, "acme", ACME, "127.0.0.1:0");
    let mut p = Person::new(shell_in(&dir.join("shell")), 1280, 720);
    p.render();
    p.add_tenant(&acme.addr, "Acme", acme.pin);
    p.sign_in_to_tenant("Acme", "right");
    p.click("close tenant");
    assert!(p.sh.open_tenants().is_empty());
    let (addr, old_pin) = (acme.addr.clone(), acme.pin);
    // the operator (or someone in the middle) puts another certificate on the same address
    acme.stop();
    let mut other = None;
    for _ in 0..50 {
        let r = std::panic::catch_unwind(|| ());
        let _ = r;
        match std::net::TcpListener::bind(&addr) {
            Ok(l) => {
                drop(l);
                other = Some(Tenant::start(&dir, "mallory", ACME, &addr));
                break;
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    let other = other.expect("the address is free again");
    assert_ne!(other.pin, old_pin);
    p.click("cloud");
    p.click("tenant row Acme");
    p.until("the warning", |s| s.tenant_dialog() == "changed");
    assert_eq!(p.sh.tenant_dialog_fingerprints(), vec![old_pin.display(), other.pin.display()], "the old and the new fingerprint are both shown");
    let labels = p.labels();
    assert!(!labels.iter().any(|l| l == "Trust this certificate" || l == "Sign in"), "nothing offers to accept or to ask for a password: {labels:?}");
    // keys do not get past the warning
    p.key(Key::Char('y'));
    p.key(Key::Enter);
    assert_ne!(p.sh.tenant_dialog(), "signin");
    assert_eq!(other.logins.load(Ordering::SeqCst), 0, "no login request reached the impostor");
    assert_eq!(Tenants::load(&ct::path_in(&dir.join("shell").join("svc"))).get("Acme").unwrap().pin, old_pin, "the trusted pin is not replaced");
    assert!(p.sh.open_tenants().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
