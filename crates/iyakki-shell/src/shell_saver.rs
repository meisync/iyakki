//! The shell's side of the screen saver and screen off: the idle clock, what keeps the screen on, waking (the first key or click only
//! wakes), the dim and off stages on the panel, drawing the saver instead of the desktop, and doing less while the screen is off.
//! The rules and the animation are `saver.rs`; the panel is `panel.rs`; the choices are `iyakki_agent::screensaver`.
//!
//! How the pieces meet:
//! * [`Shell::handle`] asks [`Shell::saver_intercept`] first. Every input event resets the idle clock. While the saver or the screen
//!   off is up, the first input wakes and goes no further (a stray click presses nothing); pointer movement only wakes.
//! * Every heartbeat [`Shell::saver_step`] reads the clock, the person's choices and what keeps the screen on, and moves the stage.
//! * [`Shell::render`] draws the saver (partial redraw) or nothing instead of the desktop while a stage covers it.
//! * The backend asks [`Shell::redraw_interval_ms`] how often it must redraw and [`Shell::take_blank_requests`] for the framebuffer's
//!   blank call; the backlight is switched here, with the previous value kept for a restart after a crash.

use super::*;
use crate::panel::{self, Panel};
use crate::saver::{self, IdleMachine, Painter, Stage, Token};
use iyakki_agent::screensaver::{Request, SaverConfig};
use iyakki_sec::policy::Role;
use std::sync::atomic::{AtomicU64, Ordering};

/// Where the shell takes its time from.
#[allow(dead_code)] // `Fake` is made by the tests
pub(super) enum SaverClock {
    /// Milliseconds since the shell started.
    Real(std::time::Instant),
    /// A test moves it.
    Fake(Arc<AtomicU64>),
    /// Never moves: no idle time ever passes (what the tests start with, so no existing test meets a saver).
    Frozen,
}

impl SaverClock {
    fn now_ms(&self) -> u64 {
        match self {
            SaverClock::Real(t) => t.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            SaverClock::Fake(a) => a.load(Ordering::SeqCst),
            SaverClock::Frozen => 0,
        }
    }
}

pub(super) struct SaverUi {
    clock: SaverClock,
    m: IdleMachine,
    cfg: SaverConfig,
    /// Used instead of the person's and the machine's choices (tests).
    cfg_override: Option<SaverConfig>,
    cfg_loaded_at: Option<u64>,
    cfg_for: Option<String>,
    painter: Painter,
    last_frame_ms: u64,
    frame_due: bool,
    black_done: bool,
    frames: u64,
    /// The panel's backlight (only on the machine's own screen).
    panel: Option<Panel>,
    blank_reqs: Vec<bool>,
    blanked: bool,
    /// A remote (browser) screen: slower frames, nothing of the host is switched.
    remote: bool,
    swallow_up: bool,
    /// Developer-mode input is being delivered: it wakes the screen and still reaches the app.
    pub(super) injecting: bool,
    on_battery: bool,
    clock_text: String,
    clock_text_at: u64,
    last_reason: String,
    /// Power profile and sleep after idle (`shell_power.rs`).
    pub(super) pw: super::power_impl::PowerIdle,
}

impl Default for SaverUi {
    fn default() -> SaverUi {
        let clock = if cfg!(test) { SaverClock::Frozen } else { SaverClock::Real(std::time::Instant::now()) };
        let now = clock.now_ms();
        SaverUi {
            clock,
            m: IdleMachine::new(now),
            cfg: SaverConfig::default(),
            cfg_override: None,
            cfg_loaded_at: None,
            cfg_for: None,
            painter: Painter::default(),
            last_frame_ms: 0,
            frame_due: false,
            black_done: false,
            frames: 0,
            panel: None,
            blank_reqs: Vec::new(),
            blanked: false,
            remote: false,
            swallow_up: false,
            injecting: false,
            on_battery: false,
            clock_text: String::new(),
            clock_text_at: u64::MAX,
            last_reason: String::new(),
            pw: Default::default(),
        }
    }
}

impl Shell {
    /// The saver's clock in milliseconds, and the seconds without input (the lock screen counts its idle minutes from these).
    pub(super) fn saver_now_ms(&self) -> u64 {
        self.saver.clock.now_ms()
    }

    pub(super) fn saver_idle_secs(&self, now: u64) -> u64 {
        self.saver.m.idle_secs(now)
    }

    /// The time for the lock screen, looked up about once a second.
    pub(super) fn saver_clock_line(&mut self) -> String {
        let now = self.saver.clock.now_ms();
        self.saver_clock_text(now)
    }

    /// The stage the screen is in now.
    pub fn screen_stage(&self) -> Stage {
        self.saver.m.stage()
    }

    /// Keep the screen on for `secs` seconds for `reason` (a long download the person is watching, a presentation...). Asking
    /// again with the same reason extends it; [`Shell::release_screen_off`] ends it.
    pub fn inhibit_screen_off(&mut self, reason: &str, secs: u64) -> Token {
        let now = self.saver.clock.now_ms();
        self.saver.m.inhibit(now, reason, secs)
    }

    pub fn release_screen_off(&mut self, t: Token) {
        self.saver.m.release(t);
    }

    /// Start the saver now, for `secs` seconds or until input (Settings -> Preview).
    pub fn start_screen_saver(&mut self, secs: Option<u64>) {
        let now = self.saver.clock.now_ms();
        self.saver.m.force_saver(now, secs);
        self.saver_step(now);
    }

    /// This shell is the machine's own screen: the backlight is switched and the framebuffer's blank call is asked for. A shell
    /// that crashed while the panel was dim or off left its brightness on file: it is put back here, and a panel still at 0 is raised.
    pub fn enable_panel_control(&mut self) {
        let n = panel::restore_leftover(std::path::Path::new(panel::RESTORE_FILE));
        let up = panel::ensure_visible(&self.fnk.sys);
        if n > 0 || up {
            log::info!("screen: the backlight was left dark by an earlier run; brightness restored");
        }
        self.saver.panel = Some(Panel::new(&self.fnk.sys, std::path::Path::new(panel::RESTORE_FILE)));
    }

    /// The shell stops (or hands the screen to another program): backlight back, and the screen asked to unblank.
    pub fn panel_shutdown(&mut self) {
        let now = self.saver.clock.now_ms();
        self.saver.m.reset(now);
        self.saver_apply(Stage::Awake);
        if let Some(p) = self.saver.panel.as_mut() {
            p.restore();
        }
    }

    /// The framebuffer's blank calls the shell wants made, in order: `true` = power down, `false` = unblank.
    pub fn take_blank_requests(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.saver.blank_reqs)
    }

    /// How often the backend must redraw at least (ms): the desktop every 100 ms, the saver once a second (its own frames come
    /// earlier), nothing while the screen is off.
    pub fn redraw_interval_ms(&self) -> u64 {
        // a paired computer is asking for the screen while the saver or the black screen covers it: draw soon, so the request wakes the
        // display. (Found on the Mac mini 2018 after six idle hours: the black screen drew once a minute, and `dev state` gave up after ten
        // seconds, so the wake in the render never ran in time.)
        if self.saver.m.stage().covers() && self.dev_capture_asked() {
            return 100;
        }
        match self.saver.m.stage() {
            Stage::Awake | Stage::Dim if self.tick < self.live_until => self.live_every_ms,
            Stage::Awake | Stage::Dim => match &self.screen {
                // nothing moves and nothing is drawn over the app: it says how rarely the safety redraw has to come
                Screen::App(app, _) if self.live_clear() => app.idle_redraw_ms(),
                _ => 100,
            },
            Stage::Saver => 1000,
            Stage::Off => 60_000,
        }
    }

    /// A remote (browser) screen: slower saver frames.
    pub(super) fn saver_set_remote(&mut self, remote: bool) {
        self.saver.remote = remote;
    }

    #[cfg(test)]
    pub(crate) fn saver_clock_for_test(&mut self, c: Arc<AtomicU64>) {
        self.saver.clock = SaverClock::Fake(c.clone());
        self.saver.m = IdleMachine::new(c.load(Ordering::SeqCst));
    }

    #[cfg(test)]
    pub(crate) fn saver_config_for_test(&mut self, c: SaverConfig) {
        self.saver.cfg_override = Some(c);
        self.saver.cfg_loaded_at = None;
    }

    #[cfg(test)]
    pub(crate) fn saver_panel_for_test(&mut self, sys: &std::path::Path, restore: &std::path::Path) {
        self.saver.panel = Some(Panel::new(sys, restore));
    }

    #[cfg(test)]
    pub(crate) fn saver_frames_for_test(&self) -> u64 {
        self.saver.frames
    }

    #[cfg(test)]
    pub(crate) fn saver_holds_for_test(&mut self) -> Vec<String> {
        let now = self.saver.clock.now_ms();
        self.saver.m.hold_reasons(now).into_iter().map(str::to_string).collect()
    }

    #[cfg(test)]
    pub(crate) fn saver_keep_on_for_test(&self) -> Option<&'static str> {
        self.saver_keep_on()
    }

    // -------------------------------------------------------------------------------------------- the choices

    fn saver_choices(&self) -> SaverConfig {
        if let Some(c) = &self.saver.cfg_override {
            return c.clone();
        }
        match &self.session {
            Some(s) if s.role != Role::Guest => SaverConfig::load(self.env.services.dir(), &s.user),
            Some(_) => SaverConfig::default(),
            None => self.env.services.settings.lock().map(|s| s.get().screensaver_signin.0.clone().normalized()).unwrap_or_else(|_| SaverConfig::signin_default()),
        }
    }

    /// Whether the machine runs on its battery (a laptop with the adapter out).
    fn saver_on_battery(&self) -> bool {
        use iyakki_agent::battery::{Presence, Status};
        let r = &self.battery.reading;
        r.presence == Presence::Present && (r.ac_online == Some(false) || (r.ac_online.is_none() && r.status == Status::Discharging))
    }

    /// Something the person is watching, listening to or has to answer keeps the screen on. The reason, for the log and the tests.
    pub(super) fn saver_keep_on(&self) -> Option<&'static str> {
        match &self.screen {
            Screen::HwCheck(_) => return Some("hardware check"),
            Screen::Setup(_) => return Some("first-run setup"),
            _ => {}
        }
        if self.session.is_some() && self.badge.approvals > 0 {
            return Some("a waiting approval");
        }
        if self.ring.is_some() {
            return Some("an alarm");
        }
        if self.battery.card_visible() {
            return Some("a battery card");
        }
        if self.loss.last.as_ref().is_some_and(|s| !s.healthy()) {
            return Some("the drive banner");
        }
        if self.login.is_some() || self.hostdlg.is_some() || !matches!(self.shot, Shot::Off) || self.fnk.power_card || self.tzask.open {
            return Some("a dialog");
        }
        if self.env.services.radio.is_active() {
            return Some("the radio");
        }
        if iyakki_agent::media::shared().now_playing().is_some() {
            return Some("the media player");
        }
        if let Screen::App(app, _) = &self.screen {
            if app.keeps_screen_on() {
                return Some("the app");
            }
        }
        None
    }

    // -------------------------------------------------------------------------------------------- the heartbeat

    /// One look at the clock (every heartbeat): choices, requests, holds, then the stage.
    pub(super) fn saver_step(&mut self, now: u64) {
        let user = self.session.as_ref().map(|s| s.user.clone());
        let due = self.saver.cfg_loaded_at.is_none_or(|t| self.tick.saturating_sub(t) >= 30) || self.saver.cfg_for != user;
        if due {
            self.saver.cfg = self.saver_choices();
            self.saver.cfg_loaded_at = Some(self.tick);
            self.saver.cfg_for = user.clone();
        }
        if self.tick % 15 == 0 {
            self.saver.on_battery = self.saver_on_battery();
            if let Some(u) = &user {
                if iyakki_agent::screensaver::take_request(self.env.services.dir(), u) == Some(Request::Preview) {
                    self.saver.m.force_saver(now, Some(5));
                }
            }
        }
        let keep = if self.saver.cfg.active() || self.saver.m.stage() != Stage::Awake { self.saver_keep_on() } else { None };
        if let Some(why) = keep {
            if self.saver.last_reason != why {
                self.saver.last_reason = why.to_string();
            }
        } else if !self.saver.last_reason.is_empty() {
            self.saver.last_reason.clear();
        }
        let cfg = self.saver.cfg.clone();
        let from = self.saver.m.stage();
        if let Some(to) = self.saver.m.step(now, &cfg, self.saver.on_battery, keep.is_some()) {
            log::info!("screen: {:?} -> {:?}{}", from, to, if self.saver.last_reason.is_empty() { String::new() } else { format!(" (kept on by {})", self.saver.last_reason) });
            self.saver_apply(to);
        }
        self.lock_tick(now);
        if self.power_idle(now, self.saver.m.idle_secs(now)) {
            // the wake-up counts as input
            self.saver.m.activity(now);
        }
        if self.saver.m.stage() == Stage::Saver {
            let every = 1000 / u64::from(saver::fps(self.saver.on_battery, self.saver.remote).max(1));
            if now.saturating_sub(self.saver.last_frame_ms) >= every {
                self.saver.frame_due = true;
            }
        }
    }

    /// The panel and the picture for a stage that was just entered.
    fn saver_apply(&mut self, to: Stage) {
        let pct = self.saver.cfg.dim_pct;
        match to {
            Stage::Awake => {
                // backlight first, then the screen, then the scene is drawn whole
                if let Some(p) = self.saver.panel.as_mut() {
                    p.restore();
                }
                if self.saver.blanked {
                    self.saver.blank_reqs.push(false);
                    self.saver.blanked = false;
                }
                self.saver.painter.reset();
                self.saver.black_done = false;
                self.saver.frame_due = false;
                self.hits.clear();
                self.stale = true;
            }
            Stage::Dim => {
                if let Some(p) = self.saver.panel.as_mut() {
                    p.dim(pct);
                }
            }
            Stage::Saver => {
                self.saver.swallow_up = false;
                if let Some(p) = self.saver.panel.as_mut() {
                    if pct > 0 {
                        p.dim(pct);
                    }
                }
                self.saver.painter.reset();
                self.saver.black_done = false;
                self.saver.frame_due = true;
                self.hits.clear();
                self.stale = true;
            }
            Stage::Off => {
                self.saver.swallow_up = false;
                self.saver.black_done = false;
                self.saver.painter.reset();
                if let Some(p) = self.saver.panel.as_mut() {
                    p.off();
                    // the screen itself powers down too (the framebuffer's blank call is made by the backend)
                    self.saver.blank_reqs.push(true);
                    self.saver.blanked = true;
                }
                self.hits.clear();
                self.stale = true;
            }
        }
    }

    /// Wake the screen (input, an alarm, ...).
    fn saver_wake(&mut self, now: u64) -> bool {
        let from = self.saver.m.stage();
        if !self.saver.m.wake(now) {
            return false;
        }
        log::info!("screen: {:?} -> Awake (input)", from);
        self.saver_apply(Stage::Awake);
        true
    }

    /// Called before every event. True when the shell has dealt with it (it goes no further).
    pub(super) fn saver_intercept(&mut self, ev: &Event) -> bool {
        let now = self.saver.clock.now_ms();
        if matches!(ev, Event::Tick) {
            self.saver_step(now);
            if self.saver.m.stage() == Stage::Off {
                // nothing to draw and nobody to answer: only the heartbeats the watchers need (about every third; the battery's and
                // the drive's own, the paired computer's input) run through the whole shell
                let n = self.tick + 1;
                if n % 3 != 0 && n % 15 != 7 && n % 10 != 3 {
                    self.tick = n;
                    return true;
                }
            }
            return false;
        }
        // any other event is input
        if self.saver.swallow_up && matches!(ev, Event::MouseUp(..)) && self.saver.m.stage() == Stage::Awake {
            self.saver.swallow_up = false;
            return true;
        }
        if matches!(ev, Event::MouseDown(..) | Event::MouseRight(..)) {
            self.saver.swallow_up = false;
        }
        self.saver.m.activity(now);
        iyakki_agent::touchfix::note_input();
        let stage = self.saver.m.stage();
        if stage == Stage::Awake && !self.saver.m.in_grace(now) {
            return false;
        }
        if self.saver.m.in_grace(now) {
            return true; // the click that started a preview is still being released
        }
        if self.saver.injecting {
            self.saver_wake(now);
            return false; // a paired computer's input also reaches the app
        }
        if !self.saver_wake(now) {
            return true;
        }
        if stage == Stage::Dim {
            return false; // the desktop was visible, only dimmed: the input is meant
        }
        if let Event::MouseMove(x, y) = ev {
            self.mouse = (*x, *y);
        }
        self.saver.swallow_up = matches!(ev, Event::MouseDown(..) | Event::MouseRight(..));
        true
    }

    /// Called after every event: while the saver or the black screen is up, only its own frames make the picture stale.
    pub(super) fn saver_after(&mut self) {
        match self.saver.m.stage() {
            Stage::Saver => {
                self.stale = self.saver.frame_due;
                self.live = false;
            }
            Stage::Off => {
                self.stale = !self.saver.black_done;
                self.live = false;
            }
            Stage::Awake | Stage::Dim => {}
        }
    }

    /// The Touch Bar was touched: wakes the screen, and that first touch does nothing else.
    pub(super) fn saver_touch_wake(&mut self) -> bool {
        let now = self.saver.clock.now_ms();
        self.saver.m.activity(now);
        if self.saver.m.stage() == Stage::Awake {
            return false;
        }
        self.saver_wake(now)
    }

    /// The Touch Bar's strip follows the screen.
    pub(super) fn saver_screen_off(&self) -> bool {
        self.saver.m.stage() == Stage::Off
    }

    // -------------------------------------------------------------------------------------------- drawing

    /// Draw the saver (or nothing) instead of the desktop. False when the desktop is to be drawn.
    pub(super) fn saver_render(&mut self, g: &mut Gfx) -> bool {
        let stage = self.saver.m.stage();
        if !stage.covers() {
            return false;
        }
        self.size = (g.width(), g.height());
        g.hits.clear();
        self.live = false;
        match stage {
            Stage::Saver => {
                let now = self.saver.clock.now_ms();
                let style = self.saver.cfg.style;
                let (w, h) = (g.width(), g.height());
                let s = scale_for(w, h);
                let text = (style == iyakki_agent::screensaver::Style::LogoClock).then(|| self.saver_clock_text(now));
                self.saver.painter.draw(g, style, now as f64 / 1000.0, text.as_deref(), s);
                self.saver.last_frame_ms = now;
                self.saver.frame_due = false;
                self.saver.frames += 1;
            }
            _ => {
                if !self.saver.black_done {
                    g.clear(saver::BLACK);
                    self.saver.black_done = true;
                }
            }
        }
        true
    }

    /// The time under the logo, looked up again about once a second.
    fn saver_clock_text(&mut self, now: u64) -> String {
        if self.saver.clock_text_at == u64::MAX || now.saturating_sub(self.saver.clock_text_at) >= 1000 {
            let time_settings = self.env.services.settings.lock().ok().map(|st| st.get().time.clone());
            let person = self.session.as_ref().map(|se| crate::uisize::load_clock(self.env.services.dir(), &se.user));
            self.clock.refresh(self.tick, time_settings.as_ref(), person);
            self.saver.clock_text = clock_time_text(&self.clock);
            self.saver.clock_text_at = now;
        }
        self.saver.clock_text.clone()
    }
}
