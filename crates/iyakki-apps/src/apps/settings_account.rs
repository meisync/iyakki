//! Settings: the Account page (lane ST-A). It reads the person's limits through [`AccountPort`] and never names the session; the text size,
//! theme and wallpaper cards take the device port's `UserPort`.

use super::account_port::AccountPort;
use super::*;

impl SettingsApp {
    pub(super) fn handle_account(&mut self, k: &Key, p: &dyn AccountPort) {
        match k {
            Key::Char('p') => {
                let f = Form::new("Change password").secret("Current password", "").secret("New password (min 8)", "").secret("Repeat new password", "");
                self.open_form(FormKind::Password, f);
            }
            Key::Char('u') => self.change_ui_size(true, p),
            Key::Char('U') => self.change_ui_size(false, p),
            Key::Char(c @ ('t' | 'T' | '1'..='9')) => self.change_theme(*c, p),
            Key::Char(c @ ('w' | 'W')) => self.change_wallpaper(*c == 'w', p),
            _ => {}
        }
    }

    pub(super) fn render_account(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn AccountPort) {
        let f = p.account_facts();
        let lines = [
            format!("Signed in as   {}", p.user()),
            format!("Role           {}", f.role),
            format!("Your files     up to {} MiB", f.vfs_quota_mb),
            format!("Each program   up to {} MiB of memory", f.max_memory_mb),
            format!("Apps           {}", f.apps.join(", ")),
            String::new(),
            "Choose Change password to set a new one. Your personal vault is re-keyed for you.".into(),
            "If an administrator resets your password, your personal vault cannot be recovered: that is on purpose, so nobody else can read it.".into(),
        ];
        // the text size card sits at the bottom of the page; the text above it is cut off rather than run into it
        let compact = Self::cards_need_compact(g, a.w, s, a.h, p);
        let card_h = Self::size_card_height(g, a.w, s, compact);
        let theme_h = Self::theme_card_height(g, a.w, s, compact, p);
        // the height of the text above the cards (each line may take up to three rows)
        let text_h: f32 = lines.iter().enumerate().map(|(i, l)| {
            let face = if i < 5 { Face::Mono } else { Face::Sans };
            g.wrap(15.0 * s, l, face, a.w).len().min(3) as f32 * 22.0 * s + if l.is_empty() { 10.0 * s } else { 0.0 }
        }).sum();
        // in a small window or at big text the text and the two cards do not fit under each other: they follow each other and the page scrolls
        let flow = text_h + 12.0 * s + theme_h + 10.0 * s + card_h > a.h;
        let (theme_card, card) = if flow {
            let ty = a.y + text_h + 12.0 * s;
            (R::new(a.x, ty, a.w, theme_h), R::new(a.x, ty + theme_h + 10.0 * s, a.w, card_h))
        } else {
            (R::new(a.x, (a.y + a.h - card_h - theme_h - 10.0 * s).max(a.y), a.w, theme_h), R::new(a.x, (a.y + a.h - card_h).max(a.y), a.w, card_h))
        };
        let mut y = a.y;
        'text: for (i, l) in lines.iter().enumerate() {
            let col = if i < 5 { TEXT } else { MUTED };
            let face = if i < 5 { Face::Mono } else { Face::Sans };
            for w in g.wrap(15.0 * s, l, face, a.w).iter().take(3) {
                if !flow && y + 22.0 * s > theme_card.y - 6.0 * s {
                    break 'text;
                }
                g.text(a.x, y, 15.0 * s, col, w, face);
                y += 22.0 * s;
            }
            if l.is_empty() {
                y += 10.0 * s;
            }
        }
        self.render_theme_card(g, theme_card, s, compact, p);
        self.render_ui_size_card(g, card, s, p, compact);
    }
}
