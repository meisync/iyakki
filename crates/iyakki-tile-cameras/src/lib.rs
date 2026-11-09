//! CAMERAS: a live view of the cameras the person may watch, in a grid of 1, 2, 4 or 6, one click to enlarge, a snapshot button, and
//! a plain statement of what each camera is doing (live, connecting, lost, in use, needs the video add-on, needs a password).
//!
//! * Privacy: while a picture of this computer's own USB camera is on the screen, a red band says so, always. Nothing is recorded here
//!   (recording is a separate feature); a snapshot happens on a click or a key, and the tile shows where the file went.
//! * Only the cameras on screen are asked for pictures; the others are not streamed (the source stops a stream a few seconds after the
//!   last ask).
//! * Passwords are typed in a masked field and go to the source; the tile never shows or keeps one, and an address is shown as the source
//!   gives it (without a user name or password).
//! The tile never touches the network or a device: it asks a `CameraSource`.

use std::collections::HashMap;

pub mod motion;
pub mod rec;

use iyakki_tile_api::cameras::{CamInfo, Frame, Kind, NewCam, Status};
use iyakki_tile_api::{App, Flow, TileCx};
use iyakki_uikit::gfx::{rgb, Face, Gfx, Icon};
use iyakki_uikit::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};
use iyakki_uikit::widgets::{render_confirm, Form, FormEvent};
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

pub const ACCENT: [u8; 4] = rgb(255, 140, 100);
const AMBER: [u8; 4] = rgb(255, 196, 90);
const PURPLE: [u8; 4] = rgb(190, 150, 255);
const RED: [u8; 4] = rgb(255, 70, 70);

const LAYOUTS: [usize; 4] = [1, 2, 4, 6];
/// The list of cameras is read again this often (ticks, ~30 a second).
const REFRESH_TICKS: u64 = 15;

enum Screen {
    Grid,
    /// One camera fills the window.
    Big(String),
    /// Add or change a camera (the id when changing).
    Form(Box<Form>, Option<String>),
    Confirm(String),
    /// The recordings page of one camera.
    Rec(Box<rec::RecView>),
    /// The motion page of one camera (zones, privacy masks, alerts, events).
    Motion(Box<motion::MotionView>),
}

pub struct CamerasApp {
    screen: Screen,
    cams: Vec<CamInfo>,
    last_refresh: Option<u64>,
    layout: usize,
    first: usize,
    sel: usize,
    note: Option<(String, bool, u64)>,
    flash: Option<(String, u64)>,
    pix: HashMap<String, (u64, Pixmap)>,
    tick: u64,
    privacy: bool,
    shown: Vec<String>,
    /// The cameras whose recorder is running (each gets a red REC mark on its picture).
    rec_ids: Vec<String>,
    /// The cameras whose pictures are being analysed for motion (each gets a MOTION mark on its picture).
    motion_ids: Vec<String>,
}

impl Default for CamerasApp {
    fn default() -> Self {
        Self::new()
    }
}

fn grid_of(n: usize) -> (usize, usize) {
    match n {
        1 => (1, 1),
        2 => (2, 1),
        4 => (2, 2),
        _ => (3, 2),
    }
}

fn status_color(s: &Status) -> [u8; 4] {
    match s {
        Status::Live => OK,
        Status::Connecting | Status::Busy(_) | Status::NeedsPassword(_) => AMBER,
        Status::Lost(_) => ERR,
        Status::NeedsAddon(_) => PURPLE,
    }
}

impl CamerasApp {
    pub fn new() -> CamerasApp {
        CamerasApp { screen: Screen::Grid, cams: vec![], last_refresh: None, layout: 4, first: 0, sel: 0, note: None, flash: None, pix: HashMap::new(), tick: 0, privacy: false, shown: vec![], rec_ids: vec![], motion_ids: vec![] }
    }

    pub fn layout(&self) -> usize {
        self.layout
    }
    pub fn selected(&self) -> Option<&CamInfo> {
        self.cams.get(self.sel)
    }
    pub fn enlarged(&self) -> Option<&str> {
        match &self.screen {
            Screen::Big(id) => Some(id),
            _ => None,
        }
    }
    /// The recordings page, when it is open.
    pub fn recordings(&self) -> Option<&rec::RecView> {
        match &self.screen {
            Screen::Rec(r) => Some(r),
            _ => None,
        }
    }
    /// The motion page, when it is open.
    pub fn motion_page(&self) -> Option<&motion::MotionView> {
        match &self.screen {
            Screen::Motion(m) => Some(m),
            _ => None,
        }
    }
    /// The cameras that showed a MOTION mark on the last frame.
    pub fn motion_marked(&self) -> &[String] {
        &self.motion_ids
    }
    /// The cameras that showed a red REC mark on the last frame.
    pub fn rec_marked(&self) -> &[String] {
        &self.rec_ids
    }
    pub fn in_form(&self) -> bool {
        matches!(self.screen, Screen::Form(..))
    }
    pub fn asking_to_remove(&self) -> bool {
        matches!(self.screen, Screen::Confirm(_))
    }
    /// The last line shown to the person (a saved snapshot, an error) and whether it was good news.
    pub fn note(&self) -> Option<(&str, bool)> {
        self.note.as_ref().map(|(t, ok, _)| (t.as_str(), *ok))
    }
    /// The red "this computer's camera is on" band was drawn on the last frame.
    pub fn privacy_banner_shown(&self) -> bool {
        self.privacy
    }
    /// The ids of the cameras that were asked for pictures on the last frame.
    pub fn shown_ids(&self) -> &[String] {
        &self.shown
    }

    fn say(&mut self, t: impl Into<String>, ok: bool) {
        self.note = Some((t.into(), ok, self.tick));
    }

    fn refresh(&mut self, cx: &TileCx, tick: u64) {
        if self.last_refresh.is_some_and(|l| tick.wrapping_sub(l) < REFRESH_TICKS && tick >= l) {
            return;
        }
        self.last_refresh = Some(tick);
        self.cams = cx.cameras.cameras();
        self.rec_ids = cx.cameras.recording_ids();
        self.motion_ids = cx.cameras.motion_ids();
        self.fix_selection();
    }

    fn fix_selection(&mut self) {
        let n = self.cams.len();
        self.sel = self.sel.min(n.saturating_sub(1));
        self.first = self.first.min(n.saturating_sub(1));
        if self.sel < self.first {
            self.first = self.sel;
        }
        if self.sel >= self.first + self.layout {
            self.first = self.sel + 1 - self.layout;
        }
        let ids: Vec<&str> = self.cams.iter().map(|c| c.id.as_str()).collect();
        self.pix.retain(|k, _| ids.contains(&k.as_str()));
    }

    fn set_layout(&mut self, n: usize) {
        self.layout = n;
        self.first = (self.sel / n) * n;
        self.fix_selection();
    }

    fn move_sel(&mut self, d: isize) {
        let n = self.cams.len() as isize;
        if n == 0 {
            return;
        }
        self.sel = (self.sel as isize + d).clamp(0, n - 1) as usize;
        self.fix_selection();
        if let Screen::Big(_) = self.screen {
            if let Some(c) = self.cams.get(self.sel) {
                self.screen = Screen::Big(c.id.clone());
            }
        }
    }

    fn cam_form(&self, cx: &TileCx, edit: Option<&CamInfo>) -> Form {
        let people = cx.cameras.people();
        let hint = if people.is_empty() { "other people who may watch, separated by commas".to_string() } else { format!("other people who may watch: {}", people.join(", ")) };
        let mut f = Form::new(if edit.is_some() { "Edit camera" } else { "Add a network camera" })
            .with_note("Use the camera's rtsp:// address (H.264 needs the video add-on; Motion-JPEG works as it is) or an http:// Motion-JPEG or snapshot address. The password goes to your vault, never to a settings file. Leave it empty when editing to keep the stored one.")
            .text("Name", edit.map_or("", |c| c.name.as_str()), "for example Front door")
            .text("Address", edit.map_or("", |c| c.address.as_str()), "rtsp://192.168.1.20:554/stream1")
            .text("User", edit.map_or("", |c| c.user.as_str()), "the camera's login name")
            .secret("Password", if edit.is_some_and(|c| c.has_password) { "stored; empty keeps it" } else { "kept in your vault" })
            .text("People", &edit.map_or(String::new(), |c| c.allowed.join(", ")), &hint);
        f.focus = 0;
        f
    }

    fn submit_form(&mut self, cx: &TileCx, mut f: Box<Form>, replacing: Option<String>) {
        let allowed: Vec<String> = f.value(4).split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
        let new = NewCam { name: f.value(0).trim().to_string(), address: f.value(1).trim().to_string(), user: f.value(2).trim().to_string(), allowed };
        let pw = f.raw(3);
        match cx.cameras.save(replacing.as_deref(), &new, if pw.is_empty() { None } else { Some(pw.as_str()) }) {
            Ok(_) => {
                self.screen = Screen::Grid;
                self.last_refresh = None;
                self.say(format!("Saved {}", new.name), true);
            }
            Err(e) => {
                f.error = Some(e);
                self.screen = Screen::Form(f, replacing);
            }
        }
    }

    fn snapshot(&mut self, cx: &TileCx, id: &str) {
        let name = self.cams.iter().find(|c| c.id == id).map_or_else(String::new, |c| c.name.clone());
        match cx.cameras.snapshot(id) {
            Ok(path) => {
                self.flash = Some((id.to_string(), self.tick + 12));
                self.say(format!("Snapshot of {name} saved: {path}"), true);
            }
            Err(e) => self.say(e, false),
        }
    }

    fn handle_grid(&mut self, ev: &Event, cx: &TileCx) -> Flow {
        let big = self.enlarged().map(str::to_string);
        match ev {
            Event::Key(Key::Esc) | Event::Key(Key::Char('q')) | Event::Key(Key::Backspace) if big.is_some() => self.screen = Screen::Grid,
            Event::Key(Key::Esc) | Event::Key(Key::Char('q')) => return Flow::Back,
            Event::Key(Key::Char(c @ ('1' | '2' | '4' | '6'))) if big.is_none() => self.set_layout(c.to_digit(10).unwrap_or(4) as usize),
            Event::Tab(t @ 1..=4) if big.is_none() => self.set_layout(LAYOUTS[*t - 1]),
            Event::Key(Key::Left) | Event::Key(Key::Up) => self.move_sel(-1),
            Event::Key(Key::Right) | Event::Key(Key::Down) => self.move_sel(1),
            Event::Key(Key::PageUp) => self.move_sel(-(self.layout as isize)),
            Event::Key(Key::PageDown) => self.move_sel(self.layout as isize),
            Event::Key(Key::Home) => self.move_sel(-(self.cams.len() as isize)),
            Event::Key(Key::End) => self.move_sel(self.cams.len() as isize),
            Event::Row(i) if big.is_none() && *i < self.cams.len() => {
                self.sel = *i;
                self.fix_selection();
                self.screen = Screen::Big(self.cams[*i].id.clone());
            }
            Event::Key(Key::Enter) if big.is_none() => {
                if let Some(c) = self.cams.get(self.sel) {
                    self.screen = Screen::Big(c.id.clone());
                }
            }
            Event::Key(Key::Enter) => self.screen = Screen::Grid,
            Event::Key(Key::Char('s')) | Event::Tab(31) => match self.cams.get(self.sel).map(|c| c.id.clone()) {
                Some(id) => self.snapshot(cx, &id),
                None => self.say("There is no camera to take a snapshot of.", false),
            },
            Event::Key(Key::Char('p')) | Event::Tab(34) => {
                let id = big.clone().or_else(|| self.cams.get(self.sel).map(|c| c.id.clone()));
                match id.and_then(|i| self.cams.iter().find(|c| c.id == i).cloned()) {
                    Some(c) if c.kind == Kind::Network => self.screen = Screen::Rec(Box::new(rec::RecView::new(cx, &c.id, &c.name))),
                    Some(_) => self.say("This computer's own camera is never recorded.", false),
                    None => self.say("There is no camera to show recordings of.", false),
                }
            }
            Event::Key(Key::Char('m')) | Event::Tab(35) => {
                let id = big.clone().or_else(|| self.cams.get(self.sel).map(|c| c.id.clone()));
                match id.and_then(|i| self.cams.iter().find(|c| c.id == i).cloned()) {
                    Some(c) if c.kind == Kind::Network => self.screen = Screen::Motion(Box::new(motion::MotionView::new(cx, &c.id, &c.name))),
                    Some(_) => self.say("This computer's own camera is never watched in the background.", false),
                    None => self.say("There is no camera to watch for motion.", false),
                }
            }
            Event::Key(Key::Char('a')) | Event::Tab(30) => self.screen = Screen::Form(Box::new(self.cam_form(cx, None)), None),
            Event::Key(Key::Char('e')) | Event::Tab(32) => match self.cams.get(self.sel).cloned() {
                Some(c) if c.kind == Kind::Network && c.manage => self.screen = Screen::Form(Box::new(self.cam_form(cx, Some(&c))), Some(c.id)),
                Some(c) if c.kind == Kind::Usb => self.say("A camera plugged into this computer has nothing to edit.", false),
                Some(_) => self.say("Only the person who added this camera, or an administrator, can change it.", false),
                None => {}
            },
            Event::Key(Key::Char('d')) | Event::Key(Key::Delete) | Event::Tab(33) => match self.cams.get(self.sel).cloned() {
                Some(c) if c.kind == Kind::Network && c.manage => self.screen = Screen::Confirm(c.id),
                Some(c) if c.kind == Kind::Usb => self.say("A camera plugged into this computer cannot be removed here; unplug it.", false),
                Some(_) => self.say("Only the person who added this camera, or an administrator, can remove it.", false),
                None => {}
            },
            _ => {}
        }
        Flow::Stay
    }

    fn draw_picture(&mut self, g: &mut Gfx, cx: &TileCx, c: &CamInfo, r: R, s: f32, big: bool, selected: bool, idx: Option<usize>) {
        iyakki_uikit::fx::well(g, r, s);
        self.shown.push(c.id.clone());
        let frame: Option<Frame> = cx.cameras.view(&c.id, big);
        if let Some(f) = frame.as_ref() {
            let stale = self.pix.get(&c.id).is_none_or(|(seq, _)| *seq != f.seq);
            if stale {
                if let Some(pm) = tiny_skia::IntSize::from_wh(f.width, f.height).and_then(|sz| Pixmap::from_vec((*f.rgba).clone(), sz)) {
                    self.pix.insert(c.id.clone(), (f.seq, pm));
                }
            }
        }
        let live_pic = matches!(c.status, Status::Live | Status::Connecting) || frame.is_some();
        match self.pix.get(&c.id).filter(|_| live_pic) {
            Some((_, pm)) => {
                let (fw, fh) = (pm.width() as f32, pm.height() as f32);
                let k = ((r.w - 4.0 * s) / fw).min((r.h - 4.0 * s) / fh);
                let (dw, dh) = (fw * k, fh * k);
                let (x, y) = (r.x + (r.w - dw) / 2.0, r.y + (r.h - dh) / 2.0);
                let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
                g.pm.draw_pixmap(0, 0, pm.as_ref(), &paint, Transform::from_row(dw / fw, 0.0, 0.0, dh / fh, x, y), None);
            }
            None => {
                let (title, detail) = match &c.status {
                    Status::Connecting => (format!("Connecting{}", ".".repeat(1 + (self.tick / 10 % 3) as usize)), String::new()),
                    other => (other.label().to_string(), other.detail().to_string()),
                };
                let tx = r.x + 12.0 * s;
                let title = g.fit(15.0 * s, &title, Face::Bold, r.w - 24.0 * s);
                g.text(tx, r.y + r.h / 2.0 - 18.0 * s, 15.0 * s, status_color(&c.status), &title, Face::Bold);
                for (i, line) in g.wrap(12.0 * s, &detail, Face::Sans, r.w - 24.0 * s).into_iter().take(4).enumerate() {
                    g.text(tx, r.y + r.h / 2.0 + 4.0 * s + i as f32 * 16.0 * s, 12.0 * s, MUTED, &line, Face::Sans);
                }
            }
        }
        // a flash after a snapshot
        if let Some((id, until)) = &self.flash {
            if *id == c.id && self.tick < *until {
                let a = ((*until - self.tick) as f32 / 12.0 * 160.0) as u8;
                g.rect(r.x, r.y, r.w, r.h, [255, 255, 255, a]);
            }
        }
        // the name and state, bottom left
        let dot = status_color(&c.status);
        let label = if c.kind == Kind::Usb { format!("{} · this computer", c.name) } else { c.name.clone() };
        let label = g.fit(12.0 * s, &label, Face::Bold, r.w * 0.55);
        let (nw, sw) = (g.text_width(12.0 * s, &label, Face::Bold), g.text_width(11.0 * s, c.status.label(), Face::Sans));
        g.rrect(r.x + 8.0 * s, r.y + r.h - 28.0 * s, nw + sw + 44.0 * s, 20.0 * s, 10.0 * s, [0, 0, 0, 170]);
        g.circle(r.x + 19.0 * s, r.y + r.h - 18.0 * s, 4.0 * s, dot);
        g.text(r.x + 28.0 * s, r.y + r.h - 25.0 * s, 12.0 * s, TEXT, &label, Face::Bold);
        g.text(r.x + 36.0 * s + nw, r.y + r.h - 25.0 * s, 11.0 * s, dot, c.status.label(), Face::Sans);
        if selected {
            g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, 2.5, ACCENT);
        }
        if c.kind == Kind::Usb {
            g.circle(r.x + r.w - 16.0 * s, r.y + 16.0 * s, 6.0 * s, RED);
        }
        // a camera that records says so on its picture, always, whatever the picture is doing
        if self.rec_ids.contains(&c.id) {
            let w = g.text_width(12.0 * s, "REC", Face::Bold) + 32.0 * s;
            g.rrect(r.x + 8.0 * s, r.y + 8.0 * s, w, 22.0 * s, 11.0 * s, [150, 20, 20, 235]);
            g.circle(r.x + 20.0 * s, r.y + 19.0 * s, 5.0 * s, rgb(255, 255, 255));
            g.text(r.x + 31.0 * s, r.y + 12.0 * s, 12.0 * s, rgb(255, 255, 255), "REC", Face::Bold);
        }
        // a camera whose pictures are being analysed for motion says so on its picture, always
        if self.motion_ids.contains(&c.id) {
            let off = if self.rec_ids.contains(&c.id) { 70.0 } else { 0.0 };
            let w = g.text_width(12.0 * s, "MOTION", Face::Bold) + 24.0 * s;
            g.rrect(r.x + (8.0 + off) * s, r.y + 8.0 * s, w, 22.0 * s, 11.0 * s, [20, 80, 150, 235]);
            g.text(r.x + (20.0 + off) * s, r.y + 12.0 * s, 12.0 * s, rgb(255, 255, 255), "MOTION", Face::Bold);
        }
        if let Some(i) = idx {
            g.hit(r, Event::Row(i), &c.name);
        }
    }

    fn draw_grid(&mut self, g: &mut Gfx, area: R, s: f32, cx: &TileCx) {
        let x = area.x + 20.0 * s;
        let w = area.w - 40.0 * s;
        let mut y = area.y + 8.0 * s;
        // layout buttons and actions
        // one row of buttons that wraps (a narrow window or a large text size makes more rows, nothing is cut off)
        let mut bar: Vec<(String, [u8; 4], Event)> = LAYOUTS.iter().enumerate().map(|(i, n)| (format!("{n}"), if *n == self.layout { ACCENT } else { MUTED }, Event::Tab(i + 1))).collect();
        for (label, tab, col) in [("Add (a)", 30, ACCENT), ("Snapshot (s)", 31, MUTED), ("Recordings (p)", 34, MUTED), ("Motion (m)", 35, MUTED), ("Edit (e)", 32, MUTED), ("Remove (d)", 33, MUTED)] {
            bar.push((label.to_string(), col, Event::Tab(tab)));
        }
        let (bar_h, _) = g.button_flow(x, y, w, 13.0 * s, 6.0 * s, &bar);
        y += bar_h.max(34.0 * s) + 6.0 * s;
        let (first, layout) = (self.first, self.layout);
        let visible: Vec<usize> = (first..(first + layout).min(self.cams.len())).collect();
        self.privacy = visible.iter().any(|i| self.cams[*i].kind == Kind::Usb);
        if self.privacy {
            g.rrect(x, y, w, 24.0 * s, 6.0 * s, [150, 20, 20, 230]);
            g.circle(x + 14.0 * s, y + 12.0 * s, 5.0 * s, rgb(255, 255, 255));
            g.text(x + 28.0 * s, y + 4.0 * s, 13.0 * s, rgb(255, 255, 255), "This computer's camera is on and shown in this window. Nothing is recorded.", Face::Bold);
            y += 30.0 * s;
        }
        let bottom = area.y + area.h - 34.0 * s;
        if self.cams.is_empty() {
            iyakki_uikit::fx::empty_state(g, R::new(x, y, w, bottom - y), s, Icon::Camera, "No cameras yet", "Press a to add a network camera (an rtsp:// or http:// address). A USB camera plugged into this computer appears here by itself.", ACCENT);
            return;
        }
        let (cols, rows) = grid_of(layout);
        let gap = 10.0 * s;
        let (cw, ch) = ((w - gap * (cols as f32 - 1.0)) / cols as f32, (bottom - y - gap * (rows as f32 - 1.0)) / rows as f32);
        for (k, idx) in visible.iter().enumerate() {
            let (col, row) = (k % cols, k / cols);
            let r = R::new(x + col as f32 * (cw + gap), y + row as f32 * (ch + gap), cw, ch);
            let c = self.cams[*idx].clone();
            self.draw_picture(g, cx, &c, r, s, false, *idx == self.sel, Some(*idx));
        }
        let info = format!("{}–{} of {}   ·   PageUp / PageDown for more", first + 1, first + visible.len(), self.cams.len());
        g.text(x, bottom + 8.0 * s, 12.0 * s, DIM, &info, Face::Sans);
    }

    fn draw_big(&mut self, g: &mut Gfx, area: R, s: f32, cx: &TileCx, id: &str) {
        let x = area.x + 20.0 * s;
        let w = area.w - 40.0 * s;
        let mut y = area.y + 8.0 * s;
        let Some(c) = self.cams.iter().find(|c| c.id == id).cloned() else {
            self.screen = Screen::Grid;
            return;
        };
        let mut bar = vec![("Back (Esc)".to_string(), MUTED, Event::Key(Key::Esc)), ("Snapshot (s)".to_string(), ACCENT, Event::Tab(31))];
        if c.kind == Kind::Network {
            bar.push(("Recordings (p)".to_string(), MUTED, Event::Tab(34)));
            bar.push(("Motion (m)".to_string(), MUTED, Event::Tab(35)));
        }
        let (bar_h, end_x) = g.button_flow(x, y, w, 13.0 * s, 8.0 * s, &bar);
        let sub = format!("{}{}", c.status.label(), if c.fps > 0.5 { format!(" · {:.0} fps", c.fps) } else { String::new() });
        let sub = format!("{sub}{}", if c.size.0 > 0 { format!(" · {}×{}", c.size.0, c.size.1) } else { String::new() });
        // the state sits after the last button when it fits there, else under the buttons
        let sw = g.text_width(13.0 * s, &sub, Face::Sans);
        let one_row = bar_h < 13.0 * s * 2.1 * 1.5;
        if one_row && end_x + 16.0 * s + sw <= x + w {
            g.text(end_x + 16.0 * s, y + 4.0 * s, 13.0 * s, status_color(&c.status), &sub, Face::Sans);
            y += bar_h.max(34.0 * s) + 6.0 * s;
        } else {
            let sub = g.fit(13.0 * s, &sub, Face::Sans, w);
            g.text(x, y + bar_h + 4.0 * s, 13.0 * s, status_color(&c.status), &sub, Face::Sans);
            y += bar_h + 28.0 * s;
        }
        self.privacy = c.kind == Kind::Usb;
        if self.privacy {
            g.rrect(x, y, w, 24.0 * s, 6.0 * s, [150, 20, 20, 230]);
            g.circle(x + 14.0 * s, y + 12.0 * s, 5.0 * s, rgb(255, 255, 255));
            g.text(x + 28.0 * s, y + 4.0 * s, 13.0 * s, rgb(255, 255, 255), "This computer's camera is on and shown in this window. Nothing is recorded.", Face::Bold);
            y += 30.0 * s;
        }
        let r = R::new(x, y, w, area.y + area.h - 34.0 * s - y);
        self.draw_picture(g, cx, &c, r, s, true, false, None);
    }
}

impl App for CamerasApp {
    fn title(&self) -> &'static str {
        "CAMERAS"
    }
    fn icon(&self) -> Icon {
        Icon::Camera
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        match self.screen {
            Screen::Grid => "1 2 4 6 views · arrows select · Enter or click enlarges · s snapshot · a add · e edit · d remove · Esc back".into(),
            Screen::Big(_) => "s snapshot · ← → other camera · Esc back to the grid".into(),
            Screen::Form(..) => "Tab next field · Enter save · Esc cancel".into(),
            Screen::Confirm(_) => "y remove · Esc cancel".into(),
            Screen::Rec(ref r) => r.hint(),
            Screen::Motion(ref m) => m.hint(),
        }
    }
    fn handle(&mut self, ev: &Event, cx: &TileCx) -> Flow {
        if matches!(ev, Event::Tick) {
            self.tick += 1;
        }
        if self.cams.is_empty() && self.last_refresh.is_none() {
            self.refresh(cx, self.tick);
        }
        let screen = std::mem::replace(&mut self.screen, Screen::Grid);
        match screen {
            Screen::Grid | Screen::Big(_) => {
                self.screen = screen;
                return self.handle_grid(ev, cx);
            }
            Screen::Rec(mut r) => {
                match r.handle(ev, cx) {
                    rec::Out::Stay => self.screen = Screen::Rec(r),
                    rec::Out::Back => self.last_refresh = None,
                }
            }
            Screen::Motion(mut m) => match m.handle(ev, cx) {
                motion::Out::Stay => self.screen = Screen::Motion(m),
                motion::Out::Back => self.last_refresh = None,
            },
            Screen::Form(mut f, replacing) => match f.handle_event(ev) {
                FormEvent::Cancel => {}
                FormEvent::Submit => self.submit_form(cx, f, replacing),
                _ => self.screen = Screen::Form(f, replacing),
            },
            Screen::Confirm(id) => match ev {
                Event::Key(Key::Char('y')) => {
                    let name = self.cams.iter().find(|c| c.id == id).map_or_else(String::new, |c| c.name.clone());
                    match cx.cameras.remove(&id) {
                        Ok(()) => {
                            self.last_refresh = None;
                            self.say(format!("Removed {name}. Its password is gone from your vault."), true);
                            self.refresh(cx, self.tick);
                        }
                        Err(e) => self.say(e, false),
                    }
                }
                Event::Key(Key::Esc) | Event::Key(Key::Char('n')) => {}
                _ => self.screen = Screen::Confirm(id),
            },
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &TileCx, tick: u64) {
        self.tick = self.tick.max(tick);
        self.refresh(cx, tick);
        self.shown.clear();
        self.privacy = false;
        match std::mem::replace(&mut self.screen, Screen::Grid) {
            Screen::Grid => {
                self.screen = Screen::Grid;
                self.draw_grid(g, area, s, cx);
            }
            Screen::Big(id) => {
                self.screen = Screen::Big(id.clone());
                self.draw_big(g, area, s, cx, &id);
            }
            Screen::Form(f, r) => {
                f.render(g, area, s, ACCENT, iyakki_uikit::blink::blink(tick));
                self.screen = Screen::Form(f, r);
            }
            Screen::Rec(mut r) => {
                r.render(g, area, s, cx, tick);
                self.screen = Screen::Rec(r);
            }
            Screen::Motion(mut m) => {
                m.render(g, area, s, cx, tick);
                self.screen = Screen::Motion(m);
            }
            Screen::Confirm(id) => {
                let name = self.cams.iter().find(|c| c.id == id).map_or("this camera", |c| c.name.as_str()).to_string();
                self.screen = Screen::Confirm(id);
                self.draw_grid(g, area, s, cx);
                render_confirm(g, area, s, &format!("Remove the camera “{name}” and its stored password?"), "Remove");
            }
        }
        if let Some((t, ok, at)) = &self.note {
            if tick.saturating_sub(*at) < 240 && !matches!(self.screen, Screen::Form(..) | Screen::Confirm(_) | Screen::Rec(_) | Screen::Motion(_)) {
                let t = g.fit(13.0 * s, t, Face::Sans, area.w - 40.0 * s);
                g.text(area.x + 20.0 * s, area.y + area.h - 26.0 * s, 13.0 * s, if *ok { OK } else { ERR }, &t, Face::Sans);
            }
        }
    }
}

#[cfg(test)]
mod motion_tests;
#[cfg(test)]
mod rec_tests;
#[cfg(test)]
mod tests;
