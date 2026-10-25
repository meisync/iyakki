//! Settings -> Battery (laptops only): how full the battery is, what it is doing, how worn it is, and three switches: the
//! percentage in the top bar, the low-battery warnings and the clean shutdown at the critical level (`iyakki_agent::battery`).
//!
//! The page is in the list only when the machine has a battery (or a battery the kernel cannot read yet). The two display switches
//! are for everyone signed in; the shutdown switch is for administrators. The choices are the machine's, kept in the settings file
//! (`battery`); the shell reads them at every heartbeat, so a change acts at once. Nothing here runs a program.

use super::*;
use iyakki_agent::battery::{self, BatteryEnv, BatterySettings, Reading};

/// Where the readings come from (tests use a fake folder), and the last reading (re-read every two seconds while the page shows).
pub(super) struct BatPanel {
    pub(super) env: BatteryEnv,
    cache: Option<(std::time::Instant, Reading)>,
    tracker: battery::Tracker,
}

impl Default for BatPanel {
    fn default() -> BatPanel {
        BatPanel { env: if cfg!(test) { BatteryEnv::at("/nonexistent-iyakki-sys") } else { BatteryEnv::real() }, cache: None, tracker: battery::Tracker::new() }
    }
}

impl SettingsApp {
    /// Look at another folder than /sys (tests); the page appears or disappears accordingly.
    pub fn with_battery_env(mut self, env: BatteryEnv) -> SettingsApp {
        self.bat = BatPanel { env, cache: None, tracker: battery::Tracker::new() };
        self.bat_sync_tab();
        self
    }

    /// The Battery page is in the list exactly when the machine has a battery (or one without a driver).
    pub(super) fn bat_sync_tab(&mut self) {
        let want = battery::read(&self.bat.env).shown();
        let cur = self.tab();
        let has = self.tabs.contains(&Tab::Battery);
        if want == has {
            return;
        }
        if want {
            self.tabs.push(Tab::Battery);
        } else {
            self.tabs.retain(|t| *t != Tab::Battery);
        }
        self.tabs.sort_by_key(|t| Tab::ORDER.iter().position(|o| o == t).unwrap_or(usize::MAX));
        let keep = if cur == Tab::Battery && !want { Tab::Overview } else { cur };
        self.go_to(keep);
    }

    pub(super) fn bat_reading(&mut self) -> Reading {
        if let Some((at, r)) = &self.bat.cache {
            if at.elapsed() < std::time::Duration::from_secs(2) {
                return r.clone();
            }
        }
        let mut r = battery::read(&self.bat.env);
        self.bat.tracker.apply(&mut r);
        self.bat.cache = Some((std::time::Instant::now(), r.clone()));
        r
    }

    pub(super) fn bat_settings(cx: &AppCx) -> BatterySettings {
        Self::svc(cx).settings.lock().map(|s| s.get().battery.clone()).unwrap_or_default()
    }

    fn bat_update(&mut self, cx: &AppCx, what: &str, f: impl FnOnce(&mut BatterySettings)) {
        let r = Self::svc(cx).settings.lock().map(|mut s| s.update(|s| f(&mut s.battery)));
        match r {
            Ok(Ok(_)) => {
                cx.audit.record(&cx.session.user, "settings.battery", what);
                self.cache_clear();
                self.ok("Saved.");
            }
            Ok(Err(e)) => self.fail(e.to_string()),
            Err(_) => self.fail("The settings are busy; try again."),
        }
    }

    fn cache_clear(&mut self) {
        self.bat.cache = None;
    }

    pub(super) fn handle_battery(&mut self, k: &Key, cx: &AppCx) {
        let cur = Self::bat_settings(cx);
        match k {
            Key::Char('p') => {
                let hide = !cur.hide_percent;
                self.bat_update(cx, &format!("hide_percent={hide}"), |s| s.hide_percent = hide);
            }
            Key::Char('w') => {
                let off = !cur.no_warnings;
                self.bat_update(cx, &format!("no_warnings={off}"), |s| s.no_warnings = off);
            }
            Key::Char('s') if self.admin(cx) => {
                let off = !cur.no_shutdown;
                self.bat_update(cx, &format!("no_shutdown={off}"), |s| s.no_shutdown = off);
            }
            _ => {}
        }
    }

    pub(super) fn battery_buttons(&self) -> Vec<Btn> {
        vec![("Percentage in the top bar", ACCENT, 'p'), ("Low battery warnings", TEXT, 'w'), ("Shut down at critical level", TEXT, 's')]
    }

    pub(super) fn render_battery(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let r = self.bat_reading();
        let cfg = Self::bat_settings(cx);
        let admin = cx.session.policy.manage_settings;
        let mut y = a.y;
        if r.readable() {
            // the bar
            let bar = R::new(a.x, y, a.w, 22.0 * s);
            g.rrect(bar.x, bar.y, bar.w, bar.h, bar.h / 2.0, rgb(26, 34, 62));
            let f = r.fill_fraction();
            if f > 0.0 {
                g.rrect(bar.x, bar.y, (bar.w * f).max(bar.h).min(bar.w), bar.h, bar.h / 2.0, crate::batterybar::fill_color(&r));
            }
            let t = format!("{} · {}", r.percent_text(), r.status_words());
            let t = g.fit(13.0 * s, &t, Face::Bold, bar.w - 20.0 * s);
            let tw = g.text_width(13.0 * s, &t, Face::Bold);
            g.text(bar.x + (bar.w - tw) / 2.0, bar.y + (bar.h - g.line_height(13.0 * s, Face::Bold)) / 2.0, 13.0 * s, if f >= 0.55 { rgb(10, 14, 28) } else { TEXT }, &t, Face::Bold);
            y += 34.0 * s;
        }
        // the switches
        let rh = 62.0 * s;
        let items = [
            ("Show the percentage in the top bar", "The battery icon is always shown; this adds the number beside it.", cfg.show_percent(), true, 'p'),
            ("Low battery warnings", "A message at 20% and 10%, and a \"Plug in now\" card at 5%.", cfg.warnings(), true, 'w'),
            ("Shut down at the critical level", "At 3% on battery the computer counts down 30 seconds and shuts down cleanly. Plugging in cancels it. Administrators change this.", cfg.shutdown_at_critical(), admin, 's'),
        ];
        if r.shown() {
            for (title, help, on, can, key) in items {
                controls::switch_row(g, R::new(a.x, y, a.w, rh), s, title, help, on, can, Event::Key(Key::Char(key)), ACCENT);
                y += rh + 8.0 * s;
            }
        }
        // the facts, as many as fit
        y += 4.0 * s;
        let label_w = (150.0 * s).min(a.w * 0.35);
        let limit = a.y + a.h;
        for (label, value) in r.rows() {
            if r.readable() && (label == "Charge" || label == "Status") {
                continue; // the bar says both
            }
            if y + 22.0 * s > limit {
                break;
            }
            g.text(a.x, y, 14.0 * s, MUTED, &label, Face::Sans);
            let lines = g.wrap(14.0 * s, &value, Face::Bold, a.w - label_w);
            for (i, l) in lines.iter().take(3).enumerate() {
                g.text(a.x + label_w, y + i as f32 * 18.0 * s, 14.0 * s, if r.readable() { TEXT } else { crate::fx::AMBER }, l, Face::Bold);
            }
            y += 24.0 * s + lines.len().clamp(1, 3).saturating_sub(1) as f32 * 18.0 * s;
        }
    }
}
