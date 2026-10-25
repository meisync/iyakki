//! STORE -> Setup, the picture: a page of tiles (one per app, tool or driver, in category sections), the search, filter and category
//! bars, and the popup a tile opens. Nothing here decides what an item is or does (that is `store_setup.rs`); it only draws the
//! `Card` the model worked out and registers the click regions.
//!
//! The tile: a badge in the category's colour with the item's icon, the name, the category, one line of purpose, and the markers:
//! a tick when installed, a green dot and "Running" for a service that is up, an UPDATE pill with an up arrow when a newer version
//! exists, a bar while it installs, a warning triangle when it failed, a small star when it is recommended for this machine.

use super::*;
use crate::fx::{over, with_alpha};
use super::super::ACCENT;

const PAD: f32 = 14.0;
/// Tile height (before scaling); the width follows the columns.
const TILE_H: f32 = 140.0;
const HEAD_H: f32 = 32.0;

/// The columns of the grid for a content width: (columns, tile width, gap). One column on a phone, up to six on a big screen.
pub fn grid(w: f32, s: f32) -> (usize, f32, f32) {
    let gap = 12.0 * s;
    let min_w = 300.0 * s;
    let cols = (((w + gap) / (min_w + gap)).floor() as usize).clamp(1, 6);
    let tile_w = ((w - gap * (cols as f32 - 1.0)) / cols as f32).max(40.0 * s);
    (cols, tile_w, gap)
}

/// What a tile says about its state, in words and colour: "Running · v1.31", "Update to 1.38", "Not installed", "Failed".
pub fn status_text(card: &Card) -> (String, [u8; 4]) {
    match &card.state {
        CardState::Installed => {
            if let Some(u) = &card.update {
                return (format!("Update to {u}"), AMBER);
            }
            let t = if card.version.is_empty() { card.chip.clone() } else { format!("{} · {}", card.chip, card.version) };
            (t, if card.running == Some(false) { AMBER } else { OK })
        }
        other => (card.chip.clone(), state_color(other)),
    }
}

/// The popup's status line: "Installed v1.37 · Running · Update available v1.38", "Not installed", "Failed", ...
pub fn status_line(card: &Card) -> (String, [u8; 4]) {
    match &card.state {
        CardState::Installed => {
            let mut t = if card.item == Item::MacHost { "Model host added".to_string() } else { "Installed".to_string() };
            if !card.version.is_empty() {
                t = format!("{t} {}", card.version);
            }
            match card.running {
                Some(true) => t.push_str(" · Running"),
                Some(false) => t = format!("{t} · {}", card.chip),
                None => {}
            }
            if let Some(u) = &card.update {
                return (format!("{t} · Update available v{u}"), AMBER);
            }
            (t, if card.running == Some(false) { AMBER } else { OK })
        }
        other => (card.chip.clone(), state_color(other)),
    }
}

fn star(g: &mut Gfx, cx: f32, cy: f32, r: f32, c: [u8; 4]) {
    let pts: Vec<(f32, f32)> = (0..10)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            let rr = if i % 2 == 0 { r } else { r * 0.45 };
            (cx + rr * a.cos(), cy + rr * a.sin())
        })
        .collect();
    g.polygon(&pts, c);
}

fn tick_badge(g: &mut Gfx, cx: f32, cy: f32, d: f32, s: f32) {
    g.circle(cx, cy, d / 2.0, OK);
    let dark = fx::PANEL;
    g.line(cx - d * 0.27, cy + d * 0.02, cx - d * 0.07, cy + d * 0.22, 2.0 * s, dark);
    g.line(cx - d * 0.07, cy + d * 0.22, cx + d * 0.28, cy - d * 0.2, 2.0 * s, dark);
}

fn up_arrow(g: &mut Gfx, cx: f32, cy: f32, h: f32, s: f32, c: [u8; 4]) {
    g.line(cx, cy + h * 0.5, cx, cy - h * 0.2, 1.8 * s, c);
    g.polygon(&[(cx, cy - h * 0.55), (cx - h * 0.4, cy - h * 0.05), (cx + h * 0.4, cy - h * 0.05)], c);
}

fn warn_badge(g: &mut Gfx, cx: f32, cy: f32, d: f32, s: f32, c: [u8; 4]) {
    let r = d / 2.0;
    g.polygon(&[(cx, cy - r), (cx + r, cy + r * 0.85), (cx - r, cy + r * 0.85)], c);
    let sz = d * 0.62;
    let w = g.text_width(sz, "!", Face::Bold);
    g.text(cx - w / 2.0, cy - sz * 0.42 + 1.0 * s, sz, fx::PANEL, "!", Face::Bold);
}

/// One tile at `r` (screen coordinates).
fn draw_tile(g: &mut Gfx, r: R, s: f32, card: &Card, selected: bool) {
    let item = card.item;
    let idx = Item::ALL.iter().position(|i| *i == item).unwrap_or(0);
    let acc = Gk::of(item).accent();
    let hot = g.hovered(r);
    fx::row_at(g, r.x, r.y, r.w, r.h, 16.0 * s, selected, hot, acc);
    let mut label = item.title().to_string();
    label.push_str(&format!(", {}", card.chip));
    if card.reason.is_some() {
        label.push_str(", recommended");
    }
    g.hit(r, Event::Tab(TILE + idx), &label);
    let pad = PAD * s;
    // the badge: the item's icon in the category's colour
    let bd = 44.0 * s;
    let (bx, by) = (r.x + pad + 4.0 * s, r.y + pad);
    g.rrect(bx, by, bd, bd, 12.0 * s, over(fx::WELL, with_alpha(acc, 64)));
    g.rrect_outline(bx, by, bd, bd, 12.0 * s, 1.2 * s, with_alpha(acc, 170));
    g.icon(icon_of(item), bx + bd / 2.0, by + bd / 2.0, bd * 0.58, acc);
    // the markers at the top right, from the edge inwards: the state, then the update pill
    let mut rx = r.x + r.w - pad;
    let md = 22.0 * s;
    let (mcx, mcy) = (rx - md / 2.0, by + md / 2.0 + 1.0 * s);
    match &card.state {
        CardState::Installed => {
            tick_badge(g, mcx, mcy, md, s);
            rx -= md + 8.0 * s;
        }
        CardState::Failed | CardState::NotRunnable(_) => {
            warn_badge(g, mcx, mcy, md, s, ERR);
            rx -= md + 8.0 * s;
        }
        CardState::Installing(_) => {
            g.ring(mcx, mcy, md * 0.4, 2.5 * s, ACCENT);
            rx -= md + 8.0 * s;
        }
        CardState::Earlier | CardState::Elsewhere => {
            g.ring(mcx, mcy, md * 0.4, 2.2 * s, AMBER);
            rx -= md + 8.0 * s;
        }
        CardState::Missing | CardState::Checking => {}
    }
    let name_x = bx + bd + 10.0 * s;
    if card.update.is_some() {
        let sz = 11.0 * s;
        let word = "Update";
        let ww = g.text_width(sz, word, Face::Bold);
        let room = rx - name_x - 70.0 * s;
        let full = ww + sz * 2.6;
        let pw = if room >= full { full } else { sz * 2.0 };
        let ph = sz * 1.9;
        let px = rx - pw;
        g.rrect(px, by, pw, ph, ph / 2.0, over(fx::WELL, with_alpha(AMBER, 70)));
        g.rrect_outline(px, by, pw, ph, ph / 2.0, 1.0 * s.max(0.8), with_alpha(AMBER, 230));
        up_arrow(g, px + sz * 1.0, by + ph / 2.0, sz * 1.1, s, AMBER);
        if pw == full {
            g.text(px + sz * 1.75, by + (ph - g.line_height(sz, Face::Bold)) / 2.0, sz, AMBER, word, Face::Bold);
        }
        rx = px - 8.0 * s;
    }
    let name_w = (rx - name_x).max(30.0 * s);
    let name = g.fit(15.0 * s, item.title(), Face::Bold, name_w);
    g.text(name_x, by + 2.0 * s, 15.0 * s, TEXT, &name, Face::Bold);
    let cat = g.fit(11.5 * s, Gk::of(item).short(), Face::Sans, (r.x + r.w - pad - name_x).max(30.0 * s));
    g.text(name_x, by + 23.0 * s, 11.5 * s, DIM, &cat, Face::Sans);
    // one line or two of purpose
    let dw = r.w - 2.0 * pad;
    let mut lines = g.wrap(12.0 * s, short_about(item), Face::Sans, dw);
    if lines.len() > 2 {
        lines.truncate(2);
        let last = format!("{}…", lines[1].trim_end_matches(['.', ',', ' ']));
        lines[1] = g.fit(12.0 * s, &last, Face::Sans, dw);
    }
    let mut y = by + bd + 10.0 * s;
    for l in &lines {
        g.text(r.x + pad, y, 12.0 * s, MUTED, l, Face::Sans);
        y += 16.0 * s;
    }
    // the status row: the running dot, the words, the star
    let sy = r.y + r.h - 34.0 * s;
    let (word, col) = status_text(card);
    let mut sx = r.x + pad;
    if card.running == Some(true) && matches!(card.state, CardState::Installed) {
        g.circle(sx + 5.0 * s, sy + 8.0 * s, 8.0 * s, with_alpha(OK, 50));
        g.circle(sx + 5.0 * s, sy + 8.0 * s, 4.0 * s, OK);
        sx += 16.0 * s;
    }
    let star_room = if card.reason.is_some() { 20.0 * s } else { 0.0 };
    let word = g.fit(12.0 * s, &word, Face::Bold, (r.x + r.w - pad - sx - star_room).max(20.0 * s));
    g.text(sx, sy, 12.0 * s, col, &word, Face::Bold);
    if card.reason.is_some() {
        star(g, r.x + r.w - pad - 7.0 * s, sy + 8.0 * s, 8.0 * s, AMBER);
    }
    if let CardState::Installing(p) = &card.state {
        let bw = r.w - 2.0 * pad;
        let by2 = r.y + r.h - 13.0 * s;
        g.rrect(r.x + pad, by2, bw, 4.0 * s, 2.0 * s, rgb(24, 30, 54));
        let frac = p.map_or(0.15, |p| (p / 100.0).clamp_safe(0.02, 1.0));
        g.rrect(r.x + pad, by2, bw * frac, 4.0 * s, 2.0 * s, ACCENT);
    }
}

/// The chips of a row, wrapped when they do not fit: (label, hit id, lit). Returns where the next chip would go (x, y).
fn chip_row(g: &mut Gfx, x: f32, mut y: f32, w: f32, s: f32, chips: &[(String, usize, bool)], accent: [u8; 4]) -> (f32, f32) {
    let lh = 12.0 * s * 1.9;
    let mut cx_ = x;
    for (label, id, lit) in chips {
        let cwid = g.text_width(12.0 * s, label, Face::Bold) + 17.0 * s;
        if cx_ + cwid > x + w && cx_ > x {
            cx_ = x;
            y += lh + 6.0 * s;
        }
        fx::chip(g, cx_, y, s, label, accent, *lit);
        g.hit(R::new(cx_, y, cwid, lh), Event::Tab(*id), label);
        cx_ += cwid + 8.0 * s;
    }
    (cx_, y)
}

impl Setup {
    /// The machine-facts line ("Apple M2 Pro, 32 GB unified memory: about 21 GB usable for models; ...") and, when the AI hardware profile
    /// hides tools, the sentence that says how many and why, with the "Show all AI tools" link for an administrator. Returns the height used.
    /// The two lines as words: ("This machine: ...", "N AI tools are hidden because ...").
    pub fn ai_text(&self) -> (Option<String>, Option<String>) {
        let facts = self.machine.ai_line().map(|l| format!("This machine: {l}"));
        // the sentence stops before "An administrator can switch on ...": the link replaces that tail for an administrator
        let hidden = self.machine.hidden_ai_note().map(|note| note.split(". An administrator").next().unwrap_or(&note).to_string() + ".");
        (facts, hidden)
    }

    fn draw_ai_lines(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32) -> f32 {
        let mut used = 0.0;
        let sz = 12.5 * s;
        let (facts, hidden) = self.ai_text();
        if let Some(text) = facts {
            for l in g.wrap(sz, &text, Face::Sans, w).iter().take(2) {
                g.text(x, y + used, sz, MUTED, l, Face::Sans);
                used += 17.0 * s;
            }
            used += 4.0 * s;
        }
        if let Some(head) = hidden {
            let lines = g.wrap(sz, &head, Face::Sans, w);
            for l in lines.iter().take(2) {
                g.text(x, y + used, sz, AMBER, l, Face::Sans);
                used += 17.0 * s;
            }
            let label = "Show all AI tools";
            if self.admin {
                let tw = g.text_width(sz, label, Face::Bold);
                g.text(x, y + used, sz, ACCENT, label, Face::Bold);
                g.rect(x, y + used + sz * 1.2, tw, (1.0 * s).max(1.0), ACCENT);
                g.hit(R::new(x, y + used - 2.0 * s, tw, sz * 1.5), Event::Tab(SHOW_ALL_AI), label);
                let rest = " shows them again, marked not recommended.";
                g.text(x + tw, y + used, sz, MUTED, rest, Face::Sans);
            } else {
                g.text(x, y + used, sz, MUTED, "An administrator can show them again (Settings, Cloud mode).", Face::Sans);
            }
            used += 17.0 * s + 4.0 * s;
        }
        used
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.admin = cx.session.policy.manage_settings;
        if !self.started {
            self.enter(cx);
        }
        self.read_live(cx);
        self.fix_cursor();
        // the cluster switcher of the popups (read now and then, never slow)
        match self.host.as_mut() {
            Some(h) => h.tick(&|_| None),
            None => self.host = Some(crate::apps::cluster_host::ClusterHost::new(SWITCH, cx, "")),
        }
        self.cluster = self.host.as_ref().and_then(|h| h.effective().cloned());
        let (x, w) = (area.x, area.w);
        let mut y = area.y;
        // search box and the count
        let (n, m) = self.counts();
        let sw = (w * 0.5).clamp_safe(170.0 * s, 360.0 * s).min(w);
        let sr = R::new(x, y, sw, 34.0 * s);
        self.query.render(g, sr, self.searching && self.confirm.is_none() && self.popup.is_none(), s, blink(tick), Face::Sans, SEARCH);
        let count = if self.narrowed() { format!("{n} of {m} shown") } else { format!("{m} items") };
        let cw = g.text_width(12.0 * s, &count, Face::Sans);
        if sr.x + sr.w + 12.0 * s + cw <= x + w {
            g.text(sr.x + sr.w + 12.0 * s, y + 9.0 * s, 12.0 * s, MUTED, &count, Face::Sans);
        }
        y += 34.0 * s + 8.0 * s;
        let lh = 12.0 * s * 1.9;
        // the category bar: All and the categories this machine has
        let mut cats: Vec<(String, usize, bool)> = vec![("All".into(), CAT, self.cat.is_none())];
        for gk in self.categories() {
            let k = Gk::ALL.iter().position(|g| *g == gk).unwrap_or(0) + 1;
            cats.push((gk.short().into(), CAT + k, self.cat == Some(gk)));
        }
        let (_, ly) = chip_row(g, x, y, w, s, &cats, ACCENT);
        y = ly + lh + 8.0 * s;
        // the filter row and Install all recommended
        let filters: Vec<(String, usize, bool)> = Filter::ALL.iter().enumerate().map(|(i, f)| (f.label().to_string(), FILTER + i, *f == self.filter)).collect();
        let (mut cx_, ly) = chip_row(g, x, y, w, s, &filters, fx::CYAN);
        y = ly;
        let n_rec = self.eligible().into_iter().filter(|i| self.recommended(*i).is_some() && i.runs_as_job()).count();
        let all_label = if n_rec > 0 { format!("Install all recommended ({n_rec})") } else { "Install all recommended".to_string() };
        let bw = g.text_width(13.0 * s, &all_label, Face::Bold) + 13.0 * 1.7 * s;
        if cx_ + bw > x + w {
            cx_ = x;
            y += lh + 6.0 * s;
        }
        let bx = (x + w - bw).max(cx_);
        let bh = 13.0 * s * 2.1;
        let by_ = y + (lh - bh) / 2.0 + 1.0 * s;
        if n_rec > 0 && self.admin {
            g.button(bx, by_, 13.0 * s, &all_label, ACCENT, Event::Tab(INSTALL_ALL));
        } else {
            g.button_off(bx, by_, 13.0 * s, &all_label);
        }
        y += lh.max(bh) + 10.0 * s;
        y += self.draw_ai_lines(g, x, y, w, s);
        if !self.admin {
            let t = g.fit(12.5 * s, "You can look at everything here. Installing changes the whole machine: ask an administrator.", Face::Sans, w);
            g.text(x, y, 12.5 * s, AMBER, &t, Face::Sans);
            y += 20.0 * s;
        }
        if let (Some((m, ok)), None) = (self.note.clone(), self.popup) {
            let lines = g.wrap(12.5 * s, &m, Face::Sans, w - 24.0 * s);
            let c = if ok { OK } else { ERR };
            g.circle(x + 5.0 * s, y + 8.0 * s, 4.0 * s, c);
            for l in lines.iter().take(2) {
                g.text(x + 18.0 * s, y, 12.5 * s, TEXT, l, Face::Sans);
                y += 17.0 * s;
            }
            y += 6.0 * s;
        }
        let view = R::new(x, y, w, (area.y + area.h - y).max(40.0 * s));
        self.draw_grid(g, view, s);
        if let Some(item) = self.popup {
            self.draw_popup(g, area, s, item);
        }
        if let Some(c) = &self.confirm {
            let lines = Setup::confirm_lines(c);
            if c.review.is_empty() {
                render_review(g, area, s, &c.title, &lines);
            } else {
                let mut all = lines;
                all.push(String::new());
                all.extend(c.review.iter().cloned());
                render_long_review(g, area, s, &c.title, &all, c.scroll);
            }
        }
    }

    fn draw_grid(&mut self, g: &mut Gfx, view: R, s: f32) {
        let secs = self.sections();
        if secs.is_empty() {
            let msg = if self.facts.is_none() { "Looking at this machine…" } else { "Nothing matches. Esc clears the search and the filter." };
            g.text(view.x + 4.0 * s, view.y + 8.0 * s, 13.0 * s, MUTED, msg, Face::Sans);
            self.list.layout(view, 0.0, 36.0 * s, s);
            self.spots.clear();
            return;
        }
        let inner_w = view.w - 18.0 * s;
        let (cols, tw, gap) = grid(inner_w, s);
        let th = TILE_H * s;
        let head_h = HEAD_H * s;
        // where everything is, in content coordinates (from the top of the list)
        let mut heads: Vec<(Gk, f32, usize, usize)> = Vec::new();
        let mut spots: Vec<(Item, R)> = Vec::new();
        let mut y = 0.0f32;
        for sec in &secs {
            heads.push((sec.gk, y, sec.items.len(), sec.total));
            y += head_h + 4.0 * s;
            for (k, it) in sec.items.iter().enumerate() {
                let (row, col) = (k / cols, k % cols);
                spots.push((*it, R::new(col as f32 * (tw + gap), y + row as f32 * (th + gap), tw, th)));
            }
            let rows = sec.items.len().div_ceil(cols);
            y += rows as f32 * (th + gap) + 6.0 * s;
        }
        self.list.layout(view, y + 6.0 * s, 36.0 * s, s);
        self.spots = spots;
        self.cols = cols;
        if std::mem::take(&mut self.follow) {
            self.show_cursor();
        }
        let off = self.list.offset;
        let cursor = self.cursor;
        let cards: Vec<(Item, Card, R)> = self.spots.iter().map(|(i, r)| (*i, self.card(*i), *r)).collect();
        let installed_in = |gk: Gk| Item::ALL.iter().filter(|i| Gk::of(**i) == gk && self.present(**i)).count();
        let counts: Vec<(Gk, usize)> = heads.iter().map(|h| (h.0, installed_in(h.0))).collect();
        let narrowed = self.narrowed();
        g.clipped(view, |g| {
            for (gk, top, shown, total) in &heads {
                let ry = view.y + top - off;
                if ry + head_h < view.y || ry > view.y + view.h {
                    continue;
                }
                let acc = gk.accent();
                g.rrect(view.x + 2.0 * s, ry + 6.0 * s, 4.0 * s, head_h - 14.0 * s, 2.0 * s, acc);
                let installed = counts.iter().find(|c| c.0 == *gk).map_or(0, |c| c.1);
                let count = if narrowed { format!("{shown} of {total}") } else if installed > 0 { format!("{total} · {installed} installed") } else { format!("{total}") };
                let cw = g.text_width(12.0 * s, &count, Face::Sans);
                let title = g.fit(15.0 * s, gk.title(), Face::Bold, (inner_w - 24.0 * s - cw - 12.0 * s).max(40.0 * s));
                let ty = ry + (head_h - g.line_height(15.0 * s, Face::Bold)) / 2.0 - 2.0 * s;
                g.text(view.x + 14.0 * s, ty, 15.0 * s, TEXT, &title, Face::Bold);
                let tw_ = g.text_width(15.0 * s, &title, Face::Bold);
                let room = inner_w - 14.0 * s - tw_ - cw - 40.0 * s;
                if room > 90.0 * s {
                    let note = g.fit(11.5 * s, gk.note(), Face::Sans, room);
                    g.text(view.x + 14.0 * s + tw_ + 12.0 * s, ry + (head_h - g.line_height(11.5 * s, Face::Sans)) / 2.0 - 2.0 * s, 11.5 * s, DIM, &note, Face::Sans);
                }
                g.text(view.x + inner_w - cw - 4.0 * s, ry + (head_h - g.line_height(12.0 * s, Face::Sans)) / 2.0 - 2.0 * s, 12.0 * s, MUTED, &count, Face::Sans);
            }
            for (it, card, r) in &cards {
                let rr = R::new(view.x + r.x, view.y + r.y - off, r.w, r.h);
                if rr.y + rr.h < view.y || rr.y > view.y + view.h {
                    continue;
                }
                draw_tile(g, rr, s, card, *it == cursor);
            }
        });
        self.list.draw_bar(g, s, BAR, ACCENT);
    }
}

// ------------------------------------------------------------------------------------------------ the popup

/// One button of the popup. `ev` None: shown but not pressable.
struct Btn {
    label: String,
    accent: [u8; 4],
    ev: Option<Event>,
}

/// A line of the popup's body.
pub enum Line {
    /// Wrapped text: colour, bold, size, lines.
    Para([u8; 4], bool, f32, Vec<String>),
    /// A label and its wrapped value.
    Row(String, Vec<String>, [u8; 4]),
    Gap(f32),
}

impl Setup {
    /// The popup's buttons: the one primary (or why there is none), then what else makes sense now.
    fn popup_buttons(&self, card: &Card, open: bool) -> Vec<Btn> {
        let admin = self.admin;
        let idx = Item::ALL.iter().position(|i| *i == card.item).unwrap_or(0);
        let mut v = Vec::new();
        if card.act.needs_admin() && !admin {
            v.push(Btn { label: "Ask an administrator".into(), accent: MUTED, ev: None });
        } else if card.blocked || card.act == Act::None {
            v.push(Btn { label: card.label.clone(), accent: MUTED, ev: None });
        } else {
            v.push(Btn { label: card.label.clone(), accent: if card.act == Act::Retry { ERR } else { ACCENT }, ev: Some(Event::Tab(PRIMARY + idx)) });
        }
        if card.item == Item::VoicePack && matches!(card.state, CardState::Installed | CardState::Earlier) && admin {
            v.push(Btn { label: "Remove (undo)".into(), accent: ERR, ev: Some(Event::Tab(REMOVE + idx)) });
        }
        if card.item == Item::K3s && card.running == Some(true) && admin && !card.blocked {
            v.push(Btn { label: "Stop".into(), accent: ERR, ev: Some(Event::Tab(STOP + idx)) });
        }
        if card.act != Act::Open && card.act != Act::Log {
            if let Some((app, _)) = open_target(card.item) {
                let page = app.starts_with("brand:");
                let there = matches!(card.state, CardState::Installed | CardState::NotRunnable(_) | CardState::Earlier);
                if (page || there) && card.item != Item::MacHost {
                    v.push(Btn { label: if page { "Open its page" } else { "Open" }.into(), accent: MUTED, ev: Some(Event::Tab(MORE + idx)) });
                }
            }
        }
        let gk = Gk::of(card.item);
        let n = self.category_targets(gk).len();
        if n > 0 && admin {
            v.push(Btn { label: format!("Install all in {} ({n})", gk.short()), accent: ACCENT, ev: Some(Event::Tab(POP_ALL)) });
        }
        if card.act != Act::Log {
            v.push(Btn { label: "View log".into(), accent: MUTED, ev: Some(Event::Tab(POP_LOG)) });
        }
        v.push(Btn { label: if open { "Hide details" } else { "Details" }.into(), accent: MUTED, ev: Some(Event::Tab(DETAILS + idx)) });
        if card.kept > 0 && card.item == Item::MacCamera {
            v.push(Btn { label: format!("Delete download ({:.1} MB)", card.kept as f64 / 1e6), accent: MUTED, ev: Some(Event::Tab(DELETE)) });
        }
        if matches!(card.item, Item::ClaudeCode | Item::Copilot) && card.notes.iter().any(|n| n.0.starts_with("Signed in as")) {
            v.push(Btn { label: "Log out".into(), accent: ERR, ev: Some(Event::Tab(LOGOUT + idx)) });
        }
        v.push(Btn { label: "Close (Esc)".into(), accent: MUTED, ev: Some(Event::Tab(POP_CLOSE)) });
        v
    }

    /// The popup's buttons as (label, pressable), in order: the one main button first, a disabled one when it is not for this role.
    pub fn popup_button_list(&self, item: Item) -> Vec<(String, bool)> {
        let card = self.card(item);
        self.popup_buttons(&card, self.details.contains(&item)).into_iter().map(|b| (b.label, b.ev.is_some())).collect()
    }

    /// The body of the popup: what it is, the facts, the notes, the technical details when asked for.
    pub(super) fn popup_body(&self, g: &mut Gfx, card: &Card, open: bool, w: f32, s: f32) -> Vec<Line> {
        let item = card.item;
        let mut v = Vec::new();
        let sz = 13.0 * s;
        v.push(Line::Para(MUTED, false, sz, g.wrap(sz, item.about(), Face::Sans, w)));
        if let Some(why) = card.reason {
            v.push(Line::Gap(6.0 * s));
            v.push(Line::Para(ACCENT, true, 12.5 * s, g.wrap(12.5 * s, &format!("Recommended: {why}"), Face::Bold, w)));
        }
        v.push(Line::Gap(10.0 * s));
        let lw = (150.0 * s).min(w * 0.45);
        let vw = (w - lw - 10.0 * s).max(40.0 * s);
        let row = |g: &mut Gfx, v: &mut Vec<Line>, label: &str, text: &str, col: [u8; 4]| {
            v.push(Line::Row(label.into(), g.wrap(12.5 * s, text, Face::Sans, vw), col));
        };
        row(g, &mut v, "Size and time", &size_text(item), MUTED);
        row(g, &mut v, "Licence", item.licence(), MUTED);
        row(g, &mut v, "Where it goes", where_text(item), MUTED);
        if let Some(t) = self.deploy_text(item) {
            row(g, &mut v, "Deploys to", &t, if self.cluster.as_ref().is_some_and(|c| !c.kind.is_local()) && item != Item::IndicTts { AMBER } else { MUTED });
        }
        let needs = item.needs();
        if let Some(d) = &card.deps {
            row(g, &mut v, "Needs", d, AMBER);
        } else if !needs.is_empty() {
            let names: Vec<&str> = needs.iter().map(|n| n.title()).collect();
            row(g, &mut v, "Needs", &format!("{} (already on this machine)", names.join(", ")), MUTED);
        }
        let by = self.needed_by(item);
        if !by.is_empty() {
            let names: Vec<&str> = by.iter().map(|n| n.title()).collect();
            row(g, &mut v, "Needed by", &names.join(", "), MUTED);
        }
        if item.min_ram_mb() > 0 {
            row(g, &mut v, "Memory", &format!("about {} GB or more", item.min_ram_mb().div_ceil(1024)), MUTED);
        }
        for (t, bad) in &card.notes {
            v.push(Line::Gap(6.0 * s));
            v.push(Line::Para(if *bad { ERR } else { AMBER }, false, 12.5 * s, g.wrap(12.5 * s, t, Face::Sans, w)));
        }
        if open {
            v.push(Line::Gap(10.0 * s));
            v.push(Line::Para(TEXT, true, 13.0 * s, vec!["Details".into()]));
            let state = self.presence(item).map_or("not read yet".to_string(), |p| p.word());
            row(g, &mut v, "On this machine", &state, MUTED);
            row(g, &mut v, "Item id", item.id(), MUTED);
            if item.runs_as_job() {
                let steps = qi::order(item, &|i| self.present(i));
                row(g, &mut v, "Installer steps", &qi::order_text(&steps), MUTED);
                row(g, &mut v, "Same as typing", &format!("iyakki quick-install {}", item.id()), MUTED);
            }
        }
        v
    }

    fn draw_popup(&mut self, g: &mut Gfx, area: R, s: f32, item: Item) {
        let card = self.card(item);
        let open = self.details.contains(&item);
        let acc = Gk::of(item).accent();
        let pad = 22.0 * s;
        let pw = (640.0 * s).min(area.w - 24.0 * s).max(160.0 * s);
        let inner_w = pw - 2.0 * pad;
        let max_h = (area.h - 24.0 * s).max(200.0 * s);
        // measure: the body, then the footer (a note and the buttons, wrapped)
        let body = self.popup_body(g, &card, open, inner_w, s);
        let lh = |size: f32| size * 1.32;
        let lw = (150.0 * s).min(inner_w * 0.45);
        let mut body_h = 0.0f32;
        for l in &body {
            body_h += match l {
                Line::Para(_, _, sz, lines) => lines.len() as f32 * lh(*sz),
                Line::Row(_, lines, _) => lines.len().max(1) as f32 * lh(12.5 * s) + 3.0 * s,
                Line::Gap(h) => *h,
            };
        }
        let btns = self.popup_buttons(&card, open);
        let bsz = 14.0 * s;
        let mut spots = Vec::new();
        let (mut bx, mut brow) = (0.0f32, 0usize);
        for b in &btns {
            let bw = g.text_width(bsz, &b.label, Face::Bold) + bsz * 1.7;
            if bx > 0.0 && bx + bw > inner_w {
                bx = 0.0;
                brow += 1;
            }
            spots.push((bx, brow));
            bx += bw + 10.0 * s;
        }
        let brow_h = bsz * 2.1;
        let btn_h = (brow + 1) as f32 * brow_h + brow as f32 * 6.0 * s;
        let note_lines: Vec<String> = self.note.as_ref().map(|(m, _)| g.wrap(12.5 * s, m, Face::Sans, inner_w - 16.0 * s).into_iter().take(3).collect()).unwrap_or_default();
        let note_h = if note_lines.is_empty() { 0.0 } else { note_lines.len() as f32 * 17.0 * s + 8.0 * s };
        let footer_h = btn_h + note_h + 14.0 * s;
        let header_h = pad + 76.0 * s;
        let h = (header_h + body_h + footer_h + 8.0 * s).min(max_h);
        let p = R::new(area.x + (area.w - pw) / 2.0, area.y + ((area.h - h) / 2.0).max(8.0 * s), pw, h);
        // the dimmed screen (a click on it closes the popup) and the panel (a click on it does nothing)
        g.clear_hits();
        g.rect(area.x, area.y, area.w, area.h, [3, 5, 14, 190]);
        g.hit(area, Event::Tab(POP_CLOSE), "backdrop");
        fx::modal_panel(g, p, s, acc, false);
        g.hit(p, Event::Tab(POP_PANEL), "popup");
        // the header: badge, name, category, status
        let bd = 48.0 * s;
        let (bx0, by0) = (p.x + pad, p.y + pad);
        g.rrect(bx0, by0, bd, bd, 14.0 * s, over(fx::WELL, with_alpha(acc, 64)));
        g.rrect_outline(bx0, by0, bd, bd, 14.0 * s, 1.3 * s, with_alpha(acc, 190));
        g.icon(icon_of(item), bx0 + bd / 2.0, by0 + bd / 2.0, bd * 0.58, acc);
        let tx = bx0 + bd + 14.0 * s;
        let name = g.fit(21.0 * s, item.title(), Face::Bold, (p.x + p.w - pad - 36.0 * s - tx).max(40.0 * s));
        g.text(tx, by0, 21.0 * s, TEXT, &name, Face::Bold);
        let cat = g.fit(12.5 * s, Gk::of(item).title(), Face::Sans, (p.x + p.w - pad - tx).max(40.0 * s));
        g.text(tx, by0 + 27.0 * s, 12.5 * s, DIM, &cat, Face::Sans);
        let (st, col) = status_line(&card);
        let mut sx = tx;
        if card.running == Some(true) && matches!(card.state, CardState::Installed) {
            g.circle(sx + 5.0 * s, by0 + 52.0 * s + 8.0 * s, 8.0 * s, with_alpha(OK, 50));
            g.circle(sx + 5.0 * s, by0 + 52.0 * s + 8.0 * s, 4.0 * s, OK);
            sx += 16.0 * s;
        }
        // an item that deploys to a cluster: the switcher under the close mark (nothing with one cluster)
        let mut sw_w = 0.0f32;
        if deploys_to_cluster(item) {
            if let Some(h) = self.host.as_mut() {
                if h.visible() {
                    sw_w = h.width(g, s).min(inner_w * 0.5);
                    if h.draw(g, p.x + p.w - pad - sw_w, by0 + 34.0 * s, sw_w, s, acc).is_none() {
                        sw_w = 0.0;
                    }
                }
            }
        }
        let st = g.fit(13.0 * s, &st, Face::Bold, (p.x + p.w - pad - sx - if sw_w > 0.0 { sw_w + 10.0 * s } else { 0.0 }).max(40.0 * s));
        g.text(sx, by0 + 50.0 * s, 13.0 * s, col, &st, Face::Bold);
        g.text(p.x + p.w - pad - 12.0 * s, by0 - 2.0 * s, 20.0 * s, MUTED, "×", Face::Bold);
        g.hit(R::new(p.x + p.w - pad - 30.0 * s, by0 - 6.0 * s, 34.0 * s, 34.0 * s), Event::Tab(POP_CLOSE), "Close");
        // the body, scrolling when it does not fit
        let view = R::new(p.x + pad, p.y + header_h, inner_w, (p.h - header_h - footer_h - 4.0 * s).max(30.0 * s));
        self.pop_list.layout(view, body_h, 28.0 * s, s);
        let off = self.pop_list.offset;
        g.clipped(view, |g| {
            let mut y = view.y - off;
            for l in &body {
                match l {
                    Line::Para(c, bold, sz, lines) => {
                        for t in lines {
                            g.text(view.x, y, *sz, *c, t, if *bold { Face::Bold } else { Face::Sans });
                            y += lh(*sz);
                        }
                    }
                    Line::Row(label, lines, c) => {
                        let lab = g.fit(12.5 * s, label, Face::Bold, lw);
                        g.text(view.x, y, 12.5 * s, DIM, &lab, Face::Bold);
                        for t in lines {
                            g.text(view.x + lw + 10.0 * s, y, 12.5 * s, *c, t, Face::Sans);
                            y += lh(12.5 * s);
                        }
                        if lines.is_empty() {
                            y += lh(12.5 * s);
                        }
                        y += 3.0 * s;
                    }
                    Line::Gap(h) => y += *h,
                }
            }
        });
        self.pop_list.draw_bar(g, s, POP_BAR, acc);
        // the footer: the note, then the buttons
        let mut fy = p.y + p.h - footer_h + 8.0 * s;
        g.rect(p.x + pad, fy - 6.0 * s, inner_w, 1.0, [255, 255, 255, 22]);
        if let Some((_, ok)) = &self.note {
            if !note_lines.is_empty() {
                g.circle(p.x + pad + 5.0 * s, fy + 8.0 * s, 4.0 * s, if *ok { OK } else { ERR });
                for t in &note_lines {
                    g.text(p.x + pad + 16.0 * s, fy, 12.5 * s, TEXT, t, Face::Sans);
                    fy += 17.0 * s;
                }
                fy += 8.0 * s;
            }
        }
        for (b, (x, row)) in btns.iter().zip(&spots) {
            let by = fy + *row as f32 * (brow_h + 6.0 * s);
            match &b.ev {
                Some(ev) => {
                    g.button(p.x + pad + x, by, bsz, &b.label, b.accent, ev.clone());
                }
                None => {
                    g.button_off(p.x + pad + x, by, bsz, &b.label);
                }
            }
        }
        // the open cluster list goes over the popup
        if deploys_to_cluster(item) {
            if let Some(h) = self.host.as_mut() {
                h.draw_popup(g, p, s, acc);
            }
        }
    }
}

/// The review of "Install all recommended" or "Install all in this category": what would be installed, in order, with sizes and the total.
fn render_review(g: &mut Gfx, area: R, s: f32, title: &str, lines: &[String]) {
    let p = fx::modal(g, area, s, 600.0 * s, 120.0 * s + lines.len() as f32 * 20.0 * s + 60.0 * s, ACCENT, false);
    let t = g.fit(18.0 * s, title, Face::Bold, p.w - 48.0 * s);
    g.text(p.x + 24.0 * s, p.y + 22.0 * s, 18.0 * s, TEXT, &t, Face::Bold);
    let avail = ((p.h - 130.0 * s) / (20.0 * s)).floor().max(1.0) as usize;
    let mut y = p.y + 58.0 * s;
    let total = lines.last().cloned().unwrap_or_default();
    let body = &lines[..lines.len().saturating_sub(1)];
    let shown = body.len().min(avail.saturating_sub(1).max(1));
    for l in &body[..shown] {
        let t = g.fit(13.0 * s, l, Face::Sans, p.w - 48.0 * s);
        g.text(p.x + 24.0 * s, y, 13.0 * s, MUTED, &t, Face::Sans);
        y += 20.0 * s;
    }
    if shown < body.len() {
        g.text(p.x + 24.0 * s, y, 13.0 * s, DIM, &format!("…and {} more step(s)", body.len() - shown), Face::Sans);
        y += 20.0 * s;
    }
    let t = g.fit(14.0 * s, &total, Face::Bold, p.w - 48.0 * s);
    g.text(p.x + 24.0 * s, y + 4.0 * s, 14.0 * s, TEXT, &t, Face::Bold);
    let by = p.y + p.h - 56.0 * s;
    let w1 = g.button(p.x + 24.0 * s, by, 14.0 * s, "Install all (y)", ACCENT, Event::Tab(CONFIRM_YES));
    g.button(p.x + 24.0 * s + w1 + 12.0 * s, by, 14.0 * s, "Cancel (Esc)", MUTED, Event::Tab(CONFIRM_NO));
}

/// The long review (the voice pack's): all its lines, scrolled, with Install and Cancel at the bottom. The licence texts are part of the
/// lines, so they are read before the button is pressed.
fn render_long_review(g: &mut Gfx, area: R, s: f32, title: &str, lines: &[String], scroll: usize) {
    let p = fx::modal(g, area, s, 760.0 * s, area.h, ACCENT, false);
    let t = g.fit(18.0 * s, title, Face::Bold, p.w - 48.0 * s);
    g.text(p.x + 24.0 * s, p.y + 22.0 * s, 18.0 * s, TEXT, &t, Face::Bold);
    let row = 18.0 * s;
    let avail = (((p.h - 150.0 * s) / row).floor() as usize).max(1);
    let top = scroll.min(lines.len().saturating_sub(1));
    let mut y = p.y + 58.0 * s;
    for l in lines.iter().skip(top).take(avail) {
        let head = !l.is_empty() && !l.starts_with(' ') && !l.starts_with("  ") && l.len() < 90 && !l.contains(" · ") && !l.starts_with(|c: char| c.is_ascii_digit());
        let t = g.fit(12.5 * s, l, if head { Face::Bold } else { Face::Sans }, p.w - 48.0 * s);
        g.text(p.x + 24.0 * s, y, 12.5 * s, if head { TEXT } else { MUTED }, &t, if head { Face::Bold } else { Face::Sans });
        y += row;
    }
    let end = (top + avail).min(lines.len());
    let hint = format!("lines {}\u{2013}{} of {} \u{b7} Up, Down, Page keys or the wheel scroll", top + 1, end, lines.len());
    g.text(p.x + 24.0 * s, p.y + p.h - 84.0 * s, 12.0 * s, DIM, &hint, Face::Sans);
    let by = p.y + p.h - 56.0 * s;
    let w1 = g.button(p.x + 24.0 * s, by, 14.0 * s, "I have read the licences: install (y)", ACCENT, Event::Tab(CONFIRM_YES));
    g.button(p.x + 24.0 * s + w1 + 12.0 * s, by, 14.0 * s, "Cancel (Esc)", MUTED, Event::Tab(CONFIRM_NO));
}
