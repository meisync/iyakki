//! LAB: the drawing. The controls are on the left (or on top when the window is narrow) and the results beside them (below),
//! with the log along the bottom. The controls scroll when they are taller than their room; nothing else needs to.

use super::lab::*;
use super::lab_pic::draw_fit;
use super::widgets;
use super::AppCx;
use crate::fx;
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::safeclamp::SafeClamp;
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};
use iyakki_agent::lab::{self, Sees};

const WARN: [u8; 4] = rgb(255, 196, 94);

struct Btn {
    label: String,
    ev: Event,
    on: bool,
    color: [u8; 4],
}

fn btn(label: &str, n: usize, on: bool, color: [u8; 4]) -> Btn {
    Btn { label: label.to_string(), ev: Event::Tab(n), on, color }
}

/// Buttons left to right, wrapping when the row is full. Returns the height used.
fn flow(g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, items: &[Btn], draw: bool) -> f32 {
    let size = 13.5 * s;
    let (h, gap) = (size * 2.1, 8.0 * s);
    let (mut cx, mut cy) = (x, y);
    for b in items {
        let bw = g.text_width(size, &b.label, Face::Bold) + size * 1.7;
        if cx > x && cx + bw > x + w {
            cx = x;
            cy += h + gap;
        }
        if draw {
            if b.on {
                g.button(cx, cy, size, &b.label, b.color, b.ev.clone());
            } else {
                g.button_off(cx, cy, size, &b.label);
            }
        }
        cx += bw + gap;
    }
    cy + h - y + 4.0 * s
}

/// Wrapped text, at most `max` lines. Returns the height used.
#[allow(clippy::too_many_arguments)]
fn para_d(g: &mut Gfx, x: f32, y: f32, w: f32, size: f32, color: [u8; 4], text: &str, max: usize, draw: bool) -> f32 {
    let lines = g.wrap(size, text, Face::Sans, w);
    let lh = g.line_height(size, Face::Sans) * 1.2;
    if draw {
        for (i, l) in lines.iter().take(max).enumerate() {
            g.text(x, y + i as f32 * lh, size, color, l, Face::Sans);
        }
    }
    lines.len().clamp_safe(1, max) as f32 * lh + 2.0 * s_of(size)
}

#[allow(clippy::too_many_arguments)]
fn para(g: &mut Gfx, x: f32, y: f32, w: f32, size: f32, color: [u8; 4], text: &str, max: usize) -> f32 {
    para_d(g, x, y, w, size, color, text, max, true)
}

fn s_of(size: f32) -> f32 {
    size / 14.0
}

/// A label then a model chooser; returns the height used.
#[allow(clippy::too_many_arguments)]
fn chooser(g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, label: &str, value: &str, open: bool, ev: usize) -> f32 {
    let size = 14.0 * s;
    let lw = 104.0 * s;
    let box_h = super::ide_chrome::tool(s);
    g.text(x, y + (box_h - g.line_height(size, Face::Sans)) / 2.0, size, MUTED, label, Face::Sans);
    widgets::select_box(g, x + lw, y, w - lw, s, value, open, ACCENT, Event::Tab(ev), label);
    box_h + 8.0 * s
}

impl LabApp {
    pub(super) fn draw(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let m = 16.0 * s;
        let (x, w) = (area.x + m, (area.w - 2.0 * m).max(40.0));
        let y = area.y + 8.0 * s;
        let tabs: Vec<(String, Event)> = vec![("PICTURE".into(), Event::Tab(T_TAB_PICTURE)), ("VISION".into(), Event::Tab(T_TAB_VISION)), ("VIDEO".into(), Event::Tab(T_TAB_VIDEO))];
        let idx = match self.tab {
            Tab::Picture => 0,
            Tab::Vision => 1,
            Tab::Video => 2,
        };
        fx::tab_strip(g, x, y, s, w, &tabs, idx, ACCENT);
        let th = 14.0 * s * 2.1;
        let rb = "Refresh models";
        let rw = g.text_width(12.5 * s, rb, Face::Bold) + 12.5 * s * 1.7;
        if w > 520.0 * s {
            g.button(x + w - rw, y, 12.5 * s, rb, MUTED, Event::Tab(T_REFRESH));
        }
        let top = y + th + 10.0 * s;
        let log_h = (area.h * 0.24).clamp_safe(96.0 * s, 230.0 * s);
        let log = R::new(x, area.y + area.h - log_h - 6.0 * s, w, log_h);
        let body = R::new(x, top, w, (log.y - top - 8.0 * s).max(60.0));
        self.mouse = g.mouse;
        let wide = body.w >= 760.0 * s;
        let (ctl, res) = if wide {
            let cw = (body.w * 0.46).max(320.0 * s);
            (R::new(body.x, body.y, cw, body.h), R::new(body.x + cw + 14.0 * s, body.y, body.w - cw - 14.0 * s, body.h))
        } else {
            let ch = (body.h * 0.64).max(120.0 * s);
            (R::new(body.x, body.y, body.w, ch), R::new(body.x, body.y + ch + 8.0 * s, body.w, (body.h - ch - 8.0).max(40.0)))
        };
        let aw = ctl.w - 10.0 * s;
        let fh = self.actions(g, ctl.x, 0.0, aw, s, false).min(ctl.h * 0.6);
        let scroll = R::new(ctl.x, ctl.y, ctl.w, (ctl.h - fh - 6.0 * s).max(40.0));
        let content = g.clipped(scroll, |g| match self.tab {
            Tab::Picture => self.picture_controls(g, scroll, s, cx, tick),
            Tab::Vision => self.vision_controls(g, scroll, s, cx, tick),
            Tab::Video => self.video_controls(g, scroll, s, cx, tick),
        });
        self.col_max = (content - scroll.h).max(0.0);
        self.col_scroll = self.col_scroll.clamp(0.0, self.col_max);
        if self.col_max > 0.0 {
            let (first, vis, total) = (self.col_scroll as usize, scroll.h as usize, content as usize);
            fx::scrollbar(g, R::new(ctl.x + ctl.w - 5.0 * s, scroll.y, 4.0 * s, scroll.h), s, first, vis, total, ACCENT);
        }
        if fh > 0.0 {
            g.rect(ctl.x, ctl.y + ctl.h - fh - 4.0 * s, ctl.w, 1.0, rgb(34, 42, 72));
            self.actions(g, ctl.x, ctl.y + ctl.h - fh, aw, s, true);
        }
        match self.tab {
            Tab::Picture => self.picture_results(g, res, s),
            Tab::Vision => self.vision_results(g, res, s),
            Tab::Video => self.video_results(g, res, s),
        }
        self.draw_log(g, log, s);
        self.draw_modal(g, area, s, tick);
    }

    // ------------------------------------------------------------------------------------------------ the log

    fn draw_log(&mut self, g: &mut Gfx, r: R, s: f32) {
        fx::panel(g, r, s, ACCENT);
        let size = 12.0 * s;
        g.text(r.x + 12.0 * s, r.y + 8.0 * s, 11.0 * s, DIM, &format!("LOG  {} call{}", self.log.len(), if self.log.len() == 1 { "" } else { "s" }), Face::Bold);
        let (cw, clw) = (g.text_width(size, "Copy log", Face::Bold) + size * 1.7, g.text_width(size, "Clear log", Face::Bold) + size * 1.7);
        let by = r.y + 4.0 * s;
        g.button(r.x + r.w - cw - clw - 20.0 * s, by, size, "Copy log", ACCENT, Event::Tab(T_LOG_COPY));
        g.button(r.x + r.w - clw - 12.0 * s, by, size, "Clear log", MUTED, Event::Tab(T_LOG_CLEAR));
        let t = R::new(r.x + 12.0 * s, r.y + 8.0 * s + size * 2.2, r.w - 24.0 * s, r.h - 16.0 * s - size * 2.2);
        if self.log.is_empty() {
            g.text(t.x, t.y, 12.5 * s, DIM, "Every call is listed here: time, provider, model, what was asked, HTTP status, duration, bytes, errors and fallbacks. No secrets.", Face::Sans);
        } else {
            self.logv.render(g, t, 12.0 * s, Face::Mono);
        }
    }

    // ------------------------------------------------------------------------------------------------ shared pieces

    /// The lines next to the buttons that say where this run goes (and that the first use of a remote provider asks).
    #[allow(clippy::too_many_arguments)]
    fn runs_on_line(&self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, models: &[Option<lab::LabModel>], draw: bool) -> f32 {
        let mut h = 0.0;
        for m in models.iter().flatten() {
            let ask = m.remote && !self.approved_for(&m.provider);
            let t = format!("{}{}", m.runs_on(), if ask { ". It asks before the first use in this session." } else { "" });
            h += para_d(g, x, y + h, w, 13.0 * s, if m.remote { WARN } else { MUTED }, &t, 2, draw);
        }
        h
    }

    fn status_block(&self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, draw: bool) -> f32 {
        let mut h = 0.0;
        if let Some(p) = self.progress_text() {
            h += para_d(g, x, y + h, w, 14.0 * s, TEXT, &p, 2, draw);
            if draw {
                // an unknown length: a block that moves along the bar
                let f = ((self.ticks % 60) as f32) / 60.0;
                fx::progress_bar(g, R::new(x, y + h, w, 8.0 * s), 0.0, ACCENT, s);
                g.rrect(x + (w - 40.0 * s) * f, y + h, 40.0 * s, 8.0 * s, 4.0 * s, ACCENT);
            }
            h += 14.0 * s;
        }
        if let Some((t, ok)) = &self.status {
            h += para_d(g, x, y + h, w, 14.0 * s, if *ok { OK } else { ERR }, t, 4, draw);
        }
        h
    }

    /// The buttons, where the run goes and the progress and status lines: pinned under the controls so they are always in view.
    /// Returns the height (with `draw` false it only measures).
    pub(super) fn actions(&self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, draw: bool) -> f32 {
        let mut h = 0.0;
        let busy = self.is_busy();
        match self.tab {
            Tab::Picture => {
                if self.cat.pictures.is_empty() {
                    return 0.0;
                }
                let compare_ok = self.pic_model(Slot::PicB).is_some();
                h += self.runs_on_line(g, x, y + h, w, s, &[self.pic_model(Slot::PicA).cloned()], draw);
                if compare_ok {
                    h += self.runs_on_line(g, x, y + h, w, s, &[self.pic_model(Slot::PicB).cloned()], draw);
                }
                let have = self.current_item().is_some();
                let items = [
                    btn("Generate", T_GENERATE, !busy, ACCENT),
                    btn("Compare (one after the other)", T_COMPARE, !busy && compare_ok, ACCENT),
                    btn("Cancel", T_CANCEL, busy, ERR),
                    btn("Save picture", T_SAVE, have, MUTED),
                    btn("Copy prompt", T_COPY_PROMPT, have || !self.prompt.is_empty(), MUTED),
                    btn("Reuse seed", T_SEED_REUSE, have, MUTED),
                ];
                h += flow(g, x, y + h, w, s, &items, draw);
            }
            Tab::Vision => {
                if self.cat.vision_choices(true).is_empty() {
                    return 0.0;
                }
                h += self.runs_on_line(g, x, y + h, w, s, &[self.vis_model(Slot::Vis)], draw);
                if self.vis_model(Slot::Vis).is_some_and(|m| m.sees == Sees::Unknown) {
                    h += para_d(g, x, y + h, w, 12.5 * s, WARN, "Not marked as a vision model: it is sent the picture and the server decides.", 2, draw);
                }
                let compare_ok = self.vis_model(Slot::VisB).is_some() && self.input.is_some();
                let items = [btn("Ask", T_ASK, !busy && self.input.is_some(), ACCENT), btn("Compare two models", T_COMPARE, !busy && compare_ok, ACCENT), btn("Cancel", T_CANCEL, busy, ERR), btn("Copy answer", T_COPY_ANSWER, self.answers.iter().any(|a| !a.text.is_empty()), MUTED)];
                h += flow(g, x, y + h, w, s, &items, draw);
            }
            Tab::Video => {
                h += self.runs_on_line(g, x, y + h, w, s, &[self.vis_model(Slot::Vis)], draw);
                let items = [btn("Take frames and ask", T_ASK, !busy && self.ffmpeg.is_ok(), ACCENT), btn("Cancel", T_CANCEL, busy, ERR), btn("Copy answer", T_COPY_ANSWER, self.answers.iter().any(|a| !a.text.is_empty()), MUTED)];
                h += flow(g, x, y + h, w, s, &items, draw);
            }
        }
        h + self.status_block(g, x, y + h, w, s, draw)
    }

    // ------------------------------------------------------------------------------------------------ picture

    fn picture_controls(&mut self, g: &mut Gfx, r: R, s: f32, _cx: &AppCx, tick: u64) -> f32 {
        let (x, w) = (r.x, r.w - 10.0 * s);
        let top = r.y - self.col_scroll;
        let mut y = top;
        let size = 14.0 * s;
        if self.cat.pictures.is_empty() {
            let missing = self.cat.pictures_not_downloaded;
            let t = if missing > 0 {
                format!("No picture model is downloaded yet ({missing} known to a Lemonade server but not downloaded). Settings \u{2192} Models downloads one (SD-Turbo is small and fast).")
            } else {
                "No picture model is available from your providers. Install a picture model in Settings \u{2192} Models (Lemonade: SD-Turbo or SDXL), or add a provider that draws pictures.".to_string()
            };
            let card = R::new(x, y, w, 120.0 * s);
            fx::panel(g, card, s, WARN);
            para(g, x + 14.0 * s, y + 12.0 * s, w - 28.0 * s, size, TEXT, &t, 5);
            g.button(x + 14.0 * s, y + 76.0 * s, 13.5 * s, "Refresh models", ACCENT, Event::Tab(T_REFRESH));
            return 130.0 * s;
        }
        let a_label = self.pic_model(Slot::PicA).map_or("none".to_string(), |m| m.label());
        y += chooser(g, x, y, w, s, "Model", &a_label, matches!(self.modal, Some(Modal::Pick { slot: Slot::PicA, .. })), T_MODEL);
        let b_label = self.pic_model(Slot::PicB).map_or("no second model".to_string(), |m| m.label());
        y += chooser(g, x, y, w, s, "Compare with", &b_label, matches!(self.modal, Some(Modal::Pick { slot: Slot::PicB, .. })), T_MODEL_B);
        let h = self.prompt.render(g, x, y, w, s, 15.0 * s, 9.0 * s, 56.0 * s, self.focus == Focus::Prompt, tick, "Describe the picture to make", ACCENT, PROMPT_AREA, "prompt", false, true);
        y += h + 6.0 * s;
        // the looks of TALES, added to the prompt
        let mut sx = x;
        g.text(sx, y + 5.0 * s, 12.5 * s, DIM, "Style", Face::Sans);
        sx += g.text_width(12.5 * s, "Style ", Face::Sans) + 6.0 * s;
        let mut row_y = y;
        for (i, (name, _)) in STYLES.iter().enumerate() {
            let bw = g.text_width(12.0 * s, name, Face::Bold) + 12.0 * s * 1.7;
            if sx + bw > x + w {
                sx = x;
                row_y += 12.0 * s * 2.1 + 6.0 * s;
            }
            g.button(sx, row_y, 12.0 * s, name, MUTED, Event::Tab(T_STYLE0 + i));
            sx += bw + 6.0 * s;
        }
        y = row_y + 12.0 * s * 2.1 + 8.0 * s;
        if self.pic_model(Slot::PicA).is_some_and(|m| m.takes_negative()) {
            let h = self.negative.render(g, x, y, w, s, 14.0 * s, 8.0 * s, 40.0 * s, self.focus == Focus::Negative, tick, "Negative prompt: what to leave out (this server takes it)", ACCENT, NEG_AREA, "negative prompt", false, true);
            y += h + 6.0 * s;
        } else if self.pic_model(Slot::PicA).is_some() {
            y += para(g, x, y, w, 12.5 * s, DIM, "No negative prompt: this server's picture call does not take one.", 2);
        }
        // size and seed
        let fh = 30.0 * s;
        let blink = super::blink(tick);
        let mut fx_ = x;
        g.text(fx_, y + (fh - g.line_height(size, Face::Sans)) / 2.0, size, MUTED, "Size", Face::Sans);
        fx_ += g.text_width(size, "Size ", Face::Sans) + 6.0 * s;
        self.width.render(g, R::new(fx_, y, 66.0 * s, fh), self.focus == Focus::Width, s, blink, Face::Sans, F_W);
        fx_ += 66.0 * s;
        g.text(fx_ + 6.0 * s, y + (fh - g.line_height(size, Face::Sans)) / 2.0, size, MUTED, "x", Face::Sans);
        fx_ += 22.0 * s;
        self.height.render(g, R::new(fx_, y, 66.0 * s, fh), self.focus == Focus::Height, s, blink, Face::Sans, F_H);
        fx_ += 66.0 * s + 14.0 * s;
        if self.pic_model(Slot::PicA).is_some_and(|m| m.takes_steps()) {
            g.text(fx_, y + (fh - g.line_height(size, Face::Sans)) / 2.0, size, MUTED, "Steps", Face::Sans);
            fx_ += g.text_width(size, "Steps ", Face::Sans) + 6.0 * s;
            self.steps.render(g, R::new(fx_, y, 60.0 * s, fh), self.focus == Focus::Steps, s, blink, Face::Sans, F_STEPS);
            fx_ += 60.0 * s + 14.0 * s;
        }
        if fx_ + 190.0 * s > x + w {
            fx_ = x;
            y += fh + 6.0 * s;
        }
        g.text(fx_, y + (fh - g.line_height(size, Face::Sans)) / 2.0, size, MUTED, "Seed", Face::Sans);
        fx_ += g.text_width(size, "Seed ", Face::Sans) + 6.0 * s;
        self.seed.render(g, R::new(fx_, y, 110.0 * s, fh), self.focus == Focus::Seed, s, blink, Face::Sans, F_SEED);
        fx_ += 110.0 * s + 6.0 * s;
        g.button(fx_, y + (fh - 12.0 * s * 2.1) / 2.0, 12.0 * s, "New", MUTED, Event::Tab(T_SEED_NEW));
        y += fh + 6.0 * s;
        let steps = self.steps_text();
        y += para(g, x, y, w, 12.5 * s, DIM, &format!("{steps}. Sizes from 64 to 2048; the log says when a server drew at another size."), 3);
        y - top + 10.0 * s
    }

    fn picture_results(&mut self, g: &mut Gfx, r: R, s: f32) {
        fx::panel(g, r, s, ACCENT);
        let shown: Vec<usize> = (0..self.shown.len()).collect();
        let hist_rows = self.history.len().min(4);
        let hist_h = if self.history.is_empty() { 0.0 } else { 30.0 * s + hist_rows as f32 * 26.0 * s };
        let pic = R::new(r.x + 10.0 * s, r.y + 10.0 * s, r.w - 20.0 * s, (r.h - hist_h - 20.0 * s).max(40.0));
        if shown.is_empty() {
            fx::empty_state(g, pic, s, Icon::Lab, "No picture yet", "Type a prompt and press Generate. Compare draws the same prompt with two models, one after the other, and shows both here.", ACCENT);
        } else {
            let n = shown.len();
            let cw = (pic.w - (n as f32 - 1.0) * 10.0 * s) / n as f32;
            for (k, i) in shown.iter().enumerate() {
                let cell = R::new(pic.x + k as f32 * (cw + 10.0 * s), pic.y, cw, pic.h);
                match &self.shown[*i] {
                    Shown::Item(id) => {
                        let Some(it) = self.item(*id) else { continue };
                        let cap_h = 3.0 * 15.0 * s;
                        let img = R::new(cell.x, cell.y, cell.w, (cell.h - cap_h).max(30.0));
                        draw_fit(g, &it.view, img);
                        let mut ty = img.y + img.h + 4.0 * s;
                        let head = format!("{} on {}{}", it.model, it.provider, if it.remote { " (remote)" } else { "" });
                        let head = g.fit(13.0 * s, &head, Face::Bold, cell.w);
                        g.text(cell.x, ty, 13.0 * s, TEXT, &head, Face::Bold);
                        ty += 16.0 * s;
                        let size = if it.size != it.asked { format!("{}x{} (asked {}x{})", it.size.0, it.size.1, it.asked.0, it.asked.1) } else { format!("{}x{}", it.size.0, it.size.1) };
                        let meta = format!("{} \u{b7} {} \u{b7} {} \u{b7} seed {}{}{}", lab::ms_text(it.ms), lab::bytes_text(it.bytes.len() as u64), size, it.seed, it.steps.map_or(String::new(), |t| format!(" \u{b7} {t} steps")), if it.negative.is_empty() { "" } else { " \u{b7} negative prompt sent" });
                        for l in g.wrap(12.0 * s, &meta, Face::Sans, cell.w).into_iter().take(2) {
                            g.text(cell.x, ty, 12.0 * s, MUTED, &l, Face::Sans);
                            ty += 14.0 * s;
                        }
                    }
                    Shown::Failed { label, error } => {
                        fx::panel(g, cell, s, ERR);
                        let head = g.fit(13.0 * s, label, Face::Bold, cell.w - 20.0 * s);
                        g.text(cell.x + 10.0 * s, cell.y + 10.0 * s, 13.0 * s, TEXT, &head, Face::Bold);
                        para(g, cell.x + 10.0 * s, cell.y + 34.0 * s, cell.w - 20.0 * s, 13.0 * s, ERR, error, 8);
                    }
                }
            }
            if n == 2 {
                g.text(pic.x + pic.w / 2.0 - 60.0 * s, r.y + 2.0 * s, 10.5 * s, DIM, "left ran first, right second", Face::Sans);
            }
        }
        // the history of this session
        if !self.history.is_empty() {
            let hy = r.y + r.h - hist_h - 4.0 * s;
            g.text(r.x + 12.0 * s, hy + 4.0 * s, 11.0 * s, DIM, &format!("THIS SESSION  {} kept (at most {})", self.history.len(), lab::HISTORY_MAX_ITEMS), Face::Bold);
            let current: Vec<u64> = self.shown.iter().filter_map(|x| if let Shown::Item(i) = x { Some(*i) } else { None }).collect();
            for (k, it) in self.history.iter().rev().take(hist_rows).enumerate() {
                let rr = R::new(r.x + 8.0 * s, hy + 24.0 * s + k as f32 * 26.0 * s, r.w - 16.0 * s, 24.0 * s);
                let on = current.contains(&it.id);
                fx::list_row(g, rr, s, on, g.hovered(rr), ACCENT);
                draw_fit(g, &it.thumb, R::new(rr.x + 3.0 * s, rr.y + 2.0 * s, 28.0 * s, 20.0 * s));
                let t = format!("{} on {} \u{b7} {} \u{b7} {}", it.model, it.provider, lab::ms_text(it.ms), it.prompt.replace('\n', " "));
                let t = g.fit(12.0 * s, &t, Face::Sans, rr.w - 44.0 * s);
                g.text(rr.x + 38.0 * s, rr.y + (rr.h - g.line_height(12.0 * s, Face::Sans)) / 2.0, 12.0 * s, if on { TEXT } else { MUTED }, &t, Face::Sans);
                g.hit(rr, Event::Tab(T_HIST0 + k), "history");
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ vision

    fn vision_model_rows(&mut self, g: &mut Gfx, x: f32, mut y: f32, w: f32, s: f32, compare: bool) -> f32 {
        let top = y;
        let any = if self.any_chat { "any chat model: ON" } else { "any chat model" };
        let a = self.vis_model(Slot::Vis).map_or("none".to_string(), |m| m.label());
        y += chooser(g, x, y, w, s, "Model", &a, matches!(self.modal, Some(Modal::Pick { slot: Slot::Vis, .. })), T_MODEL);
        if compare {
            let b = self.vis_model(Slot::VisB).map_or("no second model".to_string(), |m| m.label());
            y += chooser(g, x, y, w, s, "Compare with", &b, matches!(self.modal, Some(Modal::Pick { slot: Slot::VisB, .. })), T_MODEL_B);
        }
        let chip_w = fx::toggle_chip(g, x, y, s, any, self.any_chat, ACCENT);
        g.hit(R::new(x, y, chip_w, 34.0 * s), Event::Tab(T_ANY_CHAT), "any chat model");
        let note = if self.any_chat { "Models not marked as vision may not see the picture; the server's answer decides." } else { "Showing models marked as vision (by the server or by name)." };
        let nx = x + chip_w + 10.0 * s;
        y += para(g, nx, y + 4.0 * s, (x + w - nx).max(80.0 * s), 12.5 * s, if self.any_chat { WARN } else { DIM }, note, 3).max(36.0 * s);
        y - top
    }

    fn vision_controls(&mut self, g: &mut Gfx, r: R, s: f32, _cx: &AppCx, tick: u64) -> f32 {
        let (x, w) = (r.x, r.w - 10.0 * s);
        let top = r.y - self.col_scroll;
        let mut y = top;
        let size = 14.0 * s;
        if self.cat.vision_choices(true).is_empty() {
            let card = R::new(x, y, w, 110.0 * s);
            fx::panel(g, card, s, WARN);
            para(g, x + 14.0 * s, y + 12.0 * s, w - 28.0 * s, size, TEXT, "No chat or vision model is available from your providers. Install one in Settings \u{2192} Models (a Gemma 3 or Qwen-VL model sees pictures).", 4);
            g.button(x + 14.0 * s, y + 70.0 * s, 13.5 * s, "Refresh models", ACCENT, Event::Tab(T_REFRESH));
            return 120.0 * s;
        }
        y += self.vision_model_rows(g, x, y, w, s, true);
        // where the picture comes from
        g.text(x, y, 11.0 * s, DIM, "PICTURE", Face::Bold);
        y += 16.0 * s;
        let busy = self.is_busy();
        let items = [btn("File", T_SRC_FILE, !busy, MUTED), btn("Camera", T_SRC_CAMERA, !busy && self.has_camera, MUTED), btn("Screenshot", T_SRC_SCREEN, !busy, MUTED), btn("Clipboard picture", T_SRC_CLIP, !busy, MUTED)];
        y += flow(g, x, y, w, s, &items, true);
        let fh = 30.0 * s;
        let bw = g.text_width(13.5 * s, "Browse", Face::Bold) + 13.5 * s * 1.7;
        self.path.render(g, R::new(x, y, w - bw - 8.0 * s, fh), self.focus == Focus::Path, s, super::blink(tick), Face::Sans, F_PATH);
        g.button(x + w - bw, y + (fh - 13.5 * s * 2.1) / 2.0, 13.5 * s, "Browse", MUTED, Event::Tab(T_BROWSE));
        y += fh + 6.0 * s;
        if !self.has_camera {
            y += para(g, x, y, w, 12.0 * s, DIM, "No camera: plug in a USB camera to use it here.", 1);
        }
        if let Some(p) = &self.input {
            let tr = R::new(x, y, 96.0 * s, 72.0 * s);
            fx::well(g, tr, s);
            draw_fit(g, &p.thumb, tr);
            let tx = x + 106.0 * s;
            y += para(g, tx, y, w - 106.0 * s, 13.0 * s, TEXT, &format!("From {}", p.source), 1);
            y += para(g, tx, y, w - 106.0 * s, 12.5 * s, MUTED, &format!("{}.", p.sent_line()), 4);
            y = y.max(tr.y + tr.h) + 6.0 * s;
        } else {
            y += para(g, x, y, w, 12.5 * s, DIM, "No picture yet. Pictures larger than 1024 pixels on a side are made smaller before they are sent, and the size sent is shown here.", 3);
        }
        let h = self.question.render(g, x, y, w, s, 15.0 * s, 9.0 * s, 52.0 * s, self.focus == Focus::Question, tick, "Ask something about the picture", ACCENT, QUESTION_AREA, "question", false, true);
        y += h + 6.0 * s;
        let quick: Vec<Btn> = QUICK.iter().enumerate().map(|(i, (l, _))| btn(l, T_QUICK0 + i, true, MUTED)).collect();
        y += flow(g, x, y, w, s, &quick, true);
        y - top + 10.0 * s
    }

    /// The answers side by side (or one), each in its own selectable pane.
    fn answer_panes(&mut self, g: &mut Gfx, r: R, s: f32) {
        let n = self.answers.len();
        if n == 0 {
            return;
        }
        let gap = 10.0 * s;
        let cw = (r.w - (n as f32 - 1.0) * gap) / n as f32;
        for i in 0..n {
            let cell = R::new(r.x + i as f32 * (cw + gap), r.y, cw, r.h);
            let a = &mut self.answers[i];
            fx::glass(g, cell.x, cell.y, cell.w, cell.h, 12.0 * s, ACCENT);
            let head = g.fit(13.5 * s, &a.label, Face::Bold, cell.w - 20.0 * s);
            g.text(cell.x + 10.0 * s, cell.y + 8.0 * s, 13.5 * s, TEXT, &head, Face::Bold);
            let place = g.fit(11.5 * s, &a.place, Face::Sans, cell.w - 20.0 * s);
            g.text(cell.x + 10.0 * s, cell.y + 26.0 * s, 11.5 * s, DIM, &place, Face::Sans);
            let meta = if !a.done {
                "waiting for the answer\u{2026}".to_string()
            } else if a.error.is_some() {
                "no answer".to_string()
            } else {
                let tok = a.tokens.map_or("tokens not reported".to_string(), |(i, o)| format!("{i} tokens in, {o} out"));
                format!("{} \u{b7} {tok}{}", lab::ms_text(a.ms), if a.streamed { "" } else { " \u{b7} arrived in one piece" })
            };
            let meta = g.fit(12.0 * s, &meta, Face::Sans, cell.w - 20.0 * s);
            g.text(cell.x + 10.0 * s, cell.y + 42.0 * s, 12.0 * s, MUTED, &meta, Face::Sans);
            let mut ty = cell.y + 60.0 * s;
            if !a.note.is_empty() {
                ty += para(g, cell.x + 10.0 * s, ty, cell.w - 20.0 * s, 12.0 * s, WARN, &a.note, 2);
            }
            let text_r = R::new(cell.x + 10.0 * s, ty, cell.w - 20.0 * s, (cell.y + cell.h - ty - 8.0 * s).max(20.0));
            if let Some(e) = &a.error {
                para(g, text_r.x, text_r.y, text_r.w, 13.5 * s, ERR, e, 12);
            } else {
                a.view.render(g, text_r, 14.0 * s, Face::Sans);
            }
        }
    }

    fn vision_results(&mut self, g: &mut Gfx, r: R, s: f32) {
        fx::panel(g, r, s, ACCENT);
        let inner = R::new(r.x + 8.0 * s, r.y + 8.0 * s, r.w - 16.0 * s, r.h - 16.0 * s);
        if self.answers.is_empty() {
            fx::empty_state(g, inner, s, Icon::Lab, "No answer yet", "Choose a picture, ask a question and press Ask. Compare two models asks both the same thing, one after the other.", ACCENT);
        } else {
            self.answer_panes(g, inner, s);
        }
    }

    // ------------------------------------------------------------------------------------------------ video

    fn video_controls(&mut self, g: &mut Gfx, r: R, s: f32, _cx: &AppCx, tick: u64) -> f32 {
        let (x, w) = (r.x, r.w - 10.0 * s);
        let top = r.y - self.col_scroll;
        let mut y = top;
        let size = 14.0 * s;
        g.text(x, y, 11.0 * s, DIM, "VIDEO UNDERSTANDING  (frames are picked from a file and sent to a vision model)", Face::Bold);
        y += 18.0 * s;
        if let Err(e) = &self.ffmpeg {
            let h = 18.0 * s + g.wrap(13.0 * s, e, Face::Sans, w - 28.0 * s).len().min(6) as f32 * 13.0 * s * 1.2 + 48.0 * s;
            fx::panel(g, R::new(x, y, w, h), s, WARN);
            let used = para(g, x + 14.0 * s, y + 10.0 * s, w - 28.0 * s, 13.0 * s, TEXT, e, 6);
            g.button(x + 14.0 * s, y + used + 16.0 * s, 13.0 * s, "Open Media tools setup", ACCENT, Event::Tab(T_MEDIA_SETUP));
            y += h + 8.0 * s;
        }
        if self.cat.vision_choices(true).is_empty() {
            y += para(g, x, y, w, size, WARN, "No chat or vision model is available. Install one in Settings \u{2192} Models.", 3);
        } else {
            y += self.vision_model_rows(g, x, y, w, s, false);
        }
        let fh = 30.0 * s;
        let bw = g.text_width(13.5 * s, "Browse", Face::Bold) + 13.5 * s * 1.7;
        self.vpath.render(g, R::new(x, y, w - bw - 8.0 * s, fh), self.focus == Focus::VPath, s, super::blink(tick), Face::Sans, F_VPATH);
        g.button(x + w - bw, y + (fh - 13.5 * s * 2.1) / 2.0, 13.5 * s, "Browse", MUTED, Event::Tab(T_VIDEO_BROWSE));
        y += fh + 8.0 * s;
        // frames and how they are spread
        let mut fx_ = x;
        g.text(fx_, y + 7.0 * s, size, MUTED, &format!("Frames {}", self.nframes), Face::Sans);
        fx_ += g.text_width(size, "Frames 16 ", Face::Sans) + 4.0 * s;
        fx_ += g.button(fx_, y, 13.5 * s, "\u{2212}", MUTED, Event::Tab(T_FRAMES_DN)) + 6.0 * s;
        fx_ += g.button(fx_, y, 13.5 * s, "+", MUTED, Event::Tab(T_FRAMES_UP)) + 14.0 * s;
        let mode = if self.even { "evenly spaced" } else { "every k seconds" };
        if fx_ + g.text_width(13.5 * s, mode, Face::Bold) + 40.0 * s > x + w {
            fx_ = x;
            y += 13.5 * s * 2.1 + 6.0 * s;
        }
        fx_ += g.button(fx_, y, 13.5 * s, mode, ACCENT, Event::Tab(T_MODE)) + 8.0 * s;
        if !self.even {
            self.every.render(g, R::new(fx_, y - 3.0 * s, 56.0 * s, 30.0 * s), self.focus == Focus::Every, s, super::blink(tick), Face::Sans, F_EVERY);
            g.text(fx_ + 62.0 * s, y + 7.0 * s, 13.0 * s, DIM, "s", Face::Sans);
        }
        y += 13.5 * s * 2.1 + 8.0 * s;
        y += para(g, x, y, w, 12.5 * s, DIM, "From 4 to 16 frames. Evenly spaced picks the middle of equal parts of the video; every k seconds starts at the beginning.", 3);
        let h = self.question.render(g, x, y, w, s, 15.0 * s, 9.0 * s, 52.0 * s, self.focus == Focus::Question, tick, "Ask something about the video", ACCENT, QUESTION_AREA, "question", false, true);
        y += h + 6.0 * s;
        // video generation: offered only when a provider lists a video model
        y += 6.0 * s;
        g.text(x, y, 11.0 * s, DIM, "VIDEO GENERATION", Face::Bold);
        y += 16.0 * s;
        y += para(g, x, y, w, 13.0 * s, MUTED, &lab::video_line(&self.cat), 5);
        y - top + 10.0 * s
    }

    fn video_results(&mut self, g: &mut Gfx, r: R, s: f32) {
        fx::panel(g, r, s, ACCENT);
        let inner = R::new(r.x + 8.0 * s, r.y + 8.0 * s, r.w - 16.0 * s, r.h - 16.0 * s);
        if self.frames.is_empty() && self.answers.is_empty() {
            fx::empty_state(g, inner, s, Icon::Lab, "No frames yet", "Choose a video and a question, then press Take frames and ask. The frames appear here with their times, then the answer.", ACCENT);
            return;
        }
        let n = self.frames.len().max(1);
        let cols = ((inner.w / (130.0 * s)) as usize).clamp(1, 8).min(n);
        let rows = n.div_ceil(cols);
        let cell_w = (inner.w - (cols as f32 - 1.0) * 6.0 * s) / cols as f32;
        let cell_h = (cell_w * 0.62).min(80.0 * s);
        let strip_h = (rows as f32 * (cell_h + 16.0 * s)).min(inner.h * 0.45);
        for (i, f) in self.frames.iter().enumerate() {
            let (cx0, cy0) = (inner.x + (i % cols) as f32 * (cell_w + 6.0 * s), inner.y + (i / cols) as f32 * (cell_h + 16.0 * s));
            if cy0 + cell_h + 14.0 * s > inner.y + strip_h + 2.0 {
                break;
            }
            let rect = R::new(cx0, cy0, cell_w, cell_h);
            fx::well(g, rect, s);
            draw_fit(g, &f.thumb, rect);
            g.text(cx0 + 2.0 * s, cy0 + cell_h + 1.0 * s, 11.0 * s, MUTED, &lab::clock_text(f.time), Face::Mono);
        }
        let ans = R::new(inner.x, inner.y + strip_h + 8.0 * s, inner.w, (inner.h - strip_h - 8.0 * s).max(30.0));
        self.answer_panes(g, ans, s);
    }

    // ------------------------------------------------------------------------------------------------ cards

    fn draw_modal(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        match &mut self.modal {
            Some(Modal::Consent { providers, then }) => {
                let names: Vec<String> = providers.iter().map(|(p, place)| format!("{p} ({place})")).collect();
                let what = match then {
                    Then::Picture { .. } => "your prompt",
                    Then::Vision { .. } => "your picture and your question",
                    Then::Video => "frames from your video and your question",
                };
                let title = "Send this to another computer?";
                let text = format!("This run would send {what} to {}. That server is not this computer. Say yes once and LAB will not ask again for it in this session.", names.join(" and "));
                let p = fx::modal(g, area, s, 560.0 * s, 280.0 * s, WARN, false);
                let y = fx::modal_title(g, p, s, title, WARN);
                for (i, l) in g.wrap(14.5 * s, &text, Face::Sans, p.w - 52.0 * s).iter().take(7).enumerate() {
                    g.text(p.x + 26.0 * s, y + 6.0 * s + i as f32 * 20.0 * s, 14.5 * s, TEXT, l, Face::Sans);
                }
                let by = p.y + p.h - 58.0 * s;
                let w1 = g.button(p.x + 26.0 * s, by, 14.0 * s, "Yes, send", ACCENT, Event::Tab(T_YES));
                g.button(p.x + 26.0 * s + w1 + 12.0 * s, by, 14.0 * s, "No", MUTED, Event::Tab(T_NO));
                g.text(p.x + p.w - 150.0 * s, by + 6.0 * s, 12.5 * s, DIM, "y yes \u{b7} Esc no", Face::Sans);
            }
            Some(Modal::Source(src)) => {
                let (title, text, yes) = match src {
                    Src::Camera => ("Take one picture with the camera?", "The camera is private. One picture is taken now and kept in memory only; a red sign shows while the camera is in use. It is sent only to the model you run, and only when you press Ask.", "Take the picture"),
                    Src::Screen => ("Take a picture of your screen?", "It may show private information. It is the IYAKKI screen as drawn now (this LAB tile included), kept in memory only, and sent only to the model you run when you press Ask.", "Take the picture"),
                };
                let p = fx::modal(g, area, s, 560.0 * s, 270.0 * s, WARN, false);
                let y = fx::modal_title(g, p, s, title, WARN);
                for (i, l) in g.wrap(14.5 * s, text, Face::Sans, p.w - 52.0 * s).iter().take(6).enumerate() {
                    g.text(p.x + 26.0 * s, y + 6.0 * s + i as f32 * 20.0 * s, 14.5 * s, TEXT, l, Face::Sans);
                }
                let by = p.y + p.h - 58.0 * s;
                let w1 = g.button(p.x + 26.0 * s, by, 14.0 * s, yes, ACCENT, Event::Tab(T_YES));
                g.button(p.x + 26.0 * s + w1 + 12.0 * s, by, 14.0 * s, "Cancel", MUTED, Event::Tab(T_NO));
                g.text(p.x + p.w - 150.0 * s, by + 6.0 * s, 12.5 * s, DIM, "y yes \u{b7} Esc no", Face::Sans);
            }
            Some(Modal::Pick { picker, .. }) => picker.render(g, area, s, ACCENT),
            Some(Modal::File { picker, .. }) => picker.render(g, area, s, ACCENT, super::blink(tick)),
            None => {}
        }
    }
}
