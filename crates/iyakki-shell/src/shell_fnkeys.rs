//! The shell's side of the media and Fn keys: what each key does, the on-screen display, held-key repeat, the radio keys that wait
//! for the kernel, the power card and applying the saved keyboard settings. The pure parts (key map, curves, backlight and
//! rfkill writers, drawing) are in `fnkeys.rs`.

use super::*;
use crate::fnkeys::{self, FnKey, KbdOp, Osd, OsdKind, Radio, VolOp};

pub(crate) const NO_SOUND: &str = "No sound output";

impl Shell {
    fn fn_show(&mut self, o: Osd) {
        self.fnk.osd = Some(o);
        self.stale = true;
    }

    pub(super) fn fn_notice(&mut self, text: impl Into<String>) {
        let until = self.tick + fnkeys::OSD_TICKS;
        self.fn_show(Osd::notice(text, until));
    }

    /// True when the event was a media key or went to the power card.
    pub(super) fn handle_fn_event(&mut self, ev: &Event) -> bool {
        if let Event::Fn(key, down) = ev {
            // cloud mode: no Fn, volume, brightness or media key does anything (the power keys still do)
            if self.cloud.fn_keys_off && !matches!(key, FnKey::Power | FnKey::Sleep) {
                return true;
            }
            // the power key again closes the card it opened
            if self.fnk.power_card && *down && matches!(key, FnKey::Power | FnKey::Sleep) {
                self.fnk.power_card = false;
                self.stale = true;
                return true;
            }
            self.handle_fn(*key, *down);
            return true;
        }
        if self.fnk.power_card {
            if let Event::Key(k) = ev {
                self.power_card_key(k);
                return true;
            }
        }
        false
    }

    fn handle_fn(&mut self, key: FnKey, down: bool) {
        if !down {
            if self.fnk.held.is_some_and(|h| h.0 == key) {
                self.fnk.held = None;
            }
            return;
        }
        // the same key reported by two devices at once acts once
        if self.fnk.last_press.is_some_and(|(k, t)| k == key && self.tick.saturating_sub(t) <= 1) {
            return;
        }
        self.fnk.last_press = Some((key, self.tick));
        self.fnk.held = key.repeats().then_some((key, self.tick, self.tick));
        self.fn_act(key);
    }

    fn fn_act(&mut self, key: FnKey) {
        match key {
            FnKey::VolumeUp => self.fn_volume(VolOp::Up),
            FnKey::VolumeDown => self.fn_volume(VolOp::Down),
            FnKey::VolumeMute => self.fn_volume(VolOp::Mute),
            FnKey::MicMute => self.fn_notice("Microphone mute is not available yet"),
            FnKey::BrightnessUp | FnKey::BrightnessDown => match fnkeys::adjust_panel(&self.fnk.sys, key == FnKey::BrightnessUp) {
                Ok(c) => {
                    let until = self.tick + fnkeys::OSD_TICKS;
                    self.fn_show(Osd::level(OsdKind::Brightness, &c, until));
                }
                Err(e) => self.fn_notice(e),
            },
            FnKey::BrightnessAuto => self.fn_notice("Automatic brightness is not supported"),
            FnKey::KbdLightUp | FnKey::KbdLightDown | FnKey::KbdLightToggle => {
                let op = match key {
                    FnKey::KbdLightUp => KbdOp::Up,
                    FnKey::KbdLightDown => KbdOp::Down,
                    _ => KbdOp::Toggle,
                };
                let mut last = self.fnk.kbd_last;
                let r = fnkeys::adjust_kbd(&self.fnk.sys, op, &mut last);
                self.fnk.kbd_last = last;
                match r {
                    Ok(c) => {
                        let until = self.tick + fnkeys::OSD_TICKS;
                        let mut o = Osd::level(OsdKind::KbdLight, &c, until);
                        if c.value == 0 {
                            o.text = "Off".into();
                        }
                        self.fn_show(o);
                    }
                    Err(e) => self.fn_notice(e),
                }
            }
            FnKey::PlayPause | FnKey::Next | FnKey::Prev | FnKey::Stop => self.fn_media(key),
            FnKey::Sleep | FnKey::Power => self.fn_power_card(),
            FnKey::RfKill | FnKey::Wlan | FnKey::Bluetooth => {
                let radio = match key {
                    FnKey::Wlan => Radio::Wifi,
                    FnKey::Bluetooth => Radio::Bluetooth,
                    _ => Radio::All,
                };
                // the kernel may switch the radio by itself (rfkill-input): wait a moment, and act only if nothing changed
                self.fnk.radio = Some(fnkeys::PendingRadio { radio, at: self.tick + 4, before: fnkeys::radio_snapshot(&self.fnk.sys) });
            }
            FnKey::SwitchVideo => self.fn_notice("Display mode key: IYAKKI uses the screen as it is"),
            FnKey::Print => {
                if self.session.is_some() && self.clip.is_none() {
                    self.start_shot();
                }
            }
            FnKey::Calc => self.fn_notice("There is no calculator app yet"),
            FnKey::Homepage => {
                let allowed = self.session.as_ref().is_some_and(|s| s.policy.allows_app(PApp::Browser));
                if allowed && matches!(self.screen, Screen::Launcher) {
                    self.open(PApp::Browser);
                    self.stale = true;
                } else if allowed && !matches!(self.screen, Screen::App(_, PApp::Browser)) {
                    self.fn_notice("Return to the launcher to open the browser");
                }
            }
        }
    }

    pub(super) fn fn_volume(&mut self, op: VolOp) {
        self.fn_volume_with(|s| fnkeys::volume_apply(s, op));
    }

    /// Change IYAKKI's one master volume and show the bar, whatever moved it (a key, the top bar's speaker, its wheel or slider).
    pub(super) fn fn_volume_with(&mut self, f: impl FnOnce(&mut iyakki_agent::audioout::SoundSettings)) {
        if !self.fnk.sound_probe.unwrap_or_else(fnkeys::sound_present) {
            return self.fn_notice(NO_SOUND);
        }
        match self.env.services.update_sound(f) {
            Ok(now) => {
                let until = self.tick + fnkeys::OSD_TICKS;
                self.devpop.invalidate(); // the volume in the top bar's Devices popover follows
                // the level is saved either way; say when no control moves the sound yet instead of showing a bar that lies
                match iyakki_agent::audioout::global().volume_note() {
                    Some(note) => self.fn_notice(note),
                    None => self.fn_show(Osd::volume(&now, until)),
                }
            }
            Err(e) => self.say(e),
        }
    }

    fn fn_media(&mut self, key: FnKey) {
        let cx = self.cx();
        if let (Screen::App(app, _), Some(cx)) = (&mut self.screen, &cx) {
            if app.media_key(key, cx) {
                let radio = &self.env.services.radio;
                let t = match key {
                    FnKey::PlayPause if radio.is_paused() => "Paused",
                    FnKey::PlayPause => "Play / pause",
                    FnKey::Next => "Next",
                    FnKey::Prev => "Previous",
                    _ => "Stopped",
                };
                self.fn_notice(t);
                return;
            }
        }
        // the radio plays on in the background: it answers when no open app took the key
        let radio = &self.env.services.radio;
        let t = match key {
            FnKey::PlayPause if radio.resume() => "Radio playing",
            FnKey::PlayPause if radio.is_active() => if radio.pause() { "Radio paused" } else { "Radio is connecting" },
            FnKey::Stop if radio.is_active() => {
                radio.stop();
                "Radio stopped"
            }
            FnKey::Next | FnKey::Prev if radio.is_active() => "Open Radio to change the station",
            _ => "Nothing is playing",
        };
        self.fn_notice(t);
    }

    fn fn_power_card(&mut self) {
        let allowed = self.session.as_ref().is_some_and(|s| s.policy.power);
        if self.session.is_none() {
            // the login and setup screens of the local console have a power card of their own; a remote session never does
            if self.on_auth_screen() && !self.lbar.remote {
                self.open_login_power();
                return;
            }
            return self.fn_notice("Sign in to use the power menu");
        }
        if !allowed {
            return self.fn_notice("This account may not shut down the computer");
        }
        self.fnk.power_card = true;
        self.stale = true;
    }

    /// The power card never acts on its own: a choice is needed (y off, r restart, anything else closes it).
    fn power_card_key(&mut self, k: &Key) {
        let user = self.session.as_ref().map(|s| s.user.clone()).unwrap_or_default();
        match k {
            Key::Char('y') | Key::Char('o') => {
                self.fnk.power_card = false;
                self.env.audit.record(&user, "power.off", "power key");
                self.env.host.power_off();
            }
            Key::Char('r') => {
                self.fnk.power_card = false;
                self.env.audit.record(&user, "power.reboot", "power key");
                self.env.host.reboot();
            }
            Key::Char('s') if self.env.host.can_sleep() => {
                self.fnk.power_card = false;
                self.sleep_now(&user, "power key");
            }
            Key::Char('l') if self.session.is_some() => {
                self.fnk.power_card = false;
                self.lock_now(&user, "power key");
            }
            _ => self.fnk.power_card = false,
        }
        self.stale = true;
    }

    /// Lock: the shell has one lock, signing out (the screen asks for the password again and the vault is sealed).
    pub(super) fn lock_now(&mut self, user: &str, detail: &str) {
        self.env.audit.record(user, "power.lock", detail);
        self.sign_out();
        self.stale = true;
    }

    /// Sleep: lock the session first (the shell has one lock, signing out, which also seals the vault), audit it, then suspend.
    /// The call returns after the machine wakes, and the login screen is what is showing.
    pub(super) fn sleep_now(&mut self, user: &str, detail: &str) {
        if self.session.is_some() {
            self.sign_out();
        }
        self.stale = true;
        self.env.audit.record(if user.is_empty() { "login screen" } else { user }, "power.sleep", detail);
        self.env.host.sleep();
    }

    /// Every heartbeat: the display's time, a held key's repeat, the radio keys waiting for the kernel, the saved settings.
    pub(super) fn fn_tick(&mut self) {
        if self.fnk.osd.as_ref().is_some_and(|o| o.until <= self.tick) {
            self.fnk.osd = None;
            self.stale = true;
        }
        if let Some((key, since, last)) = self.fnk.held {
            let (delay_ms, period_ms) = crate::input::current_repeat().unwrap_or((250, 33));
            let ticks = |ms: u32| (ms as u64).div_ceil(33);
            // a key held for more than ten seconds has lost its release (device unplugged)
            if self.tick - since > 300 {
                self.fnk.held = None;
            } else if self.tick - since >= ticks(delay_ms).max(8) && self.tick - last >= ticks(period_ms).max(3) {
                self.fnk.held = Some((key, since, self.tick));
                self.fn_act(key);
            }
        }
        if let Some(p) = self.fnk.radio.clone().filter(|p| self.tick >= p.at) {
            self.fnk.radio = None;
            let text = if fnkeys::radio_snapshot(&self.fnk.sys) != p.before { fnkeys::radio_notice(&self.fnk.sys, p.radio) } else { fnkeys::toggle_radio(&self.fnk.sys, p.radio).unwrap_or_else(|e| e) };
            self.fn_notice(text);
        }
        if self.tick >= 3 && self.fnk.synced.is_none_or(|t| self.tick - t >= 300) {
            self.fnk.synced = Some(self.tick);
            self.fn_sync_keyboard();
        }
    }

    /// Write the saved keyboard settings to the kernel (the Apple keyboard modes, the key repeat). Repeated every ten seconds,
    /// which also catches a driver that loads later; it writes only what differs.
    pub(super) fn fn_sync_keyboard(&mut self) {
        let k = self.env.services.settings.lock().map(|s| s.get().keyboard.clone()).unwrap_or_default();
        match iyakki_agent::keyboard::apply_apple(&self.fnk.sys, &k) {
            Ok(done) => done.iter().for_each(|d| log::info!("keyboard: {d}")),
            Err(e) => log::warn!("keyboard: {e}"),
        }
        if let Some((d, p)) = k.repeat() {
            crate::input::request_repeat(d, p);
        }
        let (tap, _) = k.trackpad();
        crate::input::request_touchpad(tap);
        crate::scrolleng::request_config(k.scroll().into());
        crate::inputprofile::refresh(&k); // the Apple devices the kernel lists decide Automatic
    }
}

// ---- the login screens' top bar and power card (`loginbar.rs`)
impl Shell {
    pub(super) fn on_auth_screen(&self) -> bool {
        matches!(self.screen, Screen::Login(_) | Screen::Setup(_))
    }

    /// Open the card (local console, nobody signed in). Refused on a remote session.
    pub(super) fn open_login_power(&mut self) {
        if self.lbar.remote || self.session.is_some() || !self.on_auth_screen() {
            return;
        }
        self.lbar.card = Some(0);
        self.stale = true;
    }

    /// True when the event belonged to the power card (opening it, moving in it, choosing or closing it).
    pub(super) fn login_power_event(&mut self, ev: &Event) -> bool {
        if self.lbar.card.is_none() {
            // Ctrl+P opens it; on a remote session it is just another key for the form
            if matches!(ev, Event::Key(Key::Ctrl(c)) if *c == crate::loginbar::POWER_KEY) && !self.lbar.remote {
                self.open_login_power();
                return true;
            }
            return false;
        }
        let Some(sel) = self.lbar.card else { return false };
        let sleepable = self.env.host.can_sleep();
        let n = if sleepable { 4 } else { 3 }; // Power off, Restart, [Sleep], Cancel
        let Event::Key(k) = ev else { return true }; // the pointer moving and the heartbeat leave it open
        self.stale = true;
        let choice = match k {
            Key::Left | Key::Up | Key::BackTab => {
                self.lbar.card = Some((sel + n - 1) % n);
                return true;
            }
            Key::Right | Key::Down | Key::Tab => {
                self.lbar.card = Some((sel + 1) % n);
                return true;
            }
            Key::Enter | Key::Char('y') => sel,
            Key::Char('o') => 0,
            Key::Char('r') => 1,
            Key::Char('s') if sleepable => 2,
            // Esc and any other key close it without acting
            _ => n - 1,
        };
        self.lbar.card = None;
        if choice == n - 1 {
            return true;
        }
        if choice == 2 && sleepable {
            self.login_sleep_act();
        } else {
            self.login_power_act(choice == 1);
        }
        true
    }

    /// Power off or restart from the login screen: audited as "login screen", refused on a remote session, and says so when
    /// the host cannot do it.
    fn login_power_act(&mut self, restart: bool) {
        if self.lbar.remote || self.session.is_some() {
            log::warn!("login power: refused (remote session or signed in)");
            return;
        }
        if !self.env.host.can_power() {
            self.say(crate::loginbar::CANNOT);
            return;
        }
        self.env.audit.record("login screen", if restart { "power.reboot" } else { "power.off" }, "");
        if restart {
            self.env.host.reboot();
        } else {
            self.env.host.power_off();
        }
    }

    /// Sleep from the login screen (nobody is signed in, so there is nothing to lock): audited as "login screen".
    fn login_sleep_act(&mut self) {
        if self.lbar.remote || self.session.is_some() {
            log::warn!("login sleep: refused (remote session or signed in)");
            return;
        }
        self.sleep_now("", "");
    }

    /// The bar's slow work, every heartbeat: read the machine for the network and battery chips while a login screen shows.
    pub(super) fn login_bar_tick(&mut self) {
        if !self.on_auth_screen() || self.lbar.remote || self.session.is_some() {
            return;
        }
        // tests never look at the real machine
        if cfg!(test) && self.devprobe.is_none() {
            return;
        }
        if self.devpop.wants_refresh_idle() {
            let p = self.dev_probe();
            self.devpop.refresh_in_background(&p);
        }
        if self.devpop.take_fresh() {
            self.stale = true;
        }
    }

    pub(super) fn draw_login_bar(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        self.lbar.last.clear();
        self.lbar.power_rect = None;
        if !self.on_auth_screen() || self.lbar.remote || self.session.is_some() {
            self.lbar.card = None;
            return;
        }
        let snap = self.devpop.snapshot();
        let bar = crate::loginbar::Bar {
            host: self.env.hostname.clone(),
            net: crate::loginbar::network_chip(snap.as_ref()),
            battery: crate::loginbar::battery_chip(snap.as_ref()),
            time: clock_time_text(&self.clock),
            date: self.clock.date_text(),
        };
        let placed = crate::loginbar::draw(g, w, s, &bar, true);
        self.lbar.power_rect = placed.iter().find(|(c, _)| *c == crate::loginbar::Chip::Power).map(|(_, r)| *r);
        self.lbar.last = placed;
        if let Some(sel) = self.lbar.card {
            let can = self.env.host.can_power();
            crate::loginbar::draw_card(g, w, h, s, sel, can, self.env.host.can_sleep());
        }
    }
}
