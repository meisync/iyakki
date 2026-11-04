//! Cloud tenants in the shell (docs/development/CLOUD-MULTITENANT-PLAN.md, sections 3.3 and 3.4): the mode choice on the sign-in screen
//! ("This machine" | "Cloud"), the list of remembered tenants and the dialogs to add one (trust on first use, with the certificate's
//! fingerprint) and to sign in to it, the strip of chips (Personal, then every open tenant, each with a dot: live, reconnecting, closed),
//! and the shown tenant's picture with the keyboard and mouse sent to it, and only to it.
//!
//! The strip sits along the bottom edge: the top bar has no free room, and a bottom strip keeps the tenant's picture directly below the
//! top bar. It is drawn only where it can cover nothing: on the sign-in screen (under the card) and while a tenant is shown (the picture's
//! area stops above it). On Personal after sign-in the screen belongs to the launcher and the apps down to the last pixel, so there is no
//! strip; the IYAKKI menu (F10, or a click on the name) has a Cloud entry, and Ctrl+Shift+J, that open the same dialog, which lists the
//! tenants signed in to (Show), the remembered ones (sign in), Add a tenant, and Personal. The model (sessions, which one is shown, key and geometry mapping) is `cloudsession.rs`; the remembered list is
//! `iyakki_agent::cloudtenants`. Nothing of a tenant (picture, titles) is drawn while Personal is shown, only the chip's name and dot.

use super::*;
use crate::cloudsession::{self as cs, Connector, Dot, Hub, Session};
use iyakki_agent::cloudclient::{Input, Pin, Presented, Status, Target};
use iyakki_agent::cloudtenants::{self as ct, Tenant, Tenants};
use std::sync::mpsc::{channel, Receiver};

/// The launcher's top bar, below which a tenant's picture starts (before scaling): the name box ends at 74 and the icons at 68.
const LAUNCHER_BAR_H: f32 = 76.0;
/// Height of the strip (before scaling).
pub(super) const STRIP_H: f32 = 36.0;
/// The ids of the dialogs' text fields.
const FIELD_BASE: usize = 9500;
/// The id the shown tenant's picture registers as a text target, so the on-screen keyboard offers itself over it.
const DESKTOP_FIELD: usize = 9520;
/// Heartbeats a new window size waits before it is told to the tenant (a resize is a drag).
const SIZE_SETTLE: u64 = 10;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Act {
    // the strip
    Machine,
    Cloud,
    Chip(usize),
    CloseTenant,
    // the dialogs
    Pick(usize),
    Forget(usize),
    New,
    Field(usize),
    Next,
    Accept,
    Back,
    Submit,
}

pub(super) enum Panel {
    Closed,
    /// The remembered tenants; `sel` is the row a key press acts on.
    Pick { sel: usize, error: Option<String> },
    /// A new tenant: address and name.
    Add { fields: Vec<TextField>, focus: usize, error: Option<String> },
    /// The certificate a new tenant showed, for the person to accept or not.
    Trust { name: String, addr: String, shown: Presented },
    /// A remembered tenant now shows another certificate: a hard stop.
    Changed { name: String, old: Pin, new: Pin },
    /// The account and password for a tenant.
    Creds { t: Tenant, fields: Vec<TextField>, focus: usize, error: Option<String> },
}

enum Purpose {
    Add { name: String, addr: String },
    Signin(String),
}

enum Done {
    Probe(Purpose, Result<Presented, String>),
    Connect { name: String, account: String, res: Result<Box<dyn cs::RemoteDesktop>, String> },
}

/// One row of the Cloud dialog.
pub(super) struct PickRow {
    pub name: String,
    pub addr: String,
    /// Signed in: the session's place among the chips and its dot.
    pub open: Option<(usize, Dot)>,
}

pub(super) struct TenantUi {
    pub hub: Hub,
    pub net: Arc<dyn Connector>,
    pub panel: Panel,
    list: Option<Tenants>,
    /// What is clickable, from the last frame: the strip's chips and the dialog's buttons and fields.
    pub rects: Vec<(R, Act)>,
    /// The tenant's window area and where its picture was drawn, from the last frame.
    pub area: R,
    pub dest: (f32, f32, f32, f32),
    busy: Option<(String, Receiver<Done>)>,
    /// The mouse button is down in the tenant's window.
    pressed: bool,
    last_dots: Vec<Dot>,
}

impl Default for TenantUi {
    fn default() -> Self {
        TenantUi { hub: Hub::default(), net: cs::real_connector(), panel: Panel::Closed, list: None, rects: Vec::new(), area: R::new(0.0, 0.0, 0.0, 0.0), dest: (0.0, 0.0, 0.0, 0.0), busy: None, pressed: false, last_dots: Vec::new() }
    }
}

/// What a failure is called in words.
fn words(e: &str) -> String {
    let l = e.to_lowercase();
    if l.contains("certificate") || l.contains("fingerprint") || l.contains("pin") {
        "The server's certificate is not the one you trusted: its fingerprint changed. Nothing was sent.".into()
    } else if l.contains("refus") || l.contains("password") || l.contains("denied") || l.contains("wrong") {
        "The account or password was refused.".into()
    } else if l.contains("reach") || l.contains("connect") || l.contains("timed out") || l.contains("resolve") || l.contains("lookup") || l.contains("unreachable") {
        format!("Could not reach the server ({e}).")
    } else {
        format!("Could not sign in: {e}")
    }
}

/// `2030-01-31` for unix seconds (UTC).
fn date_of(unix: i64) -> String {
    let z = unix.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn overlaps(a: R, b: R) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

impl Shell {
    // ------------------------------------------------------------------ the remembered list

    fn tenant_list(&mut self) -> &mut Tenants {
        if self.tenants.list.is_none() {
            self.tenants.list = Some(Tenants::load(&ct::path_in(self.env.services.dir())));
        }
        self.tenants.list.as_mut().expect("loaded above")
    }

    fn tenant_save(&mut self) -> Result<(), String> {
        let path = ct::path_in(self.env.services.dir());
        self.tenant_list().save(&path)
    }

    /// The remembered tenants (tests and tools).
    pub fn remembered_tenants(&mut self) -> Vec<(String, String)> {
        self.tenant_list().list().iter().map(|t| (t.name.clone(), t.addr.clone())).collect()
    }

    /// Replace the network (tests).
    #[cfg(test)]
    pub(crate) fn set_tenant_net_for_test(&mut self, n: Arc<dyn Connector>) {
        self.tenants.net = n;
    }

    /// The names of the open tenants, in the order of the chips (tests and tools).
    pub fn open_tenants(&self) -> Vec<String> {
        self.tenants.hub.sessions.iter().map(|s| s.name.clone()).collect()
    }

    /// The tenant shown now; `None` is Personal.
    pub fn shown_tenant(&self) -> Option<String> {
        self.tenants.hub.current().map(|s| s.name.clone())
    }

    /// The person signed out: every tenant is signed out of too.
    pub(super) fn tenants_sign_out(&mut self) {
        self.tenants.hub.close_all();
        self.tenants.panel = Panel::Closed;
        self.tenants.busy = None;
        self.tenants.pressed = false;
    }

    /// The rows of the Cloud dialog: the tenants signed in to (in the order of the chips), then the remembered ones that are not open.
    fn pick_entries(&mut self) -> Vec<PickRow> {
        let open: Vec<(String, Dot)> = self.tenants.hub.sessions.iter().map(|x| (x.name.clone(), x.dot())).collect();
        let list: Vec<Tenant> = self.tenant_list().list().to_vec();
        let mut rows: Vec<PickRow> = open
            .iter()
            .enumerate()
            .map(|(i, (name, dot))| PickRow { name: name.clone(), addr: list.iter().find(|t| &t.name == name).map(|t| t.addr.clone()).unwrap_or_default(), open: Some((i, *dot)) })
            .collect();
        rows.extend(list.into_iter().filter(|t| !open.iter().any(|(n, _)| *n == t.name)).map(|t| PickRow { name: t.name, addr: t.addr, open: None }));
        rows
    }

    // ------------------------------------------------------------------ geometry

    fn tenant_strip_visible(&self) -> bool {
        let on_login = self.session.is_none() && matches!(self.screen, Screen::Login(_));
        (on_login && !self.lbar.remote) || (self.tenants.hub.active.is_some() && self.screen_allows_tenants()) || (self.session.is_none() && !self.tenants.hub.sessions.is_empty())
    }

    /// Where the tenant's area starts: below the whole top bar (the launcher's is taller than an app's).
    fn tenant_top(&self, s: f32) -> f32 {
        match (&self.session, &self.screen) {
            (None, _) => crate::loginbar::bar_h(s),
            (Some(_), Screen::Launcher | Screen::PowerMenu) => LAUNCHER_BAR_H * s,
            (Some(_), _) => HEADER_H * s,
        }
    }

    /// The IYAKKI menu's Cloud entry (and Ctrl+Shift+J with nothing open): the dialog with the tenants.
    pub(super) fn open_cloud_dialog(&mut self) {
        if self.screen_allows_tenants() {
            self.tenants.panel = Panel::Pick { sel: 0, error: None };
            self.stale = true;
        }
    }

    /// The window area of a tenant: below the top bar, above the strip.
    fn tenant_area(&self, w: u32, h: u32, s: f32) -> R {
        let top = self.tenant_top(s);
        let bottom = if self.tenant_strip_visible() { h as f32 - self.tenant_strip_y(h, s) } else { self.tenant_kb_height(h) };
        R::new(0.0, top, w as f32, (h as f32 - top - bottom).max(1.0))
    }

    /// How much of the bottom edge the on-screen keyboard (as drawn last frame) takes.
    fn tenant_kb_height(&self, h: u32) -> f32 {
        self.osk.shown.map_or(0.0, |kb| (h as f32 - kb.y).max(0.0))
    }

    /// Where the strip starts: along the bottom edge, or just above the on-screen keyboard while it is up (so it is never under it).
    fn tenant_strip_y(&self, h: u32, s: f32) -> f32 {
        h as f32 - STRIP_H * s - self.tenant_kb_height(h)
    }

    // ------------------------------------------------------------------ the heartbeat

    fn tenants_tick(&mut self) {
        let done = self.tenants.busy.as_ref().and_then(|(_, rx)| rx.try_recv().ok());
        if let Some(d) = done {
            self.tenants.busy = None;
            self.tenant_done(d);
            self.stale = true;
        }
        if self.tenants.hub.sessions.is_empty() {
            return;
        }
        let dots: Vec<Dot> = self.tenants.hub.sessions.iter().map(|x| x.dot()).collect();
        if dots != self.tenants.last_dots {
            self.tenants.last_dots = dots;
            self.stale = true;
        }
        let (w, h) = self.size;
        let area = self.tenant_area(w, h, scale_for(w, h));
        let tick = self.tick;
        let shown = self.tenants.hub.active;
        if let (Some(_), Some(sess)) = (shown, self.tenants.hub.current_mut()) {
            if sess.generation() != sess.drawn {
                self.stale = true;
            }
            // the window's size, told to the tenant once it has settled
            let want = (area.w as u32, area.h as u32);
            if sess.told != Some(want) {
                match sess.pending_size {
                    Some((sz, at)) if sz == want => {
                        if tick.saturating_sub(at) >= SIZE_SETTLE {
                            sess.send(Input::Size { w: want.0, h: want.1 });
                            sess.told = Some(want);
                            sess.pending_size = None;
                        }
                    }
                    _ => sess.pending_size = Some((want, tick)),
                }
            }
        }
    }

    fn tenant_done(&mut self, d: Done) {
        match d {
            Done::Probe(Purpose::Add { name, addr }, res) => match res {
                Ok(shown) => self.tenants.panel = Panel::Trust { name, addr, shown },
                Err(e) => {
                    if let Panel::Add { error, .. } = &mut self.tenants.panel {
                        *error = Some(words(&e));
                    }
                }
            },
            Done::Probe(Purpose::Signin(name), res) => {
                let t = self.tenant_list().get(&name).cloned();
                let Some(t) = t else { return };
                match res {
                    Ok(p) if p.pin == t.pin => self.tenants.panel = Self::tenant_creds_panel(t),
                    Ok(p) => self.tenants.panel = Panel::Changed { name, old: t.pin, new: p.pin },
                    Err(e) => self.tenants.panel = Panel::Pick { sel: 0, error: Some(words(&e)) },
                }
            }
            Done::Connect { name, account, res } => match res.and_then(|d| match d.status() {
                // the real client answers a definite no (a refused password, a certificate that is not the pinned one) as a client already
                // in that state, not as an error: it is an error here, so the dialog says so and nothing is opened
                Status::Refused(m) => {
                    d.close();
                    Err(format!("refused: {m}"))
                }
                Status::Closed(m) => {
                    d.close();
                    Err(m)
                }
                _ => Ok(d),
            }) {
                Ok(desktop) => {
                    self.tenants.hub.open(Session::new(&name, &account, desktop));
                    self.tenant_list().set_last_account(&name, &account);
                    let _ = self.tenant_save();
                    self.tenants.panel = Panel::Closed;
                    self.tenants.pressed = false;
                    self.env.audit.record(self.session.as_ref().map_or("-", |s| s.user.as_str()), "cloud.tenant.open", &name);
                }
                Err(e) => {
                    if let Panel::Creds { fields, error, focus, .. } = &mut self.tenants.panel {
                        fields[1].clear();
                        *focus = 1;
                        *error = Some(words(&e));
                    }
                }
            },
        }
    }

    pub(super) fn tenant_creds_panel(t: Tenant) -> Panel {
        let mut account = TextField::new("Account", false);
        account.set(&t.last_account);
        let focus = if t.last_account.is_empty() { 0 } else { 1 };
        Panel::Creds { t, fields: vec![account, TextField::new("Password", true)], focus, error: None }
    }

    fn tenant_job(&mut self, what: &str, f: impl FnOnce() -> Done + Send + 'static) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let d = f();
            if let Err(e) = tx.send(d) {
                // nobody waits any more (the dialog was cancelled): a tenant that did sign in is signed out again
                if let Done::Connect { res: Ok(d), .. } = e.0 {
                    d.close();
                }
            }
        });
        self.tenants.busy = Some((what.to_string(), rx));
    }

    // ------------------------------------------------------------------ actions

    fn tenant_probe(&mut self, purpose: Purpose, addr: String) {
        let net = self.tenants.net.clone();
        let what = format!("Checking {addr}");
        self.tenant_job(&what, move || Done::Probe(purpose, net.probe(&addr)));
    }

    fn tenant_signin_start(&mut self, name: &str) {
        let Some(t) = self.tenant_list().get(name).cloned() else { return };
        self.tenant_probe(Purpose::Signin(name.to_string()), t.addr);
    }

    fn tenant_connect(&mut self) {
        let Panel::Creds { t, fields, .. } = &self.tenants.panel else { return };
        let (account, password) = (ct::valid_account(&fields[0].value), fields[1].value.clone());
        if account.is_empty() || password.is_empty() {
            if let Panel::Creds { error, .. } = &mut self.tenants.panel {
                *error = Some("Type the account and the password".into());
            }
            return;
        }
        let (target, name) = (Target { addr: t.addr.clone(), pin: t.pin }, t.name.clone());
        let (w, h) = self.size;
        let a = self.tenant_area(w, h, scale_for(w, h));
        let size = (a.w as u32, a.h as u32);
        let net = self.tenants.net.clone();
        let what = format!("Signing in to {name}");
        self.tenant_job(&what, move || {
            let res = net.connect(&target, &account, &password, size);
            Done::Connect { name, account, res }
        });
    }

    fn tenant_act(&mut self, a: Act) {
        self.stale = true;
        match a {
            Act::Machine => {
                self.tenants.hub.show(None);
                self.tenants.panel = Panel::Closed;
            }
            Act::Cloud => {
                self.tenants.panel = Panel::Pick { sel: 0, error: None };
            }
            Act::Chip(i) => {
                self.tenants.hub.show(Some(i));
                self.tenants.panel = Panel::Closed;
                self.tenants.pressed = false;
            }
            Act::CloseTenant => {
                if let Some(i) = self.tenants.hub.active {
                    let name = self.tenants.hub.sessions[i].name.clone();
                    self.tenants.hub.close(i);
                    self.tenants.pressed = false;
                    self.env.audit.record(self.session.as_ref().map_or("-", |s| s.user.as_str()), "cloud.tenant.close", &name);
                }
            }
            Act::Pick(i) => {
                // a tenant already signed in is switched to; a remembered one is signed in to
                match self.pick_entries().get(i) {
                    Some(PickRow { open: Some((k, _)), .. }) => {
                        let k = *k;
                        self.tenants.hub.show(Some(k));
                        self.tenants.panel = Panel::Closed;
                        self.tenants.pressed = false;
                    }
                    Some(row) => {
                        let name = row.name.clone();
                        self.tenant_signin_start(&name);
                    }
                    None => {}
                }
            }
            Act::Forget(i) => match self.tenants.panel {
                Panel::Changed { ref name, .. } => {
                    let name = name.clone();
                    self.tenant_forget(&name);
                }
                _ => {
                    // only a remembered tenant that is not open can be forgotten (an open one is closed first)
                    let name = self.pick_entries().get(i).filter(|r| r.open.is_none()).map(|r| r.name.clone());
                    if let Some(name) = name {
                        self.tenant_forget(&name);
                    }
                }
            },
            Act::New => {
                self.tenants.panel = Panel::Add { fields: vec![TextField::new("Address, like cloud.example.org:443", false), TextField::new("A name for it", false)], focus: 0, error: None };
            }
            Act::Field(i) => match &mut self.tenants.panel {
                Panel::Add { focus, .. } | Panel::Creds { focus, .. } => *focus = i,
                _ => {}
            },
            Act::Next => self.tenant_add_next(),
            Act::Accept => self.tenant_accept(),
            Act::Back => {
                self.tenants.panel = match self.tenants.panel {
                    Panel::Pick { .. } => Panel::Closed,
                    _ => Panel::Pick { sel: 0, error: None },
                };
                self.tenants.busy = None;
            }
            Act::Submit => self.tenant_connect(),
        }
    }

    fn tenant_forget(&mut self, name: &str) {
        self.tenant_list().remove(name);
        let _ = self.tenant_save();
        self.tenants.panel = Panel::Pick { sel: 0, error: None };
    }

    fn tenant_add_next(&mut self) {
        let Panel::Add { fields, .. } = &self.tenants.panel else { return };
        let (addr, name) = (fields[0].value.clone(), fields[1].value.clone());
        let checked = ct::normalise_addr(&addr).and_then(|a| ct::valid_name(&name).map(|n| (a, n)));
        let checked = checked.and_then(|(a, n)| {
            let l = self.tenant_list();
            if l.get(&n).is_some() {
                Err(format!("There is already a tenant called {n}"))
            } else if l.list().iter().any(|t| t.addr == a) {
                Err("That address is already in the list".to_string())
            } else {
                Ok((a, n))
            }
        });
        match checked {
            Ok((addr, name)) => self.tenant_probe(Purpose::Add { name, addr: addr.clone() }, addr),
            Err(e) => {
                if let Panel::Add { error, .. } = &mut self.tenants.panel {
                    *error = Some(e);
                }
            }
        }
    }

    fn tenant_accept(&mut self) {
        let Panel::Trust { name, addr, shown } = &self.tenants.panel else { return };
        let (name, addr, pin) = (name.clone(), addr.clone(), shown.pin);
        let added = self.tenant_list().add(&name, &addr, pin).map(|t| t.clone());
        match added {
            Ok(t) => {
                let saved = self.tenant_save();
                self.env.audit.record(self.session.as_ref().map_or("-", |s| s.user.as_str()), "cloud.tenant.trusted", &format!("{name} {}", pin.hex()));
                self.tenants.panel = Self::tenant_creds_panel(t);
                if let (Err(e), Panel::Creds { error, .. }) = (saved, &mut self.tenants.panel) {
                    *error = Some(format!("The tenant could not be remembered: {e}"));
                }
            }
            Err(e) => self.tenants.panel = Panel::Add { fields: vec![TextField::new("Address, like cloud.example.org:443", false), TextField::new("A name for it", false)], focus: 0, error: Some(e) },
        }
    }

    // ------------------------------------------------------------------ events

    /// First look at an event: the tenant dialogs, the strip and the shown tenant's window take what is theirs. True when it is used up.
    pub(super) fn tenant_intercept(&mut self, ev: &Event) -> bool {
        if matches!(ev, Event::Tick) {
            self.tenants_tick();
            return false;
        }
        if !matches!(self.tenants.panel, Panel::Closed) {
            self.tenant_panel_event(ev);
            self.stale = true;
            return true;
        }
        if let Event::MouseDown(x, y) = ev {
            let hit = self.tenants.rects.iter().rev().find(|(r, a)| r.contains(*x, *y) && matches!(a, Act::Machine | Act::Cloud | Act::Chip(_) | Act::CloseTenant)).map(|(_, a)| a.clone());
            if let Some(a) = hit {
                self.tenant_act(a);
                return true;
            }
        }
        // Ctrl+Shift+J: the next view (Personal, then each tenant); with no tenant open it offers the Cloud dialog
        if matches!(ev, Event::Key(Key::Ctrl('J'))) && (self.session.is_some() || self.tenant_strip_visible()) && self.screen_allows_tenants() {
            if self.tenants.hub.sessions.is_empty() {
                self.open_cloud_dialog();
            } else {
                self.tenants.hub.cycle();
                self.tenants.pressed = false;
            }
            self.stale = true;
            return true;
        }
        // the shell's own popups (the IYAKKI menu, Devices, the installer list) and its function keys are not the tenant's
        let shell_popup = self.session.is_some() && (self.brand.open || self.devpop.open || self.installs.open);
        let shell_key = matches!(ev, Event::Key(Key::F(1 | 8 | 9 | 10 | 11 | 12)));
        if self.tenants.hub.active.is_some() && !shell_popup && !shell_key {
            return self.tenant_forward(ev);
        }
        false
    }

    fn screen_allows_tenants(&self) -> bool {
        !matches!(self.screen, Screen::HwCheck(_) | Screen::Setup(_))
    }

    /// The event for the shown tenant. Only the shown one gets anything.
    fn tenant_forward(&mut self, ev: &Event) -> bool {
        let area = self.tenants.area;
        let dest = self.tenants.dest;
        let pic = self.tenants.hub.current().and_then(|s| s.size());
        let map = |x: i32, y: i32| pic.and_then(|p| cs::to_picture(dest, p, x, y));
        let send = |hub: &Hub, i: Input| {
            hub.send(i);
        };
        match ev {
            Event::MouseMove(x, y) => {
                if self.tenants.pressed || area.contains(*x, *y) {
                    if let Some((px, py)) = map(*x, *y) {
                        send(&self.tenants.hub, Input::MouseMove { x: px, y: py });
                    }
                    return true;
                }
                false
            }
            Event::MouseDown(x, y) if area.contains(*x, *y) => {
                if let Some((px, py)) = map(*x, *y) {
                    self.tenants.pressed = true;
                    send(&self.tenants.hub, Input::MouseDown { x: px, y: py, button: 0 });
                }
                true
            }
            Event::MouseUp(x, y) if self.tenants.pressed => {
                self.tenants.pressed = false;
                let (px, py) = map(*x, *y).unwrap_or((0, 0));
                send(&self.tenants.hub, Input::MouseUp { x: px, y: py, button: 0 });
                true
            }
            Event::MouseRight(x, y) if area.contains(*x, *y) => {
                if let Some((px, py)) = map(*x, *y) {
                    send(&self.tenants.hub, Input::MouseDown { x: px, y: py, button: 2 });
                    send(&self.tenants.hub, Input::MouseUp { x: px, y: py, button: 2 });
                }
                true
            }
            Event::Scroll(d) if area.contains(self.mouse.0, self.mouse.1) => {
                if let Some((px, py)) = map(self.mouse.0, self.mouse.1) {
                    // positive = up on the shell's side; the page's wheel counts down as positive
                    send(&self.tenants.hub, Input::Wheel { x: px, y: py, dx: 0, dy: -d * 120 });
                }
                true
            }
            Event::Key(Key::Ctrl('v')) | Event::Key(Key::Ctrl('V')) => {
                if let Some(t) = crate::ui::clipboard::latest().filter(|t| !t.is_empty()) {
                    send(&self.tenants.hub, Input::Paste(t));
                }
                true
            }
            Event::Key(k) => {
                if let Some(i) = cs::key_input(k) {
                    send(&self.tenants.hub, i);
                }
                true
            }
            Event::Paste(t) => {
                send(&self.tenants.hub, Input::Paste(t.clone()));
                true
            }
            _ => false,
        }
    }

    fn tenant_panel_event(&mut self, ev: &Event) {
        // a job is running: only Esc (cancel) is heard
        if self.tenants.busy.is_some() {
            if matches!(ev, Event::Key(Key::Esc)) {
                self.tenants.busy = None;
            }
            return;
        }
        if let Event::MouseDown(x, y) = ev {
            let hit = self.tenants.rects.iter().rev().find(|(r, _)| r.contains(*x, *y)).cloned();
            if let Some((r, a)) = hit {
                if let Act::Field(i) = a {
                    if let Panel::Add { fields, .. } | Panel::Creds { fields, .. } = &mut self.tenants.panel {
                        if let Some(f) = fields.get_mut(i) {
                            f.click(*x as f32 - r.x);
                        }
                    }
                }
                self.tenant_act(a);
            }
            return;
        }
        let (Event::Key(_) | Event::Paste(_)) = ev else { return };
        let n = self.pick_entries().len();
        match &mut self.tenants.panel {
            Panel::Closed => {}
            Panel::Pick { sel, .. } => {
                match ev {
                    Event::Key(Key::Up) => *sel = sel.saturating_sub(1),
                    Event::Key(Key::Down) => *sel = (*sel + 1).min(n.saturating_sub(1)),
                    Event::Key(Key::Enter) if n > 0 => {
                        let i = *sel;
                        self.tenant_act(Act::Pick(i));
                    }
                    Event::Key(Key::Char('n')) | Event::Key(Key::Char('a')) => self.tenant_act(Act::New),
                    Event::Key(Key::Esc) => self.tenant_act(Act::Back),
                    _ => {}
                }
            }
            Panel::Add { .. } | Panel::Creds { .. } => {
                let is_add = matches!(self.tenants.panel, Panel::Add { .. });
                let (Panel::Add { fields, focus, error } | Panel::Creds { fields, focus, error, .. }) = &mut self.tenants.panel else { return };
                match ev {
                    Event::Key(Key::Esc) => self.tenant_act(Act::Back),
                    Event::Key(Key::Tab) | Event::Key(Key::Down) => *focus = (*focus + 1) % fields.len(),
                    Event::Key(Key::BackTab) | Event::Key(Key::Up) => *focus = (*focus + fields.len() - 1) % fields.len(),
                    Event::Key(Key::Enter) => {
                        if *focus + 1 < fields.len() {
                            *focus += 1;
                        } else if is_add {
                            self.tenant_act(Act::Next);
                        } else {
                            self.tenant_act(Act::Submit);
                        }
                    }
                    Event::Key(k) => {
                        *error = None;
                        fields[*focus].handle(k);
                    }
                    Event::Paste(t) => {
                        *error = None;
                        fields[*focus].paste(t);
                    }
                    _ => {}
                }
            }
            Panel::Trust { .. } => match ev {
                // the certificate is accepted only by a deliberate press: the button, or the letter y (never Enter)
                Event::Key(Key::Char('y')) => self.tenant_act(Act::Accept),
                Event::Key(Key::Esc) | Event::Key(Key::Char('n')) => self.tenant_act(Act::Back),
                _ => {}
            },
            Panel::Changed { .. } => {
                if matches!(ev, Event::Key(Key::Esc | Key::Enter)) {
                    self.tenant_act(Act::Back);
                }
            }
        }
    }

    // ------------------------------------------------------------------ drawing

    /// Draw the shown tenant, the open dialog and the strip, over whatever the screen drew. Called late in the frame.
    pub(super) fn draw_tenants(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        self.tenants.rects.clear();
        self.tenants.area = R::new(0.0, 0.0, 0.0, 0.0);
        if !self.screen_allows_tenants() && self.tenants.hub.sessions.is_empty() {
            return;
        }
        let area = self.tenant_area(w, h, s);
        if self.tenants.hub.active.is_some() {
            self.draw_tenant_desktop(g, area, s);
        } else {
            self.tenants.pressed = false;
        }
        if !matches!(self.tenants.panel, Panel::Closed) {
            self.draw_tenant_panel(g, w, s, area);
        }
        if self.tenant_strip_visible() {
            self.draw_tenant_strip(g, w, h, s);
        }
    }

    fn draw_tenant_desktop(&mut self, g: &mut Gfx, area: R, s: f32) {
        g.hits.retain(|hit| !overlaps(hit.rect, area));
        g.rect(area.x, area.y, area.w, area.h, [8, 10, 16, 255]);
        self.tenants.area = area;
        let Some(sess) = self.tenants.hub.current_mut() else { return };
        let status = sess.status();
        let pic = sess.picture();
        sess.drawn = sess.generation();
        let mut dest = (area.x, area.y, 0.0, 0.0);
        if let Some(p) = &pic {
            dest = cs::fit((area.x, area.y, area.w, area.h), (p.w, p.h));
            blit_scaled(g, p, dest);
        }
        let name = sess.name.clone();
        self.tenants.dest = dest;
        // a status line over the picture when it is not live
        let note = match &status {
            Status::Live => None,
            Status::Connecting => Some(format!("Connecting to {name}")),
            Status::Reconnecting => Some(format!("{name}: the link dropped, trying again")),
            Status::Refused(why) => Some(format!("{name}: refused ({why})")),
            Status::Closed(why) => Some(format!("{name}: closed ({why}). Close this tenant or sign in again from Cloud")),
        };
        if let Some(n) = note {
            let size = 15.0 * s;
            let n = g.fit(size, &n, Face::Bold, area.w - 40.0 * s);
            let tw = g.text_width(size, &n, Face::Bold);
            let (bw, bh) = (tw + 28.0 * s, size * 2.4);
            let (bx, by) = (area.x + (area.w - bw) / 2.0, area.y + 12.0 * s);
            g.rrect(bx, by, bw, bh, bh / 2.0, [14, 18, 32, 235]);
            g.text(bx + 14.0 * s, by + (bh - g.line_height(size, Face::Bold)) / 2.0, size, if matches!(status, Status::Reconnecting | Status::Connecting) { crate::fx::AMBER } else { ERR }, &n, Face::Bold);
        }
        g.hit(area, Event::Tab(DESKTOP_FIELD), "tenant desktop");
        if matches!(status, Status::Live) {
            // the picture takes typed text: the on-screen keyboard offers itself over it
            crate::ctxmenu::note_field(crate::ctxmenu::FieldNote { rect: area, id: DESKTOP_FIELD, focused: true, has_selection: false, empty: false, secret: false });
        }
    }

    fn draw_tenant_strip(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        let sh = STRIP_H * s;
        let strip = R::new(0.0, self.tenant_strip_y(h, s), w as f32, sh);
        let with_tenants = !self.tenants.hub.sessions.is_empty();
        if with_tenants {
            g.hits.retain(|hit| !overlaps(hit.rect, strip));
            g.rect(strip.x, strip.y, strip.w, strip.h, rgb(12, 15, 28));
            g.rect(strip.x, strip.y, strip.w, 1.0, rgb(52, 62, 96));
        }
        let size = 13.0 * s;
        let (ch, gap) = (sh - 8.0 * s, 6.0 * s);
        let on_login = self.session.is_none() && matches!(self.screen, Screen::Login(_));
        let panel_open = !matches!(self.tenants.panel, Panel::Closed);
        let personal = if on_login { "This machine" } else { "Personal" };
        // (label, dot, active, act), laid out left to right
        let mut chips: Vec<(String, Option<Dot>, bool, Act)> = vec![(personal.to_string(), None, self.tenants.hub.active.is_none() && !panel_open, Act::Machine)];
        let n = self.tenants.hub.sessions.len();
        let name_max = if n == 0 { 0.0 } else { ((w as f32 - 24.0 * s - 280.0 * s) / n as f32 - 40.0 * s).clamp(40.0 * s, 160.0 * s) };
        for (i, sess) in self.tenants.hub.sessions.iter().enumerate() {
            let label = g.fit(size, &sess.name, Face::Bold, name_max);
            chips.push((label, Some(sess.dot()), self.tenants.hub.active == Some(i) && !panel_open, Act::Chip(i)));
        }
        chips.push((if n == 0 { "Cloud".into() } else { "+".into() }, None, panel_open, Act::Cloud));
        let widths: Vec<f32> = chips.iter().map(|(l, d, _, _)| g.text_width(size, l, Face::Bold) + 24.0 * s + if d.is_some() { 16.0 * s } else { 0.0 }).collect();
        let total: f32 = widths.iter().sum::<f32>() + gap * (chips.len() as f32 - 1.0);
        let close_w = if self.tenants.hub.active.is_some() { g.text_width(size, "Close tenant", Face::Bold) + 24.0 * s } else { 0.0 };
        // the on-screen keyboard's button sits at the bottom right: the chips keep to its left
        let edge = (crate::osk::Osk::button_rect(w as f32, h as f32, s).x - 8.0 * s).min(w as f32 - 10.0 * s);
        let mut x = if with_tenants { 10.0 * s } else { (edge - total).max(8.0 * s) };
        let y = strip.y + 4.0 * s;
        let right_limit = edge - if close_w > 0.0 { close_w + gap } else { 0.0 };
        for ((label, dot, active, act), cw) in chips.into_iter().zip(widths) {
            if x + cw > right_limit {
                break;
            }
            let r = R::new(x, y, cw, ch);
            let hot = g.hovered(r);
            g.rrect(r.x, r.y, r.w, r.h, ch / 2.0, if active { rgb(46, 92, 170) } else if hot { rgb(40, 49, 80) } else { rgb(24, 30, 52) });
            g.rrect_outline(r.x, r.y, r.w, r.h, ch / 2.0, 1.0, if active { rgb(120, 170, 255) } else { rgb(52, 62, 96) });
            let mut tx = r.x + 12.0 * s;
            if let Some(d) = dot {
                let c = match d {
                    Dot::Live => OK,
                    Dot::Reconnecting => crate::fx::AMBER,
                    Dot::Closed => ERR,
                };
                g.circle(tx + 4.0 * s, r.y + r.h / 2.0, 4.0 * s, c);
                tx += 16.0 * s;
            }
            g.text(tx, r.y + (r.h - g.line_height(size, Face::Bold)) / 2.0, size, TEXT, &label, Face::Bold);
            let hit_label = match (&act, dot) {
                (Act::Chip(_), Some(d)) => format!("tenant {} ({})", label, cs::dot_word(d)),
                (Act::Machine, _) => personal.to_lowercase(),
                (Act::Cloud, _) => "cloud".to_string(),
                _ => label.clone(),
            };
            g.hit(r, Event::Tab(DESKTOP_FIELD + 1), &hit_label);
            self.tenants.rects.push((r, act));
            x += cw + gap;
        }
        if close_w > 0.0 {
            let r = R::new(edge - close_w, y, close_w, ch);
            let hot = g.hovered(r);
            g.rrect(r.x, r.y, r.w, r.h, ch / 2.0, if hot { rgb(90, 36, 46) } else { rgb(40, 24, 34) });
            g.rrect_outline(r.x, r.y, r.w, r.h, ch / 2.0, 1.0, rgb(160, 70, 84));
            g.text(r.x + 12.0 * s, r.y + (r.h - g.line_height(size, Face::Bold)) / 2.0, size, TEXT, "Close tenant", Face::Bold);
            g.hit(r, Event::Tab(DESKTOP_FIELD + 1), "close tenant");
            self.tenants.rects.push((r, Act::CloseTenant));
        }
    }

    fn draw_tenant_panel(&mut self, g: &mut Gfx, w: u32, s: f32, area: R) {
        // the dialog covers the screen below the top bar
        g.hits.retain(|hit| !overlaps(hit.rect, area));
        let on_login = self.session.is_none();
        if on_login {
            g.clipped(area, |g| crate::fx::background(g));
        } else {
            g.rect(area.x, area.y, area.w, area.h, [0, 0, 0, 215]);
        }
        let cw = (540.0 * s).min(w as f32 - 24.0 * s);
        let pad = 22.0 * s;
        let inner = cw - 2.0 * pad;
        let size = 14.0 * s;
        let lh = g.line_height(size, Face::Sans) * 1.25;
        let tick = self.tick;
        let busy = self.tenants.busy.as_ref().map(|(t, _)| t.clone());
        // content as a list of blocks, so the card's height is known before it is drawn
        enum B {
            Title(String),
            Text(String, [u8; 4]),
            Mono(String),
            Field(usize),
            Row(usize, String, String, bool, Option<Dot>),
            Btns(Vec<(String, Act, [u8; 4])>),
        }
        let mut blocks: Vec<B> = Vec::new();
        let mut danger = false;
        let mut focus_idx = None;
        let mut field_refs: Vec<(usize, &TextField)> = Vec::new();
        let rows: Vec<PickRow> = self.pick_entries();
        let signed_in = self.session.is_some();
        match &self.tenants.panel {
            Panel::Closed => return,
            Panel::Pick { sel, error } => {
                blocks.push(B::Title("Cloud".into()));
                if rows.is_empty() {
                    blocks.push(B::Text("No tenant is remembered yet. Add one with its address; the first time, you check its certificate.".into(), MUTED));
                }
                for (i, r) in rows.iter().enumerate() {
                    blocks.push(B::Row(i, r.name.clone(), r.addr.clone(), i == *sel, r.open.map(|(_, d)| d)));
                }
                if let Some(e) = error {
                    blocks.push(B::Text(e.clone(), ERR));
                }
                let mut btns = vec![("Add a tenant".into(), Act::New, rgb(120, 170, 255))];
                if signed_in {
                    btns.push(("Personal".into(), Act::Machine, rgb(120, 170, 255)));
                }
                btns.push(("Close".into(), Act::Back, MUTED));
                blocks.push(B::Btns(btns));
            }
            Panel::Add { fields, focus, error } => {
                blocks.push(B::Title("Add a tenant".into()));
                blocks.push(B::Text("The address of the tenant's IYAKKI, like cloud.example.org or 10.0.0.7:8443. Its certificate is shown next, for you to check.".into(), MUTED));
                for (i, f) in fields.iter().enumerate() {
                    blocks.push(B::Field(i));
                    field_refs.push((i, f));
                }
                focus_idx = Some(*focus);
                if let Some(e) = error {
                    blocks.push(B::Text(e.clone(), ERR));
                }
                blocks.push(B::Btns(vec![("Check certificate".into(), Act::Next, rgb(120, 170, 255)), ("Back".into(), Act::Back, MUTED)]));
            }
            Panel::Trust { name, addr, shown } => {
                blocks.push(B::Title(format!("Trust {name}?")));
                blocks.push(B::Text(format!("{addr} showed this certificate. Compare the fingerprint with the one the tenant's operator gave you. Accept only if it is the same."), MUTED));
                blocks.push(B::Text("Fingerprint (SHA-256)".into(), DIM));
                blocks.push(B::Mono(shown.pin.display()));
                blocks.push(B::Text(format!("Subject: {}", shown.subject), TEXT));
                blocks.push(B::Text(format!("Valid until: {}", date_of(shown.not_after)), TEXT));
                blocks.push(B::Btns(vec![("Trust this certificate".into(), Act::Accept, OK), ("Cancel".into(), Act::Back, MUTED)]));
            }
            Panel::Changed { name, old, new } => {
                danger = true;
                blocks.push(B::Title(format!("{name}: the certificate changed")));
                blocks.push(B::Text("This is not the certificate you trusted. Someone may be standing between you and the tenant, or the operator replaced it. Nothing was sent, and nothing will be unless you start again.".into(), ERR));
                blocks.push(B::Text("Trusted before".into(), DIM));
                blocks.push(B::Mono(old.display()));
                blocks.push(B::Text("Shown now".into(), DIM));
                blocks.push(B::Mono(new.display()));
                blocks.push(B::Text("If the operator confirms the new fingerprint, forget this tenant and add it again.".into(), MUTED));
                blocks.push(B::Btns(vec![("Back".into(), Act::Back, rgb(120, 170, 255)), ("Forget this tenant".into(), Act::Forget(0), ERR)]));
            }
            Panel::Creds { t, fields, focus, error } => {
                blocks.push(B::Title(format!("Sign in to {}", t.name)));
                blocks.push(B::Text(format!("{} (certificate {}...)", t.addr, &t.pin.display()[..11]), MUTED));
                for (i, f) in fields.iter().enumerate() {
                    blocks.push(B::Field(i));
                    field_refs.push((i, f));
                }
                focus_idx = Some(*focus);
                if let Some(e) = error {
                    blocks.push(B::Text(e.clone(), ERR));
                }
                blocks.push(B::Btns(vec![("Sign in".into(), Act::Submit, rgb(120, 170, 255)), ("Back".into(), Act::Back, MUTED)]));
            }
        }
        if let Some(b) = &busy {
            blocks.push(B::Text(format!("{b}... (Esc cancels)"), crate::fx::AMBER));
        }
        // heights
        let field_h = 46.0 * s;
        let btn_h = size * 2.1;
        let mut texts: Vec<Vec<String>> = Vec::new();
        let mut hsum = pad * 2.0;
        for b in &blocks {
            let (lines, hh) = match b {
                B::Title(_) => (Vec::new(), 34.0 * s),
                B::Text(t, _) => {
                    let l = g.wrap(size, t, Face::Sans, inner);
                    let n = l.len() as f32;
                    (l, n * lh + 6.0 * s)
                }
                B::Mono(m) => {
                    let l = g.wrap(size * 0.92, m, Face::Mono, inner);
                    let n = l.len() as f32;
                    (l, n * lh + 6.0 * s)
                }
                B::Field(_) => (Vec::new(), field_h + 10.0 * s),
                B::Row(..) => (Vec::new(), 44.0 * s),
                B::Btns(_) => (Vec::new(), btn_h + 12.0 * s),
            };
            texts.push(lines);
            hsum += hh;
        }
        let ch = hsum.min(area.h - 8.0 * s).max(60.0 * s);
        let mut card = R::new((w as f32 - cw) / 2.0, area.y + ((area.h - ch) / 2.0).max(4.0 * s), cw, ch);
        // the on-screen keyboard is up: the card moves above it
        if let Some(kb) = self.osk.shown {
            card.y = (kb.y - ch - 6.0 * s).max(area.y + 2.0 * s);
        }
        crate::fx::modal_panel(g, card, s, if danger { ERR } else { crate::fx::CYAN }, danger);
        let mut y = card.y + pad;
        let x = card.x + pad;
        let mut rects: Vec<(R, Act)> = Vec::new();
        for (b, lines) in blocks.iter().zip(texts.iter()) {
            match b {
                B::Title(t) => {
                    let t = g.fit(22.0 * s, t, Face::Bold, inner);
                    g.text(x, y, 22.0 * s, TEXT, &t, Face::Bold);
                    y += 34.0 * s;
                }
                B::Text(_, c) => {
                    for l in lines {
                        g.text(x, y, size, *c, l, Face::Sans);
                        y += lh;
                    }
                    y += 6.0 * s;
                }
                B::Mono(_) => {
                    for l in lines {
                        g.text(x, y, size * 0.92, crate::fx::CYAN, l, Face::Mono);
                        y += lh;
                    }
                    y += 6.0 * s;
                }
                B::Field(i) => {
                    let r = R::new(x, y, inner, field_h);
                    if let Some((_, f)) = field_refs.iter().find(|(k, _)| k == i) {
                        f.render(g, r, focus_idx == Some(*i), s, (tick / 15) % 2 == 0, Face::Sans, FIELD_BASE + i);
                    }
                    rects.push((r, Act::Field(*i)));
                    y += field_h + 10.0 * s;
                }
                B::Row(i, name, addr, sel, open) => {
                    let r = R::new(x, y, inner, 38.0 * s);
                    let hot = g.hovered(r);
                    g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, if *sel { rgb(48, 58, 96) } else if hot { rgb(40, 49, 80) } else { rgb(30, 37, 62) });
                    let fw = if open.is_some() { 0.0 } else { 70.0 * s };
                    let mut tx = r.x + 12.0 * s;
                    if let Some(d) = open {
                        let c = match d {
                            Dot::Live => OK,
                            Dot::Reconnecting => crate::fx::AMBER,
                            Dot::Closed => ERR,
                        };
                        g.circle(tx + 4.0 * s, r.y + r.h / 2.0, 4.0 * s, c);
                        tx += 16.0 * s;
                    }
                    let label = g.fit(size, &format!("{name}   {addr}"), Face::Bold, r.x + r.w - tx - fw - 12.0 * s);
                    g.text(tx, r.y + (r.h - g.line_height(size, Face::Bold)) / 2.0, size, TEXT, &label, Face::Bold);
                    let hit_label = match open {
                        Some(d) => format!("switch to {name} ({})", cs::dot_word(*d)),
                        None => format!("tenant row {name}"),
                    };
                    g.hit(r, Event::Tab(DESKTOP_FIELD + 2), &hit_label);
                    rects.push((r, Act::Pick(*i)));
                    if open.is_none() {
                        let fr = R::new(r.x + r.w - fw - 6.0 * s, r.y + 5.0 * s, fw, r.h - 10.0 * s);
                        g.rrect(fr.x, fr.y, fr.w, fr.h, fr.h / 2.0, rgb(40, 24, 34));
                        let fl = g.text_width(12.0 * s, "Forget", Face::Bold);
                        g.text(fr.x + (fr.w - fl) / 2.0, fr.y + (fr.h - g.line_height(12.0 * s, Face::Bold)) / 2.0, 12.0 * s, ERR, "Forget", Face::Bold);
                        g.hit(fr, Event::Tab(DESKTOP_FIELD + 3), &format!("forget {name}"));
                        // the later rectangle wins: Forget before the row
                        rects.push((fr, Act::Forget(*i)));
                    }
                    y += 44.0 * s;
                }
                B::Btns(btns) => {
                    let mut bx = x;
                    for (label, act, accent) in btns {
                        let bw = g.text_width(size, label, Face::Bold) + size * 1.7;
                        if bx + bw > card.x + card.w - pad + 1.0 {
                            bx = x;
                            y += btn_h + 6.0 * s;
                        }
                        g.button(bx, y, size, label, *accent, Event::Tab(DESKTOP_FIELD + 4));
                        rects.push((R::new(bx, y, bw, btn_h), act.clone()));
                        bx += bw + 10.0 * s;
                    }
                    y += btn_h + 12.0 * s;
                }
            }
        }
        self.tenants.rects.extend(rects);
    }
}

/// Draw `p` into `dest` (x, y, w, h) of the frame, scaled with nearest-neighbour sampling.
fn blit_scaled(g: &mut Gfx, p: &iyakki_agent::cloudclient::Picture, dest: (f32, f32, f32, f32)) {
    let (dx, dy, dw, dh) = (dest.0 as i32, dest.1 as i32, dest.2 as i32, dest.3 as i32);
    if dw <= 0 || dh <= 0 || p.w == 0 || p.h == 0 || p.rgba.len() < (p.w * p.h * 4) as usize {
        return;
    }
    let (gw, gh) = (g.width() as i32, g.height() as i32);
    let dst = g.pm.data_mut();
    let xs: Vec<usize> = (0..dw).map(|x| ((x as u64 * p.w as u64) / dw as u64) as usize).collect();
    for y in 0..dh {
        let gy = dy + y;
        if gy < 0 || gy >= gh {
            continue;
        }
        let sy = ((y as u64 * p.h as u64) / dh as u64) as usize;
        let row = sy * p.w as usize * 4;
        for (x, sx) in xs.iter().enumerate() {
            let gx = dx + x as i32;
            if gx < 0 || gx >= gw {
                continue;
            }
            let (a, b) = (row + sx * 4, ((gy * gw + gx) * 4) as usize);
            if b + 4 <= dst.len() {
                dst[b..b + 4].copy_from_slice(&p.rgba[a..a + 4]);
            }
        }
    }
}

impl Shell {
    /// Which dialog is open: `closed`, `pick`, `add`, `trust`, `changed` or `signin` (tests and tools).
    pub fn tenant_dialog(&self) -> &'static str {
        match self.tenants.panel {
            Panel::Closed => "closed",
            Panel::Pick { .. } => "pick",
            Panel::Add { .. } => "add",
            Panel::Trust { .. } => "trust",
            Panel::Changed { .. } => "changed",
            Panel::Creds { .. } => "signin",
        }
    }

    /// The error line of the open dialog, if any (tests and tools).
    pub fn tenant_dialog_error(&self) -> Option<String> {
        match &self.tenants.panel {
            Panel::Pick { error, .. } | Panel::Add { error, .. } | Panel::Creds { error, .. } => error.clone(),
            _ => None,
        }
    }

    /// The fingerprints the open dialog shows, in SHA-256 groups: the certificate offered for trust (one), or the one trusted before and the
    /// one shown now when it changed (two). Empty for the other dialogs (tests and tools).
    pub fn tenant_dialog_fingerprints(&self) -> Vec<String> {
        match &self.tenants.panel {
            Panel::Trust { shown, .. } => vec![shown.pin.display()],
            Panel::Changed { old, new, .. } => vec![old.display(), new.display()],
            _ => Vec::new(),
        }
    }
}
