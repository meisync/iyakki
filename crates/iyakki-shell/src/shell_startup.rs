//! What is still starting, on the screen: a quiet strip on the Home screen, a list that opens from it, and a one-line progress on the
//! sign-in screen. The facts come from the startup registry (`iyakki_agent::startup`); this file polls it cheaply, draws it, and
//! turns the clicks and keys into Retry and Open.
//!
//! * Home (main display only): a pill at the bottom left, above the host line and left of the page dots: a spinner and the most
//!   important line ("Starting Dev cluster (k3s): waiting for the clock  +2 more"), or a red dot and the failure. Everything ready:
//!   "All services ready" for three seconds, then it fades and is gone until something starts again. A click, or the key `t`,
//!   opens the list: one line per service that is not ready, with Retry and Open for an administrator (Up/Down choose, `r` retry,
//!   Enter or `o` open, Esc or `t` close). Settings -> Startup shows all of them.
//! * Sign-in and first-run screens: one line under the card with the system-wide services (network, clock, cluster, firewall).
//! * An administrator reads the steps and the reasons; everybody else reads names and steps, and "did not start" without the reason.
//! * Nothing here blocks: the poll reads small files and one cached snapshot, about once a second while something is starting and
//!   once every five seconds after that. A failure is written to the audit log once, when it begins.
//!
//! Developer mode: the strip and the list are drawn with ordinary text, so `iyakki dev state` shows them; the strip is also a button
//! called "startup status" for `iyakki dev input click`.

use super::*;
use iyakki_agent::startup::{self, Calm, Line, Phase, Registry};

/// Events of the strip and the list (`Event::Tab`).
pub(super) const TAB_STRIP: usize = 0x4D40;
pub(super) const TAB_CLOSE: usize = 0x4D41;
pub(super) const TAB_PANEL: usize = 0x4D42;
const TAB_RETRY: usize = 0x4D50;
const TAB_OPEN: usize = 0x4D60;
/// Lines the list can show.
const MAX_ROWS: usize = 12;

pub(super) fn is_tab(n: usize) -> bool {
    (TAB_STRIP..TAB_OPEN + MAX_ROWS).contains(&n)
}

/// Heartbeats between two polls: one second while something starts, five once all is calm.
const POLL_BUSY: u64 = 30;
const POLL_CALM: u64 = 150;

pub(super) struct StartupUi {
    pub reg: Arc<Registry>,
    /// A probe thread is running (see `startup_tick`).
    pub polling: Arc<std::sync::atomic::AtomicBool>,
    pub open: bool,
    /// The chosen line of the list.
    pub sel: usize,
    calm: Calm,
    calm_sys: Calm,
    pub phase: Phase,
    pub phase_sys: Phase,
    /// What the last frame showed, to know when to draw again.
    shown: String,
    /// The lines of the list as last drawn.
    pub lines: Vec<Line>,
    /// Where things were drawn in the last frame (tests and hit-testing).
    pub strip: Option<R>,
    pub panel: Option<R>,
    pub login_line: Option<R>,
    pub buttons: Vec<(usize, R)>,
}

impl StartupUi {
    pub(super) fn new() -> StartupUi {
        let reg = if cfg!(test) {
            // a test shell starts with everything ready, so no existing screen has a strip; tests that want one bring their own
            let r = Arc::new(Registry::new(|| 0));
            for d in startup::CATALOGUE.iter() {
                r.ready(d.id);
            }
            r
        } else {
            startup::global().clone()
        };
        StartupUi { reg, polling: Arc::new(std::sync::atomic::AtomicBool::new(false)), open: false, sel: 0, calm: Calm::default(), calm_sys: Calm::default(), phase: Phase::Gone, phase_sys: Phase::Gone, shown: String::new(), lines: Vec::new(), strip: None, panel: None, login_line: None, buttons: Vec::new() }
    }
}

// ------------------------------------------------------------------------------------------ layout (pure, tested)

/// The strip: bottom left, one row above the host line, ending before the page dots (centred) so the two never meet. `pages` is the
/// number of launcher pages. `None` when the room is too small to be readable.
pub(super) fn strip_rect(w: u32, h: u32, s: f32, pages: usize) -> Option<R> {
    let x = 40.0 * s;
    let nav_half = if pages > 1 { (26.0 * (pages as f32 + 2.0) + 8.0 * (pages as f32 + 1.0)) * s / 2.0 } else { 0.0 };
    let right = (w as f32 / 2.0 - nav_half - 16.0 * s).min(40.0 * s + 560.0 * s);
    let width = right - x;
    (width >= 150.0 * s).then(|| R::new(x, h as f32 - 66.0 * s, width, 26.0 * s))
}

/// The list above the strip: its panel, and for line `i` the line's rectangle and the Retry and Open buttons (when wanted).
pub(super) struct PopLayout {
    pub panel: R,
    pub rows: Vec<RowRects>,
    /// Lines that did not fit.
    pub more: usize,
}

pub(super) struct RowRects {
    pub line: R,
    pub retry: Option<R>,
    pub open: Option<R>,
    /// Where the text may go (left of the buttons).
    pub text_w: f32,
}

pub(super) fn pop_layout(w: u32, _h: u32, s: f32, strip: R, wants: &[(bool, bool)]) -> PopLayout {
    let pw = (520.0 * s).min(w as f32 - 24.0 * s);
    let x = strip.x.min(w as f32 - pw - 12.0 * s).max(12.0 * s);
    let (head, foot, row_h) = (40.0 * s, 30.0 * s, 32.0 * s);
    let bottom = strip.y - 8.0 * s;
    let top_limit = 96.0 * s;
    let room = (bottom - top_limit - head - foot).max(row_h);
    let fit = ((room / row_h) as usize).clamp(1, MAX_ROWS);
    let shown = wants.len().min(fit).max(1);
    let ph = head + foot + row_h * shown as f32;
    let panel = R::new(x, bottom - ph, pw, ph);
    let (bw_retry, bw_open) = (66.0 * s, 56.0 * s);
    let rows = (0..wants.len().min(fit))
        .map(|i| {
            let y = panel.y + head + i as f32 * row_h;
            let line = R::new(panel.x + 8.0 * s, y, pw - 16.0 * s, row_h - 2.0 * s);
            let (retry, open) = wants[i];
            let by = y + 3.0 * s;
            let mut right = line.x + line.w - 6.0 * s;
            let open_r = open.then(|| {
                right -= bw_open;
                let r = R::new(right, by, bw_open, row_h - 8.0 * s);
                right -= 6.0 * s;
                r
            });
            let retry_r = retry.then(|| {
                right -= bw_retry;
                let r = R::new(right, by, bw_retry, row_h - 8.0 * s);
                right -= 6.0 * s;
                r
            });
            RowRects { line, retry: retry_r, open: open_r, text_w: (right - (line.x + 30.0 * s)).max(40.0 * s) }
        })
        .collect();
    PopLayout { panel, rows, more: wants.len().saturating_sub(fit) }
}

/// The sign-in screen's line: centred under the card, on its own row above the host line, and never off the screen.
pub(super) fn login_rect(w: u32, h: u32, s: f32, card: R, volatile: bool) -> R {
    let lh = 22.0 * s;
    let below = card.y + card.h + if volatile { 46.0 * s } else { 16.0 * s };
    let y = below.min(h as f32 - 34.0 * s - lh - 6.0 * s).max(card.y + card.h + 4.0 * s).min(h as f32 - lh - 4.0 * s);
    let width = (w as f32 - 48.0 * s).min(760.0 * s);
    R::new((w as f32 - width) / 2.0, y, width, lh)
}

fn faded(c: [u8; 4], alpha: u8) -> [u8; 4] {
    [c[0], c[1], c[2], (c[3] as u32 * alpha as u32 / 255) as u8]
}

/// Eight dots round a centre; the bright one moves with the heartbeat.
fn spinner(g: &mut Gfx, cx: f32, cy: f32, r: f32, tick: u64, c: [u8; 4], alpha: u8) {
    let lead = (tick / 3) % 8;
    for k in 0..8u64 {
        let a = k as f32 * std::f32::consts::TAU / 8.0;
        let age = (k + 8 - lead) % 8;
        let v = (255 - age * 28).max(50) as u8;
        g.circle(cx + a.cos() * r, cy + a.sin() * r, r * 0.22, faded([c[0], c[1], c[2], v], alpha));
    }
}

impl Shell {
    fn startup_admin(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.policy.manage_settings)
    }

    /// Each heartbeat: poll now and then, write the audit line of a new failure, move the "all ready" fade, and ask for a picture when
    /// what is shown changed.
    pub(super) fn startup_tick(&mut self) {
        let calm = matches!(self.startup.phase, Phase::Gone) && matches!(self.startup.phase_sys, Phase::Gone);
        if !cfg!(test) && self.tick % if calm { POLL_CALM } else { POLL_BUSY } == 1 {
            // the assistant lives in this process: ready as soon as the shell beats
            self.startup.reg.ready("agent");
            // the probe reads the cluster, the clock, the network and more, and some of those wait for locks or programs: it runs on a
            // thread of its own (one at a time), so typing on the sign-in form never waits for the start-up sequence
            if !self.startup.polling.swap(true, std::sync::atomic::Ordering::SeqCst) {
                let (svc, reg, flag) = (self.env.services.clone(), self.startup.reg.clone(), self.startup.polling.clone());
                let spawned = std::thread::Builder::new().name("iyakki-startup-probe".into()).spawn(move || {
                    let alert = iyakki_fw::service::alert();
                    iyakki_agent::startup_probe::poll(&svc, &reg, alert);
                    flag.store(false, std::sync::atomic::Ordering::SeqCst);
                });
                if spawned.is_err() {
                    self.startup.polling.store(false, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }
        for f in self.startup.reg.take_failures() {
            // one line per transition to failed
            let why = iyakki_agent::devmode::debug::scrub(&f.reason);
            self.env.audit.record("system", "startup.failed", &format!("{}: {}", f.name, why));
        }
        let sum = self.startup.reg.summary();
        let busy = !sum.all_ready();
        let busy_sys = !sum.system_only().all_ready();
        let ms = self.tick * 1000 / 30;
        let (p, ps) = (self.startup.calm.update(busy, ms), self.startup.calm_sys.update(busy_sys, ms));
        let key = format!("{p:?}|{ps:?}|{}|{}", sum.strip(self.startup_admin()), sum.rows.iter().map(|r| r.step.as_str()).collect::<Vec<_>>().join(","));
        let animated = matches!(p, Phase::Busy) || matches!(ps, Phase::Busy);
        let fading = matches!(p, Phase::Done(a) if a < 255) || matches!(ps, Phase::Done(a) if a < 255);
        let visible = self.session.is_some() && matches!(self.screen, Screen::Launcher) || self.on_auth_screen();
        self.startup.phase = p;
        self.startup.phase_sys = ps;
        if key != self.startup.shown || (visible && ((animated && self.tick % 4 == 0) || (fading && self.tick % 2 == 0))) {
            self.startup.shown = key;
            if visible || self.startup.open {
                self.stale = true;
            }
        }
    }

    /// Home: the strip (main display only) and the list when it is open.
    pub(super) fn draw_startup_home(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        self.startup.strip = None;
        self.startup.panel = None;
        self.startup.buttons.clear();
        if !matches!(self.screen, Screen::Launcher) || self.session.is_none() || self.panes.compact || self.panes_drawing_display() != 0 {
            self.startup.open = false;
            return;
        }
        let admin = self.startup_admin();
        let n = self.group_items().len();
        let pages = launcher::page_count(n, launcher::per_page(w, h, s));
        let Some(strip) = strip_rect(w, h, s, pages) else { return };
        let sum = self.startup.reg.summary();
        let tick = self.tick;
        let phase = self.startup.phase;
        if !matches!(phase, Phase::Gone) {
            let (alpha, failed) = match phase {
                Phase::Done(a) => (a, false),
                _ => (255, sum.failed() > 0),
            };
            let accent = if failed { ERR } else if matches!(phase, Phase::Done(_)) { OK } else { crate::fx::AMBER };
            g.rrect(strip.x, strip.y, strip.w, strip.h, strip.h / 2.0, faded([18, 24, 46, 230], alpha));
            g.rrect_outline(strip.x, strip.y, strip.w, strip.h, strip.h / 2.0, s.max(1.0), faded([accent[0], accent[1], accent[2], 120], alpha));
            let (cx, cy) = (strip.x + 15.0 * s, strip.y + strip.h / 2.0);
            let text = match phase {
                Phase::Done(_) => "All services ready".to_string(),
                _ => sum.strip(admin),
            };
            match phase {
                Phase::Done(_) => g.circle(cx, cy, 5.0 * s, faded(OK, alpha)),
                _ if failed => {
                    g.circle(cx, cy, 6.0 * s, ERR);
                    g.text(cx - 2.5 * s, cy - 7.0 * s, 12.0 * s, rgb(20, 10, 12), "!", Face::Bold);
                }
                _ => spinner(g, cx, cy, 7.0 * s, tick, accent, alpha),
            }
            let text = g.fit(14.0 * s, &text, Face::Sans, strip.w - 40.0 * s);
            g.text(strip.x + 30.0 * s, strip.y + (strip.h - 14.0 * s) / 2.0 - s, 14.0 * s, faded(if failed { ERR } else { TEXT }, alpha), &text, Face::Sans);
            g.hit(strip, Event::Tab(TAB_STRIP), "startup status");
            self.startup.strip = Some(strip);
        }
        if self.startup.open {
            self.draw_startup_list(g, w, h, s, strip, &sum, admin);
        }
    }

    fn draw_startup_list(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32, strip: R, sum: &startup::Summary, admin: bool) {
        let lines = sum.lines(admin);
        let wants: Vec<(bool, bool)> = lines.iter().map(|l| (l.actions.retry, l.actions.open.item().is_some())).collect();
        let lay = pop_layout(w, h, s, strip, &wants);
        let p = lay.panel;
        crate::fx::modal_panel(g, p, s, crate::fx::CYAN, false);
        g.hit(p, Event::Tab(TAB_PANEL), "startup list");
        let head = if sum.all_ready() { format!("Startup: all {} services ready", sum.ready) } else { format!("Startup: {} of {} ready", sum.ready, sum.tracked) };
        g.text(p.x + 18.0 * s, p.y + 11.0 * s, 16.0 * s, TEXT, &head, Face::Bold);
        self.startup.sel = self.startup.sel.min(lay.rows.len().saturating_sub(1));
        if lines.is_empty() {
            g.text(p.x + 18.0 * s, p.y + 48.0 * s, 14.0 * s, MUTED, "Everything that starts at boot is running.", Face::Sans);
        }
        let tick = self.tick;
        for (i, rr) in lay.rows.iter().enumerate() {
            let l = &lines[i];
            let hot = g.hovered(rr.line);
            crate::fx::row_at(g, rr.line.x, rr.line.y, rr.line.w, rr.line.h, 8.0 * s, i == self.startup.sel, hot, crate::fx::CYAN);
            let (cx, cy) = (rr.line.x + 14.0 * s, rr.line.y + rr.line.h / 2.0);
            if l.failed {
                g.circle(cx, cy, 5.0 * s, ERR);
            } else if l.starting {
                spinner(g, cx, cy, 6.0 * s, tick, crate::fx::AMBER, 255);
            } else {
                g.circle(cx, cy, 4.0 * s, DIM);
            }
            let t = g.fit(14.0 * s, &l.text, Face::Sans, rr.text_w);
            g.text(rr.line.x + 30.0 * s, cy - 8.0 * s, 14.0 * s, if l.failed { ERR } else { TEXT }, &t, Face::Sans);
            g.hit(rr.line, Event::Row(i), &l.text);
            for (r, label, tab, col) in [(rr.retry, "Retry", TAB_RETRY + i, OK), (rr.open, "Open", TAB_OPEN + i, crate::fx::CYAN)] {
                if let Some(r) = r {
                    let hot = g.hovered(r);
                    let tc = crate::fx::pill(g, r, s, col, hot, true);
                    let tw = g.text_width(13.0 * s, label, Face::Bold);
                    g.text(r.x + (r.w - tw) / 2.0, r.y + (r.h - 13.0 * s) / 2.0 - s, 13.0 * s, tc, label, Face::Bold);
                    g.hit(r, Event::Tab(tab), &format!("{label} {}", l.text.split(':').next().unwrap_or("")));
                    self.startup.buttons.push((tab, r));
                }
            }
        }
        let foot = if lay.more > 0 { format!("{} more not shown · Esc closes", lay.more) } else if admin { "Up/Down choose · r retry · Enter open · Esc closes".to_string() } else { "Esc closes".to_string() };
        let foot = g.fit(12.0 * s, &foot, Face::Sans, p.w - 36.0 * s);
        g.text(p.x + 18.0 * s, p.y + p.h - 22.0 * s, 12.0 * s, DIM, &foot, Face::Sans);
        self.startup.panel = Some(p);
        self.startup.lines = lines;
    }

    /// The sign-in and first-run screens: the system-wide services, one line.
    pub(super) fn draw_startup_login(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        self.startup.login_line = None;
        if self.session.is_some() || !self.on_auth_screen() || self.lbar.remote || self.panes_drawing_display() != 0 {
            return;
        }
        let phase = self.startup.phase_sys;
        if matches!(phase, Phase::Gone) {
            return;
        }
        let view = startup::login_view(self.startup.reg.snapshot());
        let (ready, total, pending, wifi_bg) = (view.ready, view.total, view.pending, view.wifi_background);
        let n_fields = if let Screen::Login(f) | Screen::Setup(f) = &self.screen { f.fields.len() } else { 2 };
        let card = auth_card(w, h, s, n_fields);
        let r = login_rect(w, h, s, card, self.env.data_volatile);
        let (alpha, failed) = match phase {
            Phase::Done(a) => (a, false),
            _ => (255, pending.iter().any(|x| x.state.failed())),
        };
        let text = match phase {
            Phase::Done(_) => "All system services ready".to_string(),
            _ => {
                // names and steps only: a reason is for an administrator
                let mut parts: Vec<String> = pending.iter().take(2).map(|x| if x.state.failed() { format!("{}: did not start", x.name) } else if x.step.is_empty() { format!("{}: starting", x.name) } else { format!("{}: {}", x.name, x.step) }).collect();
                if pending.len() > 2 {
                    parts.push(format!("+{} more", pending.len() - 2));
                }
                if wifi_bg {
                    parts.push("Wi-Fi: starting in the background".to_string());
                }
                format!("Starting {} of {} ready · {}", ready, total, parts.join(" · "))
            }
        };
        let text = g.fit(14.0 * s, &text, Face::Sans, r.w - 30.0 * s);
        let tw = g.text_width(14.0 * s, &text, Face::Sans);
        let x0 = r.x + (r.w - tw - 24.0 * s) / 2.0;
        let (cx, cy) = (x0 + 8.0 * s, r.y + r.h / 2.0);
        match phase {
            Phase::Done(_) => g.circle(cx, cy, 5.0 * s, faded(OK, alpha)),
            _ if failed => g.circle(cx, cy, 5.0 * s, ERR),
            _ => spinner(g, cx, cy, 7.0 * s, self.tick, crate::fx::AMBER, 255),
        }
        g.text(x0 + 24.0 * s, r.y + (r.h - 14.0 * s) / 2.0 - s, 14.0 * s, faded(if failed { ERR } else { MUTED }, alpha), &text, Face::Sans);
        self.startup.login_line = Some(R::new(x0, r.y, tw + 24.0 * s, r.h));
    }

    pub(super) fn startup_toggle(&mut self) {
        self.startup.open = !self.startup.open;
        self.startup.sel = 0;
        self.stale = true;
    }

    fn startup_retry(&mut self, i: usize) {
        let Some(l) = self.startup.lines.get(i).cloned() else { return };
        if !l.actions.retry {
            return;
        }
        let Some(sess) = self.session.clone() else { return };
        self.env.audit.record(&sess.user, "startup.retry", l.id);
        let (svc, user) = (self.env.services.clone(), sess.user);
        std::thread::spawn(move || {
            let _ = iyakki_agent::startup_probe::retry(&svc, l.id, &user);
        });
        self.say_info(format!("Starting {} again", l.text.split(':').next().unwrap_or("it")));
    }

    fn startup_open(&mut self, i: usize) {
        let Some(item) = self.startup.lines.get(i).and_then(|l| l.actions.open.item()) else { return };
        self.startup.open = false;
        self.jump_to("settings", item);
    }

    /// A click on the strip or in the list.
    pub(super) fn startup_tab(&mut self, n: usize) {
        match n {
            TAB_STRIP => self.startup_toggle(),
            TAB_CLOSE => self.startup.open = false,
            TAB_PANEL => {}
            n if (TAB_RETRY..TAB_RETRY + MAX_ROWS).contains(&n) => self.startup_retry(n - TAB_RETRY),
            n if (TAB_OPEN..TAB_OPEN + MAX_ROWS).contains(&n) => self.startup_open(n - TAB_OPEN),
            _ => {}
        }
        self.stale = true;
    }

    /// Keys and clicks for the Home screen's list; true when taken.
    pub(super) fn startup_event(&mut self, ev: &Event) -> bool {
        if self.session.is_none() || self.panes.compact {
            return false;
        }
        match ev {
            Event::Key(Key::Char('t')) if !self.startup.open && !self.lay.modal() => {
                self.startup_toggle();
                true
            }
            Event::Key(k) if self.startup.open => {
                let n = self.startup.lines.len();
                match k {
                    Key::Esc | Key::Char('t') => self.startup.open = false,
                    Key::Up => self.startup.sel = self.startup.sel.saturating_sub(1),
                    Key::Down | Key::Tab => self.startup.sel = (self.startup.sel + 1).min(n.saturating_sub(1)),
                    Key::Char('r') => self.startup_retry(self.startup.sel),
                    Key::Enter | Key::Char('o') => self.startup_open(self.startup.sel),
                    _ => {}
                }
                self.stale = true;
                true
            }
            Event::Row(i) if self.startup.open => {
                self.startup.sel = *i;
                self.stale = true;
                true
            }
            // a click anywhere else only closes the list
            Event::MouseDown(x, y) if self.startup.open => {
                let inside = self.startup.panel.is_some_and(|p| p.contains(*x, *y)) || self.startup.strip.is_some_and(|p| p.contains(*x, *y));
                if !inside {
                    self.startup.open = false;
                    self.stale = true;
                }
                !inside
            }
            _ => false,
        }
    }
}
