//! The recordings page of the CAMERAS tile (F-CAM-03): the camera's recording setting, a timeline with a day picker, play / pause /
//! step, export of a clip after a confirmation, and the camera's log of who looked at what.
//!
//! * Recording is off unless the owner turned it on. Turning it on always shows what it stores, where, for how long, and the red
//!   REC mark; turning it off is one key or one click and asks nothing.
//! * Only the owner sees the recordings; everybody else is told so.
//! * Pictures come from the source already decoded; the tile never touches a file, a key or the network.
//! * H.264 recordings cannot be played here without the video add-on; the page says so and offers the export.

use iyakki_tile_api::cameras::{AuditRow, PlayFrame, RecChange, RecDay, RecInfo, RecMode, RecSeek};
use iyakki_tile_api::TileCx;
use iyakki_uikit::gfx::{rgb, Face, Gfx};
use iyakki_uikit::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};
use iyakki_uikit::widgets::{Form, FormEvent};
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

use crate::{ACCENT, RED};

const DAY_MS: u64 = 86_400_000;
/// 15 minutes per click target on the timeline.
const SLOT_MS: u64 = 900_000;
const SLOTS: usize = 96;
const T_PLAY: usize = 200;
const T_BACK1: usize = 201;
const T_FWD1: usize = 202;
const T_OLDER: usize = 203;
const T_NEWER: usize = 204;
const T_RECORD: usize = 205;
const T_EXPORT: usize = 206;
const T_LOG: usize = 207;
const T_SETTINGS: usize = 208;
const T_BACK: usize = 209;
const T_SLOT0: usize = 300;
/// Milliseconds of recording that pass per heartbeat while playing (the shell beats about 30 times a second).
const PLAY_STEP_MS: u64 = 33;

pub enum Out {
    Stay,
    Back,
}

enum Ask {
    Start(RecChange),
    Export { from: u64 },
}

enum View {
    Main,
    Settings(Box<Form>),
    Confirm(Ask),
    Log(Vec<AuditRow>),
}

pub struct RecView {
    pub id: String,
    pub name: String,
    info: Option<RecInfo>,
    days: Vec<RecDay>,
    day: usize,
    frame: Option<PlayFrame>,
    pix: Option<(u64, Pixmap)>,
    pos_ms: u64,
    playing: bool,
    note: Option<(String, bool, u64)>,
    view: View,
    tick: u64,
    last_info: u64,
    clip_mins: u32,
    /// The camera's motion events (F-CAM-04), shown as marks on the timeline.
    motion: Vec<iyakki_tile_api::cameras::MotionRow>,
}

fn mb_text(mb: u64) -> String {
    if mb == 0 { "no limit of its own".into() } else { format!("{:.1} GB", mb as f64 / 1024.0) }
}

fn days_text(d: u32) -> String {
    if d == 0 { "no age limit".into() } else { format!("{d} days") }
}

impl RecView {
    pub fn new(cx: &TileCx, id: &str, name: &str) -> RecView {
        let mut v = RecView { id: id.into(), name: name.into(), info: None, days: vec![], day: 0, frame: None, pix: None, pos_ms: 0, playing: false, note: None, view: View::Main, tick: 0, last_info: 0, clip_mins: 1, motion: vec![] };
        v.reload(cx);
        if let Some(d) = v.days.first() {
            let at = d.spans.first().map_or(d.day_start_ms, |s| s.0);
            v.opened(cx, at);
            v.fetch(cx, at, RecSeek::AtOrAfter);
        }
        v
    }

    pub fn recording(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.recording)
    }
    pub fn playing(&self) -> bool {
        self.playing
    }
    pub fn shows_frame(&self) -> bool {
        self.frame.is_some()
    }
    pub fn confirming(&self) -> bool {
        matches!(self.view, View::Confirm(_))
    }
    pub fn in_settings(&self) -> bool {
        matches!(self.view, View::Settings(_))
    }
    pub fn showing_log(&self) -> bool {
        matches!(self.view, View::Log(_))
    }
    pub fn note(&self) -> Option<(&str, bool)> {
        self.note.as_ref().map(|(t, ok, _)| (t.as_str(), *ok))
    }
    pub fn day_label(&self) -> Option<&str> {
        self.days.get(self.day).map(|d| d.label.as_str())
    }
    pub fn position(&self) -> u64 {
        self.pos_ms
    }
    pub fn hint(&self) -> String {
        match self.view {
            View::Main => "Space play/pause · ← → step · [ ] day · r record on/off · c settings · e export · l log · Esc back".into(),
            View::Settings(_) => "Tab next field · Enter save · Esc cancel".into(),
            View::Confirm(_) => "y yes · Esc cancel".into(),
            View::Log(_) => "Esc back".into(),
        }
    }

    fn say(&mut self, t: impl Into<String>, ok: bool) {
        self.note = Some((t.into(), ok, self.tick));
    }

    fn reload(&mut self, cx: &TileCx) {
        self.info = cx.cameras.rec_info(&self.id);
        self.last_info = self.tick;
        if self.info.as_ref().is_some_and(|i| i.owner) {
            match cx.cameras.rec_days(&self.id) {
                Ok(d) => {
                    self.days = d;
                    self.day = self.day.min(self.days.len().saturating_sub(1));
                }
                Err(e) => self.say(e, false),
            }
            self.motion = cx.cameras.motion_events(&self.id, 0, u64::MAX, 2000).unwrap_or_default();
        }
    }

    /// How many motion marks the selected day's timeline shows.
    pub fn motion_marks(&self) -> usize {
        self.days.get(self.day).map_or(0, |d| crate::motion::day_marks(&self.motion, d.day_start_ms).len())
    }

    fn opened(&mut self, cx: &TileCx, at: u64) {
        if let Err(e) = cx.cameras.rec_play_start(&self.id, at) {
            self.say(e, false);
        }
    }

    fn fetch(&mut self, cx: &TileCx, at: u64, seek: RecSeek) -> bool {
        match cx.cameras.rec_frame(&self.id, at, seek) {
            Ok(Some(f)) => {
                self.pos_ms = f.ts_ms;
                if let Some(i) = self.days.iter().position(|d| f.ts_ms >= d.day_start_ms && f.ts_ms < d.day_start_ms + DAY_MS) {
                    self.day = i;
                }
                if self.pix.as_ref().is_none_or(|(t, _)| *t != f.ts_ms) {
                    self.pix = tiny_skia::IntSize::from_wh(f.width, f.height).and_then(|sz| Pixmap::from_vec((*f.rgba).clone(), sz)).map(|p| (f.ts_ms, p));
                }
                self.frame = Some(f);
                true
            }
            Ok(None) => {
                self.playing = false;
                self.say(if seek == RecSeek::Before { "This is the first recorded picture." } else { "That was the last recorded picture." }, true);
                false
            }
            Err(e) => {
                self.playing = false;
                self.say(e, false);
                false
            }
        }
    }

    fn go_day(&mut self, cx: &TileCx, i: usize) {
        if let Some(d) = self.days.get(i) {
            let at = d.spans.first().map_or(d.day_start_ms, |s| s.0);
            self.day = i;
            self.playing = false;
            self.fetch(cx, at, RecSeek::AtOrAfter);
        }
    }

    fn seek_slot(&mut self, cx: &TileCx, slot: usize) {
        let Some(d) = self.days.get(self.day) else { return };
        let at = d.day_start_ms + slot as u64 * SLOT_MS;
        if !self.fetch(cx, at, RecSeek::AtOrAfter) {
            self.fetch(cx, at, RecSeek::Before);
        }
    }

    fn toggle_record(&mut self, cx: &TileCx) {
        let Some(info) = self.info.clone() else {
            self.say("Recording is not available here.", false);
            return;
        };
        if info.mode == RecMode::Off {
            if !info.owner {
                self.say("Only the person who added the camera can turn its recording on.", false);
            } else {
                self.view = View::Confirm(Ask::Start(RecChange { mode: Some(RecMode::Continuous), ..Default::default() }));
            }
        } else {
            self.apply(cx, &RecChange { mode: Some(RecMode::Off), ..Default::default() }, "Recording stopped. Earlier recordings are kept until their retention ends.");
        }
    }

    fn apply(&mut self, cx: &TileCx, ch: &RecChange, done: &str) {
        match cx.cameras.rec_set(&self.id, ch) {
            Ok(()) => {
                self.reload(cx);
                self.say(done, true);
            }
            Err(e) => self.say(e, false),
        }
    }

    fn settings_form(&self) -> Form {
        let i = self.info.clone();
        let mode = match i.as_ref().map(|i| i.mode) {
            Some(RecMode::Continuous) => "continuous",
            Some(RecMode::Schedule) => "schedule",
            _ => "off",
        };
        let mut f = Form::new("Recording settings")
            .with_note("Mode: off, continuous or schedule. Windows (for schedule), separated by semicolons: mon-fri 08:00-18:00; sat,sun 00:00-24:00. Old recordings are deleted by age and by size; recording stops when the disk is nearly full.")
            .text("Mode", mode, "off, continuous or schedule")
            .text("Windows", &i.as_ref().map(|i| i.windows.join("; ")).unwrap_or_default(), "mon-fri 08:00-18:00")
            .text("Keep days", &i.as_ref().map(|i| i.max_days.to_string()).unwrap_or_else(|| "14".into()), "0 for no age limit")
            .text("Most GB", &i.as_ref().map(|i| format!("{}", i.max_mb as f64 / 1024.0)).unwrap_or_else(|| "20".into()), "0 for no limit of its own");
        f.focus = 0;
        f
    }

    fn submit_settings(&mut self, cx: &TileCx, mut f: Box<Form>) {
        let mode = match f.value(0).trim().to_lowercase().as_str() {
            "off" => RecMode::Off,
            "continuous" => RecMode::Continuous,
            "schedule" => RecMode::Schedule,
            _ => {
                f.error = Some("Mode is off, continuous or schedule.".into());
                self.view = View::Settings(f);
                return;
            }
        };
        let windows: Vec<String> = f.value(1).split(';').map(|w| w.trim().to_string()).filter(|w| !w.is_empty()).collect();
        let (days, gb) = (f.value(2).trim().parse::<u32>(), f.value(3).trim().parse::<f64>());
        let (Ok(days), Ok(gb)) = (days, gb) else {
            f.error = Some("Keep days is a whole number and Most GB a number (0 means no limit).".into());
            self.view = View::Settings(f);
            return;
        };
        if !(0.0..1.0e7).contains(&gb) {
            f.error = Some("Most GB is a number (0 means no limit).".into());
            self.view = View::Settings(f);
            return;
        }
        let ch = RecChange { mode: Some(mode), windows: Some(windows), max_days: Some(days), max_mb: Some((gb * 1024.0).round() as u64) };
        let was_off = self.info.as_ref().is_none_or(|i| i.mode == RecMode::Off);
        if mode != RecMode::Off && was_off {
            self.view = View::Confirm(Ask::Start(ch));
            return;
        }
        match cx.cameras.rec_set(&self.id, &ch) {
            Ok(()) => {
                self.reload(cx);
                self.say("Recording settings saved.", true);
            }
            Err(e) => {
                f.error = Some(e);
                self.view = View::Settings(f);
            }
        }
    }

    pub fn handle(&mut self, ev: &Event, cx: &TileCx) -> Out {
        if matches!(ev, Event::Tick) {
            self.tick += 1;
            if self.tick - self.last_info > 60 && matches!(self.view, View::Main) {
                self.reload(cx);
            }
            if self.playing {
                self.pos_ms += PLAY_STEP_MS;
                match self.frame.as_ref().map(|f| f.next_ms) {
                    Some(Some(n)) if self.pos_ms >= n => {
                        self.fetch(cx, n, RecSeek::AtOrAfter);
                    }
                    Some(Some(_)) => {}
                    _ => {
                        self.playing = false;
                        if let Some(f) = &self.frame {
                            self.pos_ms = f.ts_ms;
                        }
                        self.say("End of the recording.", true);
                    }
                }
            }
            return Out::Stay;
        }
        match std::mem::replace(&mut self.view, View::Main) {
            View::Settings(mut f) => {
                match f.handle_event(ev) {
                    FormEvent::Cancel => {}
                    FormEvent::Submit => self.submit_settings(cx, f),
                    _ => self.view = View::Settings(f),
                }
                return Out::Stay;
            }
            View::Confirm(ask) => {
                match (ev, &ask) {
                    (Event::Key(Key::Char('y')), Ask::Start(ch)) => {
                        let ch = ch.clone();
                        self.apply(cx, &ch, "Recording is on. The red REC mark shows while it records; press r to stop it.");
                    }
                    (Event::Key(Key::Char('y')), Ask::Export { from }) => {
                        let (from, to) = (*from, *from + self.clip_mins as u64 * 60_000);
                        match cx.cameras.rec_export(&self.id, from, to, true) {
                            Ok(p) => self.say(format!("Exported to {p}"), true),
                            Err(e) => self.say(e, false),
                        }
                    }
                    (Event::Key(Key::Char('m')), Ask::Export { .. }) => {
                        self.clip_mins = match self.clip_mins {
                            1 => 5,
                            5 => 15,
                            15 => 60,
                            _ => 1,
                        };
                        self.view = View::Confirm(ask);
                    }
                    (Event::Key(Key::Esc), _) | (Event::Key(Key::Char('n')), _) => {}
                    _ => self.view = View::Confirm(ask),
                }
                return Out::Stay;
            }
            View::Log(rows) => {
                if !matches!(ev, Event::Key(Key::Esc) | Event::Key(Key::Char('q')) | Event::Key(Key::Char('l')) | Event::Tab(T_LOG)) {
                    self.view = View::Log(rows);
                }
                return Out::Stay;
            }
            View::Main => {}
        }
        let owner = self.info.as_ref().is_some_and(|i| i.owner);
        match ev {
            Event::Key(Key::Esc) | Event::Key(Key::Char('q')) | Event::Tab(T_BACK) => return Out::Back,
            Event::Key(Key::Char(' ')) | Event::Key(Key::Enter) | Event::Tab(T_PLAY) if owner && self.frame.is_some() => self.playing = !self.playing,
            Event::Key(Key::Right) | Event::Key(Key::Char('.')) | Event::Tab(T_FWD1) if owner && self.frame.is_some() => {
                self.playing = false;
                let at = self.pos_ms;
                self.fetch(cx, at, RecSeek::After);
            }
            Event::Key(Key::Left) | Event::Key(Key::Char(',')) | Event::Tab(T_BACK1) if owner && self.frame.is_some() => {
                self.playing = false;
                let at = self.pos_ms;
                self.fetch(cx, at, RecSeek::Before);
            }
            Event::Key(Key::Char('[')) | Event::Tab(T_OLDER) if owner => {
                let i = self.day + 1;
                self.go_day(cx, i);
            }
            Event::Key(Key::Char(']')) | Event::Tab(T_NEWER) if owner && self.day > 0 => {
                let i = self.day - 1;
                self.go_day(cx, i);
            }
            Event::Key(Key::Home) if owner => {
                let i = self.day;
                self.go_day(cx, i);
            }
            Event::Tab(t) if owner && (T_SLOT0..T_SLOT0 + SLOTS).contains(t) => self.seek_slot(cx, *t - T_SLOT0),
            Event::Key(Key::Char('r')) | Event::Tab(T_RECORD) => self.toggle_record(cx),
            Event::Key(Key::Char('c')) | Event::Tab(T_SETTINGS) => {
                if owner {
                    self.view = View::Settings(Box::new(self.settings_form()));
                } else {
                    self.say("Only the person who added the camera can change its recording.", false);
                }
            }
            Event::Key(Key::Char('e')) | Event::Tab(T_EXPORT) => match (owner, self.frame.as_ref()) {
                (true, Some(f)) => self.view = View::Confirm(Ask::Export { from: f.ts_ms }),
                (true, None) => self.say("Pick a recorded picture first; the clip starts there.", false),
                _ => self.say("Only the person who added the camera can export its recordings.", false),
            },
            Event::Key(Key::Char('l')) | Event::Tab(T_LOG) => {
                if owner {
                    match cx.cameras.rec_audit(&self.id) {
                        Ok(r) => self.view = View::Log(r),
                        Err(e) => self.say(e, false),
                    }
                } else {
                    self.say("Only the person who added the camera can read its log.", false);
                }
            }
            _ => {}
        }
        Out::Stay
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &TileCx, tick: u64) {
        self.tick = self.tick.max(tick);
        match std::mem::replace(&mut self.view, View::Main) {
            View::Settings(f) => {
                f.render(g, area, s, ACCENT, iyakki_uikit::blink::blink(tick));
                self.view = View::Settings(f);
                return;
            }
            View::Log(rows) => {
                self.draw_log(g, area, s, &rows);
                self.view = View::Log(rows);
                return;
            }
            v @ View::Confirm(_) => {
                self.view = v;
                self.draw_main(g, area, s, cx);
                self.draw_confirm(g, area, s);
                return;
            }
            View::Main => {}
        }
        self.draw_main(g, area, s, cx);
    }

    fn rec_badge(&self, g: &mut Gfx, x: f32, y: f32, s: f32) -> f32 {
        let w = g.text_width(13.0 * s, "REC", Face::Bold) + 34.0 * s;
        g.rrect(x, y, w, 24.0 * s, 12.0 * s, [150, 20, 20, 235]);
        g.circle(x + 14.0 * s, y + 12.0 * s, 5.0 * s, rgb(255, 255, 255));
        g.text(x + 26.0 * s, y + 4.0 * s, 13.0 * s, rgb(255, 255, 255), "REC", Face::Bold);
        w
    }

    fn draw_main(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &TileCx) {
        let x = area.x + 20.0 * s;
        let w = area.w - 40.0 * s;
        let mut y = area.y + 8.0 * s;
        let mut bx = x;
        bx += g.button(bx, y, 13.0 * s, "Back (Esc)", MUTED, Event::Tab(T_BACK)) + 8.0 * s;
        let info = self.info.clone();
        let on = info.as_ref().is_some_and(|i| i.mode != RecMode::Off);
        bx += g.button(bx, y, 13.0 * s, if on { "Stop recording (r)" } else { "Start recording (r)" }, if on { RED } else { ACCENT }, Event::Tab(T_RECORD)) + 8.0 * s;
        bx += g.button(bx, y, 13.0 * s, "Settings (c)", MUTED, Event::Tab(T_SETTINGS)) + 8.0 * s;
        bx += g.button(bx, y, 13.0 * s, "Export (e)", MUTED, Event::Tab(T_EXPORT)) + 8.0 * s;
        bx += g.button(bx, y, 13.0 * s, "Log (l)", MUTED, Event::Tab(T_LOG)) + 12.0 * s;
        if info.as_ref().is_some_and(|i| i.recording) {
            self.rec_badge(g, bx, y, s);
        }
        y += 40.0 * s;
        let title = g.fit(16.0 * s, &format!("Recordings of {}", self.name), Face::Bold, w);
        g.text(x, y, 16.0 * s, TEXT, &title, Face::Bold);
        y += 24.0 * s;
        let Some(info) = info else {
            g.text(x, y, 13.0 * s, MUTED, "Recording is not available for this camera.", Face::Sans);
            return;
        };
        let state_col = if info.problem { ERR } else if info.recording { RED } else { MUTED };
        let mode = match info.mode {
            RecMode::Off => "off".to_string(),
            RecMode::Continuous => "continuous".to_string(),
            RecMode::Schedule => format!("schedule ({})", info.windows.join("; ")),
        };
        let line = format!("Recording is {mode} · {} · keeps {} and at most {}", info.state, days_text(info.max_days), mb_text(info.max_mb));
        for l in g.wrap(13.0 * s, &line, Face::Sans, w).into_iter().take(2) {
            g.text(x, y, 13.0 * s, state_col, &l, Face::Sans);
            y += 17.0 * s;
        }
        for l in g.wrap(12.0 * s, &info.key_note, Face::Sans, w).into_iter().take(2) {
            g.text(x, y, 12.0 * s, DIM, &l, Face::Sans);
            y += 16.0 * s;
        }
        y += 6.0 * s;
        let bottom = area.y + area.h - 34.0 * s;
        if !info.owner {
            g.text(x, y + 8.0 * s, 14.0 * s, MUTED, "Only the person who added this camera can open its recordings.", Face::Sans);
            self.draw_note(g, area, s);
            return;
        }
        if self.days.is_empty() {
            let msg = if on { "Nothing has been recorded yet. Pictures appear here a minute after recording starts." } else { "Nothing is recorded. Recording is off; press r to start it." };
            g.text(x, y + 8.0 * s, 14.0 * s, MUTED, msg, Face::Sans);
            self.draw_note(g, area, s);
            return;
        }
        // the picture
        let tl_h = 30.0 * s;
        let ctl_h = 34.0 * s;
        let pic = R::new(x, y, w, (bottom - y - tl_h - ctl_h - 12.0 * s).max(40.0 * s));
        iyakki_uikit::fx::well(g, pic, s);
        if let Some((_, pm)) = &self.pix {
            let (fw, fh) = (pm.width() as f32, pm.height() as f32);
            let k = ((pic.w - 4.0 * s) / fw).min((pic.h - 4.0 * s) / fh);
            let (dw, dh) = (fw * k, fh * k);
            let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
            g.pm.draw_pixmap(0, 0, pm.as_ref(), &paint, Transform::from_row(dw / fw, 0.0, 0.0, dh / fh, pic.x + (pic.w - dw) / 2.0, pic.y + (pic.h - dh) / 2.0), None);
        } else {
            let t = g.fit(13.0 * s, "No picture is shown. See the message below.", Face::Sans, pic.w - 24.0 * s);
            g.text(pic.x + 12.0 * s, pic.y + pic.h / 2.0 - 8.0 * s, 13.0 * s, MUTED, &t, Face::Sans);
        }
        if info.recording {
            self.rec_badge(g, pic.x + pic.w - 70.0 * s, pic.y + 8.0 * s, s);
        }
        if let Some(f) = &self.frame {
            let t = g.fit(12.0 * s, &f.label, Face::Bold, pic.w * 0.6);
            let tw = g.text_width(12.0 * s, &t, Face::Bold);
            g.rrect(pic.x + 8.0 * s, pic.y + pic.h - 28.0 * s, tw + 20.0 * s, 20.0 * s, 10.0 * s, [0, 0, 0, 170]);
            g.text(pic.x + 18.0 * s, pic.y + pic.h - 25.0 * s, 12.0 * s, TEXT, &t, Face::Bold);
        }
        y = pic.y + pic.h + 6.0 * s;
        // controls
        let mut cx_ = x;
        cx_ += g.button(cx_, y, 13.0 * s, "◀ Older day ([)", MUTED, Event::Tab(T_OLDER)) + 6.0 * s;
        cx_ += g.button(cx_, y, 13.0 * s, "Newer day (]) ▶", MUTED, Event::Tab(T_NEWER)) + 14.0 * s;
        cx_ += g.button(cx_, y, 13.0 * s, "◀ Step (,)", MUTED, Event::Tab(T_BACK1)) + 6.0 * s;
        cx_ += g.button(cx_, y, 13.0 * s, if self.playing { "Pause (Space)" } else { "Play (Space)" }, ACCENT, Event::Tab(T_PLAY)) + 6.0 * s;
        cx_ += g.button(cx_, y, 13.0 * s, "Step (.) ▶", MUTED, Event::Tab(T_FWD1)) + 14.0 * s;
        let label = self.days.get(self.day).map(|d| format!("{} · {} pictures · {} MB", d.label, d.frames, d.bytes / (1024 * 1024))).unwrap_or_default();
        let label = g.fit(12.0 * s, &label, Face::Sans, (x + w - cx_).max(10.0));
        g.text(cx_, y + 6.0 * s, 12.0 * s, MUTED, &label, Face::Sans);
        y += ctl_h;
        // the day's timeline: recorded stretches, click anywhere to jump
        if let Some(d) = self.days.get(self.day) {
            let bar = R::new(x, y, w, 16.0 * s);
            g.rrect(bar.x, bar.y, bar.w, bar.h, 4.0 * s, rgb(24, 30, 52));
            let px = |ms: u64| bar.x + (ms.saturating_sub(d.day_start_ms).min(DAY_MS) as f32 / DAY_MS as f32) * bar.w;
            for (a, b) in &d.spans {
                let (x0, x1) = (px(*a), px(*b).max(px(*a) + 2.0));
                g.rect(x0, bar.y + 1.0, (x1 - x0).min(bar.x + bar.w - x0), bar.h - 2.0, ACCENT);
            }
            // motion events of the day, as amber ticks above the bar
            for f in crate::motion::day_marks(&self.motion, d.day_start_ms) {
                g.rect(bar.x + f * bar.w - 1.0 * s, bar.y - 7.0 * s, 2.0 * s, 7.0 * s, rgb(255, 196, 90));
            }
            for i in 0..SLOTS {
                let sx = bar.x + bar.w * i as f32 / SLOTS as f32;
                g.hit(R::new(sx, bar.y, bar.w / SLOTS as f32 + 0.5, bar.h), Event::Tab(T_SLOT0 + i), "Jump here");
            }
            if self.frame.is_some() {
                let hx = px(self.pos_ms);
                g.rect(hx - 1.0, bar.y - 3.0 * s, 2.0 * s, bar.h + 6.0 * s, rgb(255, 255, 255));
            }
            for h in (0..=24).step_by(3) {
                let tx = bar.x + bar.w * h as f32 / 24.0;
                g.text((tx - 8.0 * s).max(bar.x), bar.y + bar.h + 1.0 * s, 10.0 * s, DIM, &format!("{h:02}"), Face::Sans);
            }
        }
        self.draw_note(g, area, s);
    }

    fn draw_note(&self, g: &mut Gfx, area: R, s: f32) {
        if let Some((t, ok, at)) = &self.note {
            if self.tick.saturating_sub(*at) < 300 {
                let t = g.fit(13.0 * s, t, Face::Sans, area.w - 40.0 * s);
                g.text(area.x + 20.0 * s, area.y + area.h - 26.0 * s, 13.0 * s, if *ok { OK } else { ERR }, &t, Face::Sans);
            }
        }
    }

    fn draw_confirm(&mut self, g: &mut Gfx, area: R, s: f32) {
        let View::Confirm(ask) = &self.view else { return };
        let info = self.info.clone();
        let (title, lines, yes, danger): (String, Vec<String>, &str, bool) = match ask {
            Ask::Start(ch) => {
                let days = ch.max_days.or(info.as_ref().map(|i| i.max_days)).unwrap_or(14);
                let mb = ch.max_mb.or(info.as_ref().map(|i| i.max_mb)).unwrap_or(20_000);
                let when = match ch.mode {
                    Some(RecMode::Schedule) => format!("only during {}", ch.windows.clone().unwrap_or_default().join("; ")),
                    _ => "all the time".to_string(),
                };
                (
                    format!("Start recording “{}” {when}?", self.name),
                    vec![
                        "It stores what the camera sees on this computer, encrypted with a key kept in your vault.".into(),
                        "A red REC mark shows on the camera and a red sign on every screen while it records. One click stops it.".into(),
                        format!("Kept {} and at most {}; recording stops if the disk gets nearly full.", days_text(days), mb_text(mb)),
                        info.as_ref().map(|i| i.key_note.clone()).unwrap_or_default(),
                    ],
                    "Start recording",
                    true,
                )
            }
            Ask::Export { from } => {
                let at = self.frame.as_ref().filter(|f| f.ts_ms == *from).map(|f| f.label.clone()).unwrap_or_default();
                (
                    format!("Export {} minute{} starting {at}?", self.clip_mins, if self.clip_mins == 1 { "" } else { "s" }),
                    vec!["The pictures are copied DECRYPTED into your Pictures folder (H.264 recordings: one video file in Videos).".into(), "Anyone who can open that folder can see them. The export is written to the camera's log.".into(), "Press m to change the length (1, 5, 15 or 60 minutes).".into()],
                    "Export",
                    true,
                )
            }
        };
        let p = iyakki_uikit::fx::modal(g, area, s, 640.0 * s, 300.0 * s, ACCENT, danger);
        let t = g.fit(18.0 * s, &title, Face::Bold, p.w - 48.0 * s);
        g.text(p.x + 24.0 * s, p.y + 24.0 * s, 18.0 * s, TEXT, &t, Face::Bold);
        let mut y = p.y + 62.0 * s;
        for l in lines.iter().filter(|l| !l.is_empty()) {
            for w in g.wrap(13.0 * s, l, Face::Sans, p.w - 48.0 * s) {
                g.text(p.x + 24.0 * s, y, 13.0 * s, MUTED, &w, Face::Sans);
                y += 17.0 * s;
            }
            y += 4.0 * s;
        }
        g.button(p.x + 24.0 * s, p.y + p.h - 54.0 * s, 14.0 * s, &format!("{yes} (y)"), ERR, Event::Key(Key::Char('y')));
        g.button(p.x + 24.0 * s + 220.0 * s, p.y + p.h - 54.0 * s, 14.0 * s, "Cancel (Esc)", MUTED, Event::Key(Key::Esc));
    }

    fn draw_log(&self, g: &mut Gfx, area: R, s: f32, rows: &[AuditRow]) {
        let x = area.x + 20.0 * s;
        let w = area.w - 40.0 * s;
        let mut y = area.y + 8.0 * s;
        g.button(x, y, 13.0 * s, "Back (Esc)", MUTED, Event::Tab(T_LOG));
        y += 40.0 * s;
        g.text(x, y, 16.0 * s, TEXT, &format!("Who looked at {}", self.name), Face::Bold);
        y += 22.0 * s;
        g.text(x, y, 12.0 * s, DIM, "Live views, snapshots, playback, exports, recording on/off and deletions. No pictures, no passwords.", Face::Sans);
        y += 24.0 * s;
        let max = ((area.y + area.h - y - 20.0 * s) / (18.0 * s)).max(1.0) as usize;
        if rows.is_empty() {
            g.text(x, y, 13.0 * s, MUTED, "Nothing has happened yet.", Face::Sans);
        }
        for r in rows.iter().take(max) {
            let line = format!("{}   {}   {}", r.time, r.who, r.what);
            let t = g.fit(13.0 * s, &line, Face::Sans, w);
            g.text(x, y, 13.0 * s, if r.ok { TEXT } else { ERR }, &t, Face::Sans);
            y += 18.0 * s;
        }
    }
}
