//! The lock screen: the screen is covered by a password prompt for the person who is signed in, who is not signed out. Their apps and
//! runs go on behind it; their personal vault is closed and stays closed until the password is typed (the same account check as the
//! sign-in screen, with the same throttling, and `Services::unlock_user_vault`: nothing new in how a password is checked). The password
//! is never logged, audited or kept: the field is wiped after every attempt.
//!
//! When it locks: Super+L (or KEY_SCREENLOCK), Lock on the Power menu and the power key's card, the assistant's `lock_screen`, Settings
//! -> Lock now, when the computer goes to sleep, and after the person's idle minutes (Settings -> Display & sound -> Screen saver; off
//! until chosen). The idle count restarts while music or video plays, an approval waits or a dialog is open (the screen saver's own list
//! of what keeps the screen on), so the lock never appears under something the person is watching. A guest has no password: for a guest
//! Lock signs out, as before. A person who is not signed in sees the normal sign-in screen, unchanged.
//!
//! How the pieces meet: [`Shell::handle_awake`] asks [`Shell::lock_intercept`] first (while locked every key, click and touch goes to
//! the prompt, none reaches an app; heartbeats still run, so work in the background goes on), [`Shell::render`] draws
//! [`Shell::lock_render`] instead of the desktop, and the screen saver's heartbeat calls [`Shell::lock_tick`].

use super::*;
use iyakki_agent::lockscreen::{self, LockConfig};

pub(super) struct LockUi {
    pub(super) active: bool,
    pub(super) form: Form,
    user: String,
    /// The vault was open when the screen locked (so a failure to open it again is worth saying).
    vault_was_open: bool,
    cfg: LockConfig,
    cfg_for: Option<String>,
    cfg_at: Option<u64>,
    /// The last time something kept the screen on (the idle count for the automatic lock starts again from there), in saver ms.
    kept_ms: u64,
}

impl Default for LockUi {
    fn default() -> LockUi {
        LockUi { active: false, form: LockUi::fresh_form(), user: String::new(), vault_was_open: false, cfg: LockConfig::default(), cfg_for: None, cfg_at: None, kept_ms: 0 }
    }
}

impl LockUi {
    fn fresh_form() -> Form {
        Form { fields: vec![TextField::new("Password", true)], focus: 0, error: None }
    }
}

impl Shell {
    pub fn is_locked(&self) -> bool {
        self.lockui.active
    }

    /// Lock the screen of the signed-in person. False when there is nothing to lock (nobody signed in) or the person has no password
    /// (a guest); the caller then signs out.
    pub fn lock_screen(&mut self, why: &str) -> bool {
        let Some(s) = self.session.as_ref() else { return false };
        if s.role == iyakki_sec::policy::Role::Guest {
            return false;
        }
        if self.lockui.active {
            return true;
        }
        let user = s.user.clone();
        self.env.audit.record(&user, "lock", why);
        let vault_was_open = self.env.services.user_vault_unlocked(&user);
        self.env.services.lock_user_vault(&user);
        // nothing of the desktop stays open over the prompt
        self.ctx = None;
        self.clip = None;
        self.clipscan = None;
        self.devpop.open = false;
        self.help = false;
        self.shot = Shot::Off;
        self.fnk.power_card = false;
        self.lay.menu = None;
        if matches!(self.screen, Screen::PowerMenu) {
            self.screen = Screen::Launcher;
        }
        self.lockui.active = true;
        self.lockui.form = LockUi::fresh_form();
        self.lockui.user = user;
        self.lockui.vault_was_open = vault_was_open;
        self.hits.clear();
        self.stale = true;
        true
    }

    /// Forget the lock (the session it belonged to is over).
    pub(super) fn lock_clear(&mut self) {
        self.lockui.active = false;
        self.lockui.form = LockUi::fresh_form();
        self.lockui.user.clear();
    }

    /// The lock key (Super+L, KEY_SCREENLOCK).
    pub(super) fn lock_key(&mut self) {
        if self.session.is_none() {
            return; // the sign-in screen has nothing to lock
        }
        if !self.lock_screen("lock key") {
            self.fn_notice("Guests have no password to lock with; use Sign out on the Power menu");
        }
    }

    /// Every heartbeat (from the screen saver's step): a request from Settings or the assistant, and the automatic lock.
    pub(super) fn lock_tick(&mut self, now: u64) {
        if self.lockui.active {
            return;
        }
        let Some(s) = self.session.as_ref() else { return };
        if s.role == iyakki_sec::policy::Role::Guest {
            return;
        }
        let user = s.user.clone();
        if self.tick % 15 == 0 && lockscreen::take_request(self.env.services.dir(), &user) {
            self.lock_screen("asked for");
            return;
        }
        let due = self.lockui.cfg_at.is_none_or(|t| self.tick.saturating_sub(t) >= 30) || self.lockui.cfg_for.as_deref() != Some(user.as_str());
        if due {
            self.lockui.cfg = LockConfig::load(self.env.services.dir(), &user);
            self.lockui.cfg_at = Some(self.tick);
            self.lockui.cfg_for = Some(user);
        }
        let Some(after) = self.lockui.cfg.after_secs() else { return };
        if self.saver_keep_on().is_some() {
            self.lockui.kept_ms = now;
            return;
        }
        let quiet_ms = now.saturating_sub(self.lockui.kept_ms);
        let idle = self.saver_idle_secs(now).min(quiet_ms / 1000);
        if idle >= after {
            self.lock_screen("idle");
        }
    }

    /// While locked: true when the shell dealt with the event (it goes no further). Heartbeats are not taken, so everything behind the
    /// lock keeps running; every key, click, touch and pasted text is the prompt's.
    pub(super) fn lock_intercept(&mut self, ev: &Event) -> bool {
        if !self.lockui.active {
            return false;
        }
        match ev {
            Event::Tick => {
                // the caret blinks and the clock moves
                if self.tick % 15 == 0 {
                    self.stale = true;
                }
                return false;
            }
            Event::MouseMove(x, y) => {
                self.mouse = (*x, *y);
                self.stale = true;
            }
            Event::MouseDown(x, y) => {
                self.mouse = (*x, *y);
                if let Some(h) = self.hits.iter().rev().find(|h| h.rect.contains(*x, *y)).cloned() {
                    self.lock_prompt(&h.ev);
                }
            }
            other => self.lock_prompt(other),
        }
        true
    }

    /// A key, pasted text or a click on the prompt's own controls.
    fn lock_prompt(&mut self, ev: &Event) {
        match ev {
            Event::Key(Key::Enter) | Event::Key(Key::Ctrl('s')) => self.lock_submit(),
            Event::Key(Key::Esc) => {
                self.lockui.form.fields[0].clear();
                self.lockui.form.error = None;
            }
            Event::Key(k) => {
                self.lockui.form.error = None;
                self.lockui.form.fields[0].handle(k);
            }
            Event::Paste(t) => {
                self.lockui.form.error = None;
                self.lockui.form.fields[0].paste(t);
            }
            Event::FocusAt(_, x) => self.lockui.form.fields[0].click(*x),
            _ => {}
        }
        self.stale = true;
    }

    fn lock_submit(&mut self) {
        let pw = std::mem::take(&mut self.lockui.form.fields[0].value);
        self.lockui.form.fields[0].clear();
        if pw.is_empty() {
            self.lockui.form.error = Some("Type your password".into());
            return;
        }
        let user = self.lockui.user.clone();
        // the same check, and the same throttle, as the sign-in screen
        let r = self.env.store.lock().map_err(|_| AccountError::Storage("lock".into())).and_then(|mut s| s.authenticate(&user, &pw));
        match r {
            Ok(sess) if sess.user == user => self.lock_open(&user, &pw),
            Ok(_) => self.lockui.form.error = Some("That password does not open this screen".into()),
            Err(e) => {
                self.lockui.form.error = Some(e.to_string());
                self.env.audit.record(&user, "unlock.failed", "");
            }
        }
    }

    /// The password was right: the vault opens with it, the desktop comes back as it was.
    fn lock_open(&mut self, user: &str, pw: &str) {
        let vault = self.env.services.unlock_user_vault(user, pw);
        self.lockui.active = false;
        self.lockui.form = LockUi::fresh_form();
        self.lockui.kept_ms = self.saver_now_ms();
        self.env.audit.record(user, "unlock", "");
        if vault.is_err() && self.lockui.vault_was_open {
            self.env.audit.record(user, "vault.user.locked", "after the lock screen");
            self.say("Your personal vault could not be opened (see Settings -> Vault).");
        }
        self.hits.clear();
        self.stale = true;
    }

    /// Draw the lock instead of the desktop. False when the screen is not locked.
    pub(super) fn lock_render(&mut self, g: &mut Gfx) -> bool {
        if !self.lockui.active {
            return false;
        }
        self.size = (g.width(), g.height());
        let (w, h) = self.size;
        let s = scale_for(w, h);
        self.live = false;
        g.hits.clear();
        g.clear_probes();
        g.mouse = self.mouse;
        crate::ctxmenu::start_frame();
        crate::fx::background(g);
        let tick = self.tick;
        if !crate::fx::reduced_motion() {
            crate::splash::draw_galaxy(g, tick as f32 * 0.033, w as f32 / 2.0, h as f32 / 2.0, s, 0.55);
        }
        let lift = self.osk_auth_lift(w, h, s);
        let clock = self.saver_clock_line();
        let (user, hostname) = (self.lockui.user.clone(), self.env.hostname.clone());
        draw_lock(g, w, h, s, &self.lockui.form, &user, &clock, tick, &hostname, lift);
        self.fields = crate::ctxmenu::fields();
        self.draw_osk(g, w, h, s);
        self.hits = std::mem::take(&mut g.hits);
        self.probes = g.probes();
        true
    }
}

/// The lock card: the sign-in card's look, one field, "Unlock".
#[allow(clippy::too_many_arguments)]
fn draw_lock(g: &mut Gfx, w: u32, h: u32, s: f32, f: &Form, user: &str, clock: &str, tick: u64, hostname: &str, lift: f32) {
    let mut c = auth_card(w, h, s, 1);
    c.y -= lift;
    let (ld, gap) = auth_logo(s);
    let lt = if crate::fx::reduced_motion() { 0.0 } else { tick as f32 * 0.033 };
    crate::splash::draw_logo(g, lt, c.x + c.w / 2.0, c.y - gap - ld / 2.0, ld);
    g.radial_glow(c.x + c.w / 2.0, c.y + c.h / 2.0, c.w * 0.7, [70, 225, 255, 22]);
    crate::fx::modal_panel(g, c, s * 1.4, crate::fx::CYAN, false);
    let tw = g.text_width(54.0 * s, "IYAKKI", Face::Bold);
    g.text(c.x + (c.w - tw) / 2.0, c.y + 28.0 * s, 54.0 * s, TEXT, "IYAKKI", Face::Bold);
    name_strip(g, c.x + (c.w - tw) / 2.0, c.y + 98.0 * s, tw, 5.0 * s, s, true);
    let sub = g.fit(15.0 * s, &format!("Locked · {user}"), Face::Sans, c.w - 48.0 * s);
    let sw = g.text_width(15.0 * s, &sub, Face::Sans);
    g.text(c.x + (c.w - sw) / 2.0, c.y + 118.0 * s, 15.0 * s, MUTED, &sub, Face::Sans);
    let mut fr = auth_field_rect(w, h, s, 0, 1);
    fr.y -= lift;
    f.fields[0].render(g, fr, true, s, (tick / 15) % 2 == 0, Face::Sans, 0);
    let bw = g.text_width(16.0 * s, "Unlock", Face::Bold) + 16.0 * s * 1.7;
    g.button(c.x + (c.w - bw) / 2.0, c.y + c.h - 96.0 * s, 16.0 * s, "Unlock", rgb(120, 170, 255), Event::Key(Key::Ctrl('s')));
    if let Some(e) = &f.error {
        let e = g.fit(15.0 * s, e, Face::Sans, c.w - 64.0 * s);
        g.text(c.x + 32.0 * s, c.y + c.h - 40.0 * s, 15.0 * s, ERR, &e, Face::Sans);
    } else {
        let hint = "Your apps and runs are still going";
        let hw = g.text_width(13.0 * s, hint, Face::Sans);
        g.text(c.x + (c.w - hw) / 2.0, c.y + c.h - 36.0 * s, 13.0 * s, DIM, hint, Face::Sans);
    }
    if !clock.is_empty() {
        let cw = g.text_width(30.0 * s, clock, Face::Bold);
        g.text((w as f32 - cw) / 2.0, h as f32 - 64.0 * s, 30.0 * s, MUTED, clock, Face::Bold);
    }
    g.text(24.0 * s, h as f32 - 34.0 * s, 14.0 * s, DIM, hostname, Face::Sans);
}
