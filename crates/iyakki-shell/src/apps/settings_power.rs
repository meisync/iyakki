//! Settings -> Power: the power profile (performance, balanced, power saver), sleep after the computer has been idle, and a plain
//! list of what this machine cannot do (`iyakki_agent::power`).
//!
//! Administrators change the choices (they are the machine's, kept in the settings file as `power`); everyone can read the page. A
//! choice the machine cannot act on is greyed out and the page says why in words, never silently. The profile is written to the
//! kernel's CPU frequency files at once and again at every start; the idle rules are applied by the shell (`shell_power.rs`). Every
//! choice is a card that a click, a tap or a key sets; there is no hidden gesture.

use super::*;
use iyakki_agent::power::{self, Caps, PowerEnv, PowerSettings, Profile};

/// Where the readings come from (tests use a fake folder) and the last reading.
pub(super) struct PwPanel {
    pub(super) env: PowerEnv,
    cache: Option<(std::time::Instant, Caps, Option<Profile>)>,
}

impl Default for PwPanel {
    fn default() -> PwPanel {
        PwPanel { env: if cfg!(test) { PowerEnv::at("/nonexistent-iyakki-sys") } else { PowerEnv::real() }, cache: None }
    }
}

impl SettingsApp {
    /// Look at another folder than /sys (tests).
    #[cfg(test)]
    pub fn with_power_env(mut self, env: PowerEnv) -> SettingsApp {
        self.pw = PwPanel { env, cache: None };
        self
    }

    fn pw_read(&mut self) -> (Caps, Option<Profile>) {
        if let Some((at, c, p)) = &self.pw.cache {
            if at.elapsed() < std::time::Duration::from_secs(2) {
                return (c.clone(), *p);
            }
        }
        let (c, p) = (power::caps(&self.pw.env), power::current(&self.pw.env));
        self.pw.cache = Some((std::time::Instant::now(), c.clone(), p));
        (c, p)
    }

    fn pw_settings(cx: &AppCx) -> PowerSettings {
        Self::svc(cx).settings.lock().map(|s| s.get().power.clone()).unwrap_or_default()
    }

    fn pw_save(&mut self, cx: &AppCx, what: &str, f: impl FnOnce(&mut PowerSettings)) -> bool {
        let r = Self::svc(cx).settings.lock().map(|mut s| s.update(|s| f(&mut s.power)));
        match r {
            Ok(Ok(_)) => {
                cx.audit.record(&cx.session.user, "settings.power", what);
                self.pw.cache = None;
                true
            }
            Ok(Err(e)) => {
                self.fail(e.to_string());
                false
            }
            Err(_) => {
                self.fail("The settings are busy; try again.");
                false
            }
        }
    }

    pub(super) fn handle_power(&mut self, k: &Key, cx: &AppCx) {
        let Key::Char(c) = k else { return };
        if !matches!(c, 'e' | 'b' | 'v' | 's' | 'w' | 'x') {
            return;
        }
        if !self.admin(cx) {
            return;
        }
        let (caps, _) = self.pw_read();
        let cur = Self::pw_settings(cx);
        match c {
            'e' | 'b' | 'v' => {
                let profile = match c {
                    'e' => Profile::Performance,
                    'b' => Profile::Balanced,
                    _ => Profile::PowerSaver,
                };
                match power::set_profile(&self.pw.env, profile) {
                    Ok(_) => {
                        if self.pw_save(cx, &format!("profile={profile:?}"), |s| s.profile = profile) {
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
                if self.pw_save(cx, &format!("sleep_after_min={m}"), |s| s.sleep_after_min = m) {
                    self.ok(if m == 0 { "The computer will not sleep by itself.".to_string() } else { format!("The computer sleeps after {m} minutes without use, unless something is running.") });
                }
            }
            'w' => {
                let on = !cur.sleep_when_busy;
                if self.pw_save(cx, &format!("sleep_when_busy={on}"), |s| s.sleep_when_busy = on) {
                    self.ok(if on { "It may sleep while an install, download, job or model is working." } else { "It never sleeps while an install, download, job or model is working." });
                }
            }
            _ => {
                if !caps.t2 {
                    return;
                }
                let on = !cur.sleep_experimental;
                if self.pw_save(cx, &format!("sleep_experimental={on}"), |s| s.sleep_experimental = on) {
                    self.ok(if on { "Real sleep is tried on this Mac. If it does not wake, hold the power button and start again." } else { "When idle this Mac only switches to the power saver profile." });
                }
            }
        }
    }

    pub(super) fn power_buttons(&self) -> Vec<Btn> {
        vec![("Performance", TEXT, 'e'), ("Balanced", ACCENT, 'b'), ("Power saver", TEXT, 'v'), ("Sleep after idle", TEXT, 's'), ("Sleep while busy", TEXT, 'w')]
    }

    pub(super) fn render_power(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let (caps, now) = self.pw_read();
        let cfg = Self::pw_settings(cx);
        let admin = cx.session.policy.manage_settings;
        let limit = a.y + a.h;
        let mut y = a.y;
        let rh = 62.0 * s;
        let mut row = |g: &mut Gfx, y: &mut f32, title: &str, help: &str, on: bool, can: bool, key: char| {
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
        row(g, &mut y, &sleep_title, sleep_help, cfg.sleep_after_min > 0, admin && caps.sleep, 's');
        row(g, &mut y, "Sleep even while something is running", "Off by default. Turn it on only if you want sleep to win over a running install or job.", cfg.sleep_when_busy, admin && caps.sleep, 'w');
        if caps.t2 {
            row(g, &mut y, "Try real sleep on this Mac (experimental)", "Sleep and wake are not dependable on Apple T2 Macs. If it does not wake, hold the power button.", cfg.sleep_experimental, admin && caps.sleep, 'x');
        }
        y += 4.0 * s;
        let mut line = |g: &mut Gfx, y: &mut f32, col: [u8; 4], face: Face, text: &str| {
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
