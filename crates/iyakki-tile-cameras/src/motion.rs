//! The motion page of the CAMERAS tile (F-CAM-04): whether a camera is watched for motion, zones and privacy masks drawn with
//! clicks on the picture, alert settings and the list of events.
//!
//! * Motion watching is off unless the camera's owner turned it on; turning it on always shows what is analysed, what is stored and
//!   which sign shows. Turning it off is one key and asks nothing.
//! * Draw a shape: press `z` (zone: watch only here), `x` (exclude zone: never watch here) or `m` (privacy mask: blacked out before
//!   anything looks at the picture), click the corners on the picture, then Enter or a click on the first corner closes it and asks
//!   for a name. Backspace takes the last corner back, Esc cancels. The source checks the shape (3 or more corners, inside the picture,
//!   not crossing itself) and says why when it refuses.
//! * The tile never analyses a picture: it shows what the source says and sends the person's choices.

use iyakki_tile_api::cameras::{MotionChange, MotionInfo, MotionMode, MotionRow, ShapeInfo};
use iyakki_tile_api::TileCx;
use iyakki_uikit::gfx::{rgb, Face, Gfx};
use iyakki_uikit::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};
use iyakki_uikit::widgets::{render_confirm, Form, FormEvent};
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

use crate::{ACCENT, RED};

/// Click area of the picture.
pub const AREA_PIC: usize = 400;
const T_BACK: usize = 410;
const T_MODE: usize = 411;
const T_SENS_UP: usize = 412;
const T_SENS_DN: usize = 413;
const T_ZONE: usize = 414;
const T_EXCL: usize = 415;
const T_MASK: usize = 416;
const T_DEL: usize = 417;
const T_ALERTS: usize = 418;
const T_THUMB: usize = 419;
const T_STORE: usize = 420;
const T_EVENTS: usize = 421;
const T_SETTINGS: usize = 422;
const T_FINISH: usize = 423;
const T_UNDO: usize = 424;
const T_CANCEL: usize = 425;
const T_SEL0: usize = 440;
const AMBER: [u8; 4] = rgb(255, 196, 90);
const GREEN: [u8; 4] = rgb(90, 220, 140);
/// A click this close (in pixels, at scale 1) to the first corner closes the shape.
const CLOSE_PX: f32 = 14.0;
const DAY_MS: u64 = 86_400_000;

pub enum Out {
    Stay,
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Draw {
    Zone,
    Exclude,
    Mask,
}

enum Page {
    Main,
    Name(Box<Form>, Draw, Vec<(f32, f32)>),
    Confirm(MotionMode),
    Settings(Box<Form>),
    Events,
}

pub struct MotionView {
    pub id: String,
    pub name: String,
    info: Option<MotionInfo>,
    page: Page,
    drawing: Option<(Draw, Vec<(f32, f32)>)>,
    /// The selected shape: zones first, then masks.
    sel: usize,
    pix: Option<(u64, Pixmap)>,
    /// Where the picture was drawn on the last frame (the click area), for converting clicks.
    pic: Option<R>,
    events: Vec<MotionRow>,
    ev_sel: usize,
    thumb: Option<(u64, Pixmap)>,
    note: Option<(String, bool, u64)>,
    tick: u64,
    last_info: u64,
}

fn mode_word(m: MotionMode) -> &'static str {
    match m {
        MotionMode::Off => "off",
        MotionMode::Watched => "while watched or recorded",
        MotionMode::Always => "all the time",
    }
}

impl MotionView {
    pub fn new(cx: &TileCx, id: &str, name: &str) -> MotionView {
        let mut v = MotionView { id: id.into(), name: name.into(), info: None, page: Page::Main, drawing: None, sel: 0, pix: None, pic: None, events: vec![], ev_sel: 0, thumb: None, note: None, tick: 0, last_info: 0 };
        v.reload(cx);
        v
    }

    pub fn info(&self) -> Option<&MotionInfo> {
        self.info.as_ref()
    }
    pub fn drawing(&self) -> Option<(Draw, usize)> {
        self.drawing.as_ref().map(|(k, p)| (*k, p.len()))
    }
    pub fn naming(&self) -> bool {
        matches!(self.page, Page::Name(..))
    }
    pub fn confirming(&self) -> bool {
        matches!(self.page, Page::Confirm(_))
    }
    pub fn in_settings(&self) -> bool {
        matches!(self.page, Page::Settings(_))
    }
    pub fn showing_events(&self) -> bool {
        matches!(self.page, Page::Events)
    }
    pub fn events(&self) -> &[MotionRow] {
        &self.events
    }
    pub fn selected(&self) -> usize {
        self.sel
    }
    pub fn note(&self) -> Option<(&str, bool)> {
        self.note.as_ref().map(|(t, ok, _)| (t.as_str(), *ok))
    }
    /// Size in pixels of the picture on the last frame (tests turn fractions into clicks with it).
    pub fn picture_size(&self) -> Option<(f32, f32)> {
        self.pic.map(|r| (r.w, r.h))
    }
    pub fn hint(&self) -> String {
        match &self.page {
            Page::Main if self.drawing.is_some() => "click the corners · Enter close · Backspace undo · Esc cancel".into(),
            Page::Main => "w watch · z zone · x exclude · m mask · d delete · ↑↓ pick · a alerts · s masks in recordings · e events · c settings · Esc back".into(),
            Page::Name(..) | Page::Settings(_) => "Tab next field · Enter save · Esc cancel".into(),
            Page::Confirm(_) => "y yes · Esc cancel".into(),
            Page::Events => "↑↓ pick · Esc back".into(),
        }
    }

    fn say(&mut self, t: impl Into<String>, ok: bool) {
        self.note = Some((t.into(), ok, self.tick));
    }

    fn reload(&mut self, cx: &TileCx) {
        self.last_info = self.tick;
        self.info = cx.cameras.motion_info(&self.id);
        let n = self.info.as_ref().map_or(0, |i| i.zones.len() + i.masks.len());
        self.sel = self.sel.min(n.saturating_sub(1));
    }

    fn change(&mut self, cx: &TileCx, ch: MotionChange, done: &str) {
        match cx.cameras.motion_set(&self.id, &ch) {
            Ok(()) => {
                self.say(done, true);
                self.reload(cx);
            }
            Err(e) => self.say(e, false),
        }
    }

    fn load_events(&mut self, cx: &TileCx) {
        // newest first, at most 200
        match cx.cameras.motion_events(&self.id, 0, u64::MAX, 200) {
            Ok(v) => {
                self.events = v;
                self.ev_sel = 0;
                self.thumb = None;
            }
            Err(e) => {
                self.events.clear();
                self.say(e, false);
            }
        }
    }

    fn load_thumb(&mut self, cx: &TileCx) {
        let Some(e) = self.events.get(self.ev_sel) else { return };
        if self.thumb.as_ref().is_some_and(|(id, _)| *id == e.id) || !e.has_thumb {
            return;
        }
        let id = e.id;
        self.thumb = None;
        if let Ok(Some(f)) = cx.cameras.motion_thumb(&self.id, id) {
            if let Some(pm) = tiny_skia::IntSize::from_wh(f.width, f.height).and_then(|sz| Pixmap::from_vec((*f.rgba).clone(), sz)) {
                self.thumb = Some((id, pm));
            }
        }
    }

    fn start_draw(&mut self, kind: Draw) {
        match self.info.as_ref() {
            Some(i) if i.owner => {
                self.drawing = Some((kind, vec![]));
                self.say(
                    match kind {
                        Draw::Zone => "Click the corners of the zone where motion should count.",
                        Draw::Exclude => "Click the corners of the area where motion should be ignored.",
                        Draw::Mask => "Click the corners of the area to black out (a privacy mask).",
                    },
                    true,
                );
            }
            Some(_) => self.say("Only the person who added this camera can draw zones and masks.", false),
            None => self.say("Motion watching is not available for this camera.", false),
        }
    }

    fn finish_shape(&mut self) {
        let Some((kind, pts)) = self.drawing.take() else { return };
        if pts.len() < 3 {
            self.say("A shape needs at least 3 corners.", false);
            self.drawing = Some((kind, pts));
            return;
        }
        let title = match kind {
            Draw::Zone => "Name this zone",
            Draw::Exclude => "Name this exclude zone",
            Draw::Mask => "Name this privacy mask",
        };
        let mut f = Form::new(title).text("Name", "", "for example Driveway");
        if kind != Draw::Mask {
            f = f.text("Sensitivity", &self.info.as_ref().map_or(5, |i| i.sensitivity).to_string(), "1 (least) to 10 (most)");
        }
        f.focus = 0;
        self.page = Page::Name(Box::new(f), kind, pts);
    }

    fn submit_shape(&mut self, cx: &TileCx, mut f: Box<Form>, kind: Draw, pts: Vec<(f32, f32)>) {
        let sens: u8 = if kind == Draw::Mask { 5 } else { f.value(1).trim().parse().unwrap_or(0) };
        if kind != Draw::Mask && !(1..=10).contains(&sens) {
            f.error = Some("Sensitivity is a number from 1 to 10.".into());
            self.page = Page::Name(f, kind, pts);
            return;
        }
        let shape = ShapeInfo { name: f.value(0).trim().to_string(), include: kind == Draw::Zone, sensitivity: sens, points: pts.clone() };
        match cx.cameras.motion_shape_set(&self.id, kind == Draw::Mask, &shape) {
            Ok(()) => {
                self.say(format!("Saved “{}”.", shape.name), true);
                self.reload(cx);
            }
            Err(e) => {
                f.error = Some(e);
                self.page = Page::Name(f, kind, pts);
            }
        }
    }

    fn shapes(&self) -> Vec<(bool, ShapeInfo)> {
        self.info.as_ref().map_or(vec![], |i| i.zones.iter().map(|z| (false, z.clone())).chain(i.masks.iter().map(|m| (true, m.clone()))).collect())
    }

    fn settings_form(&self) -> Form {
        let i = self.info.clone();
        let mut f = Form::new("Alert settings")
            .with_note("Alerts go to your inbox. At most this many an hour per camera; bursts are grouped into one notice; nothing is sent in quiet hours (one summary follows). Leave quiet hours empty for none.")
            .text("Alerts per hour", &i.as_ref().map_or(4, |i| i.alerts_max_per_hour).to_string(), "1 to 30")
            .text("Quiet hours", i.as_ref().map_or("", |i| i.quiet.as_str()), "for example 22:00-06:30");
        f.focus = 0;
        f
    }

    fn submit_settings(&mut self, cx: &TileCx, mut f: Box<Form>) {
        let n: u32 = f.value(0).trim().parse().unwrap_or(0);
        if !(1..=30).contains(&n) {
            f.error = Some("Alerts per hour is a number from 1 to 30.".into());
            self.page = Page::Settings(f);
            return;
        }
        let ch = MotionChange { alerts_max_per_hour: Some(n), quiet: Some(f.value(1).trim().to_string()), ..Default::default() };
        match cx.cameras.motion_set(&self.id, &ch) {
            Ok(()) => {
                self.say("Alert settings saved.", true);
                self.reload(cx);
            }
            Err(e) => {
                f.error = Some(e);
                self.page = Page::Settings(f);
            }
        }
    }

    fn confirm_text(&self, m: MotionMode) -> String {
        let when = if m == MotionMode::Always {
            "ALL THE TIME. Its stream is kept running; a red sign shows on every screen while it does, and one click on the sign stops it."
        } else {
            "only while its picture is on a screen or it is being recorded (both already show a sign)."
        };
        format!(
            "Start watching “{}” for motion {when} The pictures are analysed on this computer only; nothing leaves it. Each event (time, zone, strength and one small thumbnail) is stored encrypted with the camera's key in your vault, for as long as its recordings are kept. This computer has no person detector: events say motion, not person.",
            self.name
        )
    }

    pub fn handle(&mut self, ev: &Event, cx: &TileCx) -> Out {
        if matches!(ev, Event::Tick) {
            self.tick += 1;
            if self.tick - self.last_info > 60 && matches!(self.page, Page::Main) {
                self.reload(cx);
            }
            return Out::Stay;
        }
        match std::mem::replace(&mut self.page, Page::Main) {
            Page::Name(mut f, kind, pts) => {
                match f.handle_event(ev) {
                    FormEvent::Cancel => self.drawing = Some((kind, pts)),
                    FormEvent::Submit => self.submit_shape(cx, f, kind, pts),
                    _ => self.page = Page::Name(f, kind, pts),
                }
                return Out::Stay;
            }
            Page::Settings(mut f) => {
                match f.handle_event(ev) {
                    FormEvent::Cancel => {}
                    FormEvent::Submit => self.submit_settings(cx, f),
                    _ => self.page = Page::Settings(f),
                }
                return Out::Stay;
            }
            Page::Confirm(m) => {
                match ev {
                    Event::Key(Key::Char('y')) => self.change(cx, MotionChange { mode: Some(m), ..Default::default() }, "Motion watching is on. Press w to change or switch it off."),
                    Event::Key(Key::Esc) | Event::Key(Key::Char('n')) => {}
                    _ => self.page = Page::Confirm(m),
                }
                return Out::Stay;
            }
            Page::Events => {
                match ev {
                    Event::Key(Key::Esc) | Event::Key(Key::Char('q')) | Event::Tab(T_BACK) => {}
                    Event::Key(Key::Up) => {
                        self.ev_sel = self.ev_sel.saturating_sub(1);
                        self.page = Page::Events;
                    }
                    Event::Key(Key::Down) => {
                        self.ev_sel = (self.ev_sel + 1).min(self.events.len().saturating_sub(1));
                        self.page = Page::Events;
                    }
                    Event::Row(i) => {
                        self.ev_sel = (*i).min(self.events.len().saturating_sub(1));
                        self.page = Page::Events;
                    }
                    _ => self.page = Page::Events,
                }
                return Out::Stay;
            }
            Page::Main => {}
        }
        // the main page, drawing or not
        if self.drawing.is_some() {
            match ev {
                Event::AreaAt(AREA_PIC, x, y) => {
                    if let Some(r) = self.pic {
                        let (fx, fy) = ((x / r.w).clamp(0.0, 1.0), (y / r.h).clamp(0.0, 1.0));
                        let close = self.drawing.as_ref().and_then(|(_, p)| p.first().copied()).is_some_and(|(x0, y0)| (((fx - x0) * r.w).powi(2) + ((fy - y0) * r.h).powi(2)).sqrt() <= CLOSE_PX) && self.drawing.as_ref().is_some_and(|(_, p)| p.len() >= 3);
                        if close {
                            self.finish_shape();
                        } else if let Some((_, p)) = self.drawing.as_mut() {
                            if p.len() < 32 {
                                p.push((fx, fy));
                            } else {
                                self.say("A shape has at most 32 corners; press Enter to close it.", false);
                            }
                        }
                    }
                }
                Event::Key(Key::Enter) | Event::Tab(T_FINISH) => self.finish_shape(),
                Event::Key(Key::Backspace) | Event::Tab(T_UNDO) => {
                    if let Some((_, p)) = self.drawing.as_mut() {
                        p.pop();
                    }
                }
                Event::Key(Key::Esc) | Event::Tab(T_CANCEL) => {
                    self.drawing = None;
                    self.say("Drawing cancelled; nothing was changed.", true);
                }
                _ => {}
            }
            return Out::Stay;
        }
        let owner = self.info.as_ref().is_some_and(|i| i.owner);
        let cur = self.info.clone();
        match ev {
            Event::Key(Key::Esc) | Event::Key(Key::Char('q')) | Event::Tab(T_BACK) => return Out::Back,
            Event::Key(Key::Char('w')) | Event::Tab(T_MODE) => match cur {
                Some(i) if i.owner => match i.mode {
                    MotionMode::Off => self.page = Page::Confirm(MotionMode::Watched),
                    MotionMode::Watched => self.page = Page::Confirm(MotionMode::Always),
                    MotionMode::Always => self.change(cx, MotionChange { mode: Some(MotionMode::Off), ..Default::default() }, "Motion watching is off."),
                },
                Some(i) if i.mode != MotionMode::Off => self.change(cx, MotionChange { mode: Some(MotionMode::Off), ..Default::default() }, "Motion watching is off."),
                Some(_) => self.say("Only the person who added this camera can turn motion watching on.", false),
                None => self.say("Motion watching is not available for this camera.", false),
            },
            Event::Key(Key::Char('+')) | Event::Tab(T_SENS_UP) | Event::Key(Key::Char('-')) | Event::Tab(T_SENS_DN) if owner => {
                let up = matches!(ev, Event::Key(Key::Char('+')) | Event::Tab(T_SENS_UP));
                let s = cur.map_or(5, |i| i.sensitivity);
                let n = if up { (s + 1).min(10) } else { s.saturating_sub(1).max(1) };
                self.change(cx, MotionChange { sensitivity: Some(n), ..Default::default() }, &format!("Sensitivity {n} of 10."));
            }
            Event::Key(Key::Char('z')) | Event::Tab(T_ZONE) => self.start_draw(Draw::Zone),
            Event::Key(Key::Char('x')) | Event::Tab(T_EXCL) => self.start_draw(Draw::Exclude),
            Event::Key(Key::Char('m')) | Event::Tab(T_MASK) => self.start_draw(Draw::Mask),
            Event::Key(Key::Up) => self.sel = self.sel.saturating_sub(1),
            Event::Key(Key::Down) => self.sel = (self.sel + 1).min(self.shapes().len().saturating_sub(1)),
            Event::Tab(t) if (T_SEL0..T_SEL0 + 40).contains(t) => self.sel = (*t - T_SEL0).min(self.shapes().len().saturating_sub(1)),
            Event::Key(Key::Char('d')) | Event::Key(Key::Delete) | Event::Tab(T_DEL) if owner => match self.shapes().get(self.sel).cloned() {
                Some((mask, s)) => match cx.cameras.motion_shape_remove(&self.id, mask, &s.name) {
                    Ok(()) => {
                        self.say(format!("Removed “{}”.", s.name), true);
                        self.reload(cx);
                    }
                    Err(e) => self.say(e, false),
                },
                None => self.say("There is no zone or mask to remove.", false),
            },
            Event::Key(Key::Char('a')) | Event::Tab(T_ALERTS) if owner => {
                let on = !cur.is_some_and(|i| i.alerts_on);
                self.change(cx, MotionChange { alerts_on: Some(on), ..Default::default() }, if on { "Alerts go to your inbox (rate limited, bursts grouped)." } else { "Alerts are off." });
            }
            Event::Key(Key::Char('t')) | Event::Tab(T_THUMB) if owner => {
                let on = !cur.is_some_and(|i| i.alerts_thumbnail);
                self.change(cx, MotionChange { alerts_thumbnail: Some(on), ..Default::default() }, if on { "A small picture may go with an alert." } else { "Alerts carry no picture." });
            }
            Event::Key(Key::Char('s')) | Event::Tab(T_STORE) if owner => {
                let on = !cur.is_some_and(|i| i.mask_storage);
                self.change(cx, MotionChange { mask_storage: Some(on), ..Default::default() }, if on { "Privacy masks are now black in recordings and snapshots too." } else { "Privacy masks apply to motion detection only; recordings and snapshots show everything." });
            }
            Event::Key(Key::Char('e')) | Event::Tab(T_EVENTS) => {
                if owner {
                    self.load_events(cx);
                    self.page = Page::Events;
                } else {
                    self.say("Only the person who added this camera can see its motion events.", false);
                }
            }
            Event::Key(Key::Char('c')) | Event::Tab(T_SETTINGS) if owner => self.page = Page::Settings(Box::new(self.settings_form())),
            _ => {}
        }
        Out::Stay
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &TileCx, tick: u64) {
        self.tick = self.tick.max(tick);
        match std::mem::replace(&mut self.page, Page::Main) {
            Page::Name(f, k, p) => {
                f.render(g, area, s, ACCENT, iyakki_uikit::blink::blink(tick));
                self.page = Page::Name(f, k, p);
                return;
            }
            Page::Settings(f) => {
                f.render(g, area, s, ACCENT, iyakki_uikit::blink::blink(tick));
                self.page = Page::Settings(f);
                return;
            }
            Page::Events => {
                self.draw_events(g, area, s, cx);
                self.page = Page::Events;
                return;
            }
            Page::Confirm(m) => {
                self.page = Page::Confirm(m);
                self.draw_main(g, area, s, cx);
                render_confirm(g, area, s, &self.confirm_text(m), "Start watching");
                return;
            }
            Page::Main => {}
        }
        self.draw_main(g, area, s, cx);
    }

    fn pill(g: &mut Gfx, x: f32, y: f32, s: f32, text: &str, bg: [u8; 4]) -> f32 {
        let w = g.text_width(12.0 * s, text, Face::Bold) + 24.0 * s;
        g.rrect(x, y, w, 22.0 * s, 11.0 * s, bg);
        g.text(x + 12.0 * s, y + 4.0 * s, 12.0 * s, rgb(255, 255, 255), text, Face::Bold);
        w
    }

    fn draw_main(&mut self, g: &mut Gfx, area: R, s: f32, cx: &TileCx) {
        let x = area.x + 20.0 * s;
        let w = area.w - 40.0 * s;
        let mut y = area.y + 8.0 * s;
        let info = self.info.clone();
        let mut bx = x;
        bx += g.button(bx, y, 13.0 * s, "Back (Esc)", MUTED, Event::Tab(T_BACK)) + 8.0 * s;
        if self.drawing.is_some() {
            bx += g.button(bx, y, 13.0 * s, "Close shape (Enter)", ACCENT, Event::Tab(T_FINISH)) + 6.0 * s;
            bx += g.button(bx, y, 13.0 * s, "Undo corner (Backspace)", MUTED, Event::Tab(T_UNDO)) + 6.0 * s;
            g.button(bx, y, 13.0 * s, "Cancel (Esc)", MUTED, Event::Tab(T_CANCEL));
        } else {
            let label = match info.as_ref().map(|i| i.mode) {
                Some(MotionMode::Off) | None => "Watch for motion (w)",
                Some(_) => "Stop watching (w)",
            };
            bx += g.button(bx, y, 13.0 * s, label, if info.as_ref().is_some_and(|i| i.mode != MotionMode::Off) { RED } else { ACCENT }, Event::Tab(T_MODE)) + 8.0 * s;
            bx += g.button(bx, y, 13.0 * s, "Zone (z)", MUTED, Event::Tab(T_ZONE)) + 6.0 * s;
            bx += g.button(bx, y, 13.0 * s, "Exclude (x)", MUTED, Event::Tab(T_EXCL)) + 6.0 * s;
            bx += g.button(bx, y, 13.0 * s, "Privacy mask (m)", MUTED, Event::Tab(T_MASK)) + 6.0 * s;
            bx += g.button(bx, y, 13.0 * s, "Events (e)", MUTED, Event::Tab(T_EVENTS)) + 6.0 * s;
            g.button(bx, y, 13.0 * s, "Alert settings (c)", MUTED, Event::Tab(T_SETTINGS));
        }
        y += 40.0 * s;
        let title = g.fit(16.0 * s, &format!("Motion watching of {}", self.name), Face::Bold, w);
        g.text(x, y, 16.0 * s, TEXT, &title, Face::Bold);
        y += 24.0 * s;
        let Some(info) = info else {
            g.text(x, y, 13.0 * s, MUTED, "Motion watching is not available for this camera.", Face::Sans);
            return;
        };
        let col = if info.mode == MotionMode::Off { MUTED } else if info.analysing { OK } else { AMBER };
        for l in g.wrap(13.0 * s, &info.state, Face::Sans, w).into_iter().take(2) {
            g.text(x, y, 13.0 * s, col, &l, Face::Sans);
            y += 17.0 * s;
        }
        let line = format!("Mode: {} · sensitivity {} of 10 (+ / -) · events kept {} days · {}", mode_word(info.mode), info.sensitivity, info.keep_days, info.person_note);
        for l in g.wrap(12.0 * s, &line, Face::Sans, w).into_iter().take(2) {
            g.text(x, y, 12.0 * s, DIM, &l, Face::Sans);
            y += 16.0 * s;
        }
        y += 6.0 * s;
        let bottom = area.y + area.h - 34.0 * s;
        let side_w = (w * 0.30).clamp(200.0 * s, 330.0 * s);
        let pic = R::new(x, y, w - side_w - 12.0 * s, (bottom - y).max(60.0 * s));
        iyakki_uikit::fx::well(g, pic, s);
        let frame = cx.cameras.view(&self.id, false);
        if let Some(f) = frame.as_ref() {
            if self.pix.as_ref().is_none_or(|(seq, _)| *seq != f.seq) {
                if let Some(pm) = tiny_skia::IntSize::from_wh(f.width, f.height).and_then(|sz| Pixmap::from_vec((*f.rgba).clone(), sz)) {
                    self.pix = Some((f.seq, pm));
                }
            }
        }
        // the picture, centred in its well; the click area is exactly the drawn picture
        let (iw, ih) = self.pix.as_ref().map_or((16.0, 9.0), |(_, p)| (p.width() as f32, p.height() as f32));
        let k = ((pic.w - 4.0 * s) / iw).min((pic.h - 4.0 * s) / ih);
        let (dw, dh) = (iw * k, ih * k);
        let (dx, dy) = (pic.x + (pic.w - dw) / 2.0, pic.y + (pic.h - dh) / 2.0);
        let img = R::new(dx, dy, dw, dh);
        self.pic = Some(img);
        if let Some((_, pm)) = &self.pix {
            let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
            g.pm.draw_pixmap(0, 0, pm.as_ref(), &paint, Transform::from_row(dw / iw, 0.0, 0.0, dh / ih, dx, dy), None);
        } else {
            g.text(pic.x + 12.0 * s, pic.y + pic.h / 2.0 - 8.0 * s, 13.0 * s, MUTED, "Waiting for a picture from the camera.", Face::Sans);
        }
        let to_px = |p: &(f32, f32)| (dx + p.0 * dw, dy + p.1 * dh);
        let shapes = self.shapes();
        for (i, (mask, sh)) in shapes.iter().enumerate() {
            let pts: Vec<(f32, f32)> = sh.points.iter().map(to_px).collect();
            let (fill, edge) = if *mask { ([0, 0, 0, 200], rgb(255, 255, 255)) } else if sh.include { ([90, 220, 140, 50], GREEN) } else { ([255, 196, 90, 60], AMBER) };
            g.polygon(&pts, fill);
            let wd = if i == self.sel { 3.0 } else { 1.5 };
            for j in 0..pts.len() {
                let (a, b) = (pts[j], pts[(j + 1) % pts.len()]);
                g.line(a.0, a.1, b.0, b.1, wd, edge);
            }
            if let Some(p) = pts.first() {
                let t = g.fit(11.0 * s, &sh.name, Face::Bold, 140.0 * s);
                g.text(p.0 + 4.0 * s, p.1 + 3.0 * s, 11.0 * s, edge, &t, Face::Bold);
            }
        }
        if let Some((kind, pts)) = &self.drawing {
            let c = match kind {
                Draw::Zone => GREEN,
                Draw::Exclude => AMBER,
                Draw::Mask => rgb(255, 255, 255),
            };
            let px: Vec<(f32, f32)> = pts.iter().map(to_px).collect();
            for j in 1..px.len() {
                g.line(px[j - 1].0, px[j - 1].1, px[j].0, px[j].1, 2.0, c);
            }
            for (j, p) in px.iter().enumerate() {
                g.circle(p.0, p.1, if j == 0 { 6.0 * s } else { 4.0 * s }, c);
            }
        }
        g.hit(img, Event::Area(AREA_PIC), if self.drawing.is_some() { "Add a corner" } else { "Picture" });
        // the marks on the picture: what is watching it, always visible
        let mut mx = pic.x + 8.0 * s;
        if info.mode != MotionMode::Off {
            mx += Self::pill(g, mx, pic.y + 8.0 * s, s, if info.analysing { "MOTION" } else { "MOTION (waiting)" }, [20, 80, 150, 235]) + 6.0 * s;
        }
        let _ = mx;
        // the side column: shapes and switches
        let mut sy = pic.y;
        let sx = pic.x + pic.w + 12.0 * s;
        g.text(sx, sy, 13.0 * s, TEXT, "Zones and masks", Face::Bold);
        sy += 20.0 * s;
        if shapes.is_empty() {
            for l in g.wrap(12.0 * s, "None yet: the whole picture is watched. Press z, x or m and click the corners on the picture.", Face::Sans, side_w).into_iter().take(4) {
                g.text(sx, sy, 12.0 * s, MUTED, &l, Face::Sans);
                sy += 16.0 * s;
            }
        }
        for (i, (mask, sh)) in shapes.iter().enumerate().take(12) {
            let kind = if *mask { "mask" } else if sh.include { "zone" } else { "exclude" };
            let line = g.fit(12.0 * s, &format!("{} · {kind}{}", sh.name, if *mask { String::new() } else { format!(" · sens {}", sh.sensitivity) }), if i == self.sel { Face::Bold } else { Face::Sans }, side_w - 8.0 * s);
            if i == self.sel {
                g.rrect(sx - 4.0 * s, sy - 2.0 * s, side_w, 20.0 * s, 6.0 * s, [255, 255, 255, 30]);
            }
            g.text(sx, sy, 12.0 * s, if i == self.sel { TEXT } else { MUTED }, &line, if i == self.sel { Face::Bold } else { Face::Sans });
            g.hit(R::new(sx - 4.0 * s, sy - 2.0 * s, side_w, 20.0 * s), Event::Tab(T_SEL0 + i), &sh.name);
            sy += 22.0 * s;
        }
        if !shapes.is_empty() && info.owner {
            sy += 4.0 * s;
            g.button(sx, sy, 12.0 * s, "Remove selected (d)", MUTED, Event::Tab(T_DEL));
            sy += 30.0 * s;
        }
        sy += 6.0 * s;
        let on_off = |b: bool| if b { "on" } else { "off" };
        let rows = [
            (format!("Alerts: {}", on_off(info.alerts_on)), "a", T_ALERTS),
            (format!("Picture with alerts: {}", if info.alerts_thumbnail { "allowed" } else { "not allowed" }), "t", T_THUMB),
            (format!("Masks also in recordings and snapshots: {}", on_off(info.mask_storage)), "s", T_STORE),
        ];
        for (label, key, tab) in rows {
            if info.owner {
                let t = g.fit(12.0 * s, &format!("{label} ({key})"), Face::Sans, side_w);
                g.text(sx, sy, 12.0 * s, TEXT, &t, Face::Sans);
                g.hit(R::new(sx, sy - 2.0 * s, side_w, 20.0 * s), Event::Tab(tab), &label);
            } else {
                let t = g.fit(12.0 * s, &label, Face::Sans, side_w);
                g.text(sx, sy, 12.0 * s, DIM, &t, Face::Sans);
            }
            sy += 20.0 * s;
        }
        if info.alerts_on {
            let q = if info.quiet.is_empty() { "no quiet hours".to_string() } else { format!("quiet hours {}", info.quiet) };
            let t = g.fit(12.0 * s, &format!("At most {} an hour · {q}", info.alerts_max_per_hour), Face::Sans, side_w);
            g.text(sx, sy, 12.0 * s, DIM, &t, Face::Sans);
        }
        self.draw_note(g, area, s);
    }

    fn draw_note(&self, g: &mut Gfx, area: R, s: f32) {
        if let Some((t, ok, _)) = &self.note {
            let t = g.fit(13.0 * s, t, Face::Sans, area.w - 40.0 * s);
            g.text(area.x + 20.0 * s, area.y + area.h - 26.0 * s, 13.0 * s, if *ok { OK } else { ERR }, &t, Face::Sans);
        }
    }

    fn draw_events(&mut self, g: &mut Gfx, area: R, s: f32, cx: &TileCx) {
        self.load_thumb(cx);
        let x = area.x + 20.0 * s;
        let w = area.w - 40.0 * s;
        let mut y = area.y + 8.0 * s;
        g.button(x, y, 13.0 * s, "Back (Esc)", MUTED, Event::Tab(T_BACK));
        y += 40.0 * s;
        let title = g.fit(16.0 * s, &format!("Motion events of {}", self.name), Face::Bold, w);
        g.text(x, y, 16.0 * s, TEXT, &title, Face::Bold);
        y += 26.0 * s;
        if self.events.is_empty() {
            g.text(x, y, 13.0 * s, MUTED, "No motion events yet. They appear here after motion watching sees something.", Face::Sans);
            self.draw_note(g, area, s);
            return;
        }
        let list_w = w * 0.62;
        for (i, e) in self.events.iter().enumerate().take(((area.y + area.h - y - 40.0 * s) / (22.0 * s)).max(1.0) as usize) {
            let dur = e.end_ms.map_or("not ended".to_string(), |t| format!("{} s", t.saturating_sub(e.start_ms) / 1000));
            let who = match e.person {
                Some(true) => " · person",
                _ => "",
            };
            let line = g.fit(12.0 * s, &format!("{} · {} · {dur} · strength {}{who}", e.label, e.zone, e.score), if i == self.ev_sel { Face::Bold } else { Face::Sans }, list_w - 8.0 * s);
            if i == self.ev_sel {
                g.rrect(x - 4.0 * s, y - 2.0 * s, list_w, 20.0 * s, 6.0 * s, [255, 255, 255, 30]);
            }
            g.text(x, y, 12.0 * s, if i == self.ev_sel { TEXT } else { MUTED }, &line, if i == self.ev_sel { Face::Bold } else { Face::Sans });
            g.hit(R::new(x - 4.0 * s, y - 2.0 * s, list_w, 20.0 * s), Event::Row(i), &e.label);
            y += 22.0 * s;
        }
        // the thumbnail of the selected one
        if let Some((_, pm)) = &self.thumb {
            let tx = x + list_w + 16.0 * s;
            let tw = (w - list_w - 16.0 * s).min(pm.width() as f32 * 2.0 * s);
            let k = tw / pm.width() as f32;
            let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
            g.pm.draw_pixmap(0, 0, pm.as_ref(), &paint, Transform::from_row(k, 0.0, 0.0, k, tx, area.y + 74.0 * s), None);
            g.text(tx, area.y + 78.0 * s + pm.height() as f32 * k, 11.0 * s, DIM, "Thumbnail (areas under a privacy mask are black)", Face::Sans);
        }
        self.draw_note(g, area, s);
    }
}

/// The marks of one day's events on the recordings timeline: fractions of the day, 0.0 to 1.0.
pub fn day_marks(events: &[MotionRow], day_start_ms: u64) -> Vec<f32> {
    events.iter().filter(|e| e.start_ms >= day_start_ms && e.start_ms < day_start_ms + DAY_MS).map(|e| (e.start_ms - day_start_ms) as f32 / DAY_MS as f32).collect()
}
