//! Sign in to Claude Code and GitHub Copilot from the screen: the Login button, the "signed in as" chip and the sign-in dialog.
//!
//! The dialog runs the vendor's own login command in a pseudo-terminal (`iyakki_agent::extagent::login`) and shows what it prints:
//! the address (large, copyable, and as a QR code to scan with a phone), a one-time device code (large, copyable), a field for the
//! code the browser shows, a live status line and Cancel. The vendor program keeps the credentials in the person's home folder;
//! this file never reads or stores a token and writes only "login started / finished: ok, failed or cancelled" to the audit.
//!
//! The buttons are drawn by any screen that lists the two agents ([`buttons`]); their clicks are `Event::Tab` numbers the shell
//! answers itself ([`LOGIN_TAB`], [`LOGOUT_TAB`]), so every screen gets the same dialog.

use iyakki_agent::extagent::login::{self, LoginSession, Method, Phase, Snapshot};
use iyakki_agent::extagent::relay;
use iyakki_agent::extagent::{self, Auth};
use iyakki_agent::qr;

use crate::agent_login_scan::{CamSource, Scan, Step};
use crate::apps::AppCx;
use crate::fx;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(255, 184, 77);
const AMBER: [u8; 4] = rgb(255, 196, 94);

/// The agents that can be signed in from here, in the order of the tab numbers.
pub const AGENTS: [&str; 2] = [extagent::CLAUDE_CODE, extagent::COPILOT];
/// `Event::Tab(LOGIN_TAB + n)` asks the shell to sign in to `AGENTS[n]`; `LOGOUT_TAB + n` to sign out.
pub const LOGIN_TAB: usize = 1_400_000;
pub const LOGOUT_TAB: usize = 1_400_010;
pub(crate) const T_START: usize = 1_400_100;
pub(crate) const T_PICK: usize = 1_400_110; // + method index
pub(crate) const T_CANCEL: usize = 1_400_120;
pub(crate) const T_COPY_URL: usize = 1_400_121;
pub(crate) const T_COPY_CODE: usize = 1_400_122;
pub(crate) const T_SEND: usize = 1_400_123;
pub(crate) const T_AGAIN: usize = 1_400_124;
pub(crate) const T_OPEN: usize = 1_400_125;
pub(crate) const T_PASTE: usize = 1_400_126;
pub(crate) const T_PASTE_OFFER: usize = 1_400_127;
pub(crate) const T_SEND_OFFER: usize = 1_400_128;
pub(crate) const T_BIGGER: usize = 1_400_129;
pub(crate) const T_SCAN: usize = 1_400_130;
pub(crate) const T_SCAN_ALLOW: usize = 1_400_131;
pub(crate) const T_SCAN_DENY: usize = 1_400_132;
pub(crate) const T_SCAN_STOP: usize = 1_400_133;
pub(crate) const T_QR_NEXT: usize = 1_400_134;

/// The tool of the CLI list (`claude-code`, `github-copilot`) as an agent id.
pub fn agent_of_cli(cli: &str) -> Option<&'static str> {
    match cli {
        "claude-code" => Some(extagent::CLAUDE_CODE),
        "github-copilot" => Some(extagent::COPILOT),
        _ => None,
    }
}

pub fn name_of(agent: &str) -> &'static str {
    if agent == extagent::CLAUDE_CODE {
        "Claude Code"
    } else {
        "GitHub Copilot"
    }
}

/// What a tab number from [`LOGIN_TAB`] / [`LOGOUT_TAB`] means: (agent id, is a logout).
pub fn agent_of_tab(n: usize) -> Option<(&'static str, bool)> {
    (0..AGENTS.len()).find_map(|i| (n == LOGIN_TAB + i).then_some((AGENTS[i], false)).or((n == LOGOUT_TAB + i).then_some((AGENTS[i], true))))
}

// ------------------------------------------------------------------------------------------------ the chip

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    NotInstalled,
    /// The status has not been looked up yet (a lookup was started).
    Checking,
    SignedOut,
    SignedIn(String),
    /// Installed, and the program has no way to say (or an API key from the vault is used).
    Unknown(String),
}

impl State {
    pub fn installed(&self) -> bool {
        !matches!(self, State::NotInstalled | State::Checking)
    }
}

/// The remembered sign-in state of an agent (a lookup runs on its own thread; the screens never wait for it). `installed` is the
/// CLI list's own record, so the button shows as soon as the tool is installed, before the first lookup is done.
pub fn state(cx: &AppCx, agent: &str, installed: bool) -> State {
    let svc = &cx.services;
    match svc.ext_status.get(agent) {
        None => {
            if !svc.ext_status.busy() {
                svc.ext_status.refresh(svc.agents_cfg());
            }
            if installed {
                State::Unknown("checking…".into())
            } else {
                State::Checking
            }
        }
        Some(st) if !st.installed && !installed => State::NotInstalled,
        Some(st) => match st.auth {
            Auth::SignedIn(who) => State::SignedIn(who),
            Auth::SignedOut => State::SignedOut,
            Auth::Key => State::Unknown("uses an API key from the vault".into()),
            Auth::Unknown => State::Unknown("sign-in cannot be checked from here".into()),
        },
    }
}

/// The words and colour of the chip.
pub fn chip(st: &State) -> (String, [u8; 4]) {
    match st {
        State::NotInstalled => ("not installed".into(), DIM),
        State::Checking => ("checking…".into(), DIM),
        State::SignedOut => ("not signed in".into(), ERR),
        State::SignedIn(who) => (format!("signed in as {who}"), OK),
        State::Unknown(why) => (why.clone(), MUTED),
    }
}

/// The Login button, and for Claude Code a Logout button when signed in. Returns the width used. Draw it only for an installed tool.
pub fn buttons(g: &mut Gfx, x: f32, y: f32, size: f32, agent: &str, st: &State) -> f32 {
    let Some(i) = AGENTS.iter().position(|a| *a == agent) else { return 0.0 };
    let mut w = g.button(x, y, size, if matches!(st, State::SignedIn(_)) { "Login again" } else { "Login" }, OK, Event::Tab(LOGIN_TAB + i)) + size * 0.7;
    if matches!(st, State::SignedIn(_)) {
        w += g.button(x + w, y, size, "Logout", ERR, Event::Tab(LOGOUT_TAB + i)) + size * 0.7;
    }
    w
}

// ------------------------------------------------------------------------------------------------ the QR size

static QR_BIG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The big-QR choice, kept for the session (not on disk): the dialog is made new every time it opens.
pub fn qr_big() -> bool {
    QR_BIG.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_qr_big(on: bool) {
    QR_BIG.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Screens wider than this many pixels get a somewhat larger QR code without being asked.
pub(crate) const WIDE_SCREEN: f32 = 2000.0;

/// How big the QR codes are drawn: `side` is the square the first one (the sign-in address) gets, `relay` the square of the
/// second one beside it (0 when it is not drawn beside: big view, or no relay).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct QrLayout {
    pub side: f32,
    pub relay: f32,
    pub big: bool,
}

/// The dialog's QR sizes. Normal: 250 (170 each when the phone relay's code is beside it) times the scale, and a third more on a
/// wide screen. Big: up to nearly the dialog's height (`panel_h`, room kept for the title, the caption and the buttons under it) and
/// never so wide that the text beside it has less than 350 units; never smaller than twice the normal size when the dialog is tall
/// enough for that (a screen at least 624 units high).
pub(crate) fn qr_layout(s: f32, screen_w: f32, panel_h: f32, panel_w: f32, two: bool, big: bool) -> QrLayout {
    let base = if two { 170.0 } else { 250.0 } * s * if screen_w > WIDE_SCREEN { 1.3 } else { 1.0 };
    if !big {
        return QrLayout { side: base, relay: if two { base } else { 0.0 }, big: false };
    }
    let normal = 250.0 * s;
    let side = (panel_h - 124.0 * s).min(panel_w - 56.0 * s - 350.0 * s).max(normal);
    QrLayout { side, relay: 0.0, big: true }
}

/// Whole-pixel size of one module when a symbol of `modules` x `modules` plus the 4-module quiet zone on every side is fitted in
/// `side`, and the side of the white square that really gets drawn. Whole pixels keep every module sharp at any scale.
pub(crate) fn qr_metrics(side: f32, modules: usize) -> (f32, f32) {
    let n = (modules + 8) as f32;
    let m = (side / n).floor().max(1.0);
    (m, m * n)
}

// ------------------------------------------------------------------------------------------------ the dialog

enum Stage {
    /// Claude Code: how to sign in.
    Choose,
    Running,
}

/// The second step as shown: where the phone can send the code, and its QR code.
struct RelayView {
    url: String,
    qr: Option<qr::Qr>,
}

pub struct LoginModal {
    agent: &'static str,
    stage: Stage,
    method: usize,
    sess: Option<LoginSession>,
    snap: Option<Snapshot>,
    field: TextField,
    qr: Option<(String, Option<qr::Qr>)>,
    /// The phone relay (step 2), when this device can serve it.
    relay: Option<RelayView>,
    relay_tried: bool,
    /// What the relay or the paste last had to say about a code (shown under the field).
    said: Option<(String, bool)>,
    /// The browser tile is on screen with the sign-in address: the dialog is out of the way, the program keeps running.
    hidden: bool,
    /// The program ended while the dialog was out of the way.
    ended_away: bool,
    /// A code-like text on the clipboard (its length), offered with one tap after the browser closes.
    clip_offer: Option<usize>,
    /// What was done with the token after the login (moved into the vault, or why not), shown under "Signed in".
    after: Option<(String, bool)>,
    /// A home folder to sign in to instead of the person's own (tests).
    home: Option<std::path::PathBuf>,
    started: bool,
    finished_audited: bool,
    /// Reading the code from a QR code on the person's phone with the camera (agent_login_scan.rs).
    scan: Scan,
    /// With the second QR (phone relay) up and the big view on: show the relay's code in the big slot.
    show_relay: bool,
    /// The width of the screen the dialog was last drawn on (the QR default depends on it).
    screen_w: f32,
    /// The white square of the first QR code as last drawn (tests).
    last_qr: Option<R>,
}

pub enum Out {
    Stay,
    Close,
    /// Show this address in the browser tile (the dialog hides until the browser is closed).
    OpenBrowser(String),
}

/// This device's address on the local network (private first), for the phone relay.
fn lan_ip() -> Option<String> {
    let all = iyakki_devctl::ipv4_addresses();
    let ip = |s: &str| s.parse::<std::net::Ipv4Addr>().ok();
    all.iter().filter_map(|(_, a)| ip(a)).find(|a| a.is_private()).or_else(|| all.iter().filter_map(|(_, a)| ip(a)).find(|a| a.is_link_local())).map(|a| a.to_string())
}

impl LoginModal {
    /// `claude-code` first asks how to sign in; `copilot` starts at once.
    pub fn new(agent: &'static str) -> LoginModal {
        let mut field = TextField::new("", false);
        field.max = 4096;
        LoginModal { agent, stage: if agent == extagent::CLAUDE_CODE { Stage::Choose } else { Stage::Running }, method: 0, sess: None, snap: None, field, qr: None, relay: None, relay_tried: false, said: None, hidden: false, ended_away: false, clip_offer: None, after: None, home: None, started: false, finished_audited: false, scan: Scan::new(CamSource::desktop()), show_relay: false, screen_w: 0.0, last_qr: None }
    }

    pub fn with_home(mut self, home: &std::path::Path) -> LoginModal {
        self.home = Some(home.to_path_buf());
        self
    }

    /// Use another camera source (tests: a camera made up in software).
    pub fn with_camera(mut self, src: std::sync::Arc<CamSource>, limit: std::time::Duration) -> LoginModal {
        let scan = Scan::new(src).with_limit(limit);
        // a busy test machine must not make the decoder give up on a frame
        self.scan = if cfg!(test) { scan.with_budget(std::time::Duration::from_secs(10)) } else { scan };
        self
    }

    pub fn scan(&self) -> &Scan {
        &self.scan
    }

    pub fn agent(&self) -> &'static str {
        self.agent
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snap.as_ref()
    }

    pub fn running(&self) -> bool {
        self.sess.is_some() && self.snap.as_ref().is_none_or(|s| !s.phase.finished())
    }

    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// The text of the code field (tests, and the shell's checks).
    pub fn field_text(&self) -> &str {
        &self.field.value
    }

    pub fn clip_offer(&self) -> Option<usize> {
        self.clip_offer
    }

    pub fn relay_url(&self) -> Option<&str> {
        self.relay.as_ref().map(|r| r.url.as_str())
    }

    /// The program ended while the dialog was out of the way (the browser was open).
    pub fn ended_while_away(&self) -> bool {
        self.ended_away
    }

    /// Show the second step for an address already served (screenshots and tests; the real one is made by the dialog itself).
    pub fn set_relay_for_demo(&mut self, url: &str) {
        self.relay_tried = true;
        self.relay = Some(RelayView { url: url.to_string(), qr: qr::encode(url.as_bytes()) });
    }

    /// The browser tile was opened with the address: step aside (the program goes on running).
    pub fn hide(&mut self) {
        self.scan.stop(None);
        self.hidden = true;
    }

    /// The browser tile was closed: the dialog is back, with the field ready; a code-like clipboard is offered with one tap and nothing
    /// is filled in without it.
    pub fn show_again(&mut self, cx: &AppCx) {
        self.hidden = false;
        self.tick(cx);
        self.refresh_clip();
        if self.snap.as_ref().is_some_and(|s| s.phase.finished()) && self.ended_away {
            self.clip_offer = None;
        }
    }

    /// Called now and then while the dialog is shown: a code copied elsewhere is offered (never filled in).
    pub fn refresh_clip_offer(&mut self) {
        self.refresh_clip();
    }

    /// The browser could not be opened: the dialog stays.
    pub fn show_hidden_again_for_shell(&mut self) {
        self.hidden = false;
    }

    fn refresh_clip(&mut self) {
        let want_state = self.agent == extagent::CLAUDE_CODE;
        let waiting = self.snap.as_ref().is_some_and(|s| s.wants_input && !s.phase.finished()) && self.field.value.is_empty();
        self.clip_offer = crate::ui::clipboard::latest().filter(|t| waiting && login::looks_like_code(t, want_state)).map(|t| login::check_code(&t, want_state).len);
    }

    fn method(&self) -> Method {
        Method::ALL[self.method.min(Method::ALL.len() - 1)]
    }

    /// Start the vendor's command (also "Try again").
    pub fn start(&mut self, cx: &AppCx) {
        let cfg = cx.services.agents_cfg();
        let Some(a) = cfg.agent(self.agent) else { return };
        let home = self.home.clone().unwrap_or_else(|| login::login_home(&cx.session.user, cx.services.dir()));
        let Some(spec) = login::spec(self.agent, &a.program(), self.method(), &home) else { return };
        // only the fact is audited: never an address, a code or a token
        cx.audit.record(&cx.session.user, "agent.login.start", self.agent);
        if self.relay.take().is_some() {
            relay::finish();
        }
        self.relay_tried = false;
        self.scan.stop(None);
        self.sess = Some(LoginSession::start(&spec));
        self.snap = None;
        self.qr = None;
        self.said = None;
        self.ended_away = false;
        self.after = None;
        self.field.set("");
        self.stage = Stage::Running;
        self.started = true;
        self.finished_audited = false;
    }

    /// Open the second step (the phone relay) once the program waits for a code and this device can serve the page.
    fn open_relay(&mut self) {
        self.relay_tried = true;
        let Some((port, https)) = relay::port() else { return };
        let Some(ip) = lan_ip() else { return };
        let token = relay::open(name_of(self.agent));
        if token.is_empty() {
            return;
        }
        let url = format!("{}://{ip}:{port}/login/{token}", if https { "https" } else { "http" });
        let q = qr::encode(url.as_bytes());
        self.relay = Some(RelayView { url, qr: q });
    }

    /// Look at the program's output (called on every heartbeat). True when the picture changed.
    pub fn tick(&mut self, cx: &AppCx) -> bool {
        let scanned = self.tick_scan();
        let Some(s) = &self.sess else { return scanned };
        let sn = s.snapshot();
        let mut changed = scanned;
        // a code that arrived from the phone goes to the program as if typed
        if let Some(raw) = self.relay.as_ref().and_then(|_| relay::take_submitted()) {
            cx.audit.record(&cx.session.user, "agent.login.relay", "used");
            let c = login::check_code(&raw, self.agent == extagent::CLAUDE_CODE);
            if let Some(p) = c.problem {
                self.said = Some((format!("The phone sent something that is not a code: {p}"), false));
                relay::rearm();
            } else {
                s.send_line(&c.code);
                self.said = Some((format!("Code received from your phone ({} characters). Checking…", c.len), true));
            }
            changed = true;
        }
        if self.relay.is_some() && sn.note.is_some() && self.snap.as_ref().is_none_or(|o| o.note != sn.note) {
            relay::rearm();
        }
        if self.relay.is_none() && !self.relay_tried && self.agent == extagent::CLAUDE_CODE && sn.wants_input && !sn.phase.finished() {
            self.open_relay();
            changed = true;
        }
        if self.snap.as_ref() == Some(&sn) {
            return changed;
        }
        if let (Some(u), true) = (&sn.url, self.qr.as_ref().is_none_or(|(old, _)| Some(old) != sn.url.as_ref())) {
            self.qr = Some((u.clone(), qr::encode(u.as_bytes())));
        }
        if sn.phase.finished() && !self.finished_audited {
            self.finished_audited = true;
            if self.relay.take().is_some() {
                relay::finish();
            }
            if self.hidden {
                self.ended_away = true;
            }
            let word = match sn.phase {
                Phase::Succeeded => "ok",
                Phase::Cancelled => "cancelled",
                _ => "failed",
            };
            cx.audit.record(&cx.session.user, "agent.login.finish", &format!("{} {word}", self.agent));
            if sn.phase == Phase::Succeeded {
                if self.agent == extagent::COPILOT {
                    let home = self.home.clone().unwrap_or_else(|| login::login_home(&cx.session.user, cx.services.dir()));
                    self.after = Some(move_token_to_vault(cx, &home));
                }
                // the chip, the picker and the model list read the new state
                cx.services.ext_status.refresh_one(cx.services.agents_cfg(), self.agent, None);
            }
        }
        self.snap = Some(sn);
        true
    }

    /// The program waits for a code (not a yes/no question) and is not finished: the only time the camera may be on.
    fn waiting_for_code(&self) -> bool {
        self.snap.as_ref().is_some_and(|s| s.wants_input && s.question.is_none() && !s.phase.finished())
    }

    /// Watch the camera: stop it when the code is no longer wanted, put a valid code read from a QR code into the field (never sent
    /// without Enter). True when the picture changed.
    fn tick_scan(&mut self) -> bool {
        if self.scan.step() == Step::Off {
            return false;
        }
        if self.scan.step() == Step::On && (!self.waiting_for_code() || self.hidden) {
            self.scan.stop(Some("The camera is off."));
            return true;
        }
        if let Some(code) = self.scan.poll(self.agent == extagent::CLAUDE_CODE) {
            self.field.set(&code);
            self.said = None;
            self.clip_offer = None;
        }
        true
    }

    /// The check of what is in the field, for the code a program waits for (not for a yes/no question).
    fn check(&self) -> Option<login::CodeCheck> {
        let sn = self.snap.as_ref()?;
        (sn.wants_input && sn.question.is_none() && !self.field.value.trim().is_empty()).then(|| login::check_code(&self.field.value, self.agent == extagent::CLAUDE_CODE))
    }

    fn send(&mut self) {
        let raw = self.field.value.clone();
        if raw.trim().is_empty() {
            return;
        }
        let text = if self.snap.as_ref().is_some_and(|s| s.question.is_none()) {
            let c = login::check_code(&raw, self.agent == extagent::CLAUDE_CODE);
            if let Some(p) = c.problem {
                self.said = Some((p, false));
                return;
            }
            c.code
        } else {
            raw.trim().to_string()
        };
        self.field.take();
        self.said = None;
        if let Some(s) = &self.sess {
            s.send_line(&text);
        }
    }

    /// While the camera question or the camera is up, the dialog behind it does not react: Esc and the buttons end the scan.
    fn handle_scan(&mut self, ev: &Event) -> Out {
        match (self.scan.step(), ev) {
            (Step::Asking, Event::Tab(T_SCAN_ALLOW)) | (Step::Asking, Event::Key(Key::Enter)) => self.scan.allow(),
            (Step::Asking, Event::Tab(T_SCAN_DENY)) | (Step::Asking, Event::Key(Key::Esc)) | (Step::Asking, Event::Tab(T_CANCEL)) => self.scan.deny(),
            (Step::On, Event::Tab(T_SCAN_STOP)) | (Step::On, Event::Key(Key::Esc)) | (Step::On, Event::Tab(T_CANCEL)) => self.scan.stop(Some("The camera is off.")),
            _ => {}
        }
        Out::Stay
    }

    pub fn handle(&mut self, ev: &Event, cx: &AppCx) -> Out {
        if self.scan.step() != Step::Off && !matches!(ev, Event::Tick) {
            return self.handle_scan(ev);
        }
        let finished = self.snap.as_ref().is_some_and(|s| s.phase.finished());
        match (&self.stage, ev) {
            (_, Event::Tab(T_CANCEL)) | (_, Event::Key(Key::Esc)) => {
                if let Some(s) = self.sess.as_ref().filter(|_| !finished) {
                    s.cancel();
                    // the dialog stays until the program is gone, so the person sees that it stopped
                    return Out::Stay;
                }
                if self.relay.take().is_some() {
                    relay::finish();
                }
                return Out::Close;
            }
            // the QR size and the camera: Ctrl+B and Ctrl+K, because plain letters belong to the code field
            (Stage::Running, Event::Tab(T_BIGGER)) | (Stage::Running, Event::Key(Key::Ctrl('b'))) | (Stage::Running, Event::Key(Key::Ctrl('B'))) => set_qr_big(!qr_big()),
            (Stage::Running, Event::Tab(T_QR_NEXT)) | (Stage::Running, Event::Key(Key::Ctrl('n'))) | (Stage::Running, Event::Key(Key::Ctrl('N'))) => {
                if self.relay.is_some() {
                    self.show_relay = !self.show_relay;
                }
            }
            (Stage::Running, Event::Tab(T_SCAN)) | (Stage::Running, Event::Key(Key::Ctrl('k'))) | (Stage::Running, Event::Key(Key::Ctrl('K'))) => {
                if self.waiting_for_code() {
                    self.scan.ask();
                }
            }
            (Stage::Choose, Event::Key(Key::Up)) => self.method = (self.method + Method::ALL.len() - 1) % Method::ALL.len(),
            (Stage::Choose, Event::Key(Key::Down)) => self.method = (self.method + 1) % Method::ALL.len(),
            (Stage::Choose, Event::Tab(n)) if (T_PICK..T_PICK + 3).contains(n) => self.method = n - T_PICK,
            (Stage::Choose, Event::Tab(T_START)) | (Stage::Choose, Event::Key(Key::Enter)) => self.start(cx),
            (Stage::Running, Event::Tab(T_AGAIN)) => {
                self.sess = None;
                self.stage = if self.agent == extagent::CLAUDE_CODE { Stage::Choose } else { Stage::Running };
                if matches!(self.stage, Stage::Running) {
                    self.start(cx);
                }
            }
            (Stage::Running, Event::Key(Key::Enter)) if finished => return Out::Close,
            (Stage::Running, Event::Tab(T_COPY_URL)) => {
                if let Some(u) = self.snap.as_ref().and_then(|s| s.url.clone()) {
                    crate::ui::clipboard::copy(&u);
                }
            }
            (Stage::Running, Event::Tab(T_OPEN)) => {
                if let Some(u) = self.snap.as_ref().filter(|s| !s.phase.finished()).and_then(|s| s.url.clone()) {
                    return Out::OpenBrowser(u);
                }
            }
            (Stage::Running, Event::Tab(T_COPY_CODE)) => {
                if let Some(c) = self.snap.as_ref().and_then(|s| s.code.clone()) {
                    crate::ui::clipboard::copy(&c);
                }
            }
            (Stage::Running, Event::Tab(T_SEND)) | (Stage::Running, Event::Tab(T_SEND_OFFER)) | (Stage::Running, Event::Key(Key::Enter)) => self.send(),
            // the clipboard, in one tap: nothing is filled in before the person taps
            (Stage::Running, Event::Tab(T_PASTE)) | (Stage::Running, Event::Tab(T_PASTE_OFFER)) | (Stage::Running, Event::Key(Key::Ctrl('v'))) => {
                if let Some(t) = crate::ui::clipboard::latest() {
                    self.field.set("");
                    self.field.paste(&t);
                    self.said = None;
                    self.clip_offer = None;
                }
            }
            (Stage::Running, Event::Paste(t)) => {
                self.field.paste(t);
            }
            (Stage::Running, Event::Key(k)) => {
                self.field.handle(k);
            }
            (Stage::Choose, Event::Tab(_)) | (Stage::Choose, Event::Key(_)) => {}
            _ => {}
        }
        Out::Stay
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        if self.hidden {
            return;
        }
        let name = name_of(self.agent);
        self.screen_w = area.w;
        // the big QR needs a taller and wider dialog; a wide screen gets a little more height for its larger default
        let (mw, mh) = if qr_big() && matches!(self.stage, Stage::Running) { (1240.0, 860.0) } else if area.w > WIDE_SCREEN { (980.0, 680.0) } else { (980.0, 600.0) };
        let p = fx::modal(g, area, s, mw * s, mh * s, ACCENT, false);
        let y0 = fx::modal_title(g, p, s, &format!("Sign in to {name}"), ACCENT);
        let (x, w) = (p.x + 28.0 * s, p.w - 56.0 * s);
        match self.stage {
            Stage::Choose => self.render_choose(g, p, x, y0, w, s),
            Stage::Running => self.render_running(g, p, x, y0, w, s, tick),
        }
        if self.scan.step() != Step::Off {
            self.render_scan(g, p, s, tick);
        }
    }

    fn render_choose(&mut self, g: &mut Gfx, p: R, x: f32, y0: f32, w: f32, s: f32) {
        g.text(x, y0 + 6.0 * s, 15.0 * s, MUTED, "How do you sign in? IYAKKI starts Anthropic's own sign-in and shows you its address; it never sees your password.", Face::Sans);
        let notes = ["For Claude Pro, Max, Team or Enterprise. This is the usual choice.", "Pay per use through your Anthropic Console account (API billing).", "Your organization's single sign-on."];
        let mut y = y0 + 44.0 * s;
        for (i, m) in Method::ALL.iter().enumerate() {
            let on = i == self.method;
            let r = R::new(x, y, w, 62.0 * s);
            g.rrect(r.x, r.y, r.w, r.h, 12.0 * s, if on { rgb(34, 44, 80) } else { rgb(22, 28, 50) });
            g.rrect_outline(r.x, r.y, r.w, r.h, 12.0 * s, if on { 1.8 * s } else { 1.0 }, if on { ACCENT } else { rgb(62, 72, 108) });
            g.circle(r.x + 26.0 * s, r.y + 31.0 * s, 9.0 * s, rgb(62, 72, 108));
            if on {
                g.circle(r.x + 26.0 * s, r.y + 31.0 * s, 6.0 * s, ACCENT);
            }
            g.text(r.x + 50.0 * s, r.y + 10.0 * s, 17.0 * s, TEXT, m.label(), Face::Bold);
            g.text(r.x + 50.0 * s, r.y + 34.0 * s, 13.0 * s, MUTED, notes[i], Face::Sans);
            g.hit(r, Event::Tab(T_PICK + i), "method");
            y += 72.0 * s;
        }
        let by = p.y + p.h - 64.0 * s;
        g.button(x, by, 15.0 * s, "Start sign-in", OK, Event::Tab(T_START));
        g.button(x + 190.0 * s, by, 15.0 * s, "Cancel", MUTED, Event::Tab(T_CANCEL));
        g.text(x + 330.0 * s, by + 8.0 * s, 12.0 * s, DIM, "Up / Down choose · Enter start · Esc cancel", Face::Sans);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_running(&mut self, g: &mut Gfx, p: R, x: f32, y0: f32, w: f32, s: f32, tick: u64) {
        let snap = self.snap.clone().unwrap_or(Snapshot { phase: Phase::Starting, url: None, code: None, wants_input: false, question: None, note: None, line: format!("Starting {}'s sign-in…", name_of(self.agent)) });
        let dots = ".".repeat(1 + (tick / 10 % 3) as usize);
        let finished = snap.phase.finished();
        // the right side: the QR code of the address (step 1) and, when the phone relay is up, a second one for the code (step 2)
        let two = self.relay.is_some() && !finished;
        let lay = qr_layout(s, self.screen_w, p.h, p.w, two, qr_big());
        let (qr_w, big) = (lay.side, lay.big);
        let right_w = if two && !big { qr_w + lay.relay + 16.0 * s } else { qr_w };
        let left_w = w - right_w - 20.0 * s;
        let mut y = y0 + 4.0 * s;
        if snap.phase == Phase::Succeeded {
            g.text(x, y + 30.0 * s, 34.0 * s, OK, "Signed in", Face::Bold);
            let t = format!("{} is ready. The chip shows your account in a moment.", name_of(self.agent));
            g.text(x, y + 84.0 * s, 15.0 * s, MUTED, &t, Face::Sans);
            if let Some((m, ok)) = &self.after {
                for (i, l) in g.wrap(14.0 * s, m, Face::Sans, w).into_iter().take(4).enumerate() {
                    g.text(x, y + 116.0 * s + i as f32 * 19.0 * s, 14.0 * s, if *ok { OK } else { AMBER }, &l, Face::Sans);
                }
            }
        } else {
            let step = match (&snap.url, &snap.code) {
                (None, None) => format!("Starting the vendor's sign-in{dots}"),
                (Some(_), Some(_)) => "1. Open the address on any device (a phone is fine).   2. Enter this code.   3. Approve.".to_string(),
                (Some(_), None) if self.agent == extagent::CLAUDE_CODE && two => "Step 1: sign in (open it here, or scan the first code). Step 2: copy the code the page shows, then paste it here, or send it from your phone with the second code.".to_string(),
                (Some(_), None) if self.agent == extagent::CLAUDE_CODE => "Step 1: sign in (open it here, or scan the code with a phone). Step 2: copy the code the page shows and paste it below.".to_string(),
                _ => "Open the address on any device (a phone is fine), then sign in.".to_string(),
            };
            for l in g.wrap(14.0 * s, &step, Face::Sans, left_w).into_iter().take(4) {
                g.text(x, y, 14.0 * s, MUTED, &l, Face::Sans);
                y += 19.0 * s;
            }
            y += 6.0 * s;
            if let Some(code) = &snap.code {
                g.text(x, y, 12.0 * s, DIM, "ONE-TIME CODE", Face::Bold);
                y += 18.0 * s;
                let spaced: String = code.chars().flat_map(|c| [c, ' ']).collect::<String>().trim_end().to_string();
                g.text(x, y, 46.0 * s, ACCENT, &spaced, Face::Mono);
                y += 60.0 * s;
                g.button(x, y, 13.0 * s, "Copy code", TEXT, Event::Tab(T_COPY_CODE));
                y += 38.0 * s;
            }
            if let Some(url) = &snap.url {
                g.text(x, y, 12.0 * s, DIM, "ADDRESS", Face::Bold);
                y += 18.0 * s;
                let big = url.len() <= 42;
                let (size, rows) = if big { (24.0 * s, 1) } else { (13.0 * s, 4) };
                for l in wrap_url(g, size, url, left_w, rows) {
                    g.text(x, y, size, TEXT, &l, Face::Mono);
                    y += size * 1.3;
                }
                y += 6.0 * s;
                let b1 = g.button(x, y, 13.0 * s, "Copy address", TEXT, Event::Tab(T_COPY_URL));
                if !finished {
                    g.button(x + b1 + 10.0 * s, y, 13.0 * s, "Open in browser", ACCENT, Event::Tab(T_OPEN));
                }
            }
        }
        let mut drawn: Option<R> = None;
        if !finished {
            let qx = x + left_w + 20.0 * s;
            let qy = y0 + 4.0 * s;
            if big {
                // one big code at a time; with the phone relay up, a button switches to the other
                let relay_now = self.show_relay && self.relay.is_some();
                let q = if relay_now { self.relay.as_ref().and_then(|r| r.qr.as_ref()) } else { self.qr.as_ref().and_then(|(_, q)| q.as_ref()) };
                match q {
                    Some(q) => drawn = Some(draw_qr(g, R::new(qx, qy, qr_w, qr_w), q)),
                    None => {
                        g.text(qx, qy + 4.0 * s, 12.0 * s, DIM, "(too long for a QR code; copy the address)", Face::Sans);
                    }
                }
                let cap = if two { if relay_now { "2  Send the code" } else { "1  Sign in" } } else { "Scan with a phone camera" };
                g.text(qx, qy + qr_w + 4.0 * s, 12.0 * s, DIM, cap, Face::Bold);
                let by = qy + qr_w + 24.0 * s;
                let b1 = g.button(qx, by, 13.0 * s, "- smaller QR  (Ctrl+B)", TEXT, Event::Tab(T_BIGGER));
                if two {
                    g.button(qx + b1 + 8.0 * s, by, 13.0 * s, if relay_now { "Show code 1  (Ctrl+N)" } else { "Show code 2  (Ctrl+N)" }, ACCENT, Event::Tab(T_QR_NEXT));
                }
            } else {
                if let Some((_, Some(q))) = &self.qr {
                    drawn = Some(draw_qr(g, R::new(qx, qy, qr_w, qr_w), q));
                    g.text(qx, y0 + 10.0 * s + qr_w, 12.0 * s, DIM, if two { "1  Sign in" } else { "Scan with a phone camera" }, Face::Bold);
                } else if matches!(&self.qr, Some((_, None))) {
                    g.text(qx, y0 + 8.0 * s, 12.0 * s, DIM, "(the address is too long for a QR code; copy it)", Face::Sans);
                }
                if self.qr.is_some() {
                    g.button(qx, y0 + 28.0 * s + qr_w, 13.0 * s, "+ bigger QR  (Ctrl+B)", TEXT, Event::Tab(T_BIGGER));
                }
                if let Some(r) = &self.relay {
                    let (rx, rw) = (qx + qr_w + 16.0 * s, lay.relay);
                    if let Some(q) = &r.qr {
                        let _ = draw_qr(g, R::new(rx, qy, rw, rw), q);
                    }
                    g.text(rx, y0 + 10.0 * s + rw, 12.0 * s, DIM, "2  Send the code", Face::Bold);
                    // the short address of the second step, for a person who types it instead
                    let host = r.url.split("/login/").next().unwrap_or("");
                    let cap = g.fit(11.0 * s, &format!("{host}/login/…"), Face::Mono, rw);
                    g.text(rx, y0 + 28.0 * s + rw, 11.0 * s, DIM, &cap, Face::Mono);
                    let note = g.wrap(11.0 * s, "Open it on the phone you signed in with and paste the code there. Your browser will warn about the certificate: the device made its own.", Face::Sans, rw);
                    for (i, l) in note.into_iter().take(5).enumerate() {
                        g.text(rx, y0 + 44.0 * s + rw + i as f32 * 14.0 * s, 11.0 * s, DIM, &l, Face::Sans);
                    }
                }
            }
        }
        self.last_qr = drawn;
        // the field for what the browser shows (or a yes/no): large monospace, a Paste button, and the check before sending. With the
        // big QR the column of text is narrower, so the buttons go under the field.
        let tw = if big { left_w } else { w };
        let fy = p.y + p.h - if big { 268.0 } else { 196.0 } * s;
        if snap.wants_input && !finished {
            let label = snap.question.clone().unwrap_or_else(|| "Code from the page (paste it here, then Enter)".to_string());
            let label = g.fit(13.0 * s, &label, Face::Bold, tw);
            g.text(x, fy, 13.0 * s, TEXT, &label, Face::Bold);
            let r = R::new(x, fy + 20.0 * s, if big { tw } else { w - 250.0 * s }, 44.0 * s);
            self.field.render(g, r, true, s, (tick / 15) % 2 == 0, Face::Mono, 0);
            let (mut bx, by) = if big { (x, r.y + r.h + 8.0 * s) } else { (r.x + r.w + 12.0 * s, r.y + 4.0 * s) };
            bx += g.button(bx, by, 14.0 * s, "Paste", TEXT, Event::Tab(T_PASTE)) + 8.0 * s;
            g.button(bx, by, 14.0 * s, "Send", OK, Event::Tab(T_SEND));
            let below = if big { r.y + r.h + 44.0 * s } else { r.y + r.h + 4.0 * s };
            // what is in the field: its length and whether it looks right, before anything is sent
            let (msg, col) = match self.check() {
                Some(c) => match (&c.problem, &c.warning) {
                    (Some(p), _) => (format!("{} characters. {p}", c.len), ERR),
                    (None, Some(wn)) => (wn.clone(), AMBER),
                    (None, None) => (format!("{} characters{}. Looks right: press Enter to send.", c.len, if c.from_url { ", taken from the address you pasted" } else { "" }), OK),
                },
                None => match self.scan.said().or(self.said.as_ref()) {
                    Some((m, ok)) => (m.clone(), if *ok { OK } else { ERR }),
                    None => (String::new(), DIM),
                },
            };
            // a code that came from the camera says so
            let from_cam = !self.field.value.is_empty() && self.scan.said().is_some_and(|(m, ok)| *ok && m.starts_with("Code read"));
            let msg = if from_cam && col == OK { format!("{msg} (read from the QR code)") } else { msg };
            if !msg.is_empty() {
                let msg = g.fit(12.5 * s, &msg, Face::Sans, tw);
                g.text(x, below, 12.5 * s, col, &msg, Face::Sans);
            }
            // the second row: the clipboard offer and the camera
            let row = below + 20.0 * s;
            let mut rx = x;
            if let (Some(n), true) = (self.clip_offer, self.field.value.is_empty()) {
                rx += g.button(rx, row, 14.0 * s, &format!("Paste the copied code ({n} characters)"), OK, Event::Tab(T_PASTE_OFFER)) + 10.0 * s;
            }
            if snap.question.is_none() {
                g.button(rx, row, 14.0 * s, "Scan with camera  (Ctrl+K)", ACCENT, Event::Tab(T_SCAN));
            }
        }
        // the live status line and the buttons
        let ly = p.y + p.h - 84.0 * s;
        let away = if self.ended_away && finished { "The sign-in ended while the browser was open (it waits 15 minutes). " } else { "" };
        let (line, ok) = match &snap.phase {
            Phase::Succeeded => ("Signed in.".to_string(), true),
            Phase::Failed(_) | Phase::Cancelled => (format!("{away}{}", snap.line), false),
            _ => (format!("{}{}", snap.line.trim_end_matches('…'), if snap.line.ends_with('…') { dots.as_str() } else { "" }), true),
        };
        let col = if matches!(snap.phase, Phase::Failed(_)) || snap.note.is_some() { ERR } else if ok { MUTED } else { AMBER };
        for (i, l) in g.wrap(13.0 * s, &line, Face::Sans, tw).into_iter().take(2).enumerate() {
            g.text(x, ly + i as f32 * 17.0 * s, 13.0 * s, col, &l, Face::Sans);
        }
        let by = p.y + p.h - 52.0 * s;
        if finished {
            g.button(x, by, 15.0 * s, "Close", if snap.phase == Phase::Succeeded { OK } else { MUTED }, Event::Tab(T_CANCEL));
            if snap.phase != Phase::Succeeded {
                g.button(x + 130.0 * s, by, 15.0 * s, "Try again", ACCENT, Event::Tab(T_AGAIN));
            }
        } else {
            g.button(x, by, 15.0 * s, "Cancel", ERR, Event::Tab(T_CANCEL));
        }
    }
}

impl LoginModal {
    /// The camera question and the live camera, as a card over the dialog: what the camera is for, a clear "camera is on" mark, the
    /// small live picture with a frame to hold the code in, the time left and a Stop button. Only this card is clickable while it is up.
    fn render_scan(&mut self, g: &mut Gfx, p: R, s: f32, tick: u64) {
        let card = crate::scan_core::Card {
            ask_title: "Turn on the camera?",
            ask_body: "IYAKKI will use the camera only to read one QR code: show the code on your phone to it. The pictures are looked at in memory, never saved and never sent. The camera turns off as soon as a code is read, when you press Esc, when this dialog closes, and after one minute.",
            hold_note: "Hold the QR code of the sign-in code on your phone inside the corners, about 20 cm from the camera. Nothing is saved.",
            allow: T_SCAN_ALLOW,
            deny: T_SCAN_DENY,
            stop: T_SCAN_STOP,
        };
        crate::scan_core::draw_card(g, p, s, tick, &self.scan, &card);
    }
}

impl Drop for LoginModal {
    fn drop(&mut self) {
        // a dialog that goes takes its relay page with it
        if self.relay.take().is_some() {
            relay::finish();
        }
    }
}

/// An address cut into lines of the width, the last one ending in … when it does not all fit.
fn wrap_url(g: &mut Gfx, size: f32, url: &str, w: f32, rows: usize) -> Vec<String> {
    let mut lines: Vec<String> = vec![];
    let mut cur = String::new();
    for c in url.chars() {
        cur.push(c);
        if g.text_width(size, &cur, Face::Mono) > w {
            let last = cur.pop().unwrap_or(' ');
            lines.push(std::mem::take(&mut cur));
            cur.push(last);
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.len() > rows {
        lines.truncate(rows);
        if let Some(l) = lines.last_mut() {
            l.pop();
            l.push('…');
        }
    }
    lines
}

/// The QR code, dark on white with the quiet zone the standard asks for, centred in `r` with whole-pixel modules.
fn draw_qr(g: &mut Gfx, r: R, q: &qr::Qr) -> R {
    let (m, side) = qr_metrics(r.w.min(r.h), q.size);
    // the corner lands on a whole pixel too, so no module edge is blurred
    let (x0, y0) = ((r.x + (r.w - side) / 2.0).round(), r.y.round());
    g.rect(x0, y0, side, side, [255, 255, 255, 255]);
    for y in 0..q.size {
        let mut x = 0;
        while x < q.size {
            if q.get(x, y) {
                let start = x;
                while x < q.size && q.get(x, y) {
                    x += 1;
                }
                g.rect(x0 + (start + 4) as f32 * m, y0 + (y + 4) as f32 * m, (x - start) as f32 * m, m, [0, 0, 0, 255]);
            } else {
                x += 1;
            }
        }
    }
    R::new(x0, y0, side, side)
}

// ------------------------------------------------------------------------------------------------ requests from other screens

/// Sign out of an agent on a thread of its own; the audit gets only the fact. Returns the words for the toast.
pub fn sign_out(cx: &AppCx, agent: &'static str) -> Result<String, String> {
    sign_out_in(cx, agent, &login::login_home(&cx.session.user, cx.services.dir()))
}

/// [`sign_out`] with the home folder given.
pub fn sign_out_in(cx: &AppCx, agent: &'static str, home: &std::path::Path) -> Result<String, String> {
    if agent == extagent::COPILOT {
        return sign_out_copilot(cx, home);
    }
    let cfg = cx.services.agents_cfg();
    let a = cfg.agent(agent).ok_or("unknown agent")?;
    let r = login::logout(agent, &a.program(), home);
    cx.audit.record(&cx.session.user, "agent.logout", &format!("{agent} {}", if r.is_ok() { "ok" } else { "failed" }));
    cx.services.ext_status.refresh_one(cfg, agent, None);
    r
}

thread_local! {
    /// Asked by a screen, taken by the shell: both run on the interface thread.
    static PENDING: std::cell::Cell<Option<(&'static str, bool)>> = const { std::cell::Cell::new(None) };
}
static RESULTS: std::sync::Mutex<Vec<(String, bool)>> = std::sync::Mutex::new(Vec::new());

/// A screen asks the shell for the sign-in dialog (or, with `logout`, to sign out) of an agent.
pub fn request(agent: &'static str, logout: bool) {
    PENDING.with(|p| p.set(Some((agent, logout))));
}

/// The shell takes the request made by a screen.
pub fn take_request() -> Option<(&'static str, bool)> {
    PENDING.with(|p| p.take())
}

/// Sign out on a thread of its own (the vendor program may talk to its service); the words come back through [`take_results`].
pub fn sign_out_in_background(cx: &AppCx, agent: &'static str) {
    let cx = cx.clone();
    std::thread::spawn(move || {
        let r = sign_out(&cx, agent);
        RESULTS.lock().unwrap_or_else(|e| e.into_inner()).push(match r {
            Ok(m) => (m, true),
            Err(e) => (e, false),
        });
    });
}

/// What finished sign-outs have to say: (words, ok).
pub fn take_results() -> Vec<(String, bool)> {
    std::mem::take(&mut *RESULTS.lock().unwrap_or_else(|e| e.into_inner()))
}

#[cfg(test)]
#[path = "agent_login_tests.rs"]
mod tests;

/// After a Copilot login: the program has written its token to a plain file in the home (it has no keychain here). The token goes into
/// the person's vault and is taken out of the file; IYAKKI then hands it to Copilot, for that one process, whenever it starts it. A
/// locked vault leaves the file alone and says so. Nothing is logged but the fact.
pub fn move_token_to_vault(cx: &AppCx, home: &std::path::Path) -> (String, bool) {
    let user = &cx.session.user;
    let Some(found) = login::copilot_file_token(home) else { return ("No token file was found to move into your vault.".into(), false) };
    if !cx.services.user_vault_unlocked(user) {
        cx.audit.record(user, "agent.login.vault", "github-copilot locked");
        return (format!("Your vault is locked, so the token stays in {} as plain text. Unlock your vault (Settings → Vault), then sign in again to move it.", home.join(".copilot/config.json").display()), false);
    }
    if let Err(e) = cx.services.user_set(user, login::COPILOT_VAULT, &login::vault_entry(&found.login, &found.token)) {
        return (format!("The token could not be put in your vault ({e}); it stays in the plain file."), false);
    }
    cx.audit.record(user, "agent.login.vault", "github-copilot imported");
    match login::scrub_copilot_file(home, false) {
        Ok(()) => ("The token is now in your vault. It was taken out of the plain file; IYAKKI gives it to Copilot when it starts it.".into(), true),
        Err(e) => (format!("The token is in your vault, but the plain file could not be cleaned: {e}"), false),
    }
}

/// Sign out of Copilot: the vault entry and any token still in the plain file go (the program has no logout command outside itself).
fn sign_out_copilot(cx: &AppCx, home: &std::path::Path) -> Result<String, String> {
    let user = &cx.session.user;
    let vault = cx.services.user_delete(user, login::COPILOT_VAULT);
    let file = login::scrub_copilot_file(home, true);
    let locked = matches!(vault, Err(iyakki_agent::vault::VaultError::Locked));
    cx.audit.record(user, "agent.logout", &format!("{} {}", extagent::COPILOT, if locked || file.is_err() { "failed" } else { "ok" }));
    cx.services.ext_status.refresh_one(cx.services.agents_cfg(), extagent::COPILOT, None);
    if locked {
        return Err("Your vault is locked, so the token in it could not be removed: unlock it (Settings → Vault) and sign out again.".into());
    }
    file.map_err(|e| format!("The token file could not be cleaned: {e}"))?;
    Ok("Signed out of GitHub Copilot: the token was removed from your vault and from the config file.".into())
}
