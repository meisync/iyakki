//! VOICE, RECORD: the drawing. The recorder panel (record / stop, time, level meter), the list of saved recordings and the
//! buttons under it, laid out from the top and the bottom so it holds from a phone-wide window to a large screen.

use crate::safeclamp::SafeClamp;
use iyakki_agent::voicedesk as vd;

use super::voice::*;
use super::voice_rec::*;
use super::widgets;
use crate::fx;
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, DIM, ERR, MUTED, OK, R, TEXT};

const WARN: [u8; 4] = rgb(255, 196, 94);
const REC: [u8; 4] = rgb(255, 82, 92);

fn size_text(b: u64) -> String {
    match b {
        0..=999 => format!("{b} B"),
        1_000..=999_999 => format!("{:.0} kB", b as f64 / 1e3),
        _ => format!("{:.1} MB", b as f64 / 1e6),
    }
}

impl VoiceApp {
    pub(super) fn draw_record(&mut self, g: &mut Gfx, body: R, s: f32, tick: u64) {
        let (x, w) = (body.x, body.w);
        let mut y = body.y;
        let size = 14.0 * s;
        let gap = 8.0 * s;
        let recording = self.rec.cap.is_some();
        // no microphone: say so, in words
        if self.mics.is_empty() && !recording {
            let text = vd::no_mic_text_t2(&self.product, vd::t2_mic_hidden(&iyakki_agent::voicedesk::proc_root()));
            let h = g.wrap(size, &text, Face::Sans, w - 56.0 * s).len().min(8) as f32 * g.line_height(size, Face::Sans) * 1.2;
            let card = R::new(x, y, w, h + 24.0 * s);
            fx::panel(g, card, s, WARN);
            g.icon(Icon::Mic, x + 22.0 * s, y + 20.0 * s, 20.0 * s, WARN);
            let lh = g.line_height(size, Face::Sans) * 1.2;
            for (i, l) in g.wrap(size, &text, Face::Sans, w - 56.0 * s).iter().take(8).enumerate() {
                g.text(x + 40.0 * s, y + 12.0 * s + i as f32 * lh, size, TEXT, l, Face::Sans);
            }
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

        // the recorder panel: a title line with the button, the level meter under it
        let ph = 74.0 * s;
        let panel = R::new(x, y, w, ph);
        fx::panel(g, panel, s, if recording { REC } else { ACCENT });
        let bsz = 14.0 * s;
        let bh = bsz * 2.1;
        let line_y = y + 10.0 * s;
        if recording {
            let pulse = if (tick / 15) % 2 == 0 { 255 } else { 150 };
            g.circle(x + 20.0 * s, line_y + bh / 2.0, 7.0 * s, [REC[0], REC[1], REC[2], pulse]);
            g.circle(x + 20.0 * s, line_y + bh / 2.0, 12.0 * s, fx::with_alpha(REC, 50));
            let secs = self.rec.cap.as_ref().map_or(0, |c| c.elapsed().as_secs());
            let bw = g.text_width(bsz, "Stop", Face::Bold) + bsz * 1.7;
            let t = format!("RECORDING  {} / {}", super::voice_rec::clock_of(secs as f32), super::voice_rec::clock_of(vd::MAX_RECORD_SECS as f32));
            let t = g.fit(15.0 * s, &t, Face::Bold, (w - 44.0 * s - bw - 24.0 * s).max(40.0 * s));
            g.text(x + 38.0 * s, line_y + (bh - g.line_height(15.0 * s, Face::Bold)) / 2.0, 15.0 * s, REC, &t, Face::Bold);
            g.button(x + w - bw - 12.0 * s, line_y, bsz, "Stop", REC, Event::Tab(T_RR_STOP));
            let mr = R::new(x + 16.0 * s, y + ph - 20.0 * s, w - 32.0 * s, 10.0 * s);
            fx::progress_bar(g, mr, self.rec.level.clamp(0.0, 1.0), if self.rec.level > 0.9 { ERR } else { OK }, s);
            g.rect(mr.x + mr.w * self.rec.peak.clamp(0.0, 1.0) - 1.0, mr.y - 3.0 * s, 2.0 * s.max(1.0), 16.0 * s, MUTED);
        } else {
            let bw = if self.mics.is_empty() { g.button_off(x + 14.0 * s, line_y, bsz, "● Record") } else { g.button(x + 14.0 * s, line_y, bsz, "● Record", REC, Event::Tab(T_RR_REC)) };
            let note = g.fit(13.0 * s, "Nothing is recorded until you press Record.", Face::Sans, (w - bw - 40.0 * s).max(40.0 * s));
            g.text(x + 14.0 * s + bw + 12.0 * s, line_y + (bh - g.line_height(13.0 * s, Face::Sans)) / 2.0, 13.0 * s, DIM, &note, Face::Sans);
            let mr = R::new(x + 16.0 * s, y + ph - 20.0 * s, w - 32.0 * s, 10.0 * s);
            fx::progress_bar(g, mr, self.rec.level.clamp(0.0, 1.0), OK, s);
        }
        y += ph + gap;

        // the bottom block first, so the list takes what is left
        let bottom_h = self.rec_bottom(g, x, 0.0, w, s, false);
        let by = (body.y + body.h - bottom_h).max(y + 60.0 * s);

        // the list
        let n = self.rec.list.len();
        g.text(x, y, 13.5 * s, MUTED, &if n == 0 { "Saved recordings".to_string() } else { format!("Saved recordings ({n})") }, Face::Bold);
        let ly = y + 13.5 * s * 1.5;
        let (rh, lh) = (34.0 * s, (by - ly - gap).max(34.0 * s));
        let cap = ((lh / rh) as usize).max(1);
        if self.rec.sel < self.rec.top {
            self.rec.top = self.rec.sel;
        } else if self.rec.sel >= self.rec.top + cap {
            self.rec.top = self.rec.sel + 1 - cap;
        }
        self.rec.top = self.rec.top.min(n.saturating_sub(cap));
        let frame = R::new(x, ly, w, rh * cap as f32);
        g.rrect(frame.x, frame.y, frame.w, frame.h, 8.0 * s, crate::ui::FIELD);
        if n == 0 {
            let msg = g.fit(13.5 * s, "No recordings yet. Press Record and speak.", Face::Sans, w - 24.0 * s);
            g.text(x + 12.0 * s, ly + (rh - g.line_height(13.5 * s, Face::Sans)) / 2.0, 13.5 * s, DIM, &msg, Face::Sans);
        }
        let playing = self.rec.playing.as_ref().map(|(n, _)| n.clone());
        for (k, e) in self.rec.list.iter().enumerate().skip(self.rec.top).take(cap) {
            let r = R::new(x, ly + (k - self.rec.top) as f32 * rh, w, rh);
            let on = k == self.rec.sel;
            if on {
                g.rrect(r.x + 2.0 * s, r.y + 2.0 * s, r.w - 4.0 * s, r.h - 4.0 * s, 6.0 * s, fx::with_alpha(ACCENT, 55));
            }
            g.hit(r, Event::Tab(T_RR_ROW + k), "recording");
            let is_playing = playing.as_deref() == Some(e.name.as_str());
            let meta = format!("{} · {}", super::voice_rec::clock_of(e.secs), size_text(e.bytes));
            let mw = g.text_width(12.5 * s, &meta, Face::Sans);
            let name = format!("{}{}", if is_playing { "▶ " } else { "" }, e.name);
            let name = g.fit(14.0 * s, &name, Face::Sans, (w - mw - 36.0 * s).max(40.0 * s));
            let ty = r.y + (rh - g.line_height(14.0 * s, Face::Sans)) / 2.0;
            g.text(r.x + 12.0 * s, ty, 14.0 * s, if on { TEXT } else { MUTED }, &name, Face::Sans);
            g.text(r.x + r.w - mw - 12.0 * s, r.y + (rh - g.line_height(12.5 * s, Face::Sans)) / 2.0, 12.5 * s, DIM, &meta, Face::Sans);
        }
        self.rec_bottom(g, x, by, w, s, true);
        self.draw_rec_confirm(g, body, s);
    }

    /// The line about where recordings go, the message, the rename box and the buttons. Returns its height.
    fn rec_bottom(&mut self, g: &mut Gfx, x: f32, y0: f32, w: f32, s: f32, draw: bool) -> f32 {
        let size = 13.0 * s;
        let mut y = y0;
        let para = |g: &mut Gfx, y: f32, c: [u8; 4], sz: f32, t: &str, max: usize| -> f32 {
            let lines = g.wrap(sz, t, Face::Sans, w);
            let lh = g.line_height(sz, Face::Sans) * 1.2;
            if draw {
                for (i, l) in lines.iter().take(max).enumerate() {
                    g.text(x, y + i as f32 * lh, sz, c, l, Face::Sans);
                }
            }
            lines.len().clamp_safe(1, max) as f32 * lh + 2.0 * s
        };
        y += para(g, y, DIM, size, "Each recording is saved as a WAV file in your Recordings folder. Sound stays on this computer.", 2);
        if let Some((t, ok)) = &self.status {
            let t = t.clone();
            y += para(g, y, if *ok { OK } else { ERR }, 14.0 * s, &t, 4);
        }
        let bsz = 13.5 * s;
        let (bh, gap) = (bsz * 2.1, 8.0 * s);
        let mut btns: Vec<(&str, usize, bool, [u8; 4])> = Vec::new();
        if matches!(self.rec.ask, Ask::Rename(_)) {
            let fh = 30.0 * s;
            if draw {
                let lab = "New name:";
                let lw = g.text_width(size, lab, Face::Sans) + 10.0 * s;
                g.text(x, y + (fh - g.line_height(size, Face::Sans)) / 2.0, size, MUTED, lab, Face::Sans);
                self.rec.name.render(g, R::new(x + lw, y, w - lw, fh), true, s, super::blink(0), Face::Sans, NAME_FIELD);
            }
            y += fh + 4.0 * s;
            btns.push(("Save name", T_RR_SAVE_NAME, true, ACCENT));
            btns.push(("Cancel", T_RR_CANCEL_NAME, true, MUTED));
        } else {
            let have = !self.rec.list.is_empty();
            let rec = self.rec.cap.is_some();
            if self.rec.playing.is_some() {
                btns.push(("Stop playing", T_RR_PSTOP, true, ACCENT));
            } else {
                btns.push(("Play", T_RR_PLAY, have && !rec, ACCENT));
            }
            btns.push(("Rename", T_RR_RENAME, have, MUTED));
            btns.push(("Delete", T_RR_DELETE, have, MUTED));
        }
        let (mut cx, mut cy) = (x, y);
        for (label, n, on, color) in btns {
            let bw = g.text_width(bsz, label, Face::Bold) + bsz * 1.7;
            if cx > x && cx + bw > x + w {
                cx = x;
                cy += bh + gap;
            }
            if draw {
                if on {
                    g.button(cx, cy, bsz, label, color, Event::Tab(n));
                } else {
                    g.button_off(cx, cy, bsz, label);
                }
            }
            cx += bw + gap;
        }
        cy + bh - y0 + 2.0 * s
    }

    /// The question before a recording is deleted.
    fn draw_rec_confirm(&mut self, g: &mut Gfx, area: R, s: f32) {
        let Ask::Delete(name) = &self.rec.ask else { return };
        let p = fx::modal(g, area, s, 480.0 * s, 210.0 * s, ERR, true);
        let y = fx::modal_title(g, p, s, "Delete this recording?", ERR);
        let t = format!("{name} will be deleted from your Recordings folder. This cannot be undone.");
        let lh = g.line_height(14.5 * s, Face::Sans) * 1.25;
        for (i, l) in g.wrap(14.5 * s, &t, Face::Sans, p.w - 52.0 * s).iter().take(3).enumerate() {
            g.text(p.x + 26.0 * s, y + 4.0 * s + i as f32 * lh, 14.5 * s, TEXT, l, Face::Sans);
        }
        let by = p.y + p.h - 52.0 * s;
        let w1 = g.button(p.x + 26.0 * s, by, 14.0 * s, "Delete", ERR, Event::Tab(T_RR_YES));
        g.button(p.x + 26.0 * s + w1 + 12.0 * s, by, 14.0 * s, "Keep it", MUTED, Event::Tab(T_RR_NO));
    }
}
