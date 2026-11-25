//! Settings: the Display page (lane ST-D, docs/development/SETTINGS-EXTRACTION-PLAN.md, section 3). The page takes a `UserPort` (who is signed in, the
//! audit log, administrators); it never reaches the shell context. Its rows are the device rows of `settings_hw.rs` (`draw_dev_rows`), its cards are
//! the text-size and theme cards of `settings_cards.rs`.

use super::*;
use super::super::settings_device_port::UserPort;

const NOTE: &str = "The desktop uses the screen's best mode. Resolution, arrangement and mirroring of several displays are in Settings, Displays.";

impl SettingsApp {
    /// The Display page's keys: text size, theme and wallpaper choices, the rows' own keys (brightness and the like, for administrators).
    pub(super) fn handle_display(&mut self, k: &Key, p: &dyn UserPort) {
        let len = self.dev_rows().len();
        match k {
            Key::Up => self.move_sel(-1, len),
            Key::Down => self.move_sel(1, len),
            Key::Char('u') => self.change_ui_size(true, p),
            Key::Char('U') => self.change_ui_size(false, p),
            Key::Char(c @ ('t' | 'T' | '1'..='9')) => self.change_theme(*c, p),
            Key::Char(c @ ('w' | 'W')) => self.change_wallpaper(*c == 'w', p),
            Key::Char('r') => {
                self.hw.invalidate();
                self.ok("Looked at the hardware again.");
            }
            other => self.dev_act(DevTab::Display, other, p),
        }
    }

    /// The Display page: the text-size and theme cards, then the device rows.
    pub(super) fn render_display(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn UserPort) {
        let admin = p.is_admin();
        // a small page: the cards drop their hint line so the rows and the help line below keep their room
        let compact = Self::cards_need_compact(g, a.w, s, a.h - 130.0 * s, p);
        let mut y = a.y;
        y += self.render_ui_size_card(g, R::new(a.x, y, a.w, 74.0 * s), s, p, compact) + 10.0 * s;
        y += self.render_theme_card(g, R::new(a.x, y, a.w, 74.0 * s), s, compact, p) + 10.0 * s;
        self.draw_dev_rows(g, a, s, y, admin, NOTE);
    }
}
