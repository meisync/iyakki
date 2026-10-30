//! CHECK-UP: the drawing. A summary line, three buttons (Check again, Share result, Open DOCTOR) and the steps as one list: every step is one
//! line (dot, name, Pass / Fail / Skipped), the chosen one opens into its words, its test panel (colour bars, a target, a level meter, the
//! camera picture) and its buttons. Every size follows the UI scale `s`; nothing is drawn or made clickable outside the area it was given.

use iyakki_agent::checkup::{Outcome, StepId};
use tiny_skia::{FilterQuality, PixmapPaint, Transform};

use super::checkup::*;
use super::AppCx;
use crate::fx::{self, AMBER};
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

fn outcome_color(o: Outcome) -> [u8; 4] {
    match o {
        Outcome::Pass => OK,
        Outcome::Fail => ERR,
        Outcome::Skipped => AMBER,
        Outcome::Pending => DIM,
    }
}

/// What a step does, in one sentence (shown before it is started).
fn blurb(id: StepId) -> &'static str {
    match id {
        StepId::Display => "Shows four colour bars and a frame. You say whether you can see them all.",
        StepId::Touch => "Asks you to touch a circle. Skipped when the computer lists no touch screen.",
        StepId::Keyboard => "Asks you to press three different keys.",
        StepId::Pointer => "Asks you to move the pointer onto a circle and click it.",
        StepId::Speaker => "Plays a short tone through the IYAKKI mixer and asks whether you heard it.",
        StepId::Microphone => "Starts only when you press the button: records 2 seconds from the microphone the Voice tile uses and shows its level. The sound is dropped, nothing is kept.",
        StepId::Camera => "Starts only when you press the button: shows one picture from the camera and drops it when you move on.",
        StepId::Monitor => "Lists the screens that are connected, the built-in one and any external monitor.",
        StepId::Network => "Looks for a network adapter, a link and a way out to other networks.",
        StepId::Wifi => "Looks for the Wi-Fi adapter and whether it is switched on.",
        StepId::Bluetooth => "Looks for the Bluetooth adapter and whether it is switched on.",
        StepId::Power => "Reads the battery (charge, charger, how worn it is) or says there is none.",
    }
}

fn start_label(id: StepId, done: bool) -> &'static str {
    if done {
        return "Run again";
    }
    match id {
        StepId::Display => "Show the test picture",
        StepId::Touch => "Start the touch test",
        StepId::Keyboard => "Start the keyboard test",
        StepId::Pointer => "Start the pointer test",
        StepId::Speaker => "Play the tone",
        StepId::Microphone => "Record 2 seconds",
        StepId::Camera => "Take a picture",
        _ => "Check now",
    }
}

/// Height of the test panel of the open step.
fn panel_height(app: &CheckupApp, i: usize, s: f32) -> f32 {
    match &app.active {
        Some(Active::Bars) if app.active_step == i => 110.0 * s,
        Some(Active::Target) if app.active_step == i => 120.0 * s,
        Some(Active::Record { .. }) if app.active_step == i => 70.0 * s,
        Some(Active::Grab(_)) if app.active_step == i => 36.0 * s,
        _ if app.step(i) == StepId::Camera && app.preview.is_some() => 190.0 * s,
        _ => 0.0,
    }
}

type Btn = (String, Event, [u8; 4]);

fn buttons(app: &CheckupApp, i: usize) -> Vec<Btn> {
    let id = app.step(i);
    let r = &app.results[i];
    let b = |l: &str, id: usize, c: [u8; 4]| (l.to_string(), Event::Tab(id), c);
    if app.active.is_some() && app.active_step == i {
        return match &app.active {
            Some(Active::Bars) => vec![b("Yes, I see them", T_YES, OK), b("No", T_NO, ERR), b("Skip", T_SKIP, MUTED)],
            Some(Active::Tone { .. }) => vec![b("Yes, I heard it", T_YES, OK), b("No", T_NO, ERR), b("Play again", T_AGAIN_TONE, TEXT), b("Skip", T_SKIP, MUTED)],
            Some(Active::Target) => vec![b("It did not work", T_NO, ERR), b("Skip", T_SKIP, MUTED)],
            Some(Active::Keys(_)) => vec![b("Done", T_RUN, ACCENT), b("Skip", T_SKIP, MUTED)],
            Some(Active::Record { .. }) | Some(Active::Grab(_)) => vec![b("Stop", T_STOP, ERR)],
            None => vec![],
        };
    }
    let mut v = vec![b(start_label(id, r.outcome != Outcome::Pending), T_RUN, ACCENT)];
    if r.outcome == Outcome::Pending && id.asks_the_person() || r.outcome == Outcome::Pending && id.is_capture() {
        v.push(b("Skip", T_SKIP, MUTED));
    }
    if r.outcome == Outcome::Fail {
        if let Some(f) = &r.fix {
            v.push((f.label.to_string(), Event::Tab(T_FIX), AMBER));
        }
        v.push(b("Open DOCTOR", T_ROW_DOCTOR, TEXT));
    }
    v
}

/// Lay buttons out in rows of at most `w`; `(x offset, row)` per button and the total height.
fn flow(g: &mut Gfx, btns: &[Btn], size: f32, w: f32, gap: f32) -> (Vec<(f32, usize)>, usize) {
    let (mut x, mut row) = (0.0f32, 0usize);
    let mut at = Vec::new();
    for (label, _, _) in btns {
        let bw = g.text_width(size, label, Face::Bold) + size * 1.7;
        if x > 0.0 && x + bw > w {
            x = 0.0;
            row += 1;
        }
        at.push((x, row));
        x += bw + gap;
    }
    (at, if btns.is_empty() { 0 } else { row + 1 })
}

struct RowM {
    h: f32,
    lines: Vec<(String, [u8; 4], bool)>,
    panel: f32,
    btns: Vec<Btn>,
    at: Vec<(f32, usize)>,
}

impl CheckupApp {
    pub(super) fn draw(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &AppCx, tick: u64) {
        let pad = 20.0 * s;
        let x = area.x + pad;
        let w = (area.w - 2.0 * pad).max(40.0 * s);
        let mut y = area.y + 10.0 * s;

        let head = g.fit(18.0 * s, &iyakki_agent::checkup::summary(&self.results), Face::Bold, w);
        let failed = self.results.iter().any(|r| r.outcome == Outcome::Fail);
        let done = self.results.iter().all(|r| r.outcome != Outcome::Pending);
        g.text(x, y, 18.0 * s, if failed { ERR } else if done { OK } else { TEXT }, &head, Face::Bold);
        y += 28.0 * s;

        let bsize = 14.0 * s;
        let bh = bsize * 2.1;
        let mut bx = x;
        for (label, ev, c) in [("Check again", Event::Tab(T_AGAIN), TEXT), ("Share result", Event::Tab(T_SHARE), ACCENT), ("Open DOCTOR", Event::Tab(T_DOCTOR), MUTED)] {
            let bw = g.text_width(bsize, label, Face::Bold) + bsize * 1.7;
            if bx > x && bx + bw > x + w {
                bx = x;
                y += bh + 6.0 * s;
            }
            bx += g.button(bx, y, bsize, label, c, ev) + 8.0 * s;
        }
        y += bh + 10.0 * s;

        let msg_h = if self.msg.is_some() { 30.0 * s } else { 0.0 };
        let view = R::new(x, y, w, (area.y + area.h - y - msg_h - 6.0 * s).max(0.0));
        self.draw_rows(g, view, s, tick);

        if let Some((m, ok)) = self.msg.clone() {
            let my = area.y + area.h - msg_h - 2.0 * s;
            fx::status_line(g, x, my, w, s, &m, ok);
            g.hit(R::new(x, my - 4.0 * s, w, msg_h), Event::Tab(T_DISMISS), "dismiss message");
        }
    }

    fn row_model(&self, g: &mut Gfx, i: usize, view_w: f32, s: f32) -> RowM {
        let pad = 14.0 * s;
        let collapsed = 40.0 * s;
        if i != self.sel {
            return RowM { h: collapsed, lines: vec![], panel: 0.0, btns: vec![], at: vec![] };
        }
        let r = &self.results[i];
        let inner = (view_w - 2.0 * pad - 14.0 * s).max(60.0 * s);
        let mut text: Vec<(String, [u8; 4])> = Vec::new();
        let prompt = self.prompt(i);
        if let Some(p) = prompt {
            text.push((p, TEXT));
        } else if r.outcome == Outcome::Pending {
            text.push((blurb(self.step(i)).to_string(), MUTED));
        }
        if self.active.is_none() || self.active_step != i {
            if !r.note.is_empty() {
                text.push((r.note.clone(), TEXT));
            }
            if !r.advice.is_empty() && r.outcome != Outcome::Pass {
                text.push((r.advice.clone(), MUTED));
            }
        }
        let mut lines = Vec::new();
        for (t, c) in text {
            for l in g.wrap(14.0 * s, &t, Face::Sans, inner) {
                lines.push((l, c, false));
            }
        }
        let panel = panel_height(self, i, s);
        let btns = buttons(self, i);
        let bsize = 13.5 * s;
        let (at, rows) = flow(g, &btns, bsize, inner, 8.0 * s);
        let bh = bsize * 2.1;
        let h = 38.0 * s + lines.len() as f32 * 19.0 * s + if panel > 0.0 { panel + 8.0 * s } else { 0.0 } + if rows > 0 { rows as f32 * (bh + 6.0 * s) + 4.0 * s } else { 0.0 } + 8.0 * s;
        RowM { h, lines, panel, btns, at }
    }

    /// The words of the running step.
    fn prompt(&self, i: usize) -> Option<String> {
        if self.active_step != i {
            return None;
        }
        Some(match self.active.as_ref()? {
            Active::Bars => "Do you see four colour bars (white, red, green, blue) inside a white frame?".to_string(),
            Active::Target => if self.step(i) == StepId::Touch { "Touch the circle with your finger." } else { "Move the pointer onto the circle and click it." }.to_string(),
            Active::Keys(k) => format!("Press three different keys. {} so far.", k.len()),
            Active::Tone { played: Ok(()), .. } => "A short two-note tone was played through the speaker. Did you hear it?".to_string(),
            Active::Tone { played: Err(e), .. } => format!("The tone could not be played: {e}"),
            Active::Record { name, .. } => if name.is_empty() { "Recording 2 seconds. Speak now.".to_string() } else { format!("Recording 2 seconds from {name}. Speak now.") },
            Active::Grab(_) => "Taking one picture...".to_string(),
        })
    }

    fn draw_rows(&mut self, g: &mut Gfx, view: R, s: f32, tick: u64) {
        let gap = 6.0 * s;
        let pad = 14.0 * s;
        let rows: Vec<RowM> = (0..self.results.len()).map(|i| self.row_model(g, i, view.w - 18.0 * s, s)).collect();
        let content: f32 = rows.iter().map(|r| r.h + gap).sum();
        self.list.layout(view, content, 40.0 * s + gap, s);
        let sel_top: f32 = rows.iter().take(self.sel).map(|r| r.h + gap).sum();
        self.list.show(sel_top, sel_top + rows.get(self.sel).map_or(0.0, |r| r.h));
        let off = self.list.offset;
        let bar = if self.list.scrolls() { 18.0 * s } else { 0.0 };
        let level = self.level;
        let progress = self.progress;
        let sel = self.sel;
        g.clipped(view, |g| {
            let mut ry = view.y - off;
            for (i, r) in rows.iter().enumerate() {
                let res = &self.results[i];
                let rect = R::new(view.x, ry, view.w - bar, r.h);
                if rect.y + rect.h >= view.y && rect.y <= view.y + view.h {
                    let c = outcome_color(res.outcome);
                    fx::row_at(g, rect.x, rect.y, rect.w, rect.h, 8.0 * s, i == sel, g.hovered(rect), c);
                    g.hit(rect, Event::Row(i), "step");
                    let tx = rect.x + pad + 14.0 * s;
                    g.circle(rect.x + pad, rect.y + 20.0 * s, 4.5 * s, c);
                    let word = res.outcome.word();
                    let ww = g.text_width(13.5 * s, word, Face::Bold);
                    g.text(rect.x + rect.w - pad - ww, rect.y + 12.0 * s, 13.5 * s, c, word, Face::Bold);
                    let title = g.fit(15.5 * s, res_title(i), Face::Bold, (rect.w - (tx - rect.x) - pad * 2.0 - ww).max(30.0 * s));
                    g.text(tx, rect.y + 11.0 * s, 15.5 * s, TEXT, &title, Face::Bold);
                    if i == sel {
                        let mut ty = rect.y + 38.0 * s;
                        for (l, lc, _) in &r.lines {
                            g.text(tx, ty, 14.0 * s, *lc, l, Face::Sans);
                            ty += 19.0 * s;
                        }
                        if r.panel > 0.0 {
                            let pr = R::new(tx, ty + 2.0 * s, (rect.w - (tx - rect.x) - pad).max(40.0 * s), r.panel);
                            self.draw_panel(g, pr, s, level, progress, tick, i);
                            ty += r.panel + 8.0 * s;
                        }
                        let bsize = 13.5 * s;
                        let bh = bsize * 2.1;
                        for ((label, ev, bc), (ox, row)) in r.btns.iter().zip(r.at.iter()) {
                            g.button(tx + ox, ty + *row as f32 * (bh + 6.0 * s), bsize, label, *bc, ev.clone());
                        }
                    }
                }
                ry += r.h + gap;
            }
        });
        self.list.draw_bar(g, s, A_LIST, ACCENT);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_panel(&self, g: &mut Gfx, r: R, s: f32, level: f32, progress: f32, tick: u64, i: usize) {
        let live = |g: &mut Gfx, x: f32, y: f32, bits: u8| -> f32 {
            // the same words as the browser's sign ("Camera on"), with a blinking red dot
            let text = super::browser_media::indicator(bits).unwrap_or_default();
            let on = (tick / 15) % 2 == 0;
            let w = fx::chip(g, x, y, s, &text, ERR, true);
            g.circle(x + w + 12.0 * s, y + 10.0 * s, 5.0 * s, if on { rgb(255, 60, 70) } else { rgb(150, 30, 40) });
            w + 24.0 * s
        };
        match &self.active {
            Some(Active::Bars) if self.active_step == i => {
                let bars = [rgb(240, 240, 240), rgb(230, 40, 40), rgb(40, 200, 80), rgb(50, 90, 240)];
                let bw = r.w / 4.0;
                for (k, c) in bars.iter().enumerate() {
                    g.rect(r.x + bw * k as f32, r.y, bw.ceil(), r.h, *c);
                }
                g.rrect_outline(r.x, r.y, r.w, r.h, 2.0 * s, 3.0 * s, rgb(255, 255, 255));
            }
            Some(Active::Target) if self.active_step == i => {
                let d = (r.h - 8.0 * s).min(r.w).max(20.0 * s);
                let t = R::new(r.x + (r.w - d) / 2.0, r.y + (r.h - d) / 2.0, d, d);
                g.circle(t.x + d / 2.0, t.y + d / 2.0, d / 2.0, ACCENT);
                g.circle(t.x + d / 2.0, t.y + d / 2.0, d / 4.0, rgb(10, 14, 28));
                g.hit(t, Event::Tab(T_TARGET), "target");
            }
            Some(Active::Record { .. }) if self.active_step == i => {
                let cw = live(g, r.x, r.y, iyakki_webview::proto::media_kind::MICROPHONE);
                g.text(r.x + cw, r.y + 3.0 * s, 13.0 * s, MUTED, "click the sign to stop", Face::Sans);
                // the level meter and how far the 2 seconds are
                let m = R::new(r.x, r.y + 34.0 * s, r.w, 14.0 * s);
                g.rrect(m.x, m.y, m.w, m.h, 7.0 * s, rgb(16, 20, 36));
                let lw = (m.w * level.clamp(0.0, 1.0)).max(if level > 0.0 { m.h } else { 0.0 });
                if lw > 0.0 {
                    g.rrect(m.x, m.y, lw.min(m.w), m.h, 7.0 * s, if level > 0.85 { AMBER } else { OK });
                }
                g.rect(m.x, m.y + m.h + 4.0 * s, m.w * progress.clamp(0.0, 1.0), 3.0 * s, ACCENT);
            }
            Some(Active::Grab(_)) if self.active_step == i => {
                live(g, r.x, r.y, iyakki_webview::proto::media_kind::CAMERA);
            }
            _ => {
                if let Some(pm) = &self.preview {
                    let (fw, fh) = (pm.width() as f32, pm.height() as f32);
                    let k = (r.w / fw).min((r.h - 6.0 * s) / fh);
                    let (dw, dh) = ((fw * k).floor().max(1.0), (fh * k).floor().max(1.0));
                    g.rrect(r.x, r.y, dw + 4.0 * s, dh + 4.0 * s, 4.0 * s, rgb(8, 10, 20));
                    let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
                    g.pm.draw_pixmap(0, 0, pm.as_ref(), &paint, Transform::from_row(dw / fw, 0.0, 0.0, dh / fh, (r.x + 2.0 * s).round(), (r.y + 2.0 * s).round()), None);
                }
            }
        }
    }
}

fn res_title(i: usize) -> &'static str {
    StepId::ALL[i].title()
}
