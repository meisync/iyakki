//! Settings -> Power: the power profile (performance, balanced, power saver), sleep after the computer has been idle, and a plain
//! list of what this machine cannot do (`iyakki_shared::power`).
//!
//! Administrators change the choices (they are the machine's, kept in the settings file as `power`); everyone can read the page. A
//! choice the machine cannot act on is greyed out and the page says why in words, never silently. The profile is written to the
//! kernel's CPU frequency files at once and again at every start; the idle rules are applied by the shell (`shell_power.rs`). Every
//! choice is a card that a click, a tap or a key sets; there is no hidden gesture.

use super::super::settings_port::SettingsDataPort;
use super::system_port::PowerPort;
use super::*;
use iyakki_shared::power::{Caps, PowerEnv};
use iyakki_shared::power::{self, PowerSettings, Profile};

/// Where the readings come from (tests use a fake folder) and the last reading.
pub(super) struct PwPanel {
    pub(super) env: PowerEnv,
    cache: Option<(std::time::Instant, Caps, Option<Profile>)>,
}

impl Default for PwPanel {
    fn default() -> PwPanel {
        PwPanel { env: if crate::TESTING { PowerEnv::at("/nonexistent-iyakki-sys") } else { PowerEnv::real() }, cache: None }
    }
}

impl SettingsApp {
    /// Look at another folder than /sys (tests).
    #[cfg(any(test, feature = "testing"))]
    pub fn with_power_env(mut self, env: PowerEnv) -> SettingsApp {
        self.pw = PwPanel { env, cache: None };
        self
    }

    fn pw_read(&mut self, p: &dyn PowerPort) -> (Caps, Option<Profile>) {
        if let Some((at, c, p)) = &self.pw.cache {
            if at.elapsed() < std::time::Duration::from_secs(2) {
                return (c.clone(), *p);
            }
        }
        let (c, cur) = (p.power_caps(&self.pw.env), p.power_current(&self.pw.env));
        self.pw.cache = Some((std::time::Instant::now(), c.clone(), cur));
        (c, cur)
    }

    fn pw_settings(p: &dyn SettingsDataPort) -> PowerSettings {
        p.settings_get().power
    }

    fn pw_save(&mut self, p: &dyn SettingsDataPort, what: &str, f: impl FnOnce(&mut PowerSettings)) -> bool {
        let mut f = Some(f);
        match p.settings_update(&mut |s| {
            if let Some(f) = f.take() {
                f(&mut s.power)
            }
        }) {
            Ok(()) => {
                p.audit("settings.power", what);
                self.pw.cache = None;
                true
            }
            Err(e) => {
                self.fail(e.message());
                false
            }
        }
    }

    pub(super) fn handle_power(&mut self, k: &Key, p: &dyn PowerPort) {
        let Key::Char(c) = k else { return };
        if !matches!(c, 'e' | 'b' | 'v' | 's' | 'w' | 'x' | 'l' | 'p' | 'f') {
            return;
        }
        if !self.admin(p) {
            return;
        }
        let (caps, _) = self.pw_read(p);
        let cur = Self::pw_settings(p);
        match c {
            'e' | 'b' | 'v' => {
                let profile = match c {
                    'e' => Profile::Performance,
                    'b' => Profile::Balanced,
                    _ => Profile::PowerSaver,
                };
                match p.power_set_profile(&self.pw.env, profile) {
                    Ok(_) => {
                        if self.pw_save(p, &format!("profile={profile:?}"), |s| s.profile = profile) {
                            self.ok(format!("Power profile: {}.", profile.label()));
                        }
                    }
                    Err(e) => self.fail(e),
                }
            }
            's' => {
                if !caps.sleep {
                    return self.fail("This computer cannot sleep, so there is nothing to set.");
                }
                let m = cur.next_sleep();
                if self.pw_save(p, &format!("sleep_after_min={m}"), |s| s.sleep_after_min = m) {
                    self.ok(if m == 0 { "The computer will not sleep by itself.".to_string() } else { format!("The computer sleeps after {m} minutes without use, unless something is running.") });
                }
            }
            'l' => {
                if !caps.lid {
                    return self.fail("This computer shows no lid switch, so there is nothing to set.");
                }
                let next = cur.lid.next();
                if self.pw_save(p, &format!("lid={next:?}"), |s| s.lid = next) {
                    self.ok(match next {
                        power::LidAction::Nothing => "Closing the lid does nothing.".to_string(),
                        power::LidAction::Lock => "Closing the lid locks the screen. Apps and runs go on.".to_string(),
                        power::LidAction::Sleep => "Closing the lid locks the screen and sleeps the computer, unless sleeping is not possible or something is running (then it only locks).".to_string(),
                    });
                }
            }
            'f' => match p.power_fan_toggle(&self.pw.env) {
                Ok(on) => {
                    p.audit("settings.fan", if on { "on" } else { "off" });
                    self.ok(if on { "IYAKKI runs the fans now (within a few seconds). The firmware's own curve takes over again on any doubt." } else { "The firmware runs the fans again (within a few seconds)." });
                }
                Err(e) => self.fail(e),
            },
            'p' => {
                let next = cur.button.next();
                if self.pw_save(p, &format!("button={next:?}"), |s| s.button = next) {
                    self.ok(format!("The power button: {}.", next.label()));
                }
            }
            'w' => {
                let on = !cur.sleep_when_busy;
                if self.pw_save(p, &format!("sleep_when_busy={on}"), |s| s.sleep_when_busy = on) {
                    self.ok(if on { "It may sleep while an install, download, job or model is working." } else { "It never sleeps while an install, download, job or model is working." });
                }
            }
            _ => {
                if !caps.t2 {
                    return;
                }
                let on = !cur.sleep_experimental;
                if self.pw_save(p, &format!("sleep_experimental={on}"), |s| s.sleep_experimental = on) {
                    self.ok(if on { "Real sleep is tried on this Mac. If it does not wake, hold the power button and start again." } else { "When idle this Mac only switches to the power saver profile." });
                }
            }
        }
    }

    pub(super) fn power_buttons(&self) -> Vec<Btn> {
        vec![("Performance", TEXT, 'e'), ("Balanced", ACCENT, 'b'), ("Power saver", TEXT, 'v'), ("Sleep after idle", TEXT, 's'), ("Sleep while busy", TEXT, 'w'), ("Lid", TEXT, 'l'), ("Power button", TEXT, 'p')]
    }

    pub(super) fn render_power(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn PowerPort) {
        let (caps, now) = self.pw_read(p);
        let cfg = Self::pw_settings(p);
        let admin = p.is_admin();
        let limit = a.y + a.h;
        let mut y = a.y;
        let rh = 62.0 * s;
        let row = |g: &mut Gfx, y: &mut f32, title: &str, help: &str, on: bool, can: bool, key: char| {
            if *y + rh > limit {
                return;
            }
            controls::switch_row(g, R::new(a.x, *y, a.w, rh), s, title, help, on, can, Event::Key(Key::Char(key)), ACCENT);
            *y += rh + 8.0 * s;
        };
        let can_profile = admin && caps.profiles;
        for (p, key) in [(Profile::Performance, 'e'), (Profile::Balanced, 'b'), (Profile::PowerSaver, 'v')] {
            row(g, &mut y, &format!("Power profile: {}", p.label()), p.help(), cfg.profile == p, can_profile, key);
        }
        let sleep_title = if cfg.sleep_after_min == 0 { "Sleep when idle: never".to_string() } else { format!("Sleep when idle: after {} minutes", cfg.sleep_after_min) };
        let sleep_help = if caps.t2 { "Experimental on this Mac: by default it only switches to the power saver profile. Never while something is running." } else { "Locks the session and sleeps. Never while an install, download, write, job or model is working. Click to change the time." };
        // the remaining cards go two to a row where the window is wide, so the page keeps its rows when the lid and the power button join it
        let wide = a.w > 820.0 * s;
        let gap = 10.0 * s;
        let half = (a.w - gap) / 2.0;
        let mut cards: Vec<(String, String, bool, bool, char)> = vec![
            (sleep_title, sleep_help.to_string(), cfg.sleep_after_min > 0, admin && caps.sleep, 's'),
            ("Sleep even while something is running".into(), "Off by default. Turn it on only if you want sleep to win over a running install or job.".into(), cfg.sleep_when_busy, admin && caps.sleep, 'w'),
        ];
        if caps.lid {
            cards.push((format!("Closing the lid: {}", cfg.lid.label()), "Click to change: nothing, lock the screen, or sleep. Sleep only when this computer can sleep and nothing is running, otherwise it locks.".into(), cfg.lid != power::LidAction::Nothing, admin, 'l'));
        }
        cards.push((format!("Power button: {}", cfg.button.label()), "Click to change: open the power card (the default), lock the screen, sleep, or nothing. Sleep falls back to the card when it is not possible.".into(), cfg.button != power::ButtonAction::Card, admin, 'p'));
        if caps.t2 {
            cards.push(("Try real sleep on this Mac (experimental)".into(), "Sleep and wake are not dependable on Apple T2 Macs. If it does not wake, hold the power button.".into(), cfg.sleep_experimental, admin && caps.sleep, 'x'));
        }
        let fan = p.power_fan_card(&self.pw.env);
        if let Some(f) = &fan {
            cards.push((format!("Fan control: {}", if f.enabled { "IYAKKI runs the fans" } else { "the firmware runs the fans" }), format!("Off by default. {}", f.line), f.enabled, admin, 'f'));
        }
        let mut i = 0;
        while i < cards.len() {
            if y + rh > limit {
                break;
            }
            let per = if wide { 2.min(cards.len() - i) } else { 1 };
            for j in 0..per {
                let (title, help, on, can, key) = &cards[i + j];
                let (x, w) = if per == 2 { (a.x + j as f32 * (half + gap), half) } else { (a.x, a.w) };
                controls::switch_row(g, R::new(x, y, w, rh), s, title, help, *on, *can, Event::Key(Key::Char(*key)), ACCENT);
            }
            y += rh + 8.0 * s;
            i += per;
        }
        y += 4.0 * s;
        let line = |g: &mut Gfx, y: &mut f32, col: [u8; 4], face: Face, text: &str| {
            for l in g.wrap(13.0 * s, text, face, a.w) {
                if *y + 13.0 * s * 1.3 > limit {
                    return;
                }
                g.text(a.x, *y, 13.0 * s, col, &l, face);
                *y += 13.0 * s * 1.45;
            }
        };
        let now_text = match now {
            Some(p) => format!("The processor is in the {} profile now.", p.label()),
            None => "The processor's speed cannot be read or changed here.".to_string(),
        };
        line(g, &mut y, MUTED, Face::Sans, &now_text);
        if !admin {
            line(g, &mut y, MUTED, Face::Sans, "Only administrators change these.");
        }
        for l in power::limits(&caps) {
            line(g, &mut y, crate::fx::AMBER, Face::Sans, &l);
        }
        line(g, &mut y, DIM, Face::Sans, "When the screen goes dark or the saver starts: Display & sound, Screen saver. Battery warnings: System, Battery.");
    }
}
