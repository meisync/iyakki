//! Settings: the cards page code moved out of `settings.rs` unchanged (plan: docs/development/SETTINGS-EXTRACTION-PLAN.md, step S0).

use super::*;
use super::super::settings_device_port::UserPort;

impl SettingsApp {
    /// The size of text and buttons, for whoever is signed in (every account has it on the Account page, administrators also
    /// on Display). Returns the height used.
    /// The text-size card's buttons: (label, level index, width). The card keeps them on the title row when they fit beside the
    /// sentence, and otherwise wraps them below it (so none runs off a narrow window).
    pub(super) fn size_card_buttons(g: &mut Gfx, s: f32) -> Vec<(String, usize, f32)> {
        crate::uisize::LEVELS.iter().enumerate().map(|(i, (name, f))| {
            let label = format!("{name} {}%", (f * 100.0).round());
            let w = g.text_width(13.0 * s, &label, Face::Bold) + 13.0 * s * 1.7;
            (label, i, w)
        }).collect()
    }

    /// Compact cards (no hint line; the title shares the first row with the buttons): where each button goes, relative to the
    /// card, and the card's height. Used when the roomy cards would not fit the page.
    pub(super) fn compact_card_layout(title_w: f32, widths: &[f32], w: f32, s: f32) -> (Vec<(f32, f32)>, f32) {
        let (left, right) = (18.0 * s, w - 18.0 * s);
        let (mut x, mut row) = (left + title_w + 14.0 * s, 0usize);
        let mut at = Vec::new();
        for bw in widths {
            if x > left && x + bw > right {
                row += 1;
                x = left;
            }
            at.push((x, 5.0 * s + row as f32 * 34.0 * s));
            x += bw + 6.0 * s;
        }
        (at, 12.0 * s + (row + 1) as f32 * 34.0 * s)
    }

    /// Whether the two cards must be drawn compact to fit under each other in `avail` pixels.
    pub(super) fn cards_need_compact(g: &mut Gfx, w: f32, s: f32, avail: f32, p: &dyn UserPort) -> bool {
        Self::size_card_height(g, w, s, false) + Self::theme_card_height(g, w, s, false, p) + 10.0 * s > avail
    }

    /// How tall the text-size card is at this width.
    pub(super) fn size_card_height(g: &mut Gfx, w: f32, s: f32, compact: bool) -> f32 {
        let btns = Self::size_card_buttons(g, s);
        if compact {
            let tw = g.text_width(16.0 * s, "Text and button size", Face::Bold);
            return Self::compact_card_layout(tw, &btns.iter().map(|b| b.2).collect::<Vec<_>>(), w, s).1;
        }
        let total: f32 = btns.iter().map(|b| b.2 + 6.0 * s).sum();
        let note = g.text_width(13.0 * s, Self::SIZE_CARD_NOTE, Face::Sans);
        if total + note + 54.0 * s <= w {
            return 74.0 * s;
        }
        let (mut rows, mut x) = (1usize, 0.0f32);
        for b in &btns {
            if x > 0.0 && x + b.2 > w - 36.0 * s {
                rows += 1;
                x = 0.0;
            }
            x += b.2 + 6.0 * s;
        }
        64.0 * s + rows as f32 * 34.0 * s
    }

    pub(super) fn render_ui_size_card(&mut self, g: &mut Gfx, card: R, s: f32, p: &dyn UserPort, compact: bool) -> f32 {
        let cur = crate::uisize::load(&p.data_dir(), &p.user());
        let card = R::new(card.x, card.y, card.w, Self::size_card_height(g, card.w, s, compact));
        crate::fx::glass(g, card.x, card.y, card.w, card.h, 14.0 * s, ACCENT);
        let title = g.fit(16.0 * s, "Text and button size", Face::Bold, card.w - 36.0 * s);
        g.text(card.x + 18.0 * s, card.y + 10.0 * s, 16.0 * s, TEXT, &title, Face::Bold);
        if compact {
            let btns = Self::size_card_buttons(g, s);
            let tw = g.text_width(16.0 * s, "Text and button size", Face::Bold);
            let (at, _) = Self::compact_card_layout(tw, &btns.iter().map(|b| b.2).collect::<Vec<_>>(), card.w, s);
            for ((label, i, _), (ox, oy)) in btns.iter().zip(at) {
                if *i == cur {
                    g.button_off(card.x + ox, card.y + oy, 13.0 * s, label);
                } else {
                    g.button(card.x + ox, card.y + oy, 13.0 * s, label, MUTED, if *i > cur { Event::Key(Key::Char('u')) } else { Event::Key(Key::Char('U')) });
                }
            }
            return card.h;
        }
        let note = g.fit(13.0 * s, Self::SIZE_CARD_NOTE, Face::Sans, card.w - 36.0 * s);
        g.text(card.x + 18.0 * s, card.y + 34.0 * s, 13.0 * s, MUTED, &note, Face::Sans);
        let btns = Self::size_card_buttons(g, s);
        let one_row = card.h <= 74.0 * s + 0.5;
        let (mut bx, mut by) = if one_row { (card.x + card.w - 18.0 * s, card.y + 20.0 * s) } else { (card.x + 18.0 * s, card.y + 62.0 * s) };
        let ordered: Vec<&(String, usize, f32)> = if one_row { btns.iter().rev().collect() } else { btns.iter().collect() };
        for (label, i, w) in ordered {
            if one_row {
                bx -= w;
            } else if bx > card.x + 18.0 * s && bx + w > card.x + card.w - 18.0 * s {
                bx = card.x + 18.0 * s;
                by += 34.0 * s;
            }
            let ev = if *i > cur { Event::Key(Key::Char('u')) } else { Event::Key(Key::Char('U')) };
            if *i == cur {
                g.button_off(bx, by, 13.0 * s, label);
            } else {
                g.button(bx, by, 13.0 * s, label, MUTED, ev);
            }
            if one_row {
                bx -= 6.0 * s;
            } else {
                bx += w + 6.0 * s;
            }
        }
        card.h
    }

    /// The theme card's buttons: (label, key that chooses it, width). The chosen one carries a tick, so it does not rely on colour.
    pub(super) fn theme_card_buttons(g: &mut Gfx, s: f32, p: &dyn UserPort) -> Vec<(String, char, f32)> {
        Self::theme_choices(p).into_iter().map(|c| {
            let label = if c.chosen { format!("\u{2713} {}", c.label) } else { c.label.clone() };
            let w = g.text_width(13.0 * s, &label, Face::Bold) + 13.0 * s * 1.7;
            (label, c.key, w)
        }).collect()
    }

    /// Every theme the person can pick: the three built-in ones (keys 1 to 3), then the theme files that can be used (keys 4 to 9, at most
    /// six), the chosen one marked. A file that cannot be used is not offered (the chat tool `display_themes` says why).
    pub(super) fn theme_choices(p: &dyn UserPort) -> Vec<ThemeChoice> {
        let (dir, user) = (&p.data_dir(), &p.user());
        let custom = if crate::theme::is_custom() { crate::theme::load_choice(dir, user).and_then(|c| c.strip_prefix(iyakki_uikit::usertheme::CHOICE_PREFIX).map(String::from)) } else { None };
        let mut v: Vec<ThemeChoice> = crate::theme::ALL.iter().enumerate().map(|(i, t)| ThemeChoice { label: t.name().to_string(), key: (b'1' + i as u8) as char, builtin: Some(*t), file: None, chosen: custom.is_none() && *t == crate::theme::current_id() }).collect();
        for (k, r) in iyakki_uikit::usertheme::available(dir, user).into_iter().filter(|(_, r)| r.is_ok()).take(6) {
            if let Ok(u) = r {
                let n = v.len();
                v.push(ThemeChoice { label: u.name, key: (b'1' + n as u8) as char, builtin: None, chosen: custom.as_deref() == Some(k.as_str()), file: Some(k) });
            }
        }
        v
    }

    pub(super) fn theme_card_height(g: &mut Gfx, w: f32, s: f32, compact: bool, p: &dyn UserPort) -> f32 {
        let btns = Self::theme_card_buttons(g, s, p);
        if compact {
            let tw = g.text_width(16.0 * s, "Theme", Face::Bold);
            return Self::compact_card_layout(tw, &btns.iter().map(|b| b.2).collect::<Vec<_>>(), w, s).1;
        }
        let total: f32 = btns.iter().map(|b| b.2 + 6.0 * s).sum();
        let note = g.text_width(13.0 * s, Self::THEME_CARD_NOTE, Face::Sans);
        if total + note + 54.0 * s <= w {
            return 74.0 * s;
        }
        let (mut rows, mut x) = (1usize, 0.0f32);
        for b in &btns {
            if x > 0.0 && x + b.2 > w - 36.0 * s {
                rows += 1;
                x = 0.0;
            }
            x += b.2 + 6.0 * s;
        }
        64.0 * s + rows as f32 * 34.0 * s
    }

    pub(super) fn render_theme_card(&mut self, g: &mut Gfx, card: R, s: f32, compact: bool, p: &dyn UserPort) -> f32 {
        let card = R::new(card.x, card.y, card.w, Self::theme_card_height(g, card.w, s, compact, p));
        crate::fx::glass(g, card.x, card.y, card.w, card.h, 14.0 * s, ACCENT);
        let title = g.fit(16.0 * s, "Theme", Face::Bold, card.w - 36.0 * s);
        g.text(card.x + 18.0 * s, card.y + 10.0 * s, 16.0 * s, TEXT, &title, Face::Bold);
        if compact {
            let btns = Self::theme_card_buttons(g, s, p);
            let tw = g.text_width(16.0 * s, "Theme", Face::Bold);
            let (at, _) = Self::compact_card_layout(tw, &btns.iter().map(|b| b.2).collect::<Vec<_>>(), card.w, s);
            for ((label, k, _), (ox, oy)) in btns.iter().zip(at) {
                let chosen = label.starts_with('\u{2713}');
                g.button(card.x + ox, card.y + oy, 13.0 * s, label, if chosen { ACCENT } else { MUTED }, Event::Key(Key::Char(*k)));
            }
            return card.h;
        }
        let note = g.fit(13.0 * s, Self::THEME_CARD_NOTE, Face::Sans, card.w - 36.0 * s);
        g.text(card.x + 18.0 * s, card.y + 34.0 * s, 13.0 * s, MUTED, &note, Face::Sans);
        let btns = Self::theme_card_buttons(g, s, p);
        let one_row = card.h <= 74.0 * s + 0.5;
        let (mut bx, mut by) = if one_row { (card.x + card.w - 18.0 * s, card.y + 20.0 * s) } else { (card.x + 18.0 * s, card.y + 62.0 * s) };
        let ordered: Vec<&(String, char, f32)> = if one_row { btns.iter().rev().collect() } else { btns.iter().collect() };
        for (label, k, w) in ordered {
            if one_row {
                bx -= w;
            } else if bx > card.x + 18.0 * s && bx + w > card.x + card.w - 18.0 * s {
                bx = card.x + 18.0 * s;
                by += 34.0 * s;
            }
            let chosen = label.starts_with('\u{2713}');
            g.button(bx, by, 13.0 * s, label, if chosen { ACCENT } else { MUTED }, Event::Key(Key::Char(*k)));
            if one_row {
                bx -= 6.0 * s;
            } else {
                bx += w + 6.0 * s;
            }
        }
        card.h
    }
}
