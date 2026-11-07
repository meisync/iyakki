//! GIT: the drawing of the list view, the selector, the cards and the status line. The repository view is drawn by
//! `git_tile_repo_ui.rs`. Lists draw whole rows only, so nothing is cut at an edge and every click region lies inside its pane.

use super::git_tile::*;
use super::widgets;
use super::AppCx;
use crate::fx::{self, AMBER};
use crate::gfx::{rgb, Face, Gfx};
use crate::safeclamp::SafeClamp;
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

pub(super) const FIRST_RUN_TITLE: &str = "Connect a Git host to browse its repositories";
pub(super) const FIRST_RUN_HINT: &str = "GitHub, GitLab, a Gitea or Forgejo server: add the host with Add connection... (Settings, Git), then choose it above. Running Gitea on your cluster? Set it up in Settings, Dev cluster, Platform services; Add connection picks it up.";

/// Lay controls out left to right and wrap when the row is full.
pub(super) struct Cursor {
    x0: f32,
    x: f32,
    y: f32,
    w: f32,
    row_h: f32,
    gap: f32,
}

impl Cursor {
    pub fn new(x: f32, y: f32, w: f32, row_h: f32, gap: f32) -> Cursor {
        Cursor { x0: x, x, y, w, row_h, gap }
    }
    pub fn place(&mut self, cw: f32) -> (f32, f32) {
        if self.x > self.x0 && self.x + cw > self.x0 + self.w {
            self.x = self.x0;
            self.y += self.row_h + self.gap;
        }
        let p = (self.x, self.y);
        self.x += cw + self.gap;
        p
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.row_h
    }
}

pub(super) fn btn_w(g: &mut Gfx, size: f32, label: &str) -> f32 {
    g.text_width(size, label, Face::Bold) + size * 1.7
}

/// A button placed by the cursor (dimmed when `on` is false).
pub(super) fn btn(g: &mut Gfx, c: &mut Cursor, size: f32, label: &str, accent: [u8; 4], ev: Event, on: bool) {
    let (x, y) = c.place(btn_w(g, size, label));
    if on {
        g.button(x, y, size, label, accent, ev);
    } else {
        g.button_off(x, y, size, label);
    }
}

/// Wrapped text; returns the height it takes.
pub(super) fn para(g: &mut Gfx, x: f32, y: f32, w: f32, size: f32, color: [u8; 4], text: &str, max: usize, draw: bool) -> f32 {
    let lines = g.wrap(size, text, Face::Sans, w);
    let lh = g.line_height(size, Face::Sans) * 1.2;
    let n = lines.len().clamp_safe(1, max);
    if draw {
        for (i, l) in lines.iter().take(max).enumerate() {
            g.text(x, y + i as f32 * lh, size, color, l, Face::Sans);
        }
    }
    n as f32 * lh
}

/// Keep the selected row in view when it moved by key; clamp the first row. Returns the first row.
pub(super) fn fit_list(l: &mut super::k8s_tile::ListState, n: usize, fit: usize) -> usize {
    l.fit = fit;
    if l.reveal && n > 0 {
        let sel = l.sel.min(n - 1);
        if sel < l.top {
            l.top = sel;
        } else if sel >= l.top + fit {
            l.top = sel + 1 - fit;
        }
        l.reveal = false;
    }
    l.top = l.top.min(n.saturating_sub(fit));
    l.top
}

pub(super) fn sbar(g: &mut Gfx, track: R, s: f32, first: usize, visible: usize, total: usize) {
    fx::scrollbar(g, track, s, first, visible, total, ACCENT);
}

pub(super) fn tone_color(t: Tone) -> [u8; 4] {
    match t {
        Tone::Normal => TEXT,
        Tone::Dim => DIM,
        Tone::Warn => AMBER,
        Tone::Bold => TEXT,
    }
}

fn dot_of(c: &iyakki_agent::gitconn::Connection) -> [u8; 4] {
    super::git_wizard::dot_color(c)
}

impl GitApp {
    pub(super) fn draw(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.last_draw = Some(std::time::Instant::now());
        self.mode = Mode::of(area.w, s);
        let m = 10.0 * s;
        let (x, w) = (area.x + m, (area.w - 2.0 * m).max(40.0));
        let mut y = area.y + 8.0 * s;
        let status = self.status_line();
        let status_h = match &status {
            Some((t, _)) => para(g, 0.0, 0.0, w, 13.0 * s, TEXT, t, 2, false) + 2.0 * s,
            None => 0.0,
        };
        if self.view.is_some() {
            y += self.draw_repo_header(g, x, y, w, s) + 8.0 * s;
            let body = R::new(x, y, w, (area.y + area.h - y - status_h - 6.0 * s).max(60.0));
            if self.show_log {
                self.draw_log(g, body, s);
            } else {
                self.draw_repo_body(g, body, s, tick);
            }
        } else {
            y += self.draw_header(g, x, y, w, s) + 8.0 * s;
            let body = R::new(x, y, w, (area.y + area.h - y - status_h - 6.0 * s).max(60.0));
            if self.show_log {
                self.draw_log(g, body, s);
            } else {
                self.draw_list(g, body, s, tick);
            }
        }
        if let Some((t, ok)) = status {
            let sy = area.y + area.h - status_h - 4.0 * s;
            para(g, x, sy, w, 13.0 * s, if ok { MUTED } else { ERR }, &t, 2, true);
        }
        self.draw_overlays(g, area, s);
        self.draw_card(g, area, s);
        let _ = cx;
        let _ = self.tip.draw(g, s);
    }

    // ------------------------------------------------------------------------------------------------ header

    fn draw_header(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32) -> f32 {
        let row_h = super::ide_chrome::tool(s);
        let mut c = Cursor::new(x, y, w, row_h, 8.0 * s);
        let src = self.src().clone();
        let label = match &src {
            Src::Conn(cn) => format!("{} \u{b7} {} \u{b7} {}", cn.name, cn.kind.label(), place_text(cn)),
            Src::Local => format!("Local clones \u{b7} this computer ({})", self.locals.list.len()),
        };
        let dot_w = 16.0 * s;
        let cw = widgets::select_width(g, s, &label).min(w - dot_w);
        let (cx0, cy0) = c.place(cw + dot_w);
        let dot = match &src {
            Src::Conn(cn) => dot_of(cn),
            Src::Local => OK,
        };
        g.circle(cx0 + 5.0 * s, cy0 + row_h / 2.0, 4.5 * s, dot);
        widgets::select_box(g, cx0 + dot_w, cy0, cw, s, &label, self.overlay == Some(Overlay::Sources), ACCENT, Event::Tab(T_SOURCE), "connection");
        let size = 13.0 * s;
        let (rx, ry) = c.place(btn_w(g, size, "Refresh"));
        g.button(rx, ry + (row_h - size * 2.1) / 2.0, size, "Refresh", MUTED, Event::Tab(T_REFRESH));
        let lab = if self.show_log { "Hide log" } else { "Show log" };
        let (lx, ly) = c.place(btn_w(g, size, lab));
        g.button(lx, ly + (row_h - size * 2.1) / 2.0, size, lab, MUTED, Event::Tab(T_LOG));
        let (ax, ay) = c.place(btn_w(g, size, "Add connection..."));
        g.button(ax, ay + (row_h - size * 2.1) / 2.0, size, "Add connection...", MUTED, Event::Tab(T_ADD));
        c.bottom() - y
    }

    // ------------------------------------------------------------------------------------------------ the list

    fn no_connections(&self) -> bool {
        self.srcs.len() <= 1
    }

    fn draw_list(&mut self, g: &mut Gfx, body: R, s: f32, tick: u64) {
        let mut r = body;
        if self.no_connections() {
            let h = self.draw_first_run(g, body, s);
            r = R::new(body.x, body.y + h + 8.0 * s, body.w, (body.h - h - 8.0 * s).max(40.0));
        }
        self.draw_table(g, r, s, tick);
    }

    /// No connection yet: a calm card that says what to do. The local clones stay listed below it.
    fn draw_first_run(&mut self, g: &mut Gfx, r: R, s: f32) -> f32 {
        let (x, w) = (r.x + 16.0 * s, r.w - 32.0 * s);
        let (t, hint) = (FIRST_RUN_TITLE, FIRST_RUN_HINT);
        let ph = para(g, 0.0, 0.0, w, 13.5 * s, MUTED, hint, 5, false);
        let size = 13.5 * s;
        let h = 14.0 * s + g.line_height(18.0 * s, Face::Bold) + 8.0 * s + ph + 10.0 * s + size * 2.1 + 12.0 * s;
        let h = h.min(r.h * 0.6);
        let card = R::new(r.x, r.y, r.w, h);
        fx::panel(g, card, s, ACCENT);
        let mut y = card.y + 14.0 * s;
        let tt = g.fit(18.0 * s, t, Face::Bold, w);
        g.text(x, y, 18.0 * s, TEXT, &tt, Face::Bold);
        y += g.line_height(18.0 * s, Face::Bold) + 8.0 * s;
        y += para(g, x, y, w, 13.5 * s, MUTED, hint, 5, true) + 10.0 * s;
        g.button(x, y, size, "Add connection...", ACCENT, Event::Tab(T_ADD));
        h
    }

    fn draw_table(&mut self, g: &mut Gfx, r: R, s: f32, tick: u64) {
        fx::panel(g, r, s, if matches!(self.focus, Focus::List | Focus::Filter) { ACCENT } else { rgb(60, 72, 112) });
        let (x, w) = (r.x + 8.0 * s, r.w - 16.0 * s);
        let n_all = self.rows_now().len();
        let local = matches!(self.src(), Src::Local);
        let conn = self.cur_conn();
        let (loaded, more, total, err_banner, first_err) = {
            let now = self.now_ms();
            let wall = self.wall();
            match conn {
                Some(_) => {
                    let b = self.board();
                    let st = b.tracker.status(now, wall);
                    (b.loaded, b.more, b.items.len(), st.banner || matches!(b.err, Some(iyakki_agent::gitbrowse::BrowseErr::Persistent(_))), b.err.clone())
                }
                None => (self.locals.loaded, false, self.locals.list.len(), false, None),
            }
        };
        // the count line
        let noun = if local { "clones" } else { "repositories" };
        let count = if n_all != total { format!("{n_all} of {total} {noun}") } else { format!("{total} {noun}") };
        let age = if loaded && !local {
            let ago = self.wall().saturating_sub(self.board().loaded_s);
            format!(" \u{b7} read {} ago", if ago < 5 { "just now".to_string() } else { iyakki_agent::k8s::age_text(ago) })
        } else {
            String::new()
        };
        let mut y = r.y + 6.0 * s;
        let t = g.fit(13.0 * s, &format!("{count}{age}"), Face::Bold, w);
        g.text(x, y, 13.0 * s, MUTED, &t, Face::Bold);
        y += 20.0 * s;
        let fh = 28.0 * s;
        let fw = if self.filter.value.is_empty() { w } else { w - 70.0 * s };
        self.filter.render(g, R::new(x, y, fw, fh), self.focus == Focus::Filter, s, super::blink(tick), Face::Sans, F_FILTER);
        if !self.filter.value.is_empty() {
            g.button(x + fw + 8.0 * s, y + (fh - 12.5 * s * 2.1) / 2.0, 12.5 * s, "Clear", MUTED, Event::Tab(T_CLEAR));
        }
        y += fh + 6.0 * s;
        // nothing to show yet, or a host that cannot be read
        if !loaded && total == 0 {
            self.list.rect = R::default();
            self.list.fit = 0;
            let (title, text, show_buttons) = match (&first_err, err_banner) {
                (Some(e), true) => ("Could not read the repositories".to_string(), e.text(), true),
                (Some(e), false) => ("Reading the repositories\u{2026}".to_string(), format!("The host did not answer; trying again. {}", e.text()), false),
                _ if local => ("Looking for clones on this computer\u{2026}".to_string(), String::new(), false),
                _ => ("Reading the repositories\u{2026}".to_string(), "The first answer takes a moment.".to_string(), false),
            };
            let tt = g.fit(16.0 * s, &title, Face::Bold, w);
            g.text(x + 4.0 * s, y + 6.0 * s, 16.0 * s, TEXT, &tt, Face::Bold);
            let mut yy = y + 6.0 * s + g.line_height(16.0 * s, Face::Bold) + 6.0 * s;
            if !text.is_empty() {
                yy += para(g, x + 4.0 * s, yy, w - 8.0 * s, 13.5 * s, MUTED, &text, 6, true) + 10.0 * s;
            }
            if show_buttons {
                let mut c = Cursor::new(x + 4.0 * s, yy, w - 8.0 * s, 13.5 * s * 2.1, 8.0 * s);
                btn(g, &mut c, 13.5 * s, "Refresh", ACCENT, Event::Tab(T_REFRESH), true);
                btn(g, &mut c, 13.5 * s, "Show log", MUTED, Event::Tab(T_LOG), true);
                btn(g, &mut c, 13.5 * s, "Edit the connection", MUTED, Event::Tab(T_EDIT_CONN), true);
            }
            return;
        }
        // the columns: drop the least important until the rest fit
        let compact = self.mode == Mode::Phone;
        let mut shown: Vec<usize> = (0..COLS.len()).collect();
        if local {
            shown.retain(|c| matches!(c, 0 | 1 | 2));
        }
        let min_w = |g: &mut Gfx, c: usize| -> f32 { (g.text_width(12.0 * s, COLS[c].title, Face::Bold) + 22.0 * s).max(COLS[c].min * s) };
        let avail = w - 22.0 * s;
        loop {
            let need: f32 = shown.iter().map(|&c| min_w(g, c)).sum();
            if need <= avail || shown.len() <= 1 {
                break;
            }
            match shown.iter().copied().filter(|&c| COLS[c].prio > 0).max_by_key(|&c| (COLS[c].prio, c)) {
                Some(c) => shown.retain(|&k| k != c),
                None => break,
            }
        }
        if compact && shown.len() > 2 {
            shown.retain(|&c| matches!(c, 0 | 4));
        }
        let mins: Vec<f32> = shown.iter().map(|&c| min_w(g, c)).collect();
        let wsum: f32 = shown.iter().map(|&c| COLS[c].weight).sum();
        let extra = (avail - mins.iter().sum::<f32>()).max(0.0);
        let widths: Vec<f32> = shown.iter().enumerate().map(|(k, &c)| mins[k] + extra * COLS[c].weight / wsum).collect();
        // header
        let hh = 22.0 * s;
        let mut cx0 = x + 18.0 * s;
        let sort = self.sort.unwrap_or(if local { (0, false) } else { (4, true) });
        for (k, &ci) in shown.iter().enumerate() {
            let arrow = if sort.0 == ci { if sort.1 { " \u{25bc}" } else { " \u{25b2}" } } else { "" };
            let t = g.fit(12.0 * s, &format!("{}{arrow}", COLS[ci].title), Face::Bold, widths[k] - 6.0 * s);
            let rr = R::new(cx0 - 2.0 * s, y, widths[k] - 2.0 * s, hh);
            if g.hovered(rr) {
                g.rrect(rr.x, rr.y, rr.w, rr.h, 4.0 * s, rgb(34, 44, 78));
            }
            g.text(cx0, y + (hh - g.line_height(12.0 * s, Face::Bold)) / 2.0, 12.0 * s, if sort.0 == ci { ACCENT } else { MUTED }, &t, Face::Bold);
            g.hit(rr, Event::Tab(T_SORT + ci), COLS[ci].title);
            cx0 += widths[k];
        }
        y += hh + 2.0 * s;
        let foot_h = if more || n_all == 0 { 28.0 * s } else { 0.0 };
        let list = R::new(x, y, w, (r.y + r.h - y - foot_h - 4.0 * s).max(10.0));
        self.list.rect = list;
        let row_h = 26.0 * s;
        let fit = ((list.h / row_h) as usize).max(1);
        let top = fit_list(&mut self.list, n_all, fit);
        let wall = self.wall();
        let order: Vec<usize> = self.order.clone();
        let focused = matches!(self.focus, Focus::List);
        for k in top..(top + fit).min(n_all) {
            let row = self.rows[order[k]].clone();
            let ry = list.y + (k - top) as f32 * row_h;
            let rr = R::new(list.x, ry, list.w - 6.0 * s, row_h - 2.0 * s);
            let sel = k == self.list.sel && focused;
            let hot = g.hovered(rr);
            if sel || hot {
                fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 6.0 * s, sel, hot, ACCENT);
            }
            // a dot when a clone of this repository is on this computer
            if row.clone.is_some() {
                g.circle(rr.x + 9.0 * s, ry + (row_h - 2.0 * s) / 2.0, 3.8 * s, OK);
            }
            let mut tx = list.x + 18.0 * s;
            let mut cut = false;
            for (j, &ci) in shown.iter().enumerate() {
                let text = cell(&row, ci, wall);
                let t = g.fit(13.0 * s, &text, Face::Sans, widths[j] - 8.0 * s);
                cut |= t != text;
                let color = match ci {
                    0 => TEXT,
                    3 if row.private == Some(true) => AMBER,
                    _ => MUTED,
                };
                g.text(tx, ry + (row_h - 2.0 * s - g.line_height(13.0 * s, Face::Sans)) / 2.0, 13.0 * s, color, &t, Face::Sans);
                tx += widths[j];
            }
            if cut && self.card.is_none() && self.overlay.is_none() {
                let tip = if row.desc.is_empty() { row.name.clone() } else { format!("{}\n{}", row.name, row.desc) };
                self.tip.offer(g, rr, sel, &tip);
            }
            g.hit(rr, Event::Tab(T_ROW + k), &row.name);
        }
        sbar(g, R::new(list.x + list.w - 5.0 * s, list.y, 4.0 * s, list.h), s, top, fit, n_all);
        let fy = list.y + list.h + 4.0 * s;
        if n_all == 0 {
            let msg = if !self.filter.value.is_empty() {
                format!("Nothing matches \u{201c}{}\u{201d}. Esc clears the filter.", self.filter.value)
            } else if local {
                format!("No clones yet. Clone a repository of a host (C) and it is listed here; the folder is {}.", self.repos_root.display())
            } else {
                "This host shows no repositories to this account.".to_string()
            };
            let t = g.fit(13.0 * s, &msg, Face::Sans, list.w - 8.0 * s);
            g.text(list.x + 4.0 * s, list.y + 4.0 * s, 13.0 * s, DIM, &t, Face::Sans);
        }
        if more {
            let busy = self.cur_conn().is_some() && self.board().job.is_some();
            if busy {
                g.text(x + 4.0 * s, fy + 4.0 * s, 13.0 * s, DIM, "Loading more\u{2026}", Face::Sans);
            } else {
                g.button(x + 4.0 * s, fy, 12.5 * s, "Load more", MUTED, Event::Tab(T_MORE));
                let bw = btn_w(g, 12.5 * s, "Load more");
                g.text(x + 4.0 * s + bw + 10.0 * s, fy + 5.0 * s, 12.0 * s, DIM, "the host has more repositories than these (m)", Face::Sans);
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ the log

    pub(super) fn draw_log(&mut self, g: &mut Gfx, r: R, s: f32) {
        fx::panel(g, r, s, ACCENT);
        let (x, w) = (r.x + 12.0 * s, r.w - 24.0 * s);
        g.text(x, r.y + 8.0 * s, 13.0 * s, MUTED, "PROBLEMS WITH THE HOSTS", Face::Bold);
        let mut c = Cursor::new(x, r.y + 28.0 * s, w, 13.0 * s * 2.1, 8.0 * s);
        btn(g, &mut c, 13.0 * s, "Copy", MUTED, Event::Tab(T_LOG_COPY), true);
        btn(g, &mut c, 13.0 * s, "Hide log", MUTED, Event::Tab(T_LOG), true);
        let mut y = c.bottom() + 8.0 * s;
        let text = self.log_text();
        let lh = g.line_height(12.5 * s, Face::Mono) * 1.15;
        for l in text.lines() {
            for p in g.wrap(12.5 * s, l, Face::Mono, w) {
                if y + lh > r.y + r.h - 6.0 * s {
                    return;
                }
                g.text(x, y, 12.5 * s, TEXT, &p, Face::Mono);
                y += lh;
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ overlays and cards

    fn draw_overlays(&mut self, g: &mut Gfx, area: R, s: f32) {
        match self.overlay {
            Some(Overlay::Sources) => {
                let rows = self.srcs.len() + 1;
                let h = 110.0 + 50.0 * rows as f32;
                let p = fx::modal(g, area, s, 560.0 * s, h * s, ACCENT, false);
                g.hit(area, Event::Tab(T_CLOSE), "close the list");
                g.hit(p, Event::Tick, "the list");
                let y = fx::modal_title(g, p, s, "Choose a connection", ACCENT);
                let row_h = 46.0 * s;
                let fit = (((p.y + p.h - y - 12.0 * s) / row_h) as usize).max(1);
                let top = fit_list(&mut self.src_list, rows, fit);
                for k in top..(top + fit).min(rows) {
                    let rr = R::new(p.x + 18.0 * s, y + (k - top) as f32 * row_h, p.w - 36.0 * s, row_h - 4.0 * s);
                    let hot = g.hovered(rr);
                    let chosen = k == self.cur;
                    if chosen || hot || k == self.src_list.sel {
                        fx::row_at(g, rr.x, rr.y, rr.w, rr.h, 6.0 * s, chosen, hot || k == self.src_list.sel, ACCENT);
                    }
                    let cy = rr.y + rr.h / 2.0;
                    if k == self.srcs.len() {
                        g.text(rr.x + 14.0 * s, cy - g.line_height(14.0 * s, Face::Bold) / 2.0, 14.0 * s, ACCENT, "Add connection...", Face::Bold);
                        g.text(rr.x + 170.0 * s, cy - g.line_height(12.0 * s, Face::Sans) / 2.0, 12.0 * s, DIM, "GitHub, GitLab, Gitea or Forgejo, any other git host", Face::Sans);
                        g.hit(rr, Event::Tab(T_SRC_ROW + k), "Add connection...");
                        continue;
                    }
                    let (name, sub, place, dot) = match &self.srcs[k] {
                        Src::Conn(c) => (c.name.clone(), format!("{} \u{b7} {}", c.kind.label(), c.url), place_text(c).to_string(), dot_of(c)),
                        Src::Local => ("Local clones".to_string(), format!("repositories on this computer, in {}", self.repos_root.display()), "this computer".to_string(), OK),
                    };
                    g.circle(rr.x + 14.0 * s, cy, 4.5 * s, dot);
                    let pw = g.text_width(12.0 * s, &place, Face::Sans);
                    let nm = g.fit(14.0 * s, &name, Face::Bold, rr.w - pw - 60.0 * s);
                    g.text(rr.x + 30.0 * s, rr.y + 4.0 * s, 14.0 * s, TEXT, &nm, Face::Bold);
                    let sb = g.fit(12.0 * s, &sub, Face::Sans, rr.w - pw - 60.0 * s);
                    g.text(rr.x + 30.0 * s, rr.y + 4.0 * s + g.line_height(14.0 * s, Face::Bold) + 1.0 * s, 12.0 * s, DIM, &sb, Face::Sans);
                    g.text(rr.x + rr.w - pw - 12.0 * s, cy - g.line_height(12.0 * s, Face::Sans) / 2.0, 12.0 * s, MUTED, &place, Face::Sans);
                    g.hit(rr, Event::Tab(T_SRC_ROW + k), &name);
                }
            }
            Some(Overlay::Refs) => self.draw_refs_overlay(g, area, s),
            None => {}
        }
    }

    fn draw_card(&mut self, g: &mut Gfx, area: R, s: f32) {
        let Some(card) = &self.card else { return };
        let (title, lines, yes, no) = (card.title.clone(), card.lines.clone(), card.yes, card.no);
        let toggle = card.toggle.as_ref().map(|t| (t.label, t.on, if t.on { t.note_on } else { t.note_off }));
        let danger = matches!(card.act, Act::Push { .. });
        let pw = 560.0 * s;
        // the height from the text
        let inner = pw.min(area.w - 40.0 * s) - 52.0 * s;
        let mut th = 0.0;
        for (t, _) in &lines {
            th += para(g, 0.0, 0.0, inner, 13.5 * s, TEXT, t, 8, false) + 6.0 * s;
        }
        let mut box_h = 0.0;
        if let Some((_, _, note)) = &toggle {
            box_h = 13.5 * s * 2.1 + para(g, 0.0, 0.0, inner, 12.0 * s, DIM, note, 4, false) + 14.0 * s;
        }
        let h = 56.0 * s + th + box_h + 20.0 * s + 13.5 * s * 2.1 + 22.0 * s;
        let p = fx::modal(g, area, s, pw, h, if danger { AMBER } else { ACCENT }, false);
        let mut y = fx::modal_title(g, p, s, &title, if danger { AMBER } else { ACCENT });
        for (t, tone) in &lines {
            let face = if *tone == Tone::Bold { Face::Bold } else { Face::Sans };
            let ls = g.wrap(13.5 * s, t, face, p.w - 52.0 * s);
            let lh = g.line_height(13.5 * s, face) * 1.2;
            for l in ls.iter().take(8) {
                g.text(p.x + 26.0 * s, y, 13.5 * s, tone_color(*tone), l, face);
                y += lh;
            }
            y += 6.0 * s;
        }
        y += 8.0 * s;
        let size = 13.5 * s;
        if let Some((label, on, note)) = toggle {
            // a box with a tick: the whole row reacts, and `s` flips it
            let bx = p.x + 26.0 * s;
            let row = R::new(bx, y, (p.w - 52.0 * s).min(size * 22.0), size * 2.1);
            let hot = g.hovered(row);
            let side = size * 1.3;
            let by = y + (row.h - side) / 2.0;
            g.rrect_outline(bx, by, side, side, 3.0 * s, 1.5 * s, if hot { ACCENT } else { MUTED });
            if on {
                g.line(bx + side * 0.2, by + side * 0.55, bx + side * 0.42, by + side * 0.78, 2.0 * s, ACCENT);
                g.line(bx + side * 0.42, by + side * 0.78, bx + side * 0.82, by + side * 0.22, 2.0 * s, ACCENT);
            }
            g.text(bx + side + 10.0 * s, y + (row.h - g.line_height(size, Face::Bold)) / 2.0, size, TEXT, label, Face::Bold);
            g.text(row.x + row.w - 40.0 * s, y + (row.h - g.line_height(12.0 * s, Face::Sans)) / 2.0, 12.0 * s, DIM, "s", Face::Sans);
            g.hit(row, Event::Tab(T_SUBMODULES), label);
            y += row.h;
            y += para(g, p.x + 26.0 * s, y, p.w - 52.0 * s, 12.0 * s, DIM, note, 4, true) + 14.0 * s;
        }
        let w1 = g.button(p.x + 26.0 * s, y, size, yes, if danger { AMBER } else { ACCENT }, Event::Tab(T_YES));
        g.button(p.x + 26.0 * s + w1 + 10.0 * s, y, size, no, MUTED, Event::Tab(T_NO));
        g.text(p.x + 26.0 * s + w1 + 20.0 * s + 100.0 * s, y + 6.0 * s, 12.0 * s, DIM, "y = yes, Esc = no", Face::Sans);
    }
}

