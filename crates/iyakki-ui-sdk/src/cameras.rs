//! The cameras port: what an SDK app is handed so that it can embed live camera pictures (see [`crate::cameraview::CameraView`]). Like the
//! storage and assistant ports it carries no device access: the host gives the app a port and the port is the core's. One port is one person
//! and one app (the host binds them), so no call names either; one port holds at most one view.
//!
//! The host-side meaning, in one paragraph: the core lists the cameras this person may watch (and no others), opens a view only after the
//! person allowed its card, hands out only the newest picture of each camera, writes nothing for a view, saves a snapshot only into the
//! person's own folder after a card, moves a camera only after a card, and writes the audit line with names only. The port types below are
//! the SDK's own copies of the core's (`iyakki_core_api::cameras`), so this crate depends on no other IYAKKI crate; the host maps them.
//! [`MemCameras`] is the in-memory port for tests and the browser preview, with the same rules.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::canvas::{Image, ImageFormat};

/// The sentence for a host that has no cameras.
pub const UNAVAILABLE: &str = "Cameras are not available here.";
/// The most cameras in one view.
pub const MAX_CELLS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CamStatus {
    Connecting,
    Live,
    Lost,
    NeedsAddon,
    Busy,
    NeedsPassword,
    Dark,
}

impl CamStatus {
    pub fn word(self) -> &'static str {
        match self {
            CamStatus::Connecting => "Connecting",
            CamStatus::Live => "Live",
            CamStatus::Lost => "Connection lost",
            CamStatus::NeedsAddon => "Needs the video add-on",
            CamStatus::Busy => "In use by another app",
            CamStatus::NeedsPassword => "Needs its password",
            CamStatus::Dark => "The picture is black",
        }
    }
}

/// One camera as the person may see it. `name` and `detail` are device text: shown, never trusted.
#[derive(Clone, Debug, PartialEq)]
pub struct CamInfo {
    pub id: String,
    pub name: String,
    pub status: CamStatus,
    pub detail: String,
    pub ptz: bool,
    pub recording: bool,
    pub motion_watch: bool,
    pub size: (u32, u32),
}

impl CamInfo {
    pub fn new(id: &str, name: &str) -> CamInfo {
        CamInfo { id: id.into(), name: name.into(), status: CamStatus::Connecting, detail: String::new(), ptz: false, recording: false, motion_watch: false, size: (0, 0) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CamAction {
    View,
    Snapshot,
    Ptz,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    AwaitingConsent,
    Open,
    Denied(String),
    Closed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewState {
    pub phase: Phase,
    /// Pictures are being pulled now: the view shows its sign while this is true.
    pub indicator: bool,
    pub cameras: Vec<CamInfo>,
}

/// A zone where motion counts (`include`) or does not, or a privacy mask; points are 0.0 to 1.0 across the picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub name: String,
    pub include: bool,
    pub mask: bool,
    pub points: Vec<(f32, f32)>,
}

/// A motion event: where (left, top, right, bottom, 0.0 to 1.0) and how strong.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub id: u64,
    pub at_ms: u64,
    pub zone: String,
    pub score: u8,
    pub bbox: [f32; 4],
    pub active: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overlay {
    pub zones: Vec<Shape>,
    pub marks: Vec<Mark>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ptz {
    Move { pan: f32, tilt: f32, zoom: f32 },
    /// Go to a place: pan and tilt -1.0 to 1.0 across the camera's range, zoom 0.0 (wide) to 1.0 (closest).
    Absolute { pan: f32, tilt: f32, zoom: f32 },
    Stop,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CamEvent {
    Approval { call: String, action: CamAction, camera: String, summary: String, detail: String },
    Decision { call: String, action: CamAction, approved: bool, by: String },
    Motion { camera: String, mark: Mark },
    Snapshot { camera: String, file: String, folder: String },
    Failed { camera: String, action: CamAction, error: String },
    Status { camera: String, status: CamStatus, detail: String },
}

pub type CamResult<T> = Result<T, String>;

/// The cameras of this host for one person and one app. A refusal is a sentence the view shows.
pub trait CameraPort {
    /// The cameras the person may watch. Free: nothing starts.
    fn list(&mut self) -> CamResult<Vec<CamInfo>> {
        Err(UNAVAILABLE.into())
    }
    /// Make the view. Nothing is pulled until the person allows the card that [`CameraPort::events`] brings.
    fn open(&mut self, _cameras: &[String], _large: bool, _label: &str) -> CamResult<Vec<CamInfo>> {
        Err(UNAVAILABLE.into())
    }
    fn decide(&mut self, _call: &str, _approve: bool) -> CamResult<()> {
        Err(UNAVAILABLE.into())
    }
    fn state(&mut self) -> CamResult<ViewState> {
        Err(UNAVAILABLE.into())
    }
    fn set_visible(&mut self, _visible: bool) -> CamResult<()> {
        Err(UNAVAILABLE.into())
    }
    /// The newest picture of a camera newer than `after`, or `None`.
    fn frame(&mut self, _camera: &str, _after: u64, _format: ImageFormat) -> CamResult<Option<Image>> {
        Err(UNAVAILABLE.into())
    }
    fn overlay(&mut self, _camera: &str) -> CamResult<Overlay> {
        Err(UNAVAILABLE.into())
    }
    /// Ask to save one picture; the answer is the call id of the card.
    fn snapshot(&mut self, _camera: &str) -> CamResult<String> {
        Err(UNAVAILABLE.into())
    }
    /// A move: `Some(call)` when a card was shown for it, `None` when it ran.
    fn ptz(&mut self, _camera: &str, _cmd: Ptz) -> CamResult<Option<String>> {
        Err(UNAVAILABLE.into())
    }
    fn events(&mut self) -> Vec<CamEvent> {
        Vec::new()
    }
    fn close(&mut self) {}
}

/// A host with no cameras: the view says so.
pub struct NoCameras;

impl CameraPort for NoCameras {}

// ------------------------------------------------------------------------------------------------ the in-memory port

struct MemCam {
    info: CamInfo,
    seq: u64,
    pic: Option<(u32, u32, Arc<Vec<u8>>)>,
    zones: Vec<Shape>,
    marks: Vec<Mark>,
    moves: Vec<Ptz>,
}

enum Pend {
    View,
    Snapshot(String),
    Ptz(String, Ptz),
}

struct Mem {
    cams: Vec<MemCam>,
    phase: Option<Phase>,
    shown: Vec<String>,
    visible: bool,
    out: Vec<CamEvent>,
    pend: BTreeMap<String, Pend>,
    moved: Vec<String>,
    next: u32,
    pulled: BTreeMap<String, u64>,
    snapshots: Vec<String>,
    audit: Vec<String>,
    no_screen: bool,
    app: String,
    calls: Vec<String>,
}

/// The in-memory port: scripted cameras and frames, the same rules as the core (consent first, hidden pulls nothing, a snapshot asks every
/// time, the first move asks), every effect recorded. Clone it: one clone goes to the view, the other stays with the test.
#[derive(Clone)]
pub struct MemCameras {
    st: Rc<RefCell<Mem>>,
}

impl MemCameras {
    pub fn new(app: &str) -> MemCameras {
        MemCameras { st: Rc::new(RefCell::new(Mem { cams: Vec::new(), phase: None, shown: Vec::new(), visible: true, out: Vec::new(), pend: BTreeMap::new(), moved: Vec::new(), next: 0, pulled: BTreeMap::new(), snapshots: Vec::new(), audit: Vec::new(), no_screen: false, app: app.into(), calls: Vec::new() })) }
    }

    pub fn add(&self, info: CamInfo) {
        self.st.borrow_mut().cams.push(MemCam { info, seq: 0, pic: None, zones: Vec::new(), marks: Vec::new(), moves: Vec::new() });
    }

    /// A new picture (RGBA); returns its sequence number.
    pub fn push_frame(&self, camera: &str, w: u32, h: u32, rgba: Vec<u8>) -> u64 {
        let mut s = self.st.borrow_mut();
        let c = s.cams.iter_mut().find(|c| c.info.id == camera).expect("a camera that was added");
        c.seq += 1;
        c.info.size = (w, h);
        c.info.status = CamStatus::Live;
        c.pic = Some((w, h, Arc::new(rgba)));
        c.seq
    }

    pub fn push_solid(&self, camera: &str, w: u32, h: u32, rgb: [u8; 3]) -> u64 {
        self.push_frame(camera, w, h, rgb.iter().copied().chain([255]).cycle().take((w * h * 4) as usize).collect())
    }

    pub fn set_status(&self, camera: &str, status: CamStatus, detail: &str) {
        let mut s = self.st.borrow_mut();
        if let Some(c) = s.cams.iter_mut().find(|c| c.info.id == camera) {
            c.info.status = status;
            c.info.detail = detail.into();
        }
        if s.phase == Some(Phase::Open) && s.shown.iter().any(|i| i == camera) {
            s.out.push(CamEvent::Status { camera: camera.into(), status, detail: detail.into() });
        }
    }

    pub fn set_zones(&self, camera: &str, zones: Vec<Shape>) {
        if let Some(c) = self.st.borrow_mut().cams.iter_mut().find(|c| c.info.id == camera) {
            c.zones = zones;
        }
    }

    pub fn push_motion(&self, camera: &str, mark: Mark) {
        let mut s = self.st.borrow_mut();
        if let Some(c) = s.cams.iter_mut().find(|c| c.info.id == camera) {
            c.marks.push(mark.clone());
        }
        if s.phase == Some(Phase::Open) && s.shown.iter().any(|i| i == camera) {
            s.out.push(CamEvent::Motion { camera: camera.into(), mark });
        }
    }

    /// Every card is answered "no" by the system (no screen to ask on).
    pub fn no_screen(&self, on: bool) {
        self.st.borrow_mut().no_screen = on;
    }

    /// Pictures handed out for a camera.
    pub fn pulled(&self, camera: &str) -> u64 {
        self.st.borrow().pulled.get(camera).copied().unwrap_or(0)
    }

    pub fn snapshots(&self) -> Vec<String> {
        self.st.borrow().snapshots.clone()
    }

    pub fn moves(&self, camera: &str) -> Vec<Ptz> {
        self.st.borrow().cams.iter().find(|c| c.info.id == camera).map(|c| c.moves.clone()).unwrap_or_default()
    }

    /// Names only, like the core's lines.
    pub fn audit(&self) -> Vec<String> {
        self.st.borrow().audit.clone()
    }

    /// The names of the port calls made, in order (`open`, `frame front`, `set_visible false`...), to prove what the view asked for.
    pub fn calls(&self) -> Vec<String> {
        self.st.borrow().calls.clone()
    }

    pub fn clear_calls(&self) {
        self.st.borrow_mut().calls.clear();
    }

    fn answer(&self, call: &str, approve: bool, by: &str) -> CamResult<()> {
        let mut s = self.st.borrow_mut();
        let Some(p) = s.pend.remove(call) else { return Err("There is no such card waiting.".into()) };
        let (action, camera) = match &p {
            Pend::View => (CamAction::View, String::new()),
            Pend::Snapshot(c) => (CamAction::Snapshot, c.clone()),
            Pend::Ptz(c, _) => (CamAction::Ptz, c.clone()),
        };
        s.out.push(CamEvent::Decision { call: call.into(), action, approved: approve, by: by.into() });
        let app = s.app.clone();
        let name = s.cams.iter().find(|c| c.info.id == camera).map(|c| c.info.name.clone()).unwrap_or_default();
        match p {
            Pend::View => {
                s.audit.push(format!("cameras.consent {app} {}", if approve { "allowed" } else { "denied" }));
                s.phase = Some(if approve { Phase::Open } else { Phase::Denied(if by == "system" { "There is no screen to ask on.".into() } else { "You said no.".into() }) });
            }
            Pend::Snapshot(c) if approve => {
                s.snapshots.push(c.clone());
                s.audit.push(format!("cameras.snapshot {app}: {name}"));
                s.out.push(CamEvent::Snapshot { camera: c, file: "camera-snapshot.jpg".into(), folder: "Pictures".into() });
            }
            Pend::Ptz(c, cmd) if approve => {
                s.audit.push(format!("cameras.ptz {app}: {name}"));
                s.moved.push(c.clone());
                if let Some(cam) = s.cams.iter_mut().find(|x| x.info.id == c) {
                    cam.moves.push(cmd);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn card(&self, action: CamAction, camera: &str, pend: Pend, summary: String, detail: &str) -> String {
        let (call, no_screen) = {
            let mut s = self.st.borrow_mut();
            s.next += 1;
            let call = format!("c{}", s.next);
            s.pend.insert(call.clone(), pend);
            s.out.push(CamEvent::Approval { call: call.clone(), action, camera: camera.into(), summary, detail: detail.into() });
            (call, s.no_screen)
        };
        if no_screen {
            let _ = self.answer(&call, false, "system");
        }
        call
    }
}

impl CameraPort for MemCameras {
    fn list(&mut self) -> CamResult<Vec<CamInfo>> {
        let mut s = self.st.borrow_mut();
        s.calls.push("list".into());
        Ok(s.cams.iter().map(|c| c.info.clone()).collect())
    }

    fn open(&mut self, cameras: &[String], _large: bool, label: &str) -> CamResult<Vec<CamInfo>> {
        let (infos, summary, app) = {
            let mut s = self.st.borrow_mut();
            s.calls.push("open".into());
            if cameras.is_empty() || cameras.len() > MAX_CELLS {
                return Err(format!("A camera view shows 1 to {MAX_CELLS} cameras."));
            }
            let mut infos = Vec::new();
            for id in cameras {
                infos.push(s.cams.iter().find(|c| c.info.id == *id).map(|c| c.info.clone()).ok_or("No such camera, or you are not on its list of people who may watch.")?);
            }
            s.shown = cameras.to_vec();
            s.phase = Some(Phase::AwaitingConsent);
            let names: Vec<String> = infos.iter().map(|i| i.name.clone()).collect();
            let app = s.app.clone();
            s.audit.push(format!("cameras.open {app}: {}", names.join(", ")));
            let what = if label.is_empty() { app.clone() } else { format!("{app} ({label})") };
            (infos, format!("{what} wants to show {} camera{}: {}", names.len(), if names.len() == 1 { "" } else { "s" }, names.join(", ")), app)
        };
        let _ = app;
        self.card(CamAction::View, "", Pend::View, summary, "Live pictures only. Nothing is recorded or saved by the view. A sign shows while it is open and visible.");
        Ok(infos)
    }

    fn decide(&mut self, call: &str, approve: bool) -> CamResult<()> {
        self.st.borrow_mut().calls.push("decide".into());
        self.answer(call, approve, "user")
    }

    fn state(&mut self) -> CamResult<ViewState> {
        let s = self.st.borrow();
        let phase = s.phase.clone().ok_or("No camera view is open.")?;
        let cameras = s.shown.iter().filter_map(|i| s.cams.iter().find(|c| c.info.id == *i)).map(|c| c.info.clone()).collect();
        Ok(ViewState { indicator: phase == Phase::Open && s.visible, phase, cameras })
    }

    fn set_visible(&mut self, visible: bool) -> CamResult<()> {
        let mut s = self.st.borrow_mut();
        s.calls.push(format!("set_visible {visible}"));
        s.visible = visible;
        Ok(())
    }

    fn frame(&mut self, camera: &str, after: u64, format: ImageFormat) -> CamResult<Option<Image>> {
        let mut s = self.st.borrow_mut();
        s.calls.push(format!("frame {camera}"));
        if !s.shown.iter().any(|i| i == camera) {
            return Err("No such camera, or you are not on its list of people who may watch.".into());
        }
        if s.phase != Some(Phase::Open) || !s.visible {
            return Ok(None);
        }
        let Some(c) = s.cams.iter().find(|c| c.info.id == camera) else { return Ok(None) };
        let Some((w, h, data)) = c.pic.clone().filter(|_| c.seq > after) else { return Ok(None) };
        let seq = c.seq;
        *s.pulled.entry(camera.into()).or_default() += 1;
        // a JPEG for the wire is the marker bytes and the size: the port does not encode
        let data = if format == ImageFormat::Jpeg { Arc::new([&[0xFF, 0xD8][..], &w.to_be_bytes(), &h.to_be_bytes(), &[0xFF, 0xD9]].concat()) } else { data };
        Ok(Some(Image { format, width: w, height: h, seq, data }))
    }

    fn overlay(&mut self, camera: &str) -> CamResult<Overlay> {
        let s = self.st.borrow();
        if s.phase != Some(Phase::Open) {
            return Ok(Overlay::default());
        }
        Ok(s.cams.iter().find(|c| c.info.id == camera).map(|c| Overlay { zones: c.zones.clone(), marks: c.marks.iter().rev().take(16).rev().cloned().collect() }).unwrap_or_default())
    }

    fn snapshot(&mut self, camera: &str) -> CamResult<String> {
        let name = {
            let s = self.st.borrow();
            if s.phase != Some(Phase::Open) {
                return Err("The view is not open: you have not allowed it.".into());
            }
            s.cams.iter().find(|c| c.info.id == camera).map(|c| c.info.name.clone()).ok_or("No such camera, or you are not on its list of people who may watch.")?
        };
        Ok(self.card(CamAction::Snapshot, camera, Pend::Snapshot(camera.into()), format!("Save one picture of {name} in your Pictures folder"), "One JPEG file. The view saves nothing else."))
    }

    fn ptz(&mut self, camera: &str, cmd: Ptz) -> CamResult<Option<String>> {
        let (name, moved) = {
            let mut s = self.st.borrow_mut();
            if s.phase != Some(Phase::Open) {
                return Err("The view is not open: you have not allowed it.".into());
            }
            let moved = s.moved.iter().any(|c| c == camera);
            let c = s.cams.iter_mut().find(|c| c.info.id == camera).ok_or("No such camera, or you are not on its list of people who may watch.")?;
            if !c.info.ptz {
                return Err("This camera has no pan, tilt or zoom control.".into());
            }
            if moved {
                c.moves.push(cmd);
                return Ok(None);
            }
            (c.info.name.clone(), moved)
        };
        let _ = moved;
        Ok(Some(self.card(CamAction::Ptz, camera, Pend::Ptz(camera.into(), cmd), format!("Let this view move the camera {name}"), "Pan, tilt and zoom for as long as this view is open. Nothing is recorded.")))
    }

    fn events(&mut self) -> Vec<CamEvent> {
        std::mem::take(&mut self.st.borrow_mut().out)
    }

    fn close(&mut self) {
        let mut s = self.st.borrow_mut();
        s.calls.push("close".into());
        if s.phase.is_some() {
            let app = s.app.clone();
            s.audit.push(format!("cameras.close {app}"));
        }
        s.phase = None;
        s.shown.clear();
        s.pend.clear();
        s.out.clear();
        s.moved.clear();
    }
}
