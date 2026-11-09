//! The shell's side of the Touch Bar strip: starts the background host on a T2 MacBook, builds what the strip shows from the shell's
//! own state, carries out the touches, and hands the firmware's function keys back whenever the strip is off or fails.
//! The model, drawing and touch handling are in `touchbar.rs`; the device is in `touchbar_host.rs`.
//!
//! Display only unless the kernel exposes the bar's touch panel (kernel 6.12 does not know it; `docs/TOUCH-BAR.md`): without touches the
//! strip still shows the volume, brightness, the clock, the recent apps and the agent's state, and the cycle chip, sliders and buttons
//! simply never fire.

use super::*;
use crate::fnkeys::{self, FnKey};
use crate::touchbar::{self, Action, AgentState, Bar, Choice, Ctx, Recent, SliderKind, Strip};
use crate::touchbar_host::Host;
use iyakki_agent::keyboard::{self, KeyboardSettings};

/// Heartbeats between two looks at the settings and the backlights (30 a second).
const SLOW_TICKS: u64 = 30;
/// Heartbeats between two slider writes while a finger drags.
const SLIDER_TICKS: u64 = 3;
/// Milliseconds per heartbeat.
const TICK_MS: u64 = 33;

pub(crate) struct TbState {
    host: Option<Host>,
    bar: Option<Bar>,
    cfg: KeyboardSettings,
    cfg_at: Option<u64>,
    pub(super) ready: bool,
    activity: u64,
    user: String,
    pub(super) saved: Choice,
    focused: Option<String>,
    lights: (Option<f32>, Option<f32>),
    pending: Option<(SliderKind, f32)>,
    last_slider: u64,
    status_text: String,
    strips_sig: String,
}

impl Default for TbState {
    fn default() -> TbState {
        TbState { host: None, bar: None, cfg: KeyboardSettings::default(), cfg_at: None, ready: false, activity: 0, user: String::new(), saved: Choice::default(), focused: None, lights: (None, None), pending: None, last_slider: 0, status_text: String::new(), strips_sig: String::new() }
    }
}

impl Shell {
    /// Any key, click or touch counts as activity (the bar goes dark after five idle minutes).
    pub(super) fn tbar_activity(&mut self) {
        self.tbar.activity = self.tick;
    }

    #[cfg(test)]
    pub(crate) fn tbar_attach_for_test(&mut self, host: Host) {
        self.tbar.host = Some(host);
    }

    #[cfg(test)]
    pub(crate) fn tbar_bar_for_test(&mut self) -> Option<&mut Bar> {
        self.tbar.bar.as_mut()
    }

    fn tbar_focused(&self) -> Option<String> {
        match &self.screen {
            Screen::App(_, pa) if self.custom_open.is_none() => Some(pa.id().to_string()),
            _ => None,
        }
    }

    fn tbar_wanted(&self) -> bool {
        if cfg!(test) || !cfg!(target_os = "linux") {
            return false;
        }
        let product = std::fs::read_to_string("/sys/class/dmi/id/product_name").unwrap_or_default();
        matches!(self.tbar.cfg.touchbar.as_str(), "" | "strip") && iyakki_drivers::macboard::is_t2(&product)
    }

    /// The firmware's keys are the Touch Bar's: give them back in the chosen mode (or the kernel's default).
    fn tbar_give_back_keys(&mut self) {
        // (the process-wide override is left alone under test: other tests share the process)
        if !cfg!(test) {
            keyboard::set_touchbar_override(None);
        }
        let n = keyboard::restore_touchbar_mode(&self.fnk.sys, &self.tbar.cfg);
        if n > 0 {
            log::info!("Touch Bar: the firmware's keys are back");
        }
        self.fn_sync_keyboard();
    }

    fn tbar_ready_changed(&mut self, ready: bool) {
        self.tbar.ready = ready;
        if ready {
            if !cfg!(test) {
                keyboard::set_touchbar_override(Some(3));
            }
            self.fn_sync_keyboard();
            if let Some(b) = self.tbar.bar.as_mut() {
                b.invalidate();
            }
        } else {
            self.tbar_give_back_keys();
        }
    }

    fn tbar_refresh_settings(&mut self) {
        self.tbar.cfg_at = Some(self.tick);
        self.tbar.cfg = self.env.services.settings.lock().map(|s| s.get().keyboard.clone()).unwrap_or_default();
        let want = self.tbar_wanted();
        if want && self.tbar.host.is_none() {
            self.tbar.host = Some(Host::start(self.tbar.cfg.touchbar_flip));
        }
        if !want && self.tbar.host.is_some() && !cfg!(test) {
            if let Some(h) = self.tbar.host.take() {
                h.stop();
            }
            self.tbar.bar = None;
            if self.tbar.ready {
                self.tbar_ready_changed(false);
            }
            touchbar::set_status("off");
        }
        if let Some(h) = &self.tbar.host {
            h.set_flip(self.tbar.cfg.touchbar_flip);
        }
        // the lights the strip's sliders show
        let sys = self.fnk.sys.clone();
        self.tbar.lights = (fnkeys::panel_backlights(&sys).first().map(|l| l.fraction()), fnkeys::kbd_backlights(&sys).first().map(|l| l.fraction()));
    }

    /// The account changed: its last strip and recent apps.
    fn tbar_sign_in_check(&mut self) {
        let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default();
        if user == self.tbar.user {
            return;
        }
        self.tbar.user = user.clone();
        self.tbar.focused = None;
        self.tbar.saved = if user.is_empty() { Choice::default() } else { Choice::load(self.env.services.dir(), &user) };
        if let (Some(b), Some(s)) = (self.tbar.bar.as_mut(), Strip::from_id(&self.tbar.saved.strip)) {
            b.set_strip(s);
        }
    }

    fn tbar_ctx(&mut self) -> Ctx {
        let snd = self.env.services.sound_settings();
        let (busy, approval) = match &self.screen {
            Screen::App(app, _) => app.touchbar_state(),
            _ => (false, false),
        };
        let focused = self.tbar_focused();
        let agent = match self.env.services.ext_status.get(iyakki_agent::extagent::CLAUDE_CODE) {
            None => AgentState::Unknown,
            Some(s) if !s.installed => AgentState::Missing,
            Some(s) if s.auth == iyakki_agent::extagent::Auth::SignedOut => AgentState::NeedsLogin,
            Some(_) if busy => AgentState::Working,
            Some(_) => AgentState::Ready,
        };
        let recents = self
            .tbar
            .saved
            .recents
            .iter()
            .filter_map(|id| {
                let t = TILES.iter().find(|t| t.app.id() == id)?;
                Some(Recent { id: id.clone(), label: t.label.to_string(), icon: t.icon })
            })
            .collect();
        Ctx {
            signed_in: self.session.is_some(),
            idle_ms: self.tick.saturating_sub(self.tbar.activity) * TICK_MS,
            screen_off: self.saver_screen_off(),
            volume: snd.volume_pct.min(100) as f32 / 100.0,
            muted: snd.muted,
            has_sound: self.fnk.sound_probe.unwrap_or_else(fnkeys::sound_present),
            brightness: self.tbar.lights.0,
            kbd_light: self.tbar.lights.1,
            playing: self.env.services.radio.is_active() || iyakki_agent::media::shared().now_playing().is_some(),
            clock: clock_time_text(&self.clock),
            recents,
            focused,
            agent_name: "Claude Code".into(),
            agent,
            busy,
            approval,
        }
    }

    /// Every heartbeat: look at the host, take touches, draw when something changed.
    pub(super) fn tbar_tick(&mut self) {
        if self.tbar.cfg_at.is_none_or(|t| self.tick.saturating_sub(t) >= SLOW_TICKS) {
            self.tbar_refresh_settings();
        }
        let Some(host) = self.tbar.host.clone() else { return };
        let status = host.status();
        let text = status.text();
        if text != self.tbar.status_text {
            log::info!("Touch Bar: {text}");
            touchbar::set_status(&text);
            self.tbar.status_text = text;
            // not drawing (waiting, a configuration switch, a failure): the firmware's keys must be there; their device may have been
            // re-created by a configuration change, so the mode file is looked up again every time
            if !status.is_ready() && !self.tbar.ready {
                keyboard::restore_touchbar_mode(&self.fnk.sys, &self.tbar.cfg);
            }
        }
        let ready = status.is_ready();
        if ready != self.tbar.ready {
            self.tbar_ready_changed(ready);
        }
        if !ready {
            self.tbar.bar = None;
            return;
        }
        self.tbar_sign_in_check();
        let strips: Vec<&str> = self.tbar.cfg.strips();
        let sig = format!("{strips:?}");
        let (w, h) = host.size().unwrap_or((2170, 60));
        if self.tbar.bar.as_ref().is_none_or(|b| b.w != w || b.h != h) || sig != self.tbar.strips_sig {
            let mut b = Bar::new(w, h, &strips);
            if let Some(s) = Strip::from_id(&self.tbar.saved.strip) {
                b.set_strip(s);
            }
            self.tbar.bar = Some(b);
            self.tbar.strips_sig = sig;
        }
        // touches (none arrive when the kernel gives no touch panel: the strip is then display only)
        for ev in host.take_touches() {
            self.tbar_activity();
            if self.saver_touch_wake() {
                continue; // the first touch only wakes the screen
            }
            let acts = self.tbar.bar.as_mut().map(|b| b.touch(ev)).unwrap_or_default();
            for a in acts {
                self.tbar_act(a);
            }
        }
        self.tbar_flush_slider();
        // recents: the app that came to the front
        let focused = self.tbar_focused();
        if focused != self.tbar.focused {
            self.tbar.focused = focused.clone();
            if let (Some(id), false) = (focused, self.tbar.user.is_empty()) {
                if self.tbar.saved.touch_app(&id) {
                    self.tbar_save();
                }
            }
        }
        if self.tick % 3 == 0 {
            let ctx = self.tbar_ctx();
            let now_ms = self.tick * TICK_MS;
            let chosen = self.tbar.bar.as_mut().and_then(|b| {
                let p = b.update(&ctx, now_ms);
                p.map(|p| (p, b.chosen()))
            });
            if let Some((p, strip)) = chosen {
                host.submit(p);
                if strip.id() != self.tbar.saved.strip {
                    self.tbar.saved.strip = strip.id().to_string();
                    self.tbar_save();
                }
            }
        }
    }

    fn tbar_save(&self) {
        if !self.tbar.user.is_empty() {
            self.tbar.saved.save(self.env.services.dir(), &self.tbar.user);
        }
    }

    fn tbar_act(&mut self, a: Action) {
        match a {
            Action::None | Action::Cycle => {}
            Action::Esc => self.handle(&Event::Key(Key::Esc)),
            Action::Fn(k) => {
                self.handle(&Event::Fn(k, true));
                self.handle(&Event::Fn(k, false));
            }
            Action::ToggleMute => {
                self.handle(&Event::Fn(FnKey::VolumeMute, true));
                self.handle(&Event::Fn(FnKey::VolumeMute, false));
            }
            Action::Key(k) => self.handle(&Event::Key(k)),
            Action::Open(id) => self.tbar_open(&id),
            Action::Slider(kind, v) => {
                self.tbar.pending = Some((kind, v));
                if self.tick.saturating_sub(self.tbar.last_slider) >= SLIDER_TICKS {
                    self.tbar_flush_slider();
                }
            }
        }
        self.stale = true;
    }

    /// Open an app, or return to it. From another app this is "Esc, then open": an app that keeps the key (a question about unsaved work)
    /// is never closed by the strip.
    fn tbar_open(&mut self, id: &str) {
        let Some(pa) = PApp::ALL.into_iter().find(|a| a.id() == id) else { return };
        if self.lockui.active {
            return; // the strip opens nothing behind the lock screen
        }
        match &self.screen {
            Screen::App(_, cur) if *cur == pa && self.custom_open.is_none() => {}
            Screen::Launcher => self.open(pa),
            Screen::App(..) => {
                self.handle(&Event::Key(Key::Esc));
                if matches!(self.screen, Screen::Launcher) {
                    self.open(pa);
                } else {
                    self.say("Close the current app first (Esc), then tap again");
                }
            }
            _ => {}
        }
    }

    fn tbar_flush_slider(&mut self) {
        let Some((kind, v)) = self.tbar.pending else { return };
        if self.tick.saturating_sub(self.tbar.last_slider) < SLIDER_TICKS {
            return;
        }
        self.tbar.pending = None;
        self.tbar.last_slider = self.tick;
        let pct = touchbar::percent(v);
        match kind {
            SliderKind::Volume => {
                if self.fnk.sound_probe.unwrap_or_else(fnkeys::sound_present) {
                    let _ = self.env.services.update_sound(|s| {
                        s.volume_pct = pct as u32;
                        s.muted = false;
                    });
                    self.devpop.invalidate();
                }
            }
            SliderKind::Brightness | SliderKind::KeyboardLight => {
                let sys = self.fnk.sys.clone();
                let lights = if kind == SliderKind::Brightness { fnkeys::panel_backlights(&sys) } else { fnkeys::kbd_backlights(&sys) };
                if let Some(l) = lights.first() {
                    // never fully dark: the panel's lowest step
                    let floor = if kind == SliderKind::Brightness { fnkeys::brightness_levels(l.max).into_iter().find(|v| *v > 0).unwrap_or(1) } else { 0 };
                    let value = ((l.max as f32 * pct as f32 / 100.0).round() as u32).max(floor).min(l.max);
                    let _ = std::fs::write(l.dir.join("brightness"), value.to_string());
                }
            }
        }
        // the strip draws what the machine now says
        self.tbar.cfg_at = Some(self.tick.saturating_sub(SLOW_TICKS));
    }
}
