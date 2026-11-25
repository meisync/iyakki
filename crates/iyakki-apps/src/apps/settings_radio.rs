//! Settings -> Radio: what the radio does when a live station's connection ends or fails while it plays (F-RADIO-17): how many times it
//! reconnects the same station (0 to 5, default 2) and whether it then goes on to the next station of the list (on by default).
//!
//! Administrators change the two choices (they are the machine's, kept in the settings file as `radio`, the same ones the chat tools
//! `radio_settings_status` and `radio_settings_set` change); everyone who has the RADIO app can read the page. A change is audited.
//! Starting the first station always needs the person.

use super::*;
use super::super::settings_port::SettingsDataPort;
use iyakki_shared::radiorecover::{RadioRecover, MAX_RETRIES};

impl SettingsApp {
    fn rr_settings(p: &dyn SettingsDataPort) -> RadioRecover {
        p.settings_get().radio
    }

    fn rr_save(&mut self, p: &dyn SettingsDataPort, what: &str, f: impl FnOnce(&mut RadioRecover)) {
        let mut f = Some(f);
        match p.settings_update(&mut |s| if let Some(f) = f.take() { f(&mut s.radio) }) {
            Ok(()) => {
                p.audit("settings.radio", what);
                self.ok("Saved.");
            }
            Err(e) => self.fail(e.message()),
        }
    }

    pub(super) fn handle_radio(&mut self, k: &Key, p: &dyn SettingsDataPort) {
        let Key::Char(c) = k else { return };
        if !matches!(c, 'r' | '+' | '-' | 'a') {
            return;
        }
        if !self.admin(p) {
            return self.fail("Only administrators change these.");
        }
        let cur = Self::rr_settings(p);
        match c {
            'r' | '+' | '-' => {
                let n = match c {
                    '+' => (cur.retries() + 1).min(MAX_RETRIES),
                    '-' => cur.retries().saturating_sub(1),
                    _ => (cur.retries() + 1) % (MAX_RETRIES + 1),
                };
                self.rr_save(p, &format!("retries={n}"), |s| s.retries = Some(n));
            }
            _ => {
                let off = cur.auto_switch();
                self.rr_save(p, &format!("auto_switch={}", !off), |s| s.no_auto_switch = off);
            }
        }
    }

    pub(super) fn radio_buttons(&self) -> Vec<Btn> {
        vec![("Reconnect tries", ACCENT, 'r'), ("More", TEXT, '+'), ("Fewer", TEXT, '-'), ("Next station", TEXT, 'a')]
    }

    pub(super) fn render_radio(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn SettingsDataPort) {
        let cur = Self::rr_settings(p);
        let admin = self.admin(p);
        let mut y = a.y;
        let rh = 62.0 * s;
        let n = cur.retries();
        controls::switch_row(g, R::new(a.x, y, a.w, rh), s, &format!("Reconnect tries: {n}"), "When a live station drops, the radio first plays out the sound it already has, then connects the same station again this many times (0 to 5). It says \"Reconnecting 1 of 2\" while it does.", n > 0, admin, Event::Key(Key::Char('r')), ACCENT);
        y += rh + 8.0 * s;
        controls::switch_row(g, R::new(a.x, y, a.w, rh), s, "Go on to the next station", "When the tries fail, switch to the next station of your list (it wraps at the end and stops after one full pass, saying so). Off: the radio stops and tells you.", cur.auto_switch(), admin, Event::Key(Key::Char('a')), ACCENT);
        y += rh + 12.0 * s;
        let lines = [
            (MUTED, "The first station always starts only when you press play. Every switch is written to the audit log."),
            (MUTED, if admin { "Keys: r next number of tries, + more, - fewer, a switch to the next station on or off." } else { "Only administrators change these." }),
        ];
        for (col, t) in lines {
            for l in g.wrap(13.0 * s, t, Face::Sans, a.w) {
                if y + 18.0 * s > a.y + a.h {
                    return;
                }
                g.text(a.x, y, 13.0 * s, col, &l, Face::Sans);
                y += 18.0 * s;
            }
        }
    }
}
