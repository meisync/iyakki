//! DOCTOR: the drawing. A summary line, two buttons (Check again, Share diagnostics), the findings as rows (a coloured dot and word,
//! the sentence, the administrator's detail line, one fix button), the answer to the last action, and the share preview as a
//! dialog. Every size follows the UI scale `s`; nothing is drawn or made clickable outside the area it was given.

use iyakki_agent::doctor::Severity;

use super::doctor::*;
use super::AppCx;
use crate::fx::{self, AMBER};
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

fn sev_color(s: Severity) -> [u8; 4] {
    match s {
        Severity::Problem => ERR,
        Severity::Warning => AMBER,
        Severity::Note => fx::CYAN,
    }
}

/// What DOCTOR looks at, for the calm "all fine" screen.
const CHECKED: &str = "Checked: the dev cluster and its address, services open to the network, developer mode and the firewall, the vault, the web engine, sound, disk space, the clock, model servers and every service that failed to start.";

impl DoctorApp {
    pub(super) fn draw(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &AppCx, tick: u64) {
        let pad = 20.0 * s;
        let x = area.x + pad;
        let w = (area.w - 2.0 * pad).max(40.0 * s);
        let mut y = area.y + 10.0 * s;

        // the summary
        let (head, color) = if self.found.is_empty() {
            if self.checking { ("Looking...".to_string(), MUTED) } else { ("Everything checked is fine.".to_string(), OK) }
        } else {
            let n = self.found.len();
            let worst = self.found.iter().map(|f| f.severity).min().unwrap_or(Severity::Note);
            (if n == 1 { "1 thing to look at".to_string() } else { format!("{n} things to look at") }, sev_color(worst))
        };
        let head = g.fit(19.0 * s, &head, Face::Bold, w);
        g.text(x, y, 19.0 * s, color, &head, Face::Bold);
        if self.checking && !self.found.is_empty() {
            let dots = ".".repeat(1 + (tick / 10 % 3) as usize);
            let hw = g.text_width(19.0 * s, &head, Face::Bold);
            g.text(x + hw + 10.0 * s, y + 4.0 * s, 13.0 * s, DIM, &format!("still checking{dots}"), Face::Sans);
        }
        y += 30.0 * s;

        // the buttons (on a row of their own, so a phone-wide screen holds them)
        let bsize = 14.0 * s;
        let bh = bsize * 2.1;
        let mut bx = x;
        for (label, ev, c) in [("Check again", Event::Tab(T_RECHECK), TEXT), ("Share diagnostics", Event::Tab(T_SHARE), ACCENT)] {
            let bw = g.text_width(bsize, label, Face::Bold) + bsize * 1.7;
            if bx > x && bx + bw > x + w {
                bx = x;
                y += bh + 6.0 * s;
            }
            bx += g.button(bx, y, bsize, label, c, ev) + 8.0 * s;
        }
        y += bh + 10.0 * s;

        // the answer to the last action
        let msg_h = if self.msg.is_some() { 30.0 * s } else { 0.0 };
        let view = R::new(x, y, w, (area.y + area.h - y - msg_h - 6.0 * s).max(0.0));

        if self.found.is_empty() {
            if !self.checking {
                let mut ty = view.y;
                for l in g.wrap(13.5 * s, CHECKED, Face::Sans, view.w) {
                    if ty + 18.0 * s > view.y + view.h {
                        break;
                    }
                    g.text(view.x, ty, 13.5 * s, MUTED, &l, Face::Sans);
                    ty += 19.0 * s;
                }
            }
        } else {
            self.draw_rows(g, view, s);
        }

        if let Some((m, ok)) = self.msg.clone() {
            let my = area.y + area.h - msg_h - 2.0 * s;
            fx::status_line(g, x, my, w, s, &m, ok);
            g.hit(R::new(x, my - 4.0 * s, w, msg_h), Event::Tab(T_DISMISS), "dismiss message");
        }

        if self.preview.is_some() {
            self.draw_preview(g, area, s);
        }
        let _ = self.tip.draw(g, s);
    }

    fn draw_rows(&mut self, g: &mut Gfx, view: R, s: f32) {
        let stacked = view.w < 560.0 * s;
        let pad = 14.0 * s;
        let bsize = 13.5 * s;
        let bh = bsize * 2.1;
        let gap = 8.0 * s;
        // measure first: the row heights decide how far the list scrolls
        struct Row {
            lines: Vec<String>,
            detail: String,
            h: f32,
            btn_w: f32,
        }
        let mut rows: Vec<Row> = Vec::new();
        for f in &self.found {
            let btn_w = f.fix.as_ref().map_or(0.0, |fx| (g.text_width(bsize, fx.label, Face::Bold) + bsize * 1.7).min(view.w * 0.45));
            let text_w = (view.w - 2.0 * pad - 14.0 * s - if stacked { 0.0 } else { btn_w + pad }).max(60.0 * s);
            let mut lines = g.wrap(15.0 * s, &f.text, Face::Sans, text_w);
            lines.truncate(3);
            let detail = if self.admin { f.detail.clone() } else { String::new() };
            let mut h = 16.0 * s + lines.len() as f32 * 20.0 * s + if detail.is_empty() { 0.0 } else { 18.0 * s };
            if stacked && f.fix.is_some() {
                h += bh + 6.0 * s;
            }
            h = h.max(if f.fix.is_some() && !stacked { bh + 16.0 * s } else { 0.0 });
            rows.push(Row { lines, detail, h, btn_w });
        }
        let content: f32 = rows.iter().map(|r| r.h + gap).sum();
        self.list.layout(view, content, 60.0 * s + gap, s);
        let sel_top: f32 = rows.iter().take(self.sel).map(|r| r.h + gap).sum();
        self.list.show(sel_top, sel_top + rows.get(self.sel).map_or(0.0, |r| r.h));
        let off = self.list.offset;
        let sel = self.sel;
        let found = self.found.clone();
        let tip_ok = self.preview.is_none();
        let mut tips: Vec<(R, bool, String)> = Vec::new();
        g.clipped(view, |g| {
            let mut ry = view.y - off;
            for (i, (r, f)) in rows.iter().zip(found.iter()).enumerate() {
                let rect = R::new(view.x, ry, view.w - if self.list.scrolls() { 18.0 * s } else { 0.0 }, r.h);
                if rect.y + rect.h >= view.y && rect.y <= view.y + view.h {
                    fx::row_at(g, rect.x, rect.y, rect.w, rect.h, 8.0 * s, i == sel, g.hovered(rect), sev_color(f.severity));
                    g.hit(rect, Event::Row(i), "finding");
                    let c = sev_color(f.severity);
                    let tx = rect.x + pad + 14.0 * s;
                    g.circle(rect.x + pad, rect.y + 17.0 * s, 4.5 * s, c);
                    let mut ty = rect.y + 8.0 * s;
                    for l in &r.lines {
                        g.text(tx, ty, 15.0 * s, TEXT, l, Face::Sans);
                        ty += 20.0 * s;
                    }
                    if !r.detail.is_empty() {
                        let avail = rect.w - (tx - rect.x) - pad - if stacked { 0.0 } else { r.btn_w + pad };
                        let t = g.fit(12.5 * s, &r.detail, Face::Mono, avail.max(40.0 * s));
                        g.text(tx, ty, 12.5 * s, DIM, &t, Face::Mono);
                        if t != r.detail && tip_ok {
                            tips.push((rect, i == sel, r.detail.clone()));
                        }
                        ty += 18.0 * s;
                    }
                    if let Some(fix) = &f.fix {
                        let (bx, by) = if stacked { (tx, ty + 2.0 * s) } else { (rect.x + rect.w - pad - r.btn_w, rect.y + (rect.h - bh) / 2.0) };
                        let label = g.fit(bsize, fix.label, Face::Bold, (r.btn_w - bsize * 1.7).max(30.0 * s));
                        g.button(bx, by, bsize, &label, c, Event::Tab(T_FIX + i));
                    }
                }
                ry += r.h + gap;
            }
        });
        for (rect, selected, text) in tips {
            self.tip.offer(g, rect, selected, &text);
        }
        self.list.draw_bar(g, s, A_LIST, ACCENT);
    }

    fn draw_preview(&mut self, g: &mut Gfx, area: R, s: f32) {
        let msg = self.msg.clone();
        let Some(p) = self.preview.as_mut() else { return };
        let panel = fx::modal(g, area, s, 920.0 * s, area.h, ACCENT, false);
        let mut y = fx::modal_title(g, panel, s, "Share diagnostics", ACCENT);
        let x = panel.x + 26.0 * s;
        let w = panel.w - 52.0 * s;
        for l in g.wrap(13.5 * s, "This is exactly what will be saved, line for line. Secrets, cookies, passwords, page addresses and your account name are removed. Nothing is sent anywhere: you save the file and send it yourself.", Face::Sans, w) {
            g.text(x, y + 4.0 * s, 13.5 * s, MUTED, &l, Face::Sans);
            y += 18.0 * s;
        }
        y += 14.0 * s;
        // the buttons below, the text between
        let bsize = 14.0 * s;
        let foot = bsize * 2.1 + 12.0 * s + if msg.is_some() { 26.0 * s } else { 0.0 };
        let view = R::new(x, y, w, (panel.y + panel.h - y - foot - 16.0 * s).max(40.0 * s));
        fx::well(g, view, s);
        let lh = g.line_height(12.5 * s, Face::Mono).max(8.0);
        let inner = R::new(view.x + 8.0 * s, view.y + 6.0 * s, view.w - 16.0 * s, (view.h - 12.0 * s).max(0.0));
        p.scroll.layout(inner, p.lines.len() as f32 * lh, lh, s);
        let first = (p.scroll.offset / lh) as usize;
        let rows = (inner.h / lh) as usize + 2;
        g.clipped(inner, |g| {
            for (k, l) in p.lines.iter().enumerate().skip(first).take(rows) {
                let ly = inner.y + k as f32 * lh - p.scroll.offset;
                let t = g.fit(12.5 * s, l, Face::Mono, inner.w - if p.scroll.scrolls() { 12.0 * s } else { 0.0 });
                let c = if l.starts_with("=== ") { ACCENT } else { TEXT };
                g.text(inner.x, ly, 12.5 * s, c, &t, Face::Mono);
            }
        });
        p.scroll.draw_bar(g, s, A_PREVIEW, ACCENT);
        g.hit(view, Event::Area(A_PREVIEW), "preview text");
        // the answer, then the buttons
        let mut by = view.y + view.h + 8.0 * s;
        if let Some((m, ok)) = msg {
            fx::status_line(g, x, by, w, s, &m, ok);
            by += 26.0 * s;
        }
        let mut bx = x;
        if p.saved.is_none() {
            for (i, d) in p.dests.iter().enumerate().take(5) {
                let label = if i == 0 { format!("Save in {}", d.label) } else { format!("Save on {}", d.label) };
                let label = g.fit(bsize, &label, Face::Bold, w * 0.6);
                let bw = g.text_width(bsize, &label, Face::Bold) + bsize * 1.7;
                if bx > x && bx + bw > x + w {
                    break;
                }
                bx += g.button(bx, by, bsize, &label, if i == 0 { ACCENT } else { TEXT }, Event::Tab(T_SAVE + i)) + 8.0 * s;
            }
        } else {
            bx += g.button(bx, by, bsize, "Copy the file's path", ACCENT, Event::Tab(T_COPY)) + 8.0 * s;
        }
        let cw = g.text_width(bsize, "Close", Face::Bold) + bsize * 1.7;
        let cx_ = (x + w - cw).max(bx);
        g.button(cx_, by, bsize, "Close", MUTED, Event::Tab(T_CLOSE));
    }
}
