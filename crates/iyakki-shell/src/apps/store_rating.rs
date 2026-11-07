//! The "Your rating" row on a package's page (Discover and Installed): five stars and a short private comment, kept per account in
//! the data folder (`iyakki_agent::store::ratings`). It reaches nobody: there is no network code in this file or in the store it
//! uses. Mouse and touch: click a star, "Comment" or "Forget my rating". Keyboard: 1 to 5 set the stars (the same number again takes
//! them away), w writes the comment, f forgets the rating. A comment that holds a credential is refused with a plain sentence.

use super::*;

pub(super) const TAB_STAR: usize = 5700; // 5701..=5705 are 1 to 5 stars
pub(super) const TAB_WRITE: usize = 5710;
pub(super) const TAB_FORGET: usize = 5711;

/// What a page is about: the item key, its name (for the audit) and the version on the page.
struct Subject {
    key: String,
    name: String,
    title: String,
    version: String,
}

impl StoreApp {
    fn rating_subject(&self) -> Option<Subject> {
        if self.section == Section::Discover {
            let it = self.current()?;
            let e = &it.entry;
            return Some(Subject { key: iyakki_agent::store::organize::item_key(&e.kind, &e.name), name: e.name.clone(), title: it.title().to_string(), version: e.version.clone() });
        }
        let e = self.iselected()?;
        Some(Subject { key: Self::okey(&e), name: e.name.clone(), title: self.shown_title(&e).to_string(), version: e.version.clone() })
    }

    /// Draw the row at (x, y) and return its height. Registers the stars and the two buttons as clickable.
    pub(super) fn render_rating(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32) -> f32 {
        let Some(sub) = self.rating_subject() else { return 0.0 };
        let mine = self.rate.get(&sub.key).cloned();
        let stars = mine.as_ref().map_or(0, |r| r.stars);
        let bh = 14.0 * 2.1 * s;
        let cell = 28.0 * s;
        let label_size = 13.0 * s;
        let label = "Your rating";
        let lw = g.text_width(label_size, label, Face::Bold);
        g.text(x, y + (bh - g.line_height(label_size, Face::Bold)) / 2.0, label_size, MUTED, label, Face::Bold);
        let mut sx = x + lw + 12.0 * s;
        for n in 1..=5u8 {
            let on = n <= stars;
            let r = R::new(sx, y + (bh - cell) / 2.0, cell, cell);
            let hot = g.hovered(r);
            let col = if on { AMBER } else if hot { fx::with_alpha(AMBER, 170) } else { fx::with_alpha(MUTED, 150) };
            star(g, r.x + cell / 2.0, r.y + cell / 2.0 + 1.0 * s, cell * 0.46, on, col, s);
            g.hit(r, Event::Tab(TAB_STAR + n as usize), &format!("Rate {n} star{} ({n})", if n == 1 { "" } else { "s" }));
            sx += cell + 2.0 * s;
        }
        let mut bx = sx + 10.0 * s;
        let mut by = y;
        let mut bottom = y + bh;
        let has_comment = mine.as_ref().is_some_and(|r| !r.comment.is_empty());
        let mut buttons = vec![(if has_comment { "Edit comment (w)" } else { "Add a comment (w)" }, MUTED, TAB_WRITE)];
        if mine.is_some() {
            buttons.push(("Forget my rating (f)", ERR, TAB_FORGET));
        }
        for (label, col, ev) in buttons {
            let bw = g.text_width(14.0 * s, label, Face::Bold) + 14.0 * 1.7 * s;
            if bx + bw > x + w && bx > x {
                bx = x;
                by += bh + 6.0 * s;
                bottom = by + bh;
            }
            g.button(bx, by, 14.0 * s, label, col, Event::Tab(ev));
            bx += bw + 8.0 * s;
        }
        let mut ty = bottom + 6.0 * s;
        let size = 12.0 * s;
        match mine.as_ref().filter(|r| !r.comment.is_empty()) {
            Some(r) => {
                let text = format!("“{}”", r.comment);
                let lines = g.wrap(size, &text, Face::Sans, w);
                for (i, l) in lines.iter().take(2).enumerate() {
                    let shown = if i == 1 && lines.len() > 2 { g.fit(size, &lines[1..].join(" "), Face::Sans, w) } else { l.clone() };
                    g.text(x, ty, size, TEXT, &shown, Face::Sans);
                    ty += g.line_height(size, Face::Sans) + 2.0 * s;
                }
            }
            None => {
                let note = g.fit(size, "Private to you and kept on this computer; it is never sent anywhere. Keys: 1 to 5, w, f.", Face::Sans, w);
                g.text(x, ty, size, DIM, &note, Face::Sans);
                ty += g.line_height(size, Face::Sans) + 2.0 * s;
            }
        }
        ty - y + 8.0 * s
    }

    /// Clicks and keys of the row; true when the event was for it. Only on a package's page.
    pub(super) fn rating_event(&mut self, ev: &Event, cx: &AppCx) -> bool {
        let on_page = if self.section == Section::Discover { self.detail.is_some() } else { self.section == Section::Installed && self.idetail.is_some() };
        if !on_page {
            return false;
        }
        match ev {
            Event::Tab(n) if (TAB_STAR + 1..=TAB_STAR + 5).contains(n) => self.set_my_stars((n - TAB_STAR) as u8, cx),
            Event::Tab(TAB_WRITE) | Event::Key(Key::Char('w')) => self.edit_comment(),
            Event::Tab(TAB_FORGET) | Event::Key(Key::Char('f')) => self.forget_rating(cx),
            Event::Key(Key::Char(c @ '1'..='5')) => self.set_my_stars(*c as u8 - b'0', cx),
            _ => return false,
        }
        true
    }

    fn set_my_stars(&mut self, n: u8, cx: &AppCx) {
        let Some(sub) = self.rating_subject() else { return };
        let now = self.rate.get(&sub.key).map_or(0, |r| r.stars);
        let to = if now == n { 0 } else { n };
        match self.rate.set_stars(&sub.key, to, &sub.version) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "store.rate", &sub.name);
                self.org_rev += 1;
                self.say(if to == 0 { format!("Your stars for “{}” are taken away.", sub.title) } else { format!("You gave “{}” {to} star{}.", sub.title, if to == 1 { "" } else { "s" }) }, true);
            }
            Err(e) => self.say(e, false),
        }
    }

    fn edit_comment(&mut self) {
        let Some(sub) = self.rating_subject() else { return };
        let cur = self.rate.get(&sub.key).map(|r| r.comment.clone()).unwrap_or_default();
        self.open_edit(
            manage::EditKind::Comment(sub.key),
            &format!("Your comment on “{}”", sub.title),
            "A note for yourself, at most 500 characters. It stays on this computer and is never sent anywhere. Leave empty to remove it. Do not put passwords or keys in it.",
            &cur,
        );
    }

    /// Enter in the comment dialog.
    pub(super) fn save_comment(&mut self, key: &str, text: &str, cx: &AppCx) {
        let Some(sub) = self.rating_subject().filter(|s| s.key == key) else { return };
        match self.rate.set_comment(key, text, &sub.version) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "store.rate-comment", &sub.name);
                self.org_rev += 1;
                self.say(if text.is_empty() { "Your comment is removed." } else { "Your comment is saved on this computer." }, true);
            }
            Err(e) => self.say(e, false),
        }
    }

    fn forget_rating(&mut self, cx: &AppCx) {
        let Some(sub) = self.rating_subject() else { return };
        match self.rate.forget(&sub.key) {
            Ok(true) => {
                cx.audit.record(&cx.session.user, "store.rate-forget", &sub.name);
                self.org_rev += 1;
                self.say(format!("Your rating and comment for “{}” are forgotten.", sub.title), true);
            }
            Ok(false) => self.say("There is no rating of yours to forget.", false),
            Err(e) => self.say(e, false),
        }
    }
}

/// A five-point star centred on (cx, cy), filled or as an outline.
fn star(g: &mut Gfx, cx: f32, cy: f32, r: f32, filled: bool, col: [u8; 4], s: f32) {
    let pts: Vec<(f32, f32)> = (0..10)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            let rr = if i % 2 == 0 { r } else { r * 0.42 };
            (cx + rr * a.cos(), cy + rr * a.sin())
        })
        .collect();
    if filled {
        g.polygon(&pts, col);
    } else {
        for i in 0..10 {
            let (a, b) = (pts[i], pts[(i + 1) % 10]);
            g.line(a.0, a.1, b.0, b.1, 1.4 * s, col);
        }
    }
}
