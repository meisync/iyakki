//! AUTH: the drawing. Groups on the left (a list that scrolls), the chosen group's codes on the right as cards: service and account, the
//! code in big grouped digits, a bar for the seconds it has left and, in the last five, the next code; Copy, Edit, Move and Delete
//! beside it. Forms and questions are drawn over the page. Colours come from the shared palette, so every theme applies.

use super::*;
use crate::fx;
use crate::gfx::{Face, Gfx};
use crate::safeclamp::SafeClamp;
use crate::scan_core::Card;
use crate::ui::{DIM, ERR, MUTED, OK, R, TEXT};

const AMBER: [u8; 4] = rgb(255, 200, 90);

/// Text cut with an ellipsis to `w`.
fn put(g: &mut Gfx, x: f32, y: f32, size: f32, c: [u8; 4], t: &str, face: Face, w: f32) {
    let t = g.fit(size, t, face, w);
    g.text(x, y, size, c, &t, face);
}

/// A button's width the way `Gfx::button` works it out.
fn btn_w(g: &mut Gfx, size: f32, label: &str) -> f32 {
    g.text_width(size, label, Face::Bold) + size * 1.7
}

/// A question over the page: a title, a few lines of explanation, `y` to confirm and Esc to cancel.
fn confirm(g: &mut Gfx, area: R, s: f32, title: &str, body: &str, yes: &str) {
    let p = fx::modal(g, area, s, 560.0 * s, 230.0 * s, ERR, true);
    let w = p.w - 48.0 * s;
    put(g, p.x + 24.0 * s, p.y + 26.0 * s, 19.0 * s, TEXT, title, Face::Bold, w);
    let mut y = p.y + 62.0 * s;
    for l in g.wrap(13.5 * s, body, Face::Sans, w).into_iter().take(4) {
        g.text(p.x + 24.0 * s, y, 13.5 * s, MUTED, &l, Face::Sans);
        y += 19.0 * s;
    }
    let by = p.y + p.h - 52.0 * s;
    g.button(p.x + 24.0 * s, by, 14.0 * s, yes, ERR, Event::Key(Key::Char('y')));
    g.button(p.x + 24.0 * s + 150.0 * s, by, 14.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
    g.text(p.x + 24.0 * s, by - 20.0 * s, 12.0 * s, DIM, "Press y to confirm, or Esc to cancel.", Face::Sans);
}

impl AuthApp {
    pub(super) fn draw(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        if self.ready() {
            self.draw_page(g, area, s);
        } else {
            self.draw_locked(g, area, s);
        }
        self.draw_over(g, area, s, tick);
    }

    /// The vault cannot be used: why, and what to do. No names, no codes.
    fn draw_locked(&mut self, g: &mut Gfx, area: R, s: f32) {
        let w = (560.0 * s).min(area.w - 24.0 * s);
        let x = area.x + (area.w - w) / 2.0;
        let (title, body, btn) = match &self.load {
            Load::Problem(m) => ("Your codes could not be read", format!("{m} Nothing was changed. Your vault still holds them."), "Try again"),
            _ => ("Your vault is locked", locked_words().to_string(), "Try again"),
        };
        let mut y = area.y + (area.h * 0.12).max(14.0 * s);
        let box_r = R::new(x, y, w, (area.h - (y - area.y) - 10.0 * s).max(60.0 * s));
        fx::glass(g, box_r.x, box_r.y, box_r.w, box_r.h, 12.0 * s, ACCENT);
        let (ix, iw) = (box_r.x + 18.0 * s, box_r.w - 36.0 * s);
        y += 16.0 * s;
        put(g, ix, y, 20.0 * s, TEXT, title, Face::Bold, iw);
        y += 34.0 * s;
        let lh = 19.0 * s;
        let room = ((box_r.y + box_r.h - y - 52.0 * s) / lh).max(1.0) as usize;
        for l in g.wrap(14.0 * s, &body, Face::Sans, iw).into_iter().take(room) {
            g.text(ix, y, 14.0 * s, MUTED, &l, Face::Sans);
            y += lh;
        }
        g.button(ix, (box_r.y + box_r.h - 44.0 * s).max(y + 4.0 * s), 14.0 * s, btn, ACCENT, Event::Tab(T_RETRY));
        if let Some((m, good)) = self.note_text() {
            if m != locked_words() {
                put(g, ix + 140.0 * s, (box_r.y + box_r.h - 40.0 * s).max(y + 8.0 * s), 13.0 * s, if good { OK } else { ERR }, m, Face::Sans, iw - 140.0 * s);
            }
        }
    }

    fn draw_page(&mut self, g: &mut Gfx, area: R, s: f32) {
        let pad = 10.0 * s;
        let now = (self.now)();
        let clock_ok = self.clock_ok();

        // ---- the groups
        let lw = (area.w * 0.27).clamp_safe(100.0 * s, 210.0 * s).min(area.w * 0.42);
        put(g, area.x + pad, area.y + pad, 11.5 * s, DIM, "Groups", Face::Bold, lw - pad);
        let btn_h = 13.0 * s * 2.1;
        let view = R::new(area.x + pad * 0.6, area.y + pad + 20.0 * s, lw - pad * 0.6, (area.h - pad * 2.0 - 20.0 * s - btn_h - 8.0 * s).max(30.0 * s));
        let n = self.book.groups.len();
        let rh = (view.h / n.max(1) as f32).clamp_safe(26.0 * s, 38.0 * s);
        self.gscroll.layout(view, n as f32 * rh, rh, s);
        let names: Vec<(String, usize)> = self.book.groups.iter().map(|gn| (gn.clone(), self.book.count_in(gn))).collect();
        g.clipped(view, |g| {
            for (i, (name, count)) in names.iter().enumerate() {
                let r = R::new(view.x, view.y + i as f32 * rh - self.gscroll.offset, view.w - if self.gscroll.scrolls() { 14.0 * s } else { 0.0 }, rh - 3.0 * s);
                let sel = i == self.group;
                fx::row_at(g, r.x, r.y, r.w, r.h, 7.0 * s, sel, g.hovered(r), ACCENT);
                if sel && self.focus == Focus::Groups {
                    g.rrect_outline(r.x, r.y, r.w, r.h, 7.0 * s, 2.0, ACCENT);
                }
                let size = (14.0 * s).min(r.h * 0.55);
                let cw = g.text_width(size, &count.to_string(), Face::Sans);
                let ty = r.y + (r.h - g.line_height(size, Face::Sans)) / 2.0;
                put(g, r.x + 10.0 * s, ty, size, if sel { TEXT } else { MUTED }, name, if sel { Face::Bold } else { Face::Sans }, r.w - cw - 26.0 * s);
                g.text(r.x + r.w - cw - 8.0 * s, ty, size, DIM, &count.to_string(), Face::Sans);
                g.hit(r, Event::Tab(T_GROUP + i), name);
            }
        });
        self.gscroll.draw_bar(g, s, A_GBAR, ACCENT);
        g.button(area.x + pad, area.y + area.h - pad - btn_h, 13.0 * s, "+ New group", ACCENT, Event::Tab(T_NEWGROUP));

        // ---- the head of the page
        let (x, w) = (area.x + lw + pad, (area.w - lw - 2.0 * pad).max(40.0));
        let (hs, ts, bs) = (17.0 * s, 12.5 * s, 12.5 * s);
        let mut y = area.y + pad;
        let gname = self.group_name();
        put(g, x, y, hs, TEXT, &gname, Face::Bold, w);
        y += g.line_height(hs, Face::Bold) + 2.0 * s;
        let count = self.entries_len();
        put(g, x, y, ts, MUTED, &format!("{count} code{}", if count == 1 { "" } else { "s" }), Face::Sans, w);
        y += g.line_height(ts, Face::Sans) + 6.0 * s;

        let mut btns: Vec<(&str, usize, [u8; 4])> = vec![("Add code", T_ADD, ACCENT), ("Paste link", T_LINK, MUTED), ("Scan QR", T_SCAN, MUTED), ("Rename group", T_RENAME, MUTED)];
        if n > 1 {
            btns.push(("Delete group", T_DELGROUP, ERR));
        }
        let (bh, mut bx) = (bs * 2.1, 0.0f32);
        for (label, id, c) in btns {
            let bw = btn_w(g, bs, label);
            if bx > 0.0 && bx + bw > w {
                bx = 0.0;
                y += bh + 5.0 * s;
            }
            g.button(x + bx, y, bs, label, c, Event::Tab(id));
            bx += bw + 8.0 * s;
        }
        y += bh + 8.0 * s;

        // a note ("Copied."), or the clock warning
        if !clock_ok {
            let t = "The clock of this computer is not set, so no codes can be shown. Connect to the network and it sets itself, or set the time in Settings. Your codes are safe in your vault.";
            let lines = g.wrap(ts, t, Face::Sans, w);
            for l in lines.iter().take(3) {
                g.text(x, y, ts, AMBER, l, Face::Sans);
                y += g.line_height(ts, Face::Sans);
            }
            y += 6.0 * s;
        } else if let Some((m, good)) = self.note_text() {
            put(g, x, y, ts, if good { OK } else { ERR }, m, Face::Sans, w);
            y += g.line_height(ts, Face::Sans) + 6.0 * s;
        }

        // ---- the codes
        let view = R::new(x, y, w, (area.y + area.h - y - pad).max(30.0 * s));
        if count == 0 {
            let t = "No codes in this group yet. Press Add code to type one, Paste link to use a link from the clipboard, or Scan QR to read the site's QR code with the camera.";
            for (i, l) in g.wrap(13.5 * s, t, Face::Sans, view.w).into_iter().take(4).enumerate() {
                g.text(view.x, view.y + i as f32 * 19.0 * s, 13.5 * s, DIM, &l, Face::Sans);
            }
            self.scroll.layout(view, 0.0, 20.0 * s, s);
            return;
        }
        let ch = if area.h < 420.0 * s { 78.0 * s } else { 92.0 * s };
        let gap = 6.0 * s;
        self.scroll.layout(view, count as f32 * (ch + gap), ch + gap, s);
        self.sel = self.sel.min(count - 1);
        let key = (self.group, self.sel);
        if self.shown_sel != key {
            self.shown_sel = key;
            self.scroll.show(self.sel as f32 * (ch + gap), self.sel as f32 * (ch + gap) + ch);
        }
        let bar = if self.scroll.scrolls() { 18.0 * s } else { 0.0 };
        let entries: Vec<authcodes_view::Row> = self.book.in_group(&gname).map(|e| authcodes_view::Row::of(e, now, clock_ok)).collect();
        g.clipped(view, |g| {
            for (i, row) in entries.iter().enumerate() {
                let r = R::new(view.x, view.y + i as f32 * (ch + gap) - self.scroll.offset, view.w - bar, ch);
                if r.y > view.y + view.h || r.y + r.h < view.y {
                    continue;
                }
                self.draw_card(g, r, s, i, row);
            }
        });
        self.scroll.draw_bar(g, s, A_BAR, ACCENT);
    }

    fn draw_card(&self, g: &mut Gfx, r: R, s: f32, i: usize, row: &authcodes_view::Row) {
        let sel = i == self.sel;
        fx::row_at(g, r.x, r.y, r.w, r.h, 9.0 * s, sel, g.hovered(r), ACCENT);
        if sel && self.focus == Focus::Codes {
            g.rrect_outline(r.x, r.y, r.w, r.h, 9.0 * s, 2.0, ACCENT);
        }
        g.hit(r, Event::Tab(T_SEL + i), "code card");

        // the four buttons on the right, two by two
        let bs = 12.0 * s;
        let bh = bs * 2.1;
        let labels = [["Copy", "Edit"], ["Move", "Delete"]];
        let ids = [[T_COPY + i, T_EDIT + i], [T_MOVE + i, T_DEL + i]];
        let colw = [labels[0][0], labels[1][0]].iter().map(|l| btn_w(g, bs, l)).fold(0.0, f32::max);
        let colw2 = [labels[0][1], labels[1][1]].iter().map(|l| btn_w(g, bs, l)).fold(0.0, f32::max);
        let block = colw + colw2 + 6.0 * s;
        let bx = r.x + r.w - block - 10.0 * s;
        let by = r.y + (r.h - (bh * 2.0 + 6.0 * s)) / 2.0;
        for (ri, rowl) in labels.iter().enumerate() {
            g.button(bx, by + ri as f32 * (bh + 6.0 * s), bs, rowl[0], if ri == 0 { ACCENT } else { MUTED }, Event::Tab(ids[ri][0]));
            g.button(bx + colw + 6.0 * s, by + ri as f32 * (bh + 6.0 * s), bs, rowl[1], if ri == 1 { ERR } else { MUTED }, Event::Tab(ids[ri][1]));
        }

        // names, code, bar
        let (tx, tw) = (r.x + 14.0 * s, (bx - r.x - 24.0 * s).max(40.0));
        let ns = 14.5 * s;
        let issuer = if row.issuer.is_empty() { row.account.as_str() } else { row.issuer.as_str() };
        let iw = g.text_width(ns, issuer, Face::Bold).min(tw * 0.7);
        put(g, tx, r.y + 8.0 * s, ns, TEXT, issuer, Face::Bold, tw * 0.7);
        if !row.issuer.is_empty() && !row.account.is_empty() {
            put(g, tx + iw + 8.0 * s, r.y + 9.0 * s, 12.0 * s, MUTED, &row.account, Face::Sans, (tw - iw - 8.0 * s).max(10.0));
        }
        let cs = (r.h * 0.36).min(30.0 * s);
        let cy = r.y + 8.0 * s + g.line_height(ns, Face::Bold) + 2.0 * s;
        let (text, colour) = match &row.code {
            Some(c) => (totp::grouped(c), if row.low { AMBER } else { TEXT }),
            None if !row.clock => ("------".to_string(), DIM),
            None => ("damaged".to_string(), ERR),
        };
        let cwid = g.text_width(cs, &text, Face::Mono);
        put(g, tx, cy, cs, colour, &text, Face::Mono, tw);
        if row.code.is_some() {
            g.hit(R::new(tx, cy, cwid.min(tw), g.line_height(cs, Face::Mono)), Event::Tab(T_COPY + i), "code");
        }
        if let Some(next) = &row.next {
            let t = format!("next {}", totp::grouped(next));
            if cwid + 16.0 * s + g.text_width(12.5 * s, &t, Face::Sans) <= tw {
                g.text(tx + cwid + 16.0 * s, cy + (g.line_height(cs, Face::Mono) - g.line_height(12.5 * s, Face::Sans)) / 2.0, 12.5 * s, DIM, &t, Face::Sans);
            }
        }
        if row.code.is_some() {
            let by = r.y + r.h - 14.0 * s;
            let secs_w = 34.0 * s;
            let track = R::new(tx, by, (tw - secs_w).max(20.0), 5.0 * s);
            g.rrect(track.x, track.y, track.w, track.h, 2.5 * s, rgb(24, 30, 54));
            let frac = (row.left as f32 / row.period.max(1) as f32).clamp(0.0, 1.0);
            g.rrect(track.x, track.y, (track.w * frac).max(5.0 * s), track.h, 2.5 * s, if row.low { AMBER } else { ACCENT });
            g.text(track.x + track.w + 8.0 * s, by - 4.0 * s, 12.0 * s, if row.low { AMBER } else { DIM }, &format!("{} s", row.left), Face::Sans);
        }
    }

    /// Forms, questions and the camera card, drawn over the page.
    fn draw_over(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        // a form has fixed rows: on a short window it is drawn smaller rather than cut off
        let k = ((area.h - 16.0 * s) / (440.0 * s)).clamp_safe(0.6, 1.0);
        let blink = super::super::blink(tick);
        match &self.mode {
            Mode::Browse => {}
            Mode::Add(f, _) | Mode::Edit(_, f) | Mode::Move(_, f) | Mode::NewGroup(f) | Mode::Rename(_, f) => f.render(g, area, s * k, ACCENT, blink),
            Mode::DelGroup(name) => {
                let first = self.book.groups.iter().find(|x| *x != name).cloned().unwrap_or_default();
                let n = self.book.count_in(name);
                let body = if self.book.groups.len() < 2 {
                    "This is your only group, so it cannot be deleted.".to_string()
                } else if n == 0 {
                    "The group is empty. Nothing else is lost.".to_string()
                } else {
                    format!("Its {n} code{} are not deleted: they move to {first}.", if n == 1 { "" } else { "s" })
                };
                confirm(g, area, s, &format!("Delete the group {name}?"), &body, "Delete group");
            }
            Mode::DelEntry(id) => {
                let t = self.book.entry(*id).map(|e| e.title()).unwrap_or_default();
                confirm(g, area, s, &format!("Delete the code for {t}?"), "The secret is removed from your vault and cannot be shown again. Make sure you can still sign in to that account some other way (a recovery code, or a new setup from its site) before you delete.", "Delete");
            }
            Mode::Scan(sess) => {
                let card = Card {
                    ask_title: "Turn on the camera?",
                    ask_body: "IYAKKI will use the camera only to read the QR code a site shows when you set up one-time codes. You will see what was read before anything is saved. The pictures are looked at in memory, never saved and never sent. The camera turns off as soon as a code is read, when you press Esc, when this closes, and after one minute.",
                    hold_note: "Hold the site's QR code inside the corners, about 20 cm from the camera. Nothing is saved.",
                    allow: T_ALLOW,
                    deny: T_DENY,
                    stop: T_STOP,
                };
                scan_core::draw_card(g, area, s, tick, sess, &card);
                if let Some((m, _)) = sess.said() {
                    put(g, area.x + 20.0 * s, area.y + area.h - 22.0 * s, 13.0 * s, AMBER, m, Face::Sans, area.w - 40.0 * s);
                }
            }
        }
    }
}

/// What one card shows, worked out once per frame.
mod authcodes_view {
    use iyakki_agent::authcodes::Entry;
    use iyakki_agent::totp;

    pub struct Row {
        pub issuer: String,
        pub account: String,
        pub code: Option<String>,
        pub next: Option<String>,
        pub left: u32,
        pub period: u32,
        pub low: bool,
        pub clock: bool,
    }

    impl Row {
        pub fn of(e: &Entry, now: i64, clock: bool) -> Row {
            let left = totp::seconds_left(now, e.period);
            let code = if clock { e.code_at(now) } else { None };
            // the next code only in the last seconds, and only when the step is long enough for that to mean something
            let low = left <= super::super::NEXT_AT && e.period > super::super::NEXT_AT;
            let next = if low { code.as_ref().and_then(|_| e.code_at(now + left as i64)) } else { None };
            Row { issuer: e.issuer.clone(), account: e.account.clone(), code, next, left, period: e.period, low, clock }
        }
    }
}
