//! DEV TOOLS: the drawing. The tool list is on the left (all thirteen rows always, smaller on a short window, names cut with an ellipsis
//! on a narrow one); the page of the chosen tool is on the right, laid out top to bottom: title, input box, second box, buttons,
//! then the output filling what is left, scrolled by a [`ScrollList`].

use super::tools::{Kind, DIFF, PING, PORTS, TOOLS};
use super::*;
use crate::fx;
use crate::safeclamp::SafeClamp;
use crate::gfx::{Face, Gfx};
use crate::ui::{DIM, ERR, FIELD, MUTED, OK, R, TEXT};

/// The output wrapped for the width it is drawn at, kept until the text, the count shown or the size changes.
#[derive(Default)]
pub(super) struct Visual {
    key: (u64, usize, i32, i32),
    lines: Vec<(String, [u8; 4])>,
}

const AMBER: [u8; 4] = rgb(255, 200, 90);
/// The most characters of one output line that are drawn (the copy has all of them).
const LINE_CAP: usize = 3000;

fn tint(tool: usize, line: &str, err: bool) -> [u8; 4] {
    if err {
        return ERR;
    }
    match tool {
        DIFF if line.starts_with("+ ") => OK,
        DIFF if line.starts_with("- ") => ERR,
        DIFF if line.starts_with("  ...") => DIM,
        3 if line.contains("EXPIRED") || line.contains("NOT VALID YET") || line.starts_with("INVALID") => ERR,
        3 if line.contains("still valid") || line.starts_with("VALID") => OK,
        3 if line.starts_with("NOT VERIFIED") => AMBER,
        PORTS if line.contains("  open ") => OK,
        PORTS if line.contains("  filtered ") => AMBER,
        PORTS if line.contains("  closed ") => MUTED,
        PING if line.contains("no reply") || line.contains("no answer") => ERR,
        PING if line.contains("time=") => OK,
        7 if line.starts_with("STOPPED") => AMBER,
        _ => TEXT,
    }
}

/// Text cut with an ellipsis to `w`.
fn put(g: &mut Gfx, x: f32, y: f32, size: f32, c: [u8; 4], t: &str, face: Face, w: f32) {
    let t = g.fit(size, t, face, w);
    g.text(x, y, size, c, &t, face);
}

impl DevToolsApp {
    pub(super) fn draw(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        let pad = 10.0 * s;
        // the tool list: every row always, shrinking with the window
        let lw = (area.w * 0.27).clamp_safe(92.0 * s, 210.0 * s).min(area.w * 0.45);
        let n = TOOLS.len() as f32;
        let rh = ((area.h - 2.0 * pad) / n).min(40.0 * s).max(8.0);
        let ls = (14.0 * s).min(rh * 0.52);
        for (i, t) in TOOLS.iter().enumerate() {
            let r = R::new(area.x + pad * 0.6, area.y + pad + i as f32 * rh, lw - pad * 0.6, rh - 3.0 * s);
            let sel = i == self.tool;
            fx::row_at(g, r.x, r.y, r.w, r.h, 7.0 * s, sel, g.hovered(r), ACCENT);
            if sel && self.focus == Focus::List {
                g.rrect_outline(r.x, r.y, r.w, r.h, 7.0 * s, 2.0, ACCENT);
            }
            let label = g.fit(ls, t.name, if sel { Face::Bold } else { Face::Sans }, r.w - 14.0 * s);
            g.text(r.x + 8.0 * s, r.y + (r.h - g.line_height(ls, Face::Sans)) / 2.0, ls, if sel { TEXT } else { MUTED }, &label, if sel { Face::Bold } else { Face::Sans });
            g.hit(r, Event::Tab(T_TOOL + i), t.name);
        }

        let (x, w) = (area.x + lw + pad, (area.w - lw - 2.0 * pad).max(40.0));
        let tool = &TOOLS[self.tool];
        let (hs, ts, fs, bs) = (17.0 * s, 12.5 * s, 11.5 * s, 13.0 * s);
        let mut y = area.y + pad;
        put(g, x, y, hs, TEXT, tool.name, Face::Bold, w);
        y += g.line_height(hs, Face::Bold) + 2.0 * s;
        let blurb = g.wrap(ts, tool.blurb, Face::Sans, w);
        let tl = g.line_height(ts, Face::Sans);
        // a short window keeps only the first lines of the blurb
        self.short = area.h < 420.0 * s;
        let keep = match (self.short, tool.kind) {
            (true, Kind::Qr) => 0,
            (true, _) => 1,
            _ => 3,
        };
        for (k, l) in blurb.iter().take(keep).enumerate() {
            let l = if k + 1 == keep && blurb.len() > keep { g.fit(ts, &format!("{l} ..."), Face::Sans, w) } else { l.clone() };
            g.text(x, y, ts, MUTED, &l, Face::Sans);
            y += tl;
        }
        y += 4.0 * s;

        // the boxes: their height follows the window
        let lh = g.line_height(14.0 * s, Face::Sans);
        let rows = (((area.h * 0.17) / lh) as usize).clamp(2, 7);
        let rows = if self.short && tool.kind == Kind::Qr { 2 } else { rows };
        self.input.max_rows = rows;
        self.input.min_rows = rows.min(3);
        let focus = self.focus;
        let (label, hint) = if tool.kind == Kind::Qr { (qrcode::PRESETS[self.qr.preset].label, qrcode::PRESETS[self.qr.preset].hint) } else { (tool.input_label, tool.input_hint) };
        put(g, x, y, fs, DIM, label, Face::Sans, w);
        y += g.line_height(fs, Face::Sans);
        y += self.input.render(g, x, y, w, s, 14.0 * s, 8.0 * s, 30.0 * s, focus == Focus::Input, tick, hint, ACCENT, A_INPUT, "input", false, true) + 6.0 * s;
        if let Some((label, arows, hint)) = tool.aux {
            put(g, x, y, fs, DIM, label, Face::Sans, w);
            y += g.line_height(fs, Face::Sans);
            self.aux.max_rows = (arows).min(rows);
            self.aux.min_rows = arows.min(rows);
            y += self.aux.render(g, x, y, w, s, 14.0 * s, 8.0 * s, 30.0 * s, focus == Focus::Aux, tick, hint, ACCENT, A_AUX, "second box", false, true) + 6.0 * s;
        }

        // the buttons, wrapped to rows
        let btns = self.buttons();
        self.btn = self.btn.min(btns.len() - 1);
        let (bh, mut bx) = (bs * 2.1, 0.0f32);
        for (i, (label, id)) in btns.iter().enumerate() {
            let label = g.fit(bs, label, Face::Bold, (w - bs * 1.7).max(20.0));
            let bw = g.text_width(bs, &label, Face::Bold) + bs * 1.7;
            if bx > 0.0 && bx + bw > w {
                bx = 0.0;
                y += bh + 5.0 * s;
            }
            let chosen = *id >= T_QR_KIND && *id < T_QR_KIND + qrcode::PRESETS.len() && *id - T_QR_KIND == self.qr.preset;
            let accent = if *id >= T_ACT && *id < T_ACT + 20 || *id == T_STOP || chosen || *id == T_SAVE_PNG || *id == T_SAVE_SVG { ACCENT } else { MUTED };
            g.button(x + bx, y, bs, &label, if *id == T_STOP { ERR } else { accent }, Event::Tab(*id));
            if chosen {
                g.rrect_outline(x + bx - 1.0, y - 1.0, bw + 2.0, bh + 2.0, bh / 2.0, 1.5, ACCENT);
            }
            if focus == Focus::Buttons && i == self.btn {
                g.rrect_outline(x + bx - 2.0, y - 2.0, bw + 4.0, bh + 4.0, bh / 2.0, 2.0, TEXT);
            }
            bx += bw + 8.0 * s;
        }
        y += bh + 8.0 * s;
        if tool.kind == Kind::Qr {
            return self.draw_qr(g, (x, y, w), area, s);
        }

        // the output
        let status = if self.job.is_some() {
            "Output (running...)".to_string()
        } else if self.out.text.is_empty() {
            "Output".to_string()
        } else if self.out.total > self.out.shown {
            format!("Output: first {} of {} lines", self.out.shown, self.out.total)
        } else {
            format!("Output: {} line{}", self.out.total, if self.out.total == 1 { "" } else { "s" })
        };
        put(g, x, y, fs, DIM, &status, Face::Sans, w);
        y += g.line_height(fs, Face::Sans) + 2.0 * s;
        let frame = R::new(x, y, w, (area.y + area.h - y - pad).max(30.0 * s));
        g.rrect(frame.x, frame.y, frame.w, frame.h, 8.0 * s, FIELD);
        g.rrect_outline(frame.x, frame.y, frame.w, frame.h, 8.0 * s, 1.0, if focus == Focus::Buttons { ACCENT } else { fx::with_alpha(ACCENT, 70) });
        g.hit(frame, Event::Area(A_OUT), "output");
        let view = frame.inset(6.0 * s);
        let (size, bar) = (13.0 * s, 16.0 * s);
        let tw = (view.w - bar).max(20.0);
        let key = (self.out.ver, self.out.shown, tw as i32, (s * 1000.0) as i32);
        if self.vis.key != key {
            self.vis.key = key;
            self.vis.lines.clear();
            for l in self.out.text.lines().take(self.out.shown) {
                let l: String = if l.chars().count() > LINE_CAP { l.chars().take(LINE_CAP).collect::<String>() + " ... (cut here; Copy output has all of it)" } else { l.to_string() };
                let c = tint(self.tool, &l, self.out.err);
                // `wrap` collapses leading spaces, so a line that fits is kept as it is (the indent
                // is what makes formatted JSON readable) and a longer one keeps its indent on every part
                if g.text_width(size, &l, Face::Mono) <= tw {
                    self.vis.lines.push((l, c));
                } else {
                    let body = l.trim_start_matches(' ');
                    let indent = " ".repeat(l.len() - body.len());
                    let room = (tw - g.text_width(size, &indent, Face::Mono)).max(20.0);
                    for part in g.wrap(size, body, Face::Mono, room) {
                        self.vis.lines.push((format!("{indent}{part}"), c));
                    }
                }
                if self.vis.lines.len() > 60_000 {
                    break;
                }
            }
        }
        let ol = g.line_height(size, Face::Mono);
        self.scroll.layout(view, self.vis.lines.len() as f32 * ol, ol, s);
        let first = (self.scroll.offset / ol) as usize;
        let count = (view.h / ol) as usize + 2;
        let top = view.y - self.scroll.offset + first as f32 * ol;
        g.clipped(view, |g| {
            for (k, (l, c)) in self.vis.lines.iter().skip(first).take(count).enumerate() {
                g.text(view.x, top + k as f32 * ol, size, *c, l, Face::Mono);
            }
        });
        if self.out.text.is_empty() && self.job.is_none() {
            put(g, view.x, view.y, size, DIM, "Press a button to see the result here.", Face::Sans, view.w);
        }
        self.scroll.draw_bar(g, s, A_BAR, ACCENT);
    }
}

impl DevToolsApp {
    /// The QR code page below the buttons: what the code is (or what is wrong), the code itself, large, black on white with its
    /// quiet zone whatever the theme, centred in a frame that scrolls when the window is too small for it.
    fn draw_qr(&mut self, g: &mut Gfx, (x, mut y, w): (f32, f32, f32), area: R, s: f32) {
        let (fs, size, pad) = (11.5 * s, 13.0 * s, 10.0 * s);
        let typed = self.input.text.clone();
        let note = self.qr.note.clone();
        let (wifi, open) = (self.qr.preset == qrcode::WIFI, self.qr.security == iyakki_agent::qr::WifiSecurity::Open);
        let m = self.qr.made(&typed);
        let say = |g: &mut Gfx, y: &mut f32, text: &str, c: [u8; 4]| {
            for l in g.wrap(fs, text, Face::Sans, w) {
                g.text(x, *y, fs, c, &l, Face::Sans);
                *y += g.line_height(fs, Face::Sans);
            }
        };
        say(g, &mut y, &m.status, if m.err { ERR } else { DIM });
        if wifi && !open {
            say(g, &mut y, "This code holds the Wi-Fi password in plain form: anyone who can see or scan it can join the network.", AMBER);
        }
        if let Some((text, bad)) = &note {
            say(g, &mut y, text, if *bad { ERR } else { OK });
        }
        y += 4.0 * s;
        let frame = R::new(x, y, w, (area.y + area.h - y - pad).max(30.0 * s));
        g.rrect(frame.x, frame.y, frame.w, frame.h, 8.0 * s, FIELD);
        g.rrect_outline(frame.x, frame.y, frame.w, frame.h, 8.0 * s, 1.0, if self.focus == Focus::Buttons { ACCENT } else { fx::with_alpha(ACCENT, 70) });
        g.hit(frame, Event::Area(A_OUT), "QR code");
        let view = frame.inset(6.0 * s);
        let bar = 16.0 * s;
        let Some(q) = &m.qr else {
            self.scroll.layout(view, 0.0, size, s);
            put(g, view.x, view.y, size, DIM, "The code is drawn here.", Face::Sans, view.w);
            return;
        };
        // whole pixels to a module keep the edges crisp; a window too small for it scrolls
        let n = q.size + 2 * qrcode::QUIET;
        let room = (view.w - bar).min(view.h.max(200.0 * s)).max(n as f32);
        let module = (room / n as f32).floor().max(1.0);
        let side = module * n as f32;
        self.scroll.layout(view, side + 2.0 * s, 24.0 * s, s);
        let x0 = (view.x + ((view.w - bar - side) / 2.0).max(0.0)).round();
        let y0 = (view.y - self.scroll.offset).round();
        g.clipped(view, |g| {
            g.rect(x0, y0, side, side, [255, 255, 255, 255]);
            for row in 0..q.size {
                let mut col = 0;
                while col < q.size {
                    if q.get(col, row) {
                        let start = col;
                        while col < q.size && q.get(col, row) {
                            col += 1;
                        }
                        g.rect(x0 + (start + qrcode::QUIET) as f32 * module, y0 + (row + qrcode::QUIET) as f32 * module, (col - start) as f32 * module, module, [0, 0, 0, 255]);
                    } else {
                        col += 1;
                    }
                }
            }
        });
        self.scroll.draw_bar(g, s, A_BAR, ACCENT);
    }
}
