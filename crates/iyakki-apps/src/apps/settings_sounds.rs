//! Settings -> System sounds: the short cues for signing in, signing out, a new notification and a finished job
//! (`iyakki_agent::syssounds`). A master switch (on by default), the level of the cues below IYAKKI's volume (12 dB by default, so
//! they never startle), a switch for each cue and a Preview button beside it. The cues go through the output and the volume of
//! Settings -> Sound, are silent when sound is muted or there is no output, and the page is hidden in cloud mode with the Sound page.

use crate::safeclamp::SafeClamp;
use iyakki_core_api::soundtypes::Cue;
use iyakki_shared::syssounds as ss;

use super::*;
use super::super::settings_device_port::SystemSoundsPort;

/// Master switch, level, then one row for each cue.
pub(super) const ROWS: usize = 2 + Cue::ALL.len();

/// How far a press of + or - moves the level.
const STEP_DB: i32 = 3;

impl SettingsApp {
    fn sysnd_cue(row: usize) -> Cue {
        row.checked_sub(2).and_then(|i| Cue::ALL.get(i).copied()).unwrap_or(Cue::Login)
    }

    pub(super) fn sysnd_buttons(&self) -> Vec<Btn> {
        vec![("Switch on or off", ACCENT, 'e'), ("Preview", ACCENT, 'p'), ("Quieter", TEXT, '-'), ("Louder", TEXT, '+')]
    }

    pub(super) fn handle_sysnd(&mut self, k: &Key, p: &dyn SystemSoundsPort) {
        let sel = self.sel();
        match k {
            Key::Up => self.move_sel(-1, ROWS),
            Key::Down => self.move_sel(1, ROWS),
            Key::Home => self.sel[self.tab] = 0,
            Key::End => self.sel[self.tab] = ROWS - 1,
            Key::Enter | Key::Char(' ' | 'e') => self.sysnd_toggle(sel, p),
            Key::Char('p') => self.sysnd_preview(sel, p),
            Key::Char('+' | '=') => self.sysnd_level(STEP_DB, p),
            Key::Char('-') => self.sysnd_level(-STEP_DB, p),
            _ => {}
        }
    }

    /// A button or switch on a row: `Y` switches, `P` previews, `+` and `-` change the level.
    pub(super) fn sysnd_sub(&mut self, row: usize, c: char, p: &dyn SystemSoundsPort) {
        self.sel[self.tab] = row.min(ROWS - 1);
        match c {
            'Y' => self.sysnd_toggle(row, p),
            'P' => self.sysnd_preview(row, p),
            '+' => self.sysnd_level(STEP_DB, p),
            '-' => self.sysnd_level(-STEP_DB, p),
            _ => {}
        }
    }

    pub(super) fn sysnd_slider(&mut self, x: f32, w: f32, p: &dyn SystemSoundsPort) {
        let v = controls::value_at(x, w, ss::MIN_OFFSET_DB as i64, ss::MAX_OFFSET_DB as i64, 1) as i32;
        self.sysnd_set_level(v, p);
    }

    fn sysnd_toggle(&mut self, row: usize, p: &dyn SystemSoundsPort) {
        if !self.admin(p) {
            return;
        }
        let now = p.system_sounds();
        let r = match row {
            0 => p.update_system_sounds(&mut |s| s.enabled = !now.enabled).map(|n| (if n.enabled { "System sounds are on." } else { "System sounds are off." }.to_string(), "all", n.enabled)),
            1 => return self.ok("Use the slider, + or - to change the level."),
            _ => {
                let c = Self::sysnd_cue(row);
                p.update_system_sounds(&mut |s| s.set(c, !now.get(c))).map(|n| (format!("{} sound: {}.", c.label(), if n.get(c) { "on" } else { "off" }), c.id(), n.get(c)))
            }
        };
        match r {
            Ok((m, what, on)) => {
                p.audit("settings.sysnd", &format!("{what} {}", if on { "on" } else { "off" }));
                self.ok(m);
            }
            Err(e) => self.fail(e),
        }
    }

    fn sysnd_level(&mut self, delta: i32, p: &dyn SystemSoundsPort) {
        let now = p.system_sounds().offset_db;
        self.sysnd_set_level(now + delta, p);
    }

    fn sysnd_set_level(&mut self, db: i32, p: &dyn SystemSoundsPort) {
        if !self.admin(p) {
            return;
        }
        let db = db.clamp_safe(ss::MIN_OFFSET_DB, ss::MAX_OFFSET_DB);
        match p.update_system_sounds(&mut |s| s.offset_db = db) {
            Ok(n) => {
                p.audit("settings.sysnd.level", &n.offset_db.to_string());
                self.ok(format!("System sounds are {} dB from IYAKKI's volume.", n.offset_db));
            }
            Err(e) => self.fail(e),
        }
    }

    fn sysnd_preview(&mut self, row: usize, p: &dyn SystemSoundsPort) {
        let cue = Self::sysnd_cue(row);
        p.audit("settings.sysnd.preview", cue.id());
        let job = p.preview_job(cue);
        self.start("playing the sound", move || JobOut::Msg(job()));
    }

    pub(super) fn render_sysnd(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn SystemSoundsPort) {
        let sounds = p.system_sounds();
        let muted = p.sound_muted();
        let admin = p.is_admin();
        let low = a.y + a.h - 44.0 * s;
        let step = |i: usize| if i == 1 { 90.0 * s } else { 64.0 * s };
        let sel = self.sel().min(ROWS - 1);
        let steps: Vec<f32> = (0..ROWS).map(step).collect();
        let first = self.win[self.tab].layout(sel, &steps, low - a.y + 8.0 * s);
        let mut y = a.y;
        for i in first..ROWS {
            if y + if i == 1 { 82.0 * s } else { 58.0 * s } > low + 8.0 * s {
                break;
            }
            match i {
                0 => {
                    let r = R::new(a.x, y, a.w, 58.0 * s);
                    controls::switch_row(g, r, s, "System sounds", "Short quiet sounds for signing in and out, notifications and finished jobs.", sounds.enabled, admin, Event::Sub(0, Key::Char('Y')), ACCENT);
                }
                1 => {
                    let card = R::new(a.x, y, a.w, 82.0 * s);
                    g.hit(card, Event::Row(1), "level");
                    let track = controls::slider_row(
                        g,
                        card,
                        s,
                        &Slider { id: SLIDER_SYSND, title: "Level", help: "How much quieter than IYAKKI's volume the cues are.", value: sounds.offset_db as i64, min: ss::MIN_OFFSET_DB as i64, max: ss::MAX_OFFSET_DB as i64, unit: "dB", minus: Event::Sub(1, Key::Char('-')), plus: Event::Sub(1, Key::Char('+')), enabled: admin },
                        ACCENT,
                    );
                    self.slider_w.push((SLIDER_SYSND, track.w));
                }
                _ => {
                    let c = Self::sysnd_cue(i);
                    let r = R::new(a.x, y, a.w, 58.0 * s);
                    controls::switch_row(g, r, s, c.label(), c.feel(), sounds.get(c), admin, Event::Sub(i as u8, Key::Char('Y')), ACCENT);
                    let size = 13.0 * s;
                    let bw = g.text_width(size, "Preview", Face::Bold) + size * 1.7;
                    let bx = r.x + r.w - 18.0 * s - 52.0 * s - 14.0 * s - bw;
                    g.button(bx, r.y + (r.h - size * 2.1) / 2.0, size, "Preview", ACCENT, Event::Sub(i as u8, Key::Char('P')));
                }
            }
            if i == sel {
                let h = if i == 1 { 82.0 * s } else { 58.0 * s };
                g.rrect_outline(a.x, y, a.w, h, 14.0 * s, 2.0 * s, ACCENT);
            }
            y += step(i);
        }
        let note = if muted {
            "Sound is muted in Settings -> Sound, so the cues are silent too.".to_string()
        } else {
            format!("The cues go through the output and volume of Settings -> Sound, {} dB below it. No sound when there is no output.", sounds.offset_db.abs())
        };
        for (n, l) in g.wrap(13.0 * s, &note, Face::Sans, a.w).iter().take(2).enumerate() {
            g.text(a.x, a.y + a.h - 38.0 * s + n as f32 * 17.0 * s, 13.0 * s, if muted { ERR } else { DIM }, l, Face::Sans);
        }
    }
}
