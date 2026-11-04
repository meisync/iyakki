//! Tenant sessions in the shell (docs/development/CLOUD-MULTITENANT-PLAN.md, sections 3.3 and 3.4): the model, without any drawing.
//!
//! A session is one signed-in tenant desktop, seen through the [`RemoteDesktop`] trait. The real one is [`ClientDesktop`], an adapter over
//! the desktop connector's client (`iyakki_agent::cloudclient`); tests use a fake. The [`Hub`] holds one session per tenant, keeps them all
//! alive in the background, and knows which one is shown (`None` is Personal). Input goes only to the shown one. The shell's drawing and
//! event wiring are in `shell_tenant.rs`.

use crate::ui::{Key, Nav};
use iyakki_agent::cloudclient::{self as cc, Input, Picture, Presented, Status, Target};
use std::sync::Arc;

/// A tenant's desktop as the shell sees it.
pub trait RemoteDesktop: Send {
    fn status(&self) -> Status;
    /// Changes whenever the picture does.
    fn generation(&self) -> u64;
    fn picture(&self) -> Option<Picture>;
    fn send(&self, i: Input);
    /// The size of the latest picture.
    fn size(&self) -> Option<(u32, u32)>;
    /// Sign out of the tenant and end the connection.
    fn close(&self);
}

/// The real thing: the connector's client.
pub struct ClientDesktop {
    c: cc::Client,
    size: std::sync::Mutex<Option<(u32, u32)>>,
}

impl ClientDesktop {
    pub fn new(c: cc::Client) -> ClientDesktop {
        ClientDesktop { c, size: std::sync::Mutex::new(None) }
    }
}

impl RemoteDesktop for ClientDesktop {
    fn status(&self) -> Status {
        self.c.status()
    }
    fn generation(&self) -> u64 {
        self.c.generation()
    }
    fn picture(&self) -> Option<Picture> {
        let p = self.c.picture();
        if let (Some(p), Ok(mut s)) = (&p, self.size.lock()) {
            *s = Some((p.w, p.h));
        }
        p
    }
    fn send(&self, i: Input) {
        self.c.send(i)
    }
    fn size(&self) -> Option<(u32, u32)> {
        self.size.lock().ok().and_then(|s| *s)
    }
    fn close(&self) {
        self.c.close()
    }
}

/// What the shell asks of the network: look at a server's certificate, and sign in. Tests replace it.
pub trait Connector: Send + Sync {
    fn probe(&self, addr: &str) -> Result<Presented, String>;
    fn connect(&self, t: &Target, user: &str, password: &str, size: (u32, u32)) -> Result<Box<dyn RemoteDesktop>, String>;
}

pub struct RealConnector;

impl Connector for RealConnector {
    fn probe(&self, addr: &str) -> Result<Presented, String> {
        cc::probe(addr, std::time::Duration::from_secs(6))
    }
    fn connect(&self, t: &Target, user: &str, password: &str, size: (u32, u32)) -> Result<Box<dyn RemoteDesktop>, String> {
        cc::Client::connect(t, user, password, size).map(|c| Box::new(ClientDesktop::new(c)) as Box<dyn RemoteDesktop>)
    }
}

pub fn real_connector() -> Arc<dyn Connector> {
    Arc::new(RealConnector)
}

/// The dot on a chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dot {
    Live,
    Reconnecting,
    Closed,
}

pub fn dot_of(s: &Status) -> Dot {
    match s {
        Status::Live => Dot::Live,
        Status::Connecting | Status::Reconnecting => Dot::Reconnecting,
        Status::Refused(_) | Status::Closed(_) => Dot::Closed,
    }
}

pub fn dot_word(d: Dot) -> &'static str {
    match d {
        Dot::Live => "live",
        Dot::Reconnecting => "reconnecting",
        Dot::Closed => "closed",
    }
}

/// One open tenant.
pub struct Session {
    pub name: String,
    pub account: String,
    desktop: Box<dyn RemoteDesktop>,
    /// The generation drawn last, to know whether a new frame is due.
    pub drawn: u64,
    /// The size last told to the tenant.
    pub told: Option<(u32, u32)>,
    pub pending_size: Option<((u32, u32), u64)>,
}

impl Session {
    pub fn new(name: &str, account: &str, desktop: Box<dyn RemoteDesktop>) -> Session {
        Session { name: name.into(), account: account.into(), desktop, drawn: u64::MAX, told: None, pending_size: None }
    }
    pub fn status(&self) -> Status {
        self.desktop.status()
    }
    pub fn dot(&self) -> Dot {
        dot_of(&self.desktop.status())
    }
    pub fn generation(&self) -> u64 {
        self.desktop.generation()
    }
    pub fn picture(&self) -> Option<Picture> {
        self.desktop.picture()
    }
    pub fn size(&self) -> Option<(u32, u32)> {
        self.desktop.size()
    }
    pub fn send(&self, i: Input) {
        self.desktop.send(i)
    }
}

/// All open tenants and which one is shown.
#[derive(Default)]
pub struct Hub {
    pub sessions: Vec<Session>,
    /// Index into `sessions`; `None` is Personal.
    pub active: Option<usize>,
}

impl Hub {
    /// Add a session and show it. A tenant already open is closed first (one live session per tenant).
    pub fn open(&mut self, s: Session) {
        if let Some(i) = self.sessions.iter().position(|x| x.name == s.name) {
            self.close(i);
        }
        self.sessions.push(s);
        self.active = Some(self.sessions.len() - 1);
    }

    pub fn is_open(&self, name: &str) -> bool {
        self.sessions.iter().any(|s| s.name == name)
    }

    /// Close one tenant (signing out of it); Personal is shown if it was the shown one.
    pub fn close(&mut self, i: usize) {
        if i >= self.sessions.len() {
            return;
        }
        self.sessions.remove(i).desktop.close();
        self.active = match self.active {
            Some(a) if a == i => None,
            Some(a) if a > i => Some(a - 1),
            other => other,
        };
    }

    pub fn close_all(&mut self) {
        for s in self.sessions.drain(..) {
            s.desktop.close();
        }
        self.active = None;
    }

    /// Show Personal (`None`) or tenant `i`.
    pub fn show(&mut self, i: Option<usize>) {
        self.active = i.filter(|i| *i < self.sessions.len());
    }

    /// The next view in the order Personal, tenant 1, tenant 2, ... and back to Personal.
    pub fn cycle(&mut self) {
        self.active = match self.active {
            None if !self.sessions.is_empty() => Some(0),
            Some(a) if a + 1 < self.sessions.len() => Some(a + 1),
            _ => None,
        };
    }

    pub fn current(&self) -> Option<&Session> {
        self.active.and_then(|a| self.sessions.get(a))
    }

    pub fn current_mut(&mut self) -> Option<&mut Session> {
        self.active.and_then(|a| self.sessions.get_mut(a))
    }

    /// Input for the shown tenant, and only for it. False when Personal is shown (nothing is sent).
    pub fn send(&self, i: Input) -> bool {
        match self.current() {
            Some(s) => {
                s.send(i);
                true
            }
            None => false,
        }
    }
}

/// The shell's key as the connector's input: printable characters as text, everything else as a named key.
pub fn key_input(k: &Key) -> Option<Input> {
    let named = |name: &str, ctrl: bool, shift: bool| Some(Input::Key { name: name.into(), ctrl, shift, alt: false });
    match k {
        Key::Char(c) if c.is_control() => None,
        Key::Char(c) => Some(Input::Text(c.to_string())),
        Key::Ctrl(c) => named(&c.to_ascii_lowercase().to_string(), true, c.is_ascii_uppercase()),
        Key::Enter => named("Enter", false, false),
        Key::Backspace => named("Backspace", false, false),
        Key::Delete => named("Delete", false, false),
        Key::Tab => named("Tab", false, false),
        Key::BackTab => named("Tab", false, true),
        Key::Esc => named("Escape", false, false),
        Key::Up => named("ArrowUp", false, false),
        Key::Down => named("ArrowDown", false, false),
        Key::Left => named("ArrowLeft", false, false),
        Key::Right => named("ArrowRight", false, false),
        Key::Home => named("Home", false, false),
        Key::End => named("End", false, false),
        Key::PageUp => named("PageUp", false, false),
        Key::PageDown => named("PageDown", false, false),
        Key::F(n) => named(&format!("F{n}"), false, false),
        Key::Nav { nav, shift, ctrl } => {
            let name = match nav {
                Nav::Left => "ArrowLeft",
                Nav::Right => "ArrowRight",
                Nav::Up => "ArrowUp",
                Nav::Down => "ArrowDown",
                Nav::Home => "Home",
                Nav::End => "End",
                Nav::PageUp => "PageUp",
                Nav::PageDown => "PageDown",
            };
            named(name, *ctrl, *shift)
        }
    }
}

/// Where a picture of `pic` pixels is drawn inside `area` (x, y, w, h): scaled to fit, aspect kept, centred. Never larger than the area.
pub fn fit(area: (f32, f32, f32, f32), pic: (u32, u32)) -> (f32, f32, f32, f32) {
    let (ax, ay, aw, ah) = area;
    if pic.0 == 0 || pic.1 == 0 || aw <= 0.0 || ah <= 0.0 {
        return (ax, ay, 0.0, 0.0);
    }
    let k = (aw / pic.0 as f32).min(ah / pic.1 as f32);
    let (w, h) = ((pic.0 as f32 * k).floor().max(1.0).min(aw), (pic.1 as f32 * k).floor().max(1.0).min(ah));
    (ax + ((aw - w) / 2.0).floor(), ay + ((ah - h) / 2.0).floor(), w, h)
}

/// A point on screen to a point of the picture, when it is inside the drawn picture.
pub fn to_picture(dest: (f32, f32, f32, f32), pic: (u32, u32), x: i32, y: i32) -> Option<(i32, i32)> {
    let (dx, dy, dw, dh) = dest;
    if dw <= 0.0 || dh <= 0.0 || (x as f32) < dx || (y as f32) < dy || (x as f32) >= dx + dw || (y as f32) >= dy + dh {
        return None;
    }
    let px = (((x as f32 - dx) * pic.0 as f32 / dw) as i32).clamp(0, pic.0 as i32 - 1);
    let py = (((y as f32 - dy) * pic.1 as f32 / dh) as i32).clamp(0, pic.1 as i32 - 1);
    Some((px, py))
}

#[cfg(test)]
pub use fake::*;

#[cfg(test)]
mod fake {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeState {
        pub status: Option<Status>,
        pub gen: u64,
        pub pic: Option<Picture>,
        pub sent: Vec<Input>,
        pub closed: bool,
    }

    /// A tenant's desktop for tests: its state is shared so a test can look at what was sent and change the picture.
    #[derive(Clone)]
    pub struct FakeDesktop(pub Arc<Mutex<FakeState>>);

    impl FakeDesktop {
        pub fn live(w: u32, h: u32, rgb: [u8; 3]) -> FakeDesktop {
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for _ in 0..w * h {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            FakeDesktop(Arc::new(Mutex::new(FakeState { status: Some(Status::Live), gen: 1, pic: Some(Picture { w, h, rgba, generation: 1 }), ..Default::default() })))
        }
        pub fn sent(&self) -> Vec<Input> {
            self.0.lock().unwrap().sent.clone()
        }
        pub fn set_status(&self, s: Status) {
            self.0.lock().unwrap().status = Some(s);
        }
        pub fn is_closed(&self) -> bool {
            self.0.lock().unwrap().closed
        }
    }

    impl RemoteDesktop for FakeDesktop {
        fn status(&self) -> Status {
            self.0.lock().unwrap().status.clone().unwrap_or(Status::Connecting)
        }
        fn generation(&self) -> u64 {
            self.0.lock().unwrap().gen
        }
        fn picture(&self) -> Option<Picture> {
            self.0.lock().unwrap().pic.clone()
        }
        fn send(&self, i: Input) {
            self.0.lock().unwrap().sent.push(i);
        }
        fn size(&self) -> Option<(u32, u32)> {
            self.0.lock().unwrap().pic.as_ref().map(|p| (p.w, p.h))
        }
        fn close(&self) {
            let mut s = self.0.lock().unwrap();
            s.closed = true;
            s.status = Some(Status::Closed("closed".into()));
        }
    }

    /// The network for tests: a certificate it presents per address, and a result per sign-in.
    #[derive(Default)]
    pub struct FakeNet {
        pub certs: Mutex<Vec<(String, Presented)>>,
        pub accounts: Mutex<Vec<(String, String)>>,
        pub made: Mutex<Vec<(String, FakeDesktop)>>,
        pub down: Mutex<bool>,
    }

    impl FakeNet {
        pub fn serve(&self, addr: &str, pin: u8) {
            let mut c = self.certs.lock().unwrap();
            c.retain(|(a, _)| a != addr);
            c.push((addr.into(), Presented { pin: cc::Pin([pin; 32]), subject: format!("{addr} subject"), not_after: 1_900_000_000 }));
        }
        pub fn desktop_of(&self, addr: &str) -> Option<FakeDesktop> {
            self.made.lock().unwrap().iter().rev().find(|(a, _)| a == addr).map(|(_, d)| d.clone())
        }
    }

    impl Connector for FakeNet {
        fn probe(&self, addr: &str) -> Result<Presented, String> {
            if *self.down.lock().unwrap() {
                return Err("could not reach the server".into());
            }
            self.certs.lock().unwrap().iter().find(|(a, _)| a == addr).map(|(_, p)| p.clone()).ok_or_else(|| "could not reach the server".to_string())
        }
        fn connect(&self, t: &Target, user: &str, password: &str, _size: (u32, u32)) -> Result<Box<dyn RemoteDesktop>, String> {
            let shown = self.probe(&t.addr)?;
            if shown.pin != t.pin {
                return Err("the server's certificate is not the one that was trusted".into());
            }
            if !self.accounts.lock().unwrap().iter().any(|(u, p)| u == user && p == password) {
                return Err("The account or password was refused".into());
            }
            let d = FakeDesktop::live(64, 40, [20, 120, 60]);
            self.made.lock().unwrap().push((t.addr.clone(), d.clone()));
            Ok(Box::new(d))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(name: &str) -> (Session, FakeDesktop) {
        let d = FakeDesktop::live(10, 10, [1, 2, 3]);
        (Session::new(name, "alice", Box::new(d.clone())), d)
    }

    #[test]
    fn the_hub_keeps_one_session_per_tenant_and_sends_only_to_the_shown_one() {
        let mut hub = Hub::default();
        let ((a, da), (b, db)) = (sess("A"), sess("B"));
        hub.open(a);
        hub.open(b);
        assert_eq!(hub.active, Some(1));
        assert!(hub.send(Input::Text("x".into())));
        assert!(da.sent().is_empty() && db.sent().len() == 1);
        hub.show(None);
        assert!(!hub.send(Input::Text("y".into())), "Personal: nothing is sent");
        assert_eq!(db.sent().len(), 1);
        // the same tenant again replaces the old session and closes it
        let (a2, da2) = sess("A");
        hub.open(a2);
        assert!(da.is_closed() && !da2.is_closed());
        assert_eq!(hub.sessions.len(), 2);
        // closing the first keeps the shown one shown
        hub.show(Some(1));
        hub.close(0);
        assert_eq!(hub.active, Some(0));
        assert_eq!(hub.sessions[0].name, "A");
        hub.close_all();
        assert!(db.is_closed() && da2.is_closed() && hub.sessions.is_empty() && hub.active.is_none());
    }

    #[test]
    fn keys_become_text_or_named_keys() {
        assert_eq!(key_input(&Key::Char('a')), Some(Input::Text("a".into())));
        assert_eq!(key_input(&Key::Char('\u{7}')), None);
        assert_eq!(key_input(&Key::Ctrl('V')), Some(Input::Key { name: "v".into(), ctrl: true, shift: true, alt: false }));
        assert_eq!(key_input(&Key::BackTab), Some(Input::Key { name: "Tab".into(), ctrl: false, shift: true, alt: false }));
        assert_eq!(key_input(&Key::F(5)), Some(Input::Key { name: "F5".into(), ctrl: false, shift: false, alt: false }));
        assert_eq!(key_input(&Key::Nav { nav: Nav::Left, shift: true, ctrl: true }), Some(Input::Key { name: "ArrowLeft".into(), ctrl: true, shift: true, alt: false }));
    }

    #[test]
    fn the_picture_is_fitted_with_its_aspect_ratio_and_points_map_back() {
        let d = fit((0.0, 40.0, 1000.0, 500.0), (640, 480));
        assert!(d.2 <= 1000.0 && d.3 <= 500.0);
        assert!((d.2 / d.3 - 640.0 / 480.0).abs() < 0.02);
        assert!(d.0 > 0.0, "centred");
        assert_eq!(to_picture(d, (640, 480), d.0 as i32, d.1 as i32), Some((0, 0)));
        assert_eq!(to_picture(d, (640, 480), d.0 as i32 - 1, d.1 as i32), None);
        assert_eq!(fit((0.0, 0.0, 0.0, 0.0), (10, 10)).2, 0.0);
    }
}
