//! Settings -> Notifications: for each source that leaves notes in the inbox (new devices, finished jobs, flows, reminders...),
//! the person chooses how much of it they want: Show (counted in the bell by kind), Quiet (kept in the inbox panel only: no
//! number, no dot, no sound) or Off (not kept at all). The choice is per person and is kept in their inbox file
//! (`iyakki_agent::inbox::Inbox::set_level`). The same choice is one key away in the inbox itself (`m` on a note).

use iyakki_agent::inbox::Level;

use super::*;

impl SettingsApp {
    fn notif_rows(cx: &AppCx) -> Vec<(String, String)> {
        Self::svc(cx).inbox.sources(&cx.session.user)
    }

    pub(super) fn notif_len(cx: &AppCx) -> usize {
        Self::notif_rows(cx).len()
    }

    pub(super) fn notifs_buttons(&self) -> Vec<Btn> {
        vec![("Show", OK, 's'), ("Quiet (panel only)", ACCENT, 'q'), ("Off", ERR, 'o'), ("Next level", TEXT, 'e'), ("Matrix account...", ACCENT, 'a'), ("Test message", TEXT, 't'), ("Remove Matrix account", ERR, 'x')]
    }

    pub(super) fn handle_notifs(&mut self, k: &Key, cx: &AppCx) {
        let rows = Self::notif_rows(cx);
        match k {
            Key::Up => self.move_sel(-1, rows.len()),
            Key::Down => self.move_sel(1, rows.len()),
            Key::Char('a') => self.matrix_open_form(""),
            Key::Char('t') => self.matrix_test_send(cx),
            Key::Char('x') => self.matrix_remove(cx),
            Key::Char('s') => self.notif_set(self.sel(), Some(Level::Show), cx),
            Key::Char('q') => self.notif_set(self.sel(), Some(Level::Quiet), cx),
            Key::Char('o') => self.notif_set(self.sel(), Some(Level::Off), cx),
            Key::Enter | Key::Char(' ' | 'e') | Key::Right => self.notif_set(self.sel(), None, cx),
            _ => {}
        }
    }

    /// A chip on a row: `s`, `q` or `o`.
    pub(super) fn notif_sub(&mut self, row: usize, c: char, cx: &AppCx) {
        let level = match c {
            's' => Some(Level::Show),
            'q' => Some(Level::Quiet),
            'o' => Some(Level::Off),
            _ => None,
        };
        self.notif_set(row, level, cx);
    }

    /// `None`: the next level.
    fn notif_set(&mut self, row: usize, level: Option<Level>, cx: &AppCx) {
        let rows = Self::notif_rows(cx);
        let Some((src, label)) = rows.get(row).cloned() else { return };
        self.sel[self.tab] = row;
        let (svc, user) = (Self::svc(cx), &cx.session.user);
        let level = level.unwrap_or_else(|| svc.inbox.level(user, &src).next());
        match svc.inbox.set_level(user, &src, level) {
            Ok(()) => {
                cx.audit.record(user, "settings.notifications", &format!("{src} {}", level.name()));
                self.ok(format!("{label}: {}.", level.label()));
            }
            Err(e) => self.fail(e),
        }
    }

    pub(super) fn render_notifs(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let rows = Self::notif_rows(cx);
        let levels = Self::svc(cx).inbox.levels(&cx.session.user);
        let sel = self.sel().min(rows.len().saturating_sub(1));
        let row_h = 44.0 * s;
        let reach = Self::matrix_reach_line(cx);
        let reach_lines = g.wrap(12.5 * s, &reach, Face::Sans, a.w);
        let reach_h = reach_lines.iter().take(2).count() as f32 * 16.0 * s + 8.0 * s;
        let low = a.y + a.h - 40.0 * s - reach_h;
        let room = (((low - a.y) / row_h) as usize).max(1);
        let start = sel.saturating_sub(room - 1);
        let narrow = a.w < 520.0 * s;
        let mut y = a.y;
        for (i, (src, label)) in rows.iter().enumerate().skip(start).take(room) {
            let lv = levels.get(src).copied().unwrap_or(Level::Show);
            let r = R::new(a.x, y, a.w, row_h - 4.0 * s);
            if i == sel {
                g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, rgb(30, 38, 68));
            }
            g.hit(r, Event::Row(i), "row");
            // the three choices, drawn as chips from the right edge; the current one is filled
            let size = 12.5 * s;
            let mut right = r.x + r.w - 8.0 * s;
            for (lvl, short, key) in [(Level::Off, "Off", 'o'), (Level::Quiet, if narrow { "Quiet" } else { "Quiet (panel only)" }, 'q'), (Level::Show, "Show", 's')] {
                let w = g.text_width(size, short, Face::Bold) + size * 1.7;
                right -= w;
                let col = match lvl {
                    Level::Show => OK,
                    Level::Quiet => ACCENT,
                    Level::Off => ERR,
                };
                if lv == lvl {
                    g.rrect(right, r.y + (r.h - size * 2.1) / 2.0, w, size * 2.1, 6.0 * s, col);
                    g.text(right + size * 0.85, r.y + (r.h - size * 2.1) / 2.0 + size * 0.5, size, rgb(10, 12, 20), short, Face::Bold);
                    g.hit(R::new(right, r.y, w, r.h), Event::Sub(i as u8, Key::Char(key)), short);
                } else {
                    g.button(right, r.y + (r.h - size * 2.1) / 2.0, size, short, col, Event::Sub(i as u8, Key::Char(key)));
                }
                right -= 6.0 * s;
            }
            let text_w = (right - r.x - 24.0 * s).max(40.0 * s);
            let t = g.fit(14.0 * s, label, Face::Bold, text_w);
            g.text(r.x + 12.0 * s, r.y + 4.0 * s, 14.0 * s, if i == sel { TEXT } else { MUTED }, &t, Face::Bold);
            let sub = g.fit(12.0 * s, src, Face::Sans, text_w);
            g.text(r.x + 12.0 * s, r.y + 23.0 * s, 12.0 * s, DIM, &sub, Face::Sans);
            y += row_h;
        }
        for (n, l) in reach_lines.iter().take(2).enumerate() {
            g.text(a.x, low + 2.0 * s + n as f32 * 16.0 * s, 12.5 * s, ACCENT, l, Face::Sans);
        }
        let note = "Show: counted in the bell by kind. Quiet: in the inbox only, no number, no dot, no sound. Off: not kept. Information, new devices and finished jobs never add to the bell's number.";
        for (n, l) in g.wrap(12.5 * s, note, Face::Sans, a.w).iter().take(3).enumerate() {
            g.text(a.x, low + reach_h + n as f32 * 16.0 * s - 2.0 * s, 12.5 * s, DIM, l, Face::Sans);
        }
    }
}
