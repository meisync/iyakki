//! VOICE: the drawing. Everything is laid out from the top (tabs, controls) and the bottom (messages, buttons) with the text box
//! taking what is left, so the page holds from a phone-wide window to a large screen; buttons wrap onto more lines when the
//! width is short.

use crate::safeclamp::SafeClamp;
use iyakki_agent::story::Status;
use iyakki_agent::voicedesk as vd;

use super::voice::*;
use super::widgets;
use super::AppCx;
use crate::fx;
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

const WARN: [u8; 4] = rgb(255, 196, 94);
const REC: [u8; 4] = rgb(255, 82, 92);

/// A button to draw: its label, what a click sends, whether it works, and its colour.
struct Btn {
    label: String,
    ev: Event,
    on: bool,
    color: [u8; 4],
}

fn btn(label: &str, n: usize, on: bool, color: [u8; 4]) -> Btn {
    Btn { label: label.to_string(), ev: Event::Tab(n), on, color }
}

/// Lay buttons out left to right, wrapping when the row is full. Returns the height used.
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
    cy + h - y
}

/// Wrapped text, at most `max` lines. Returns the height used.
#[allow(clippy::too_many_arguments)]
fn para(g: &mut Gfx, x: f32, y: f32, w: f32, size: f32, color: [u8; 4], text: &str, max: usize, draw: bool) -> f32 {
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

fn clock(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

impl VoiceApp {
    pub(super) fn draw(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let m = 16.0 * s;
        let (x, w) = (area.x + m, (area.w - 2.0 * m).max(40.0));
        let y = area.y + 10.0 * s;
        let tabs: Vec<(String, Event)> = vec![("READ ALOUD".into(), Event::Tab(T_TAB_READ)), ("DICTATE".into(), Event::Tab(T_TAB_DICT)), ("RECORD".into(), Event::Tab(super::voice_rec::T_TAB_REC))];
        fx::tab_strip(g, x, y, s, w, &tabs, match self.area {
            Area::Read => 0,
            Area::Dictate => 1,
            Area::Record => 2,
        }, ACCENT);
        let top = y + 14.0 * s * 2.1 + 12.0 * s;
        let body = R::new(x, top, w, (area.y + area.h - top - 8.0 * s).max(40.0));
        match self.area {
            Area::Read => self.draw_read(g, body, s, cx, tick),
            Area::Dictate => self.draw_dictate(g, body, s, cx, tick),
            Area::Record => self.draw_record(g, body, s, tick),
        }
        self.draw_modal(g, area, s, tick);
    }

    // ------------------------------------------------------------------------------------------------ read aloud

    /// The line about the language and the line about where the text goes, with the colours they get.
    pub(super) fn read_notes(&self, cx: &AppCx) -> Vec<(String, [u8; 4])> {
        let mut v = Vec::new();
        let voice = self.voice();
        if let Some(h) = vd::detect_language(&self.text.text) {
            match vd::language_warning(&self.text.text, voice) {
                Some(w) => v.push((w, WARN)),
                None => v.push((format!("Looks like {}.", h.name), DIM)),
            }
        }
        match voice {
            Some(vc) => v.push((vd::where_text(&cx.services, &vc.engine, "Text is"), MUTED)),
            None => v.push(("Text is not sent anywhere until a voice is set up.".into(), MUTED)),
        }
        v
    }

    fn progress_line(&self) -> (String, [u8; 4], Option<f32>) {
        let snap = self.reader.snapshot();
        let of = |n: usize| format!("sentence {} of {}", snap.chunk + 1, n.max(1));
        match snap.status {
            Status::Buffering => ("Getting the voice ready…".into(), MUTED, Some(snap.fraction)),
            Status::Playing => (format!("Reading {}", of(snap.chunks)), OK, Some(snap.fraction)),
            Status::Paused => (format!("Paused at {}", of(snap.chunks)), WARN, Some(snap.fraction)),
            Status::Done if self.status.is_none() => ("Finished reading.".into(), OK, Some(1.0)),
            _ => match &self.status {
                Some((t, ok)) => (t.clone(), if *ok { OK } else { ERR }, None),
                None => (String::new(), MUTED, None),
            },
        }
    }

    fn draw_read(&mut self, g: &mut Gfx, body: R, s: f32, cx: &AppCx, tick: u64) {
        let (x, w) = (body.x, body.w);
        let mut y = body.y;
        let size = 14.0 * s;
        let gap = 8.0 * s;
        // a voice has to exist before anything can be read
        if self.voices.is_empty() {
            let h = para(g, x + 12.0 * s, y + 10.0 * s, w - 24.0 * s, size, TEXT, "No reading voice is set up. Kokoro is a natural English voice that runs on this computer; Store \u{2192} Setup sets it up.", 3, false);
            let bh = 13.5 * s * 2.1;
            let card = R::new(x, y, w, h + bh + 28.0 * s);
            fx::panel(g, card, s, WARN);
            para(g, x + 12.0 * s, y + 10.0 * s, w - 24.0 * s, size, TEXT, "No reading voice is set up. Kokoro is a natural English voice that runs on this computer; Store \u{2192} Setup sets it up.", 3, true);
            let items = [btn("Set up Kokoro", T_SETUP, true, ACCENT), btn("Voice settings", T_SETTINGS_VOICE, true, MUTED)];
            flow(g, x + 12.0 * s, y + h + 18.0 * s, w - 24.0 * s, s, &items, true);
            y += card.h + gap;
        }
        // the voice and the speed
        let label = self.voice().map_or("No voice".to_string(), |v| v.label.clone());
        let sw = widgets::select_width(g, s, &label).clamp_safe(150.0 * s, (w * 0.55).max(150.0 * s));
        let sel = widgets::select_box(g, x, y, sw, s, &label, matches!(self.modal, Some(Modal::VoicePick(_))), ACCENT, Event::Tab(T_VOICE), "voice");
        let wide = w > 640.0 * s;
        let (mut sx, mut sy) = if wide { (x + sw + 20.0 * s, y) } else { (x, y + sel.h + gap) };
        let takes = self.voice().is_none_or(|v| v.takes_speed());
        let txt = if takes { format!("Speed {:.2}x", self.speed) } else { "Speed: fixed for this voice".to_string() };
        g.text(sx, sy + (sel.h - g.line_height(size, Face::Sans)) / 2.0, size, if takes { TEXT } else { DIM }, &txt, Face::Sans);
        sx += g.text_width(size, "Speed 1.50x", Face::Sans) + 12.0 * s;
        if takes {
            let bw = sel.h;
            g.button(sx, sy + (sel.h - 13.5 * s * 2.1) / 2.0, 13.5 * s, "−", MUTED, Event::Tab(T_SPEED_DN));
            sx += 13.5 * s * 2.1 + 6.0 * s;
            let track_w = (x + w - sx - 13.5 * s * 3.0).clamp_safe(60.0 * s, 260.0 * s);
            let tr = R::new(sx, sy + sel.h / 2.0 - 4.0 * s, track_w, 8.0 * s);
            let f = (self.speed - vd::SPEED_MIN) / (vd::SPEED_MAX - vd::SPEED_MIN);
            fx::progress_bar(g, tr, f, ACCENT, s);
            g.circle(tr.x + tr.w * f.clamp(0.0, 1.0), tr.y + tr.h / 2.0, 7.0 * s, TEXT);
            let hit = R::new(tr.x - 6.0 * s, sy, tr.w + 12.0 * s, bw);
            g.hit(hit, Event::Area(SPEED_AREA), "speed");
            self.speed_rect = R::new(hit.x + 6.0 * s, hit.y, tr.w, hit.h);
            sx += track_w + 8.0 * s;
            g.button(sx, sy + (sel.h - 13.5 * s * 2.1) / 2.0, 13.5 * s, "+", MUTED, Event::Tab(T_SPEED_UP));
            sx += 13.5 * s * 2.1;
        }
        let _ = sx;
        sy += sel.h;
        y = if wide { y + sel.h } else { sy } + gap;

        // the bottom block first, so the text box takes what is left
        let notes = self.read_notes(cx);
        let bottom_h = self.read_bottom(g, x, 0.0, w, s, &notes, false);
        let by = (body.y + body.h - bottom_h).max(y + 80.0 * s);
        let (text_h, lh) = ((by - y - gap - 18.0 * s).max(40.0 * s), g.line_height(15.0 * s, Face::Sans));
        let rows = (((text_h - 18.0 * s) / lh) as usize).clamp(2, 40);
        self.text.min_rows = rows;
        self.text.max_rows = rows;
        self.text.render(g, x, y, w, s, 15.0 * s, 9.0 * s, 40.0 * s, self.focus == Focus::Text, tick, "Paste or type the text to read aloud (Ctrl+V pastes)", ACCENT, TEXT_AREA, "text", false, true);
        self.read_bottom(g, x, by, w, s, &notes, true);
    }

    /// The notes, the progress and the buttons under the text box. Returns its height.
    #[allow(clippy::too_many_arguments)]
    fn read_bottom(&mut self, g: &mut Gfx, x: f32, y0: f32, w: f32, s: f32, notes: &[(String, [u8; 4])], draw: bool) -> f32 {
        let size = 13.0 * s;
        let mut y = y0;
        for (t, c) in notes {
            y += para(g, x, y, w, size, *c, t, 2, draw) + 2.0 * s;
        }
        // progress
        let (line, color, frac) = self.progress_line();
        if !line.is_empty() || frac.is_some() {
            y += para(g, x, y, w, 14.0 * s, color, &line, 3, draw) + 2.0 * s;
        }
        let bar_h = 10.0 * s;
        if let Some(f) = frac {
            if draw {
                let r = R::new(x, y, w, bar_h);
                fx::progress_bar(g, r, f, ACCENT, s);
                g.hit(R::new(x, y - 4.0 * s, w, bar_h + 8.0 * s), Event::Area(PROGRESS_AREA), "progress");
                self.progress_rect = r;
            }
            y += bar_h + 8.0 * s;
        }
        if self.focus == Focus::Path {
            let fh = 30.0 * s;
            if draw {
                let lab = "Open a text file:";
                let lw = g.text_width(size, lab, Face::Sans) + 10.0 * s;
                g.text(x, y + (fh - g.line_height(size, Face::Sans)) / 2.0, size, MUTED, lab, Face::Sans);
                self.path.render(g, R::new(x + lw, y, w - lw, fh), true, s, super::blink(0), Face::Sans, PATH_FIELD);
            }
            y += fh + 4.0 * s;
            y += para(g, x, y, w, size, DIM, "Enter opens it. Esc cancels. Only files your account may read open.", 2, draw) + 2.0 * s;
        }
        let active = self.reader.is_active();
        let paused = self.reader.status() == Status::Paused;
        let sel = self.text.selection().is_some() && !active;
        let items = [
            btn(if paused { "Resume" } else if active { "Pause" } else { "Play" }, if active { T_PAUSE } else { T_PLAY }, true, ACCENT),
            btn("Stop", T_STOP, active, MUTED),
            btn("Read selection", T_SEL, sel, ACCENT),
            btn("Paste", T_PASTE, true, MUTED),
            btn("Open file", T_LOAD, true, MUTED),
            btn("Clear", T_CLEAR, !self.text.is_empty(), MUTED),
        ];
        y += flow(g, x, y, w, s, &items, draw);
        y - y0 + 2.0 * s
    }

    // ------------------------------------------------------------------------------------------------ dictate

    fn draw_dictate(&mut self, g: &mut Gfx, body: R, s: f32, cx: &AppCx, tick: u64) {
        let (x, w) = (body.x, body.w);
        let mut y = body.y;
        let size = 14.0 * s;
        let gap = 8.0 * s;
        let recording = self.cap.is_some();
        // no microphone: say so, in words
        if self.mics.is_empty() && !recording {
            let text = vd::no_mic_text_t2(&self.product, vd::t2_mic_hidden(std::path::Path::new("/proc")));
            let h = para(g, 0.0, 0.0, w - 24.0 * s, size, TEXT, &text, 8, false);
            let card = R::new(x, y, w, h + 24.0 * s);
            fx::panel(g, card, s, WARN);
            g.icon(Icon::Mic, x + 22.0 * s, y + 20.0 * s, 20.0 * s, WARN);
            para(g, x + 40.0 * s, y + 12.0 * s, w - 56.0 * s, size, TEXT, &text, 8, true);
            y += card.h + gap;
        } else {
            let label = match self.mic_i.checked_sub(1).and_then(|i| self.mics.get(i)) {
                Some(m) => m.label.clone(),
                None => "Automatic".to_string(),
            };
            let mw = widgets::select_width(g, s, &label).clamp_safe(170.0 * s, (w * 0.7).max(170.0 * s));
            g.text(x, y + 6.0 * s, size, MUTED, "Microphone", Face::Sans);
            let lx = x + g.text_width(size, "Microphone", Face::Sans) + 10.0 * s;
            let r = widgets::select_box(g, lx.min(x + w - 120.0 * s), y, mw.min(w - (lx - x) - 4.0 * s), s, &label, matches!(self.modal, Some(Modal::MicPick(_))), ACCENT, Event::Tab(T_MIC), "microphone");
            y += r.h + gap + 2.0 * s;
        }

        // bottom block
        let bottom_h = self.dictate_bottom(g, x, 0.0, w, s, cx, false);
        let by = (body.y + body.h - bottom_h).max(y + 90.0 * s);

        // the recording panel
        let ph = 52.0 * s;
        let panel = R::new(x, y, w, ph);
        let busy = self.transcribing;
        fx::panel(g, panel, s, if recording { REC } else { ACCENT });
        if recording {
            let pulse = if (tick / 15) % 2 == 0 { 255 } else { 150 };
            g.circle(x + 22.0 * s, y + ph / 2.0, 7.0 * s, [REC[0], REC[1], REC[2], pulse]);
            g.circle(x + 22.0 * s, y + ph / 2.0, 12.0 * s, fx::with_alpha(REC, 50));
            let secs = self.cap.as_ref().map_or(0, |c| c.elapsed().as_secs());
            let t = format!("RECORDING  {} / {}", clock(secs), clock(vd::MAX_RECORD_SECS as u64));
            g.text(x + 42.0 * s, y + (ph - g.line_height(15.0 * s, Face::Bold)) / 2.0, 15.0 * s, REC, &t, Face::Bold);
            // the level meter
            let tw = g.text_width(15.0 * s, &t, Face::Bold);
            let mx = x + 42.0 * s + tw + 16.0 * s;
            let bw = (x + w - mx - 140.0 * s).max(40.0 * s);
            fx::progress_bar(g, R::new(mx, y + ph / 2.0 - 5.0 * s, bw, 10.0 * s), self.level.clamp(0.0, 1.0), if self.level > 0.9 { ERR } else { OK }, s);
            g.rect(mx + bw * self.peak.clamp(0.0, 1.0) - 1.0, y + ph / 2.0 - 8.0 * s, 2.0 * s.max(1.0), 16.0 * s, MUTED);
            let bx = x + w - 112.0 * s;
            g.button(bx.max(mx + bw + 8.0 * s), y + (ph - 13.5 * s * 2.1) / 2.0, 13.5 * s, "Stop", REC, Event::Tab(T_REC_STOP));
        } else if busy {
            let dots = ".".repeat(1 + (tick / 10 % 3) as usize);
            g.text(x + 16.0 * s, y + (ph - g.line_height(15.0 * s, Face::Bold)) / 2.0, 15.0 * s, TEXT, &format!("Turning your speech into text{dots}"), Face::Bold);
        } else {
            let can = !self.mics.is_empty();
            if can {
                g.button(x + 14.0 * s, y + (ph - 14.5 * s * 2.1) / 2.0, 14.5 * s, "● Record", REC, Event::Tab(T_REC));
                g.text(x + 150.0 * s, y + (ph - g.line_height(13.0 * s, Face::Sans)) / 2.0, 13.0 * s, DIM, "Nothing is recorded until you press Record.", Face::Sans);
            } else {
                g.button_off(x + 14.0 * s, y + (ph - 14.5 * s * 2.1) / 2.0, 14.5 * s, "● Record");
            }
        }
        let ty = y + ph + gap;
        let (text_h, lh) = ((by - ty - gap - 18.0 * s).max(40.0 * s), g.line_height(15.0 * s, Face::Sans));
        let rows = (((text_h - 18.0 * s) / lh) as usize).clamp(2, 40);
        self.transcript.min_rows = rows;
        self.transcript.max_rows = rows;
        self.transcript.render(g, x, ty, w, s, 15.0 * s, 9.0 * s, 40.0 * s, self.focus == Focus::Transcript, tick, "The transcript appears here. You can edit it.", ACCENT, TRANS_AREA, "transcript", false, true);
        self.dictate_bottom(g, x, by, w, s, cx, true);
    }

    fn dictate_bottom(&mut self, g: &mut Gfx, x: f32, y0: f32, w: f32, s: f32, cx: &AppCx, draw: bool) -> f32 {
        let size = 13.0 * s;
        let mut y = y0;
        let where_line = match vd::stt_engine(&cx.services) {
            Some(e) => vd::where_text(&cx.services, &e, "Sound is"),
            None => "No speech-to-text is set up. Install Whisper with Settings -> Models (Lemonade), or set one in Settings -> Voice.".to_string(),
        };
        y += para(g, x, y, w, size, MUTED, &where_line, 2, draw) + 2.0 * s;
        y += para(g, x, y, w, size, DIM, "The sound is kept in memory only and is never saved to disk.", 1, draw) + 2.0 * s;
        if let Some((t, ok)) = &self.status {
            y += para(g, x, y, w, 14.0 * s, if *ok { OK } else { ERR }, t, 4, draw) + 2.0 * s;
        }
        if self.stt_missing(cx) {
            let items = [btn("Voice settings", T_SETTINGS_VOICE, true, ACCENT)];
            y += flow(g, x, y, w, s, &items, draw) + 4.0 * s;
        }
        let has = !self.transcript.text.trim().is_empty();
        let items = [btn("Copy to clipboard", T_COPY, has, ACCENT), btn("Copy and clear", T_COPY_CLEAR, has, MUTED), btn("Read it back", T_READ_BACK, has, MUTED), btn("Clear", T_T_CLEAR, has, MUTED)];
        y += flow(g, x, y, w, s, &items, draw);
        y - y0 + 2.0 * s
    }

    fn stt_missing(&self, cx: &AppCx) -> bool {
        vd::stt_engine(&cx.services).is_none()
    }

    // ------------------------------------------------------------------------------------------------ cards

    fn draw_modal(&mut self, g: &mut Gfx, area: R, s: f32, _tick: u64) {
        match &self.modal {
            Some(Modal::Consent { text, dest, then }) => {
                let listening = matches!(then, After::Record);
                let title = if listening { "Send your voice to another computer?" } else { "Send this text to another computer?" };
                let p = fx::modal(g, area, s, 560.0 * s, 300.0 * s, WARN, false);
                let y = fx::modal_title(g, p, s, title, WARN);
                let lines = g.wrap(14.5 * s, text, Face::Sans, p.w - 52.0 * s);
                let lh = g.line_height(14.5 * s, Face::Sans) * 1.25;
                for (i, l) in lines.iter().take(7).enumerate() {
                    g.text(p.x + 26.0 * s, y + 6.0 * s + i as f32 * lh, 14.5 * s, TEXT, l, Face::Sans);
                }
                let place = format!("{} · {} · {}", dest.name, dest.address, dest.place_words());
                let place = g.fit(13.0 * s, &place, Face::Sans, p.w - 52.0 * s);
                g.text(p.x + 26.0 * s, p.y + p.h - 96.0 * s, 13.0 * s, DIM, &place, Face::Sans);
                let by = p.y + p.h - 58.0 * s;
                let w1 = g.button(p.x + 26.0 * s, by, 14.0 * s, "Yes, send", ACCENT, Event::Tab(T_YES));
                g.button(p.x + 26.0 * s + w1 + 12.0 * s, by, 14.0 * s, "No", MUTED, Event::Tab(T_NO));
                g.text(p.x + p.w - 150.0 * s, by + 6.0 * s, 12.5 * s, DIM, "y yes · Esc no", Face::Sans);
            }
            Some(Modal::VoicePick(p)) | Some(Modal::MicPick(p)) => p.render(g, area, s, ACCENT),
            None => {}
        }
    }
}
