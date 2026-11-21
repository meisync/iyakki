//! `CameraView`: live camera pictures as a component an SDK app places in its layout (docs/development/UI-SDK.md, "Embedding camera views").
//!
//! The app keeps a `CameraView` in its state, gives it a [`CameraPort`] (the core's), a [`ViewConfig`] and an action number range, and a
//! rectangle to draw in. What the view shows, in the order a person meets it: the cameras the person may watch (a list to tick, or the
//! cameras the app named), the consent card ("Allow" and "Deny"; nothing is pulled before Allow), then the live pictures: one camera or a
//! grid of up to 4, a sign while pictures are being pulled, a name and a state sentence on each cell, the owner's motion zones and recent
//! motion boxes over the picture (read only), a REC mark on a camera that is being recorded, a snapshot button (a card every time), pan, tilt
//! and zoom buttons when the camera has them (a card the first time), full screen, and Close. A hidden view pulls nothing.
//!
//! What the app does, in four lines:
//! ```text
//! event(Tick(t)):  cams.tick(t)                         event(Action): if cams.owns(a) { cams.action(a) }
//! event(Key):      cams.key(k) while it has the focus   pane hidden or shown: cams.set_visible(v)
//! view:            cams.draw(c, th, &metrics, rect)     leaving the screen: cams.close()
//! ```
//! Pictures are never queued: each tick the view asks for the newest one newer than what it has, at most 5 times a second for a grid cell and
//! 10 for one large camera (the core's own caps), keeps one picture per camera, and draws it letterboxed with [`Canvas::image`]. Colours come
//! from the theme and sizes from the metrics only.

use std::collections::BTreeMap;

use crate::cameras::*;
use crate::canvas::{contain, fit, Canvas, Hit, Image, ImageFormat, Role, TextStyle};
use crate::chatview::wrap;
use crate::event::UiKey;
use crate::geom::Rect;
use crate::size::{Metrics, SizeClass, TypeRole};
use crate::tokens::{Line, Radius, Space};
use crate::theme::Theme;

/// How many action numbers a camera view uses, from the base the app gave it.
pub const ACTIONS: u32 = 40;
const A_SHOW: u32 = 0;
const A_ALLOW: u32 = 1;
const A_DENY: u32 = 2;
const A_SNAP: u32 = 3;
const A_CLOSE: u32 = 4;
const A_FULL: u32 = 5;
const A_CELL: u32 = 6; // 6..=9
const A_PTZ: u32 = 10; // 10..=16: left, right, up, down, zoom in, zoom out, stop
const A_REFRESH: u32 = 17;
const A_RETRY: u32 = 18;
const A_PICK: u32 = 20; // 20..=39
const MAX_PICK_ROWS: usize = 20;
/// Pictures a second for a grid cell and for one large camera (the core's caps; asking faster gets `None`).
pub const FPS_SMALL: u64 = 5;
pub const FPS_LARGE: u64 = 10;
const EVENTS_EVERY_MS: u64 = 250;
const STATE_EVERY_MS: u64 = 1000;
const OVERLAY_EVERY_MS: u64 = 1000;
const MAX_MARKS: usize = 16;

#[derive(Clone, Debug, PartialEq)]
pub struct ViewConfig {
    /// The cameras to show without asking the person to choose (ids from the list); empty: the person ticks them.
    pub cameras: Vec<String>,
    /// What the card says besides the app's name ("Doorbell panel").
    pub label: String,
    /// The pictures' format: `Rgba` for a pixel canvas, `Jpeg` for a canvas that makes a display list for a browser or client.
    pub format: ImageFormat,
}

impl Default for ViewConfig {
    fn default() -> ViewConfig {
        ViewConfig { cameras: Vec::new(), label: String::new(), format: ImageFormat::Rgba }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    /// The list of cameras.
    Picking,
    /// The card is shown.
    Consent,
    Live,
    /// The person said no, or the core could not ask.
    Denied(String),
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub call: String,
    pub action: CamAction,
    pub camera: String,
    pub summary: String,
    pub detail: String,
}

pub struct CameraView {
    port: Box<dyn CameraPort>,
    cfg: ViewConfig,
    base: u32,
    stage: Stage,
    listed: bool,
    list: Vec<CamInfo>,
    picked: Vec<String>,
    shown: Vec<CamInfo>,
    frames: BTreeMap<String, Image>,
    fetched_at: BTreeMap<String, u64>,
    cell_err: BTreeMap<String, String>,
    overlays: BTreeMap<String, Overlay>,
    card: Option<Card>,
    enlarged: Option<usize>,
    fullscreen: bool,
    indicator: bool,
    visible: bool,
    note: Option<String>,
    error: Option<String>,
    now: u64,
    events_at: Option<u64>,
    state_at: Option<u64>,
    overlay_at: Option<u64>,
    /// The view has the keyboard focus (the host says so): `y`, `n`, `s` and the arrows work.
    pub focused: bool,
}

fn due(now: u64, last: Option<u64>, every: u64) -> bool {
    last.is_none_or(|t| now.saturating_sub(t) >= every)
}

impl CameraView {
    pub fn new(port: Box<dyn CameraPort>, cfg: ViewConfig, action_base: u32) -> CameraView {
        CameraView { port, cfg, base: action_base, stage: Stage::Picking, listed: false, list: Vec::new(), picked: Vec::new(), shown: Vec::new(), frames: BTreeMap::new(), fetched_at: BTreeMap::new(), cell_err: BTreeMap::new(), overlays: BTreeMap::new(), card: None, enlarged: None, fullscreen: false, indicator: false, visible: true, note: None, error: None, now: 0, events_at: None, state_at: None, overlay_at: None, focused: true }
    }

    /// Whether an action number belongs to this view.
    pub fn owns(&self, action: u32) -> bool {
        (self.base..self.base + ACTIONS).contains(&action)
    }

    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    pub fn card(&self) -> Option<&Card> {
        self.card.as_ref()
    }

    /// The sign: pictures are being pulled now.
    pub fn indicator(&self) -> bool {
        self.indicator
    }

    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// The camera filling the view, if one does.
    pub fn enlarged(&self) -> Option<&CamInfo> {
        self.enlarged.and_then(|i| self.shown.get(i))
    }

    pub fn shown(&self) -> &[CamInfo] {
        &self.shown
    }

    /// The last refusal or failure as a sentence.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The sentence about the last snapshot ("Saved camera-front.jpg in Pictures").
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    /// The newest picture the view holds for a camera.
    pub fn frame_of(&self, camera: &str) -> Option<&Image> {
        self.frames.get(camera)
    }

    /// The sentence shown in a camera's cell when it has no picture or a failure.
    pub fn cell_error(&self, camera: &str) -> Option<&str> {
        self.cell_err.get(camera).map(String::as_str)
    }

    /// The view is on screen or hidden (a tab behind, a pane scrolled away). A hidden view pulls nothing and tells the core.
    pub fn set_visible(&mut self, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if matches!(self.stage, Stage::Live | Stage::Consent) {
            if let Err(e) = self.port.set_visible(visible) {
                self.error = Some(e);
            }
        }
        if !visible {
            self.indicator = false;
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    // ------------------------------------------------------------------------------------------ the clock

    /// The heartbeat (`now_ms` from the host's `Tick`). Reads the core at the view's own pace: events four times a second, the state and the
    /// overlays once a second, pictures as often as the cells allow, none while hidden.
    pub fn tick(&mut self, now_ms: u64) {
        self.now = now_ms;
        if !self.listed && self.stage == Stage::Picking {
            self.refresh();
            if !self.cfg.cameras.is_empty() && self.stage == Stage::Picking && self.error.is_none() {
                let ids = self.cfg.cameras.clone();
                self.open(&ids);
            }
        }
        if due(now_ms, self.events_at, EVENTS_EVERY_MS) {
            self.events_at = Some(now_ms);
            self.poll_events();
        }
        if !matches!(self.stage, Stage::Live) || !self.visible {
            return;
        }
        if due(now_ms, self.state_at, STATE_EVERY_MS) {
            self.state_at = Some(now_ms);
            self.poll_state();
        }
        if self.stage != Stage::Live {
            return;
        }
        self.pull_frames();
        if due(now_ms, self.overlay_at, OVERLAY_EVERY_MS) {
            self.overlay_at = Some(now_ms);
            self.pull_overlays();
        }
    }

    fn visible_cells(&self) -> Vec<usize> {
        match self.enlarged {
            Some(i) if i < self.shown.len() => vec![i],
            _ => (0..self.shown.len()).collect(),
        }
    }

    fn pull_frames(&mut self) {
        let cells = self.visible_cells();
        let gap = 1000 / if cells.len() == 1 && self.enlarged.is_some() { FPS_LARGE } else { FPS_SMALL };
        for i in cells {
            let id = self.shown[i].id.clone();
            if self.fetched_at.get(&id).is_some_and(|t| self.now.saturating_sub(*t) < gap) {
                continue;
            }
            self.fetched_at.insert(id.clone(), self.now);
            let after = self.frames.get(&id).map(|f| f.seq).unwrap_or(0);
            match self.port.frame(&id, after, self.cfg.format) {
                Ok(Some(img)) if img.check().is_ok() => {
                    self.frames.insert(id.clone(), img);
                    self.cell_err.remove(&id);
                }
                Ok(Some(_)) => {
                    self.cell_err.insert(id, "The picture could not be read.".into());
                }
                Ok(None) => {}
                Err(e) => {
                    self.cell_err.insert(id, e);
                }
            }
        }
    }

    fn pull_overlays(&mut self) {
        for i in self.visible_cells() {
            let id = self.shown[i].id.clone();
            if let Ok(o) = self.port.overlay(&id) {
                self.overlays.insert(id, o);
            }
        }
    }

    fn poll_state(&mut self) {
        match self.port.state() {
            Ok(st) => {
                self.indicator = st.indicator && self.visible;
                for c in st.cameras {
                    if let Some(s) = self.shown.iter_mut().find(|s| s.id == c.id) {
                        *s = c;
                    }
                }
                match st.phase {
                    Phase::Denied(why) => self.stage = Stage::Denied(why),
                    Phase::Closed => self.stage = Stage::Closed,
                    Phase::Open => {
                        if self.stage == Stage::Consent {
                            self.stage = Stage::Live;
                        }
                    }
                    Phase::AwaitingConsent => {}
                }
            }
            Err(e) => {
                self.error = Some(e);
                self.indicator = false;
            }
        }
    }

    fn poll_events(&mut self) {
        let evs = self.port.events();
        if evs.is_empty() {
            return;
        }
        for e in evs {
            match e {
                CamEvent::Approval { call, action, camera, summary, detail } => self.card = Some(Card { call, action, camera, summary, detail }),
                CamEvent::Decision { call, action, approved, .. } => {
                    if self.card.as_ref().is_some_and(|c| c.call == call) {
                        self.card = None;
                    }
                    if action == CamAction::View && !approved {
                        self.stage = Stage::Denied("You said no.".into());
                        self.indicator = false;
                    }
                }
                CamEvent::Motion { camera, mark } => {
                    let o = self.overlays.entry(camera).or_default();
                    o.marks.retain(|m| m.id != mark.id);
                    o.marks.push(mark);
                    if o.marks.len() > MAX_MARKS {
                        o.marks.remove(0);
                    }
                }
                CamEvent::Snapshot { file, folder, .. } => self.note = Some(format!("Saved {file} in {folder}.")),
                CamEvent::Failed { camera, error, .. } => {
                    self.error = Some(error.clone());
                    if self.shown.iter().any(|c| c.id == camera) {
                        self.cell_err.insert(camera, error);
                    }
                }
                CamEvent::Status { camera, status, detail } => {
                    if let Some(c) = self.shown.iter_mut().find(|c| c.id == camera) {
                        c.status = status;
                        c.detail = detail;
                    }
                }
            }
        }
        // a card for the whole view moves the stage; the state tells the rest
        if self.card.as_ref().is_some_and(|c| c.action == CamAction::View) && self.stage == Stage::Picking {
            self.stage = Stage::Consent;
        }
        if self.stage == Stage::Consent || self.stage == Stage::Live {
            self.state_at = Some(self.now);
            self.poll_state();
        }
    }

    fn refresh(&mut self) {
        match self.port.list() {
            Ok(l) => {
                self.picked.retain(|p| l.iter().any(|c| c.id == *p));
                self.list = l;
                self.error = None;
            }
            Err(e) => {
                self.list.clear();
                self.error = Some(e);
            }
        }
        self.listed = true;
    }

    fn open(&mut self, ids: &[String]) {
        self.error = None;
        self.note = None;
        match self.port.open(ids, ids.len() == 1, &self.cfg.label) {
            Ok(cams) => {
                self.shown = cams;
                self.frames.clear();
                self.fetched_at.clear();
                self.cell_err.clear();
                self.overlays.clear();
                self.enlarged = (self.shown.len() == 1).then_some(0);
                self.fullscreen = false;
                self.stage = Stage::Consent;
                self.events_at = None;
                self.poll_events();
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Close the view: nothing more is pulled, the core is told, the list comes back.
    pub fn close(&mut self) {
        if !matches!(self.stage, Stage::Picking | Stage::Closed) {
            self.port.close();
        }
        self.stage = Stage::Picking;
        self.shown.clear();
        self.frames.clear();
        self.overlays.clear();
        self.cell_err.clear();
        self.card = None;
        self.enlarged = None;
        self.fullscreen = false;
        self.indicator = false;
        self.note = None;
        self.listed = false;
        self.cfg.cameras.clear();
    }

    // ------------------------------------------------------------------------------------------ input

    /// An action number of this view was used (a tap, a click, Enter on a control). `true` if it was one of ours.
    pub fn action(&mut self, a: u32) -> bool {
        if !self.owns(a) {
            return false;
        }
        let k = a - self.base;
        match k {
            A_SHOW => {
                let ids = self.picked.clone();
                if !ids.is_empty() {
                    self.open(&ids);
                }
            }
            A_ALLOW | A_DENY => self.decide(k == A_ALLOW),
            A_SNAP => self.snapshot(),
            A_CLOSE => self.close(),
            A_FULL => {
                if self.enlarged.is_some() {
                    self.fullscreen = !self.fullscreen;
                } else if !self.shown.is_empty() {
                    self.enlarged = Some(0);
                    self.fullscreen = true;
                }
            }
            c if (A_CELL..A_CELL + 4).contains(&c) => {
                let i = (c - A_CELL) as usize;
                if i < self.shown.len() && self.shown.len() > 1 {
                    self.enlarged = if self.enlarged == Some(i) { None } else { Some(i) };
                    self.fullscreen = false;
                    self.fetched_at.clear();
                }
            }
            p if (A_PTZ..A_PTZ + 7).contains(&p) => self.ptz((p - A_PTZ) as usize),
            A_REFRESH => {
                self.listed = false;
                self.refresh();
            }
            A_RETRY => {
                self.stage = Stage::Picking;
                self.shown.clear();
                self.listed = false;
                self.error = None;
            }
            r if (A_PICK..A_PICK + MAX_PICK_ROWS as u32).contains(&r) => {
                if let Some(c) = self.list.get((r - A_PICK) as usize) {
                    let id = c.id.clone();
                    if let Some(p) = self.picked.iter().position(|x| *x == id) {
                        self.picked.remove(p);
                    } else if self.picked.len() < MAX_CELLS {
                        self.picked.push(id);
                    } else {
                        self.error = Some(format!("A view shows at most {MAX_CELLS} cameras."));
                    }
                }
            }
            _ => {}
        }
        true
    }

    fn decide(&mut self, approve: bool) {
        let Some(card) = self.card.clone() else { return };
        if let Err(e) = self.port.decide(&card.call, approve) {
            self.error = Some(e);
            return;
        }
        self.card = None;
        self.events_at = None;
        self.poll_events();
        self.state_at = None;
        if self.stage == Stage::Consent || self.stage == Stage::Live {
            self.poll_state();
        }
    }

    fn snapshot(&mut self) {
        let Some(c) = self.enlarged().or(self.shown.first()).map(|c| c.id.clone()) else { return };
        self.note = None;
        match self.port.snapshot(&c) {
            Ok(_) => {
                self.events_at = None;
                self.poll_events();
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn ptz(&mut self, which: usize) {
        let Some(c) = self.enlarged().filter(|c| c.ptz).map(|c| c.id.clone()) else { return };
        let s = 0.5;
        let cmd = match which {
            0 => Ptz::Move { pan: -s, tilt: 0.0, zoom: 0.0 },
            1 => Ptz::Move { pan: s, tilt: 0.0, zoom: 0.0 },
            2 => Ptz::Move { pan: 0.0, tilt: s, zoom: 0.0 },
            3 => Ptz::Move { pan: 0.0, tilt: -s, zoom: 0.0 },
            4 => Ptz::Move { pan: 0.0, tilt: 0.0, zoom: s },
            5 => Ptz::Move { pan: 0.0, tilt: 0.0, zoom: -s },
            _ => Ptz::Stop,
        };
        match self.port.ptz(&c, cmd) {
            Ok(_) => {
                self.events_at = None;
                self.poll_events();
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// A key while the view has the focus. `true` if it was used.
    pub fn key(&mut self, k: UiKey) -> bool {
        if !self.focused {
            return false;
        }
        let live = self.stage == Stage::Live;
        match k {
            UiKey::Char('y') | UiKey::Char('Y') if self.card.is_some() => self.decide(true),
            UiKey::Char('n') | UiKey::Char('N') if self.card.is_some() => self.decide(false),
            UiKey::Esc if self.fullscreen => self.fullscreen = false,
            UiKey::Esc if live && self.enlarged.is_some() && self.shown.len() > 1 => self.enlarged = None,
            UiKey::Esc if !matches!(self.stage, Stage::Picking) => self.close(),
            UiKey::Char('s') | UiKey::Char('S') if live && self.card.is_none() => self.snapshot(),
            UiKey::Char('f') | UiKey::Char('F') if live => {
                self.action(self.base + A_FULL);
            }
            UiKey::Char(c) if live && ('1'..='4').contains(&c) => {
                self.action(self.base + A_CELL + (c as u32 - '1' as u32));
            }
            UiKey::Left if live => self.ptz(0),
            UiKey::Right if live => self.ptz(1),
            UiKey::Up if live => self.ptz(2),
            UiKey::Down if live => self.ptz(3),
            UiKey::Enter if self.stage == Stage::Picking => {
                self.action(self.base + A_SHOW);
            }
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------------------------------------ drawing

    pub fn draw(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, area: Rect) {
        c.rrect(area, m.radius(Radius::Md), th.well);
        let pad = (m.gutter() / 2.0).max(m.gap(Space::Sm));
        let inner = Rect::new(area.x + pad, area.y + pad, (area.w - 2.0 * pad).max(0.0), (area.h - 2.0 * pad).max(0.0));
        c.clipped(area, &mut |cc| {
            if self.fullscreen && self.stage == Stage::Live {
                self.draw_live(cc, th, m, area, true);
                return;
            }
            let body = self.draw_header(cc, th, m, inner);
            match self.stage.clone() {
                Stage::Picking => self.draw_picking(cc, th, m, body),
                Stage::Consent => self.draw_consent(cc, th, m, body),
                Stage::Live => self.draw_live(cc, th, m, body, false),
                Stage::Denied(why) => self.draw_message(cc, th, m, body, "Not shown", &why, true),
                Stage::Closed => self.draw_message(cc, th, m, body, "The view was closed", "Open it again from the list.", true),
            }
        });
    }

    fn button_h(&self, m: &Metrics) -> f32 {
        m.min_target().max(m.size(TypeRole::Label) * 1.6)
    }

    /// The title, the sign and the error line; returns what is left.
    fn draw_header(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, r: Rect) -> Rect {
        let size = m.size(TypeRole::Label);
        let mut y = r.y;
        let live = self.stage == Stage::Live;
        let title = if live && !self.shown.is_empty() { self.shown.iter().map(|s| s.name.clone()).collect::<Vec<_>>().join(", ") } else { "Cameras".to_string() };
        let sign = if self.indicator { "\u{25CF} Live" } else { "" };
        let sign_w = if sign.is_empty() { 0.0 } else { c.measure(size, sign, TextStyle::Bold) + m.gap(Space::Md) };
        let t = fit(c, size, &title, TextStyle::Bold, (r.w - sign_w).max(0.0));
        c.text(r.x, y, size, th.text, &t, TextStyle::Bold);
        if !sign.is_empty() {
            c.text(r.right() - sign_w + m.gap(Space::Md), y, size, th.err, sign, TextStyle::Bold);
        }
        y += size * 1.5;
        for line in self.error.iter().chain(self.note.iter()).cloned().collect::<Vec<_>>() {
            let col = if self.error.as_deref() == Some(line.as_str()) { th.err } else { th.ok };
            for l in wrap(c, m.size(TypeRole::Small), TextStyle::Regular, &line, r.w) {
                if y + m.size(TypeRole::Small) > r.bottom() {
                    break;
                }
                c.text(r.x, y, m.size(TypeRole::Small), col, &l, TextStyle::Regular);
                y += m.size(TypeRole::Small) * 1.3;
            }
        }
        Rect::new(r.x, y + 2.0, r.w, (r.bottom() - y - 2.0).max(0.0))
    }

    fn draw_picking(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, r: Rect) {
        let size = m.size(TypeRole::Body);
        if self.list.is_empty() {
            let msg = if self.error.is_some() { "No cameras to show." } else { "No cameras that you may watch." };
            c.text(r.x, r.y, size, th.muted, msg, TextStyle::Regular);
            let h = self.button_h(m);
            crate::widgets::button(c, th, m, Rect::new(r.x, r.y + size * 1.6, r.w.min(m.em(10.7)), h), "Refresh", self.base + A_REFRESH, false);
            return;
        }
        let h = self.button_h(m);
        let mut y = r.y;
        let reserve = h + m.gap(Space::Md);
        let list: Vec<CamInfo> = self.list.iter().take(MAX_PICK_ROWS).cloned().collect();
        let mut hidden = 0;
        for (i, cam) in list.iter().enumerate() {
            if y + h > r.bottom() - reserve {
                hidden = list.len() - i;
                break;
            }
            let on = self.picked.contains(&cam.id);
            let label = format!("{} {} ({})", if on { "[x]" } else { "[ ]" }, cam.name, cam.status.word());
            crate::widgets::row(c, th, m, Rect::new(r.x, y, r.w, h), &label, on, self.base + A_PICK + i as u32);
            y += h + m.gap(Space::Xs);
        }
        if hidden > 0 {
            c.text(r.x, (r.bottom() - reserve - size * 1.3).max(y), size * 0.9, th.dim, &format!("{hidden} more not shown; close another view or make the pane taller."), TextStyle::Regular);
        }
        let label = if self.picked.is_empty() { "Show (tick a camera)".to_string() } else { format!("Show {} camera{}", self.picked.len(), if self.picked.len() == 1 { "" } else { "s" }) };
        crate::widgets::button(c, th, m, Rect::new(r.x, (r.bottom() - h).max(y), r.w, h), &label, self.base + A_SHOW, !self.picked.is_empty());
    }

    fn draw_consent(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, r: Rect) {
        let Some(card) = self.card.clone() else {
            c.text(r.x, r.y, m.size(TypeRole::Body), th.muted, "Waiting for the question to be shown...", TextStyle::Regular);
            return;
        };
        let size = m.size(TypeRole::Label);
        let h = self.button_h(m);
        let card_r = Rect::new(r.x, r.y, r.w, (r.h - h - m.gap(Space::Lg)).max(0.0));
        c.rrect(card_r, m.radius(Radius::Md), th.card);
        c.outline(card_r, m.radius(Radius::Md), m.line(Line::Normal) * th.ring, th.border);
        let pad = m.gap(Space::Lg);
        let mut y = card_r.y + pad;
        c.clipped(card_r, &mut |cc| {
            for l in wrap(cc, size, TextStyle::Bold, &card.summary, card_r.w - 2.0 * pad) {
                cc.text(card_r.x + pad, y, size, th.text, &l, TextStyle::Bold);
                y += size * 1.35;
            }
            y += m.gap(Space::Xs);
            for l in wrap(cc, m.size(TypeRole::Small), TextStyle::Regular, &card.detail, card_r.w - 2.0 * pad) {
                cc.text(card_r.x + pad, y, m.size(TypeRole::Small), th.muted, &l, TextStyle::Regular);
                y += m.size(TypeRole::Small) * 1.3;
            }
        });
        let by = (r.bottom() - h).max(card_r.y);
        let half = (r.w - m.gap(Space::Md)) / 2.0;
        crate::widgets::button(c, th, m, Rect::new(r.x, by, half, h), "Allow", self.base + A_ALLOW, true);
        crate::widgets::button(c, th, m, Rect::new(r.x + half + m.gap(Space::Md), by, half, h), "Deny", self.base + A_DENY, false);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_message(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, r: Rect, title: &str, text: &str, back: bool) {
        let size = m.size(TypeRole::Label);
        c.text(r.x, r.y, size, th.text, title, TextStyle::Bold);
        let mut y = r.y + size * 1.5;
        for l in wrap(c, m.size(TypeRole::Body), TextStyle::Regular, text, r.w) {
            c.text(r.x, y, m.size(TypeRole::Body), th.muted, &l, TextStyle::Regular);
            y += m.size(TypeRole::Body) * 1.35;
        }
        if back {
            crate::widgets::button(c, th, m, Rect::new(r.x, y + m.gap(Space::Sm), r.w.min(m.em(13.3)), self.button_h(m)), "Back to the list", self.base + A_RETRY, true);
        }
    }

    /// Lay the toolbar buttons out in rows of the width `w`; returns `(label index, x, row)` and the number of rows.
    fn flow(c: &mut dyn Canvas, m: &Metrics, w: f32, labels: &[&str]) -> (Vec<(usize, f32, f32, usize)>, usize) {
        let size = m.size(TypeRole::Label);
        let (mut x, mut row) = (0.0f32, 0usize);
        let mut out = Vec::new();
        for (i, l) in labels.iter().enumerate() {
            let bw = (c.measure(size, l, TextStyle::Bold) + 2.0 * m.gap(Space::Xl)).max(m.min_target()).min(w);
            if x > 0.0 && x + bw > w {
                x = 0.0;
                row += 1;
            }
            out.push((i, x, bw, row));
            x += bw + m.gap(Space::Sm);
        }
        (out, row + 1)
    }

    fn draw_live(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, r: Rect, full: bool) {
        let h = self.button_h(m);
        let enlarged = self.enlarged().cloned();
        let mut labels: Vec<(&str, u32)> = Vec::new();
        if !full {
            labels.push(("Snapshot", A_SNAP));
            labels.push((if self.enlarged.is_some() { "Full screen" } else { "Enlarge" }, A_FULL));
            if enlarged.as_ref().is_some_and(|e| e.ptz) {
                labels.extend([("Left", A_PTZ), ("Right", A_PTZ + 1), ("Up", A_PTZ + 2), ("Down", A_PTZ + 3), ("Zoom in", A_PTZ + 4), ("Zoom out", A_PTZ + 5), ("Stop", A_PTZ + 6)]);
            }
            labels.push(("Close", A_CLOSE));
        } else {
            labels.push(("Exit full screen", A_FULL));
        }
        let names: Vec<&str> = labels.iter().map(|l| l.0).collect();
        let (placed, rows) = Self::flow(c, m, r.w, &names);
        let bar_h = if full { 0.0 } else { rows as f32 * (h + m.gap(Space::Sm)) };
        let grid = Rect::new(r.x, r.y, r.w, (r.h - bar_h).max(0.0));
        // the pictures first, so the full-screen button lies over them
        let cells = self.cell_rects(grid, m);
        for (i, cell) in cells {
            self.draw_cell(c, th, m, i, cell);
        }
        if let Some(card) = self.card.clone() {
            self.draw_inline_card(c, th, m, grid, &card);
        }
        if full {
            let (_, x, bw, _) = placed[0];
            let _ = x;
            let bw = bw.min((r.w - 2.0 * m.gap(Space::Md)).max(0.0));
            crate::widgets::button(c, th, m, Rect::new((r.right() - bw - m.gap(Space::Md)).max(r.x), r.y + m.gap(Space::Md), bw, h), labels[0].0, self.base + labels[0].1, false);
            return;
        }
        let by = grid.bottom() + m.gap(Space::Sm);
        for (i, x, bw, row) in placed {
            let (label, a) = labels[i];
            crate::widgets::button(c, th, m, Rect::new(r.x + x, by + row as f32 * (h + m.gap(Space::Sm)), bw, h), label, self.base + a, a == A_SNAP);
        }
    }

    fn draw_inline_card(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, r: Rect, card: &Card) {
        let size = m.size(TypeRole::Body);
        let h = self.button_h(m);
        let lines = wrap(c, size, TextStyle::Bold, &card.summary, (r.w - 2.0 * m.gap(Space::Xl)).max(m.gap(Space::Lg)));
        let ch = (lines.len() as f32 * size * 1.35 + h + 2.0 * m.gap(Space::Xl)).min(r.h);
        let cr = Rect::new(r.x + m.gap(Space::Md), r.bottom() - ch - m.gap(Space::Md), (r.w - 2.0 * m.gap(Space::Md)).max(0.0), ch);
        c.rrect(cr, m.radius(Radius::Md), th.card);
        c.outline(cr, m.radius(Radius::Md), m.line(Line::Thick), th.accent);
        let mut y = cr.y + m.gap(Space::Md);
        for l in lines {
            if y + size > cr.bottom() - h - m.gap(Space::Md) {
                break;
            }
            c.text(cr.x + m.gap(Space::Lg), y, size, th.text, &l, TextStyle::Bold);
            y += size * 1.35;
        }
        let half = (cr.w - 2.0 * m.gap(Space::Lg) - m.gap(Space::Md)) / 2.0;
        crate::widgets::button(c, th, m, Rect::new(cr.x + m.gap(Space::Lg), cr.bottom() - h - m.gap(Space::Md), half, h), "Allow", self.base + A_ALLOW, true);
        crate::widgets::button(c, th, m, Rect::new(cr.x + m.gap(Space::Lg) + m.gap(Space::Md) + half, cr.bottom() - h - m.gap(Space::Md), half, h), "Deny", self.base + A_DENY, false);
    }

    /// The rectangle of each visible cell: one fills the area; two stack on a compact pane and sit side by side otherwise; three or four are 2 by 2.
    fn cell_rects(&self, r: Rect, m: &Metrics) -> Vec<(usize, Rect)> {
        let gap = m.gap(Space::Sm);
        if let Some(i) = self.enlarged.filter(|i| *i < self.shown.len()) {
            return vec![(i, r)];
        }
        let n = self.shown.len();
        match n {
            0 => vec![],
            1 => vec![(0, r)],
            2 if m.class == SizeClass::Compact => {
                let h = (r.h - gap) / 2.0;
                vec![(0, Rect::new(r.x, r.y, r.w, h)), (1, Rect::new(r.x, r.y + h + gap, r.w, h))]
            }
            2 => {
                let w = (r.w - gap) / 2.0;
                vec![(0, Rect::new(r.x, r.y, w, r.h)), (1, Rect::new(r.x + w + gap, r.y, w, r.h))]
            }
            _ => {
                let (w, h) = ((r.w - gap) / 2.0, (r.h - gap) / 2.0);
                (0..n).map(|i| (i, Rect::new(r.x + (i % 2) as f32 * (w + gap), r.y + (i / 2) as f32 * (h + gap), w, h))).collect()
            }
        }
    }

    fn draw_cell(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, i: usize, cell: Rect) {
        let Some(cam) = self.shown.get(i).cloned() else { return };
        c.rrect(cell, m.radius(Radius::Sm), th.panel);
        let multi = self.shown.len() > 1;
        let frame = self.frames.get(&cam.id).cloned();
        let overlay = self.overlays.get(&cam.id).cloned();
        let err = self.cell_err.get(&cam.id).cloned();
        let size = m.size(TypeRole::Small);
        let (base, idx) = (self.base, i as u32);
        c.clipped(cell, &mut |cc| {
            if let Some(img) = &frame {
                let pic = contain(cell, img.width, img.height);
                cc.image(pic, img);
                if let Some(o) = &overlay {
                    Self::draw_overlay(cc, th, pic, o);
                }
            } else {
                let sentence = err.clone().unwrap_or_else(|| if cam.detail.is_empty() { cam.status.word().to_string() } else { format!("{}: {}", cam.status.word(), cam.detail) });
                let mut y = cell.y + (cell.h / 2.0 - size * 1.2).max(m.gap(Space::Xs));
                for l in wrap(cc, size, TextStyle::Regular, &sentence, (cell.w - 2.0 * m.gap(Space::Md)).max(m.gap(Space::Lg))).into_iter().take(4) {
                    let w = cc.measure(size, &l, TextStyle::Regular);
                    cc.text(cell.x + ((cell.w - w) / 2.0).max(m.gap(Space::Xs)), y, size, th.muted, &l, TextStyle::Regular);
                    y += size * 1.3;
                }
            }
            // the name strip and the marks
            let strip = Rect::new(cell.x, cell.bottom() - size * 1.8, cell.w, size * 1.8);
            cc.rect(strip, th.panel);
            let mut label = cam.name.clone();
            if cam.motion_watch {
                label.push_str(" (motion watch)");
            }
            let rec_w = if cam.recording { cc.measure(size, "REC", TextStyle::Bold) + m.gap(Space::Lg) } else { 0.0 };
            let t = fit(cc, size, &label, TextStyle::Regular, (strip.w - 2.0 * m.gap(Space::Lg) - rec_w).max(0.0));
            cc.text(strip.x + m.gap(Space::Sm), strip.y + size * 0.25, size, th.text, &t, TextStyle::Regular);
            if cam.recording {
                cc.text(strip.right() - rec_w, strip.y + size * 0.25, size, th.err, "REC", TextStyle::Bold);
            }
            let name = format!("{}, {}{}", cam.name, cam.status.word(), if cam.recording { ", recording" } else { "" });
            if multi {
                cc.hit(cell, Hit { action: base + A_CELL + idx, name: format!("{name}. Enlarge"), role: Role::Button, ..Default::default() });
            } else {
                cc.hit(cell, Hit { action: base + A_CELL + idx, name, role: Role::Image, ..Default::default() });
            }
        });
    }

    fn draw_overlay(c: &mut dyn Canvas, th: &Theme, pic: Rect, o: &Overlay) {
        let at = |p: (f32, f32)| (pic.x + p.0.clamp(0.0, 1.0) * pic.w, pic.y + p.1.clamp(0.0, 1.0) * pic.h);
        for z in &o.zones {
            let col = if z.mask { th.magenta } else if z.include { th.cyan } else { th.amber };
            for (k, p) in z.points.iter().enumerate() {
                let (a, b) = (at(*p), at(z.points[(k + 1) % z.points.len()]));
                c.line(a.0, a.1, b.0, b.1, 1.5, col);
            }
        }
        for mk in &o.marks {
            let (l, t) = at((mk.bbox[0], mk.bbox[1]));
            let (r, b) = at((mk.bbox[2], mk.bbox[3]));
            c.outline(Rect::new(l, t, (r - l).max(1.0), (b - t).max(1.0)), 0.0, if mk.active { 2.0 } else { 1.0 }, th.err);
        }
    }
}
