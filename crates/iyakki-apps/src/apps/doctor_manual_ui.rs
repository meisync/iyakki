//! DOCTOR, tab "Manual tests": the drawing. A header (progress, the summary of what is still open, filter / export / import buttons), the
//! scenarios by area on the left, the chosen scenario on the main canvas (steps with the expected result under each, a note, the answer
//! buttons at the bottom). On a narrow screen the list and the canvas take turns. Every size follows the UI scale `s`; nothing is drawn
//! or made clickable outside the area it was given, and the answer buttons are big enough for a finger.

use iyakki_shared::manualtest::{State, Verdict};

use crate::safeclamp::SafeClamp;
use super::doctor::ACCENT;
use super::doctor_manual::*;
use crate::fx::{self, AMBER, CYAN};
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

fn state_color(s: State) -> [u8; 4] {
    match s {
        State::NotStarted => DIM,
        State::InProgress => CYAN,
        State::Pass => OK,
        State::Fail => ERR,
        State::CannotTest => AMBER,
    }
}

fn verdict_color(v: Verdict) -> [u8; 4] {
    match v {
        Verdict::Pass => OK,
        Verdict::Fail => ERR,
        Verdict::CannotTest => AMBER,
        Verdict::Skip => MUTED,
    }
}

/// A clickable area clipped to the view it is drawn in (a row that is half scrolled away must not reach outside).
fn hit_in(g: &mut Gfx, r: R, view: R, ev: Event, label: &str) {
    let (x0, y0) = (r.x.max(view.x), r.y.max(view.y));
    let (x1, y1) = ((r.x + r.w).min(view.x + view.w), (r.y + r.h).min(view.y + view.h));
    if x1 - x0 > 1.0 && y1 - y0 > 1.0 {
        g.hit(R::new(x0, y0, x1 - x0, y1 - y0), ev, label);
    }
}

/// How tall `button_flow` is for these labels.
fn flow_h(g: &mut Gfx, w: f32, size: f32, gap: f32, labels: &[&str]) -> f32 {
    let h = size * 2.1;
    let (mut x, mut rows) = (0.0f32, 1.0f32);
    for l in labels {
        let bw = g.text_width(size, l, Face::Bold) + size * 1.7;
        if x > 0.0 && x + bw > w {
            x = 0.0;
            rows += 1.0;
        }
        x += bw + gap;
    }
    rows * h + (rows - 1.0) * gap
}

impl ManualTab {
    pub(super) fn draw(&mut self, g: &mut Gfx, area: R, s: f32) {
        let pad = 16.0 * s;
        let x = area.x + pad;
        let w = (area.w - 2.0 * pad).max(40.0 * s);
        let mut y = area.y + 6.0 * s;
        let (done, total, open, failed) = self.counts();
        let head = format!("{done} of {total} steps answered");
        let head = g.fit(18.0 * s, &head, Face::Bold, w);
        g.text(x, y, 18.0 * s, TEXT, &head, Face::Bold);
        y += 26.0 * s;
        let mut sum = format!("{open} scenario{} still open", if open == 1 { "" } else { "s" });
        if failed > 0 {
            sum.push_str(&format!(" · {failed} failed"));
        }
        sum.push_str(&format!(" · {}", if self.filter.this_machine { "this machine only" } else { "all machines" }));
        let sum = g.fit(13.0 * s, &sum, Face::Sans, w);
        g.text(x, y, 13.0 * s, MUTED, &sum, Face::Sans);
        y += 22.0 * s;
        let items = vec![
            (if self.filter.this_machine { "This machine only: on" } else { "This machine only: off" }.to_string(), if self.filter.this_machine { ACCENT } else { TEXT }, Event::Tab(T_F_MACHINE)),
            (if self.filter.open_only { "Needs a person: on" } else { "Needs a person: off" }.to_string(), if self.filter.open_only { ACCENT } else { TEXT }, Event::Tab(T_F_OPEN)),
            ("Export (e)".to_string(), TEXT, Event::Tab(T_EXPORT)),
            ("Import (i)".to_string(), TEXT, Event::Tab(T_IMPORT)),
        ];
        let (bh, _) = g.button_flow(x, y, w, 13.5 * s, 8.0 * s, &items);
        y += bh + 10.0 * s;

        let msg_h = if self.msg.is_some() { 30.0 * s } else { 0.0 };
        let body = R::new(area.x, y, area.w, (area.y + area.h - y - msg_h - 4.0 * s).max(0.0));
        if self.scenarios.is_empty() {
            self.draw_empty(g, R::new(x, body.y, w, body.h), s);
        } else {
            self.narrow = area.w < 720.0 * s;
            if self.narrow {
                if self.opened {
                    self.draw_canvas(g, R::new(x, body.y, w, body.h), s);
                } else {
                    self.draw_nav(g, R::new(x, body.y, w, body.h), s);
                }
            } else {
                let nw = (area.w * 0.32).clamp_safe(240.0 * s, 360.0 * s);
                self.draw_nav(g, R::new(x, body.y, nw, body.h), s);
                let cx = x + nw + 16.0 * s;
                self.draw_canvas(g, R::new(cx, body.y, (area.x + area.w - pad - cx).max(60.0 * s), body.h), s);
            }
        }
        if let Some((m, ok)) = self.msg.clone() {
            let my = area.y + area.h - msg_h - 2.0 * s;
            fx::status_line(g, x, my, w, s, &m, ok);
            g.hit(R::new(x, my, w, msg_h - 2.0 * s), Event::Tab(T_DISMISS), "dismiss message");
        }
    }

    fn draw_empty(&mut self, g: &mut Gfx, view: R, s: f32) {
        let mut ty = view.y;
        let mut lines = g.wrap(14.0 * s, "No manual test checklists were found on this computer. They ship with the image (the folder dist/manual in the source); a computer that did not start from an image has none.", Face::Sans, view.w);
        lines.extend(self.problems.iter().take(4).cloned());
        for l in lines {
            if ty + 18.0 * s > view.y + view.h {
                break;
            }
            let l = g.fit(13.5 * s, &l, Face::Sans, view.w);
            g.text(view.x, ty, 13.5 * s, MUTED, &l, Face::Sans);
            ty += 19.0 * s;
        }
    }

    fn draw_nav(&mut self, g: &mut Gfx, view: R, s: f32) {
        let vis = self.vis();
        if vis.is_empty() {
            let t = g.fit(14.0 * s, "No scenario matches the filters.", Face::Sans, view.w);
            g.text(view.x, view.y + 4.0 * s, 14.0 * s, MUTED, &t, Face::Sans);
            return;
        }
        let (head_h, item_h, gap) = (28.0 * s, 54.0 * s, 4.0 * s);
        // rows: (Some(scenario) | None for an area header, y from the top of the content)
        let mut rows: Vec<(Option<usize>, f32, f32, String)> = Vec::new();
        let mut y = 0.0;
        let mut last = "";
        for &i in &vis {
            if self.scenarios[i].area != last {
                last = &self.scenarios[i].area;
                rows.push((None, y, head_h, self.scenarios[i].area.clone()));
                y += head_h;
            }
            rows.push((Some(i), y, item_h, String::new()));
            y += item_h + gap;
        }
        self.nav.layout(view, y, item_h, s);
        if let Some((top, h)) = rows.iter().find(|r| r.0 == Some(self.sel)).map(|r| (r.1, r.2)) {
            self.nav.show(top, top + h);
        }
        let off = self.nav.offset;
        let states = self.states();
        let sel = self.sel;
        let w = view.w - if self.nav.scrolls() { 18.0 * s } else { 0.0 };
        g.clipped(view, |g| {
            for (which, top, h, area) in &rows {
                let ry = view.y + top - off;
                if ry + h < view.y || ry > view.y + view.h {
                    continue;
                }
                match which {
                    None => {
                        let t = g.fit(12.5 * s, &area.to_uppercase(), Face::Bold, w);
                        g.text(view.x + 4.0 * s, ry + 8.0 * s, 12.5 * s, ACCENT, &t, Face::Bold);
                    }
                    Some(i) => {
                        let rect = R::new(view.x, ry, w, *h);
                        let st = states[*i];
                        fx::row_at(g, rect.x, rect.y, rect.w, rect.h, 8.0 * s, *i == sel, g.hovered(rect), state_color(st));
                        hit_in(g, rect, view, Event::Tab(T_NAV + *i), "scenario");
                        g.circle(rect.x + 14.0 * s, rect.y + 15.0 * s, 5.0 * s, state_color(st));
                        let sc = &self.scenarios[*i];
                        let t = g.fit(14.5 * s, &sc.title, Face::Bold, rect.w - 36.0 * s);
                        g.text(rect.x + 28.0 * s, rect.y + 6.0 * s, 14.5 * s, TEXT, &t, Face::Bold);
                        let sub = format!("{} · {}", st.label(), sc.machines_text());
                        let t = g.fit(12.0 * s, &sub, Face::Sans, rect.w - 36.0 * s);
                        g.text(rect.x + 28.0 * s, rect.y + 28.0 * s, 12.0 * s, state_color(st), &t, Face::Sans);
                    }
                }
            }
        });
        self.nav.draw_bar(g, s, A_NAV, ACCENT);
    }

    fn draw_canvas(&mut self, g: &mut Gfx, rect: R, s: f32) {
        let mut top = rect.y;
        if self.narrow {
            let bw = g.button(rect.x, top, 13.5 * s, "< All scenarios", TEXT, Event::Tab(T_BACKLIST));
            let _ = bw;
            top += 13.5 * s * 2.1 + 8.0 * s;
        }
        // the answer buttons at the bottom
        let (big, small, gap) = (17.0 * s, 13.5 * s, 8.0 * s);
        let h1 = flow_h(g, rect.w, big, gap, &["Pass (1)", "Fail (2)", "Cannot test (3)", "Skip (4)"]);
        let h2 = flow_h(g, rect.w, small, gap, &["Next step (Enter)", "Edit note (t)", "Previous (p)", "Next (n)"]);
        let h3 = flow_h(g, rect.w, small, gap, &["Whole scenario:", "All pass", "All fail", "All cannot test"]);
        let bar_h = h1 + h2 + h3 + 3.0 * gap + 4.0 * s;
        let bar_y = (rect.y + rect.h - bar_h).max(top);
        let view = R::new(rect.x, top, rect.w, (bar_y - top - 6.0 * s).max(0.0));
        // measure, then draw
        let (total, cur_top, cur_bottom) = self.canvas_content(g, view, view, s, false);
        self.main.layout(view, total, 60.0 * s, s);
        self.main.show(cur_top, cur_bottom);
        let off = self.main.offset;
        g.clipped(view, |g| {
            self.canvas_content(g, R::new(view.x, view.y - off, view.w, view.h), view, s, true);
        });
        // the content was drawn with its hits clipped to the view only through hit_in, which needs the real view
        self.main.draw_bar(g, s, A_MAIN, ACCENT);

        let mut y = bar_y;
        let v_items: Vec<(String, [u8; 4], Event)> = VERDICTS.iter().enumerate().map(|(k, v)| (format!("{} ({})", v.label(), k + 1), verdict_color(*v), Event::Tab(T_SVERD + k))).collect();
        let (h, _) = g.button_flow(rect.x, y, rect.w, big, gap, &v_items);
        y += h + gap;
        let items = vec![
            ("Next step (Enter)".to_string(), TEXT, Event::Tab(T_NEXT_STEP)),
            (if self.editing { "Done with note (Enter)" } else { "Edit note (t)" }.to_string(), ACCENT, Event::Tab(T_NOTE)),
            ("Previous (p)".to_string(), TEXT, Event::Tab(T_PREV)),
            ("Next (n)".to_string(), TEXT, Event::Tab(T_NEXT)),
        ];
        let (h, _) = g.button_flow(rect.x, y, rect.w, small, gap, &items);
        y += h + gap;
        let lw = g.text_width(small, "Whole scenario:", Face::Bold) + small;
        let mut all: Vec<(String, [u8; 4], Event)> = Vec::new();
        for (k, l) in ["All pass", "All fail", "All cannot test"].iter().enumerate() {
            all.push((l.to_string(), verdict_color(VERDICTS[k]), Event::Tab(T_AVERD + k)));
        }
        if rect.w > lw + 3.0 * small * 6.0 {
            g.text(rect.x, y + (small * 2.1 - g.line_height(small, Face::Bold)) / 2.0, small, MUTED, "Whole scenario:", Face::Bold);
            g.button_flow(rect.x + lw, y, rect.w - lw, small, gap, &all);
        } else {
            g.button_flow(rect.x, y, rect.w, small, gap, &all);
        }
    }

    /// The scrolling part of the canvas, drawn from `area.y` (negative when scrolled). With `draw` false nothing is drawn: it only measures.
    /// Returns the height and the top and bottom of the chosen step.
    fn canvas_content(&mut self, g: &mut Gfx, area: R, clip: R, s: f32, draw: bool) -> (f32, f32, f32) {
        let sc = self.scenarios[self.sel].clone();
        let verdicts = self.verdicts[self.sel].clone();
        let note = self.notes[self.sel].clone();
        let (x, w) = (area.x, area.w - 18.0 * s);
        let base = area.y;
        let mut y = base;
        let text = |g: &mut Gfx, y: &mut f32, x: f32, w: f32, size: f32, c: [u8; 4], t: &str, face: Face, lh: f32| {
            for l in g.wrap(size, t, face, w) {
                if draw {
                    g.text(x, *y, size, c, &l, face);
                }
                *y += lh;
            }
        };
        text(g, &mut y, x, w, 20.0 * s, TEXT, &sc.title, Face::Bold, 26.0 * s);
        y += 2.0 * s;
        text(g, &mut y, x, w, 13.0 * s, MUTED, &format!("Rows covered: {}", sc.rows.join(", ")), Face::Sans, 18.0 * s);
        text(g, &mut y, x, w, 13.0 * s, MUTED, &format!("For: {}{}", sc.machines_text(), if sc.needs_person { " · needs a person (Manual: needed)" } else { "" }), Face::Sans, 18.0 * s);
        let st = self.state(self.sel);
        text(g, &mut y, x, w, 13.0 * s, state_color(st), &format!("State: {} · step {} of {}", st.label(), self.step + 1, sc.steps.len()), Face::Bold, 20.0 * s);
        y += 8.0 * s;
        if !sc.prereq.is_empty() {
            text(g, &mut y, x, w, 14.0 * s, TEXT, "Before you start", Face::Bold, 20.0 * s);
            for p in &sc.prereq {
                text(g, &mut y, x + 12.0 * s, w - 12.0 * s, 14.0 * s, MUTED, p, Face::Sans, 19.0 * s);
            }
            y += 8.0 * s;
        }
        // the note
        text(g, &mut y, x, w, 13.0 * s, MUTED, "Note, kept with your answers (t to type, Enter ends)", Face::Bold, 18.0 * s);
        let shown = if self.editing { format!("{note}_") } else if note.is_empty() { "No note yet.".to_string() } else { note.clone() };
        let lines = g.wrap(14.0 * s, &shown, Face::Sans, w - 20.0 * s);
        let nh = (lines.len().max(1) as f32 * 19.0 * s + 14.0 * s).max(40.0 * s);
        if draw {
            let r = R::new(x, y, w, nh);
            fx::well(g, r, s);
            if self.editing {
                g.rrect_outline(r.x, r.y, r.w, r.h, 6.0 * s, 1.6 * s, ACCENT);
            }
            for (k, l) in lines.iter().enumerate() {
                g.text(x + 10.0 * s, y + 7.0 * s + k as f32 * 19.0 * s, 14.0 * s, if note.is_empty() && !self.editing { DIM } else { TEXT }, l, Face::Sans);
            }
            hit_in(g, r, clip, Event::Tab(T_NOTE), "note");
        }
        y += nh + 14.0 * s;
        // the steps
        let (mut cur_top, mut cur_bottom) = (0.0, 0.0);
        for (i, step) in sc.steps.iter().enumerate() {
            let pad = 12.0 * s;
            let tw = (w - 2.0 * pad - 40.0 * s).max(40.0 * s);
            let t_lines = g.wrap(15.0 * s, &step.text, Face::Sans, tw);
            let e_lines = g.wrap(14.0 * s, &format!("Expect: {}", step.expect), Face::Sans, tw);
            let mut extra = 0.0;
            if !step.shot.is_empty() {
                extra += 17.0 * s;
            }
            if !step.clause.is_empty() {
                extra += 17.0 * s;
            }
            let ans = verdicts.get(i).copied().flatten();
            if ans.is_some() {
                extra += 20.0 * s;
            }
            let h = pad * 2.0 + t_lines.len() as f32 * 20.0 * s + 4.0 * s + e_lines.len() as f32 * 19.0 * s + extra;
            if i == self.step {
                cur_top = y - base;
                cur_bottom = y - base + h;
            }
            if draw {
                let r = R::new(x, y, w, h);
                fx::row_at(g, r.x, r.y, r.w, r.h, 8.0 * s, i == self.step, g.hovered(r), ans.map_or(ACCENT, verdict_color));
                hit_in(g, r, clip, Event::Tab(T_STEP + i), "step");
                let c = ans.map_or(DIM, verdict_color);
                g.circle(x + pad + 11.0 * s, y + pad + 10.0 * s, 11.0 * s, c);
                let n = (i + 1).to_string();
                let nw = g.text_width(12.5 * s, &n, Face::Bold);
                g.text(x + pad + 11.0 * s - nw / 2.0, y + pad + 10.0 * s - g.line_height(12.5 * s, Face::Bold) / 2.0, 12.5 * s, crate::gfx::rgb(10, 14, 26), &n, Face::Bold);
                let tx = x + pad + 34.0 * s;
                let mut ty = y + pad;
                for l in &t_lines {
                    g.text(tx, ty, 15.0 * s, TEXT, l, Face::Sans);
                    ty += 20.0 * s;
                }
                ty += 4.0 * s;
                for l in &e_lines {
                    g.text(tx, ty, 14.0 * s, MUTED, l, Face::Sans);
                    ty += 19.0 * s;
                }
                if !step.shot.is_empty() {
                    let l = g.fit(12.0 * s, &format!("Example screenshot: {}", step.shot), Face::Sans, tw);
                    g.text(tx, ty, 12.0 * s, DIM, &l, Face::Sans);
                    ty += 17.0 * s;
                }
                if !step.clause.is_empty() {
                    let l = g.fit(12.0 * s, &format!("Evidence for: {}", step.clause), Face::Mono, tw);
                    g.text(tx, ty, 12.0 * s, DIM, &l, Face::Mono);
                    ty += 17.0 * s;
                }
                if let Some(v) = ans {
                    g.text(tx, ty, 13.5 * s, verdict_color(v), &format!("Answer: {}", v.label()), Face::Bold);
                }
            }
            y += h + 8.0 * s;
        }
        (y - base, cur_top, cur_bottom)
    }
}
