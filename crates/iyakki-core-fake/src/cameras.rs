//! The fake camera views: scripted cameras with scripted frames, no network, no device, nothing written. It follows the same rules as the
//! real core (the limits and sentences of `iyakki_core_api::cameras`, a camera is visible to its owner and the people on its list only, a
//! view starts nothing until the person allows its card, a snapshot asks every time, the first move of a camera asks) so a test of an SDK
//! app against it means the same thing as against the device. It records `pulled` (how many frames it handed out per camera), the audit
//! lines and the snapshots it was asked to save; it never writes a file, so a test can prove that a view writes nothing.

use iyakki_core_api::cameras::{SWITCHED_OFF, check_spec, clean_name, view_card, CamAction, CamEvent, CamFrame, CamStatus, CameraInfo, Cameras, FrameFormat, MotionMark, Overlay, OverlayShape, PtzCommand, SourceKind, ViewInfo, ViewPhase, ViewSpec, ViewState, MAX_MARKS, MAX_QUEUED_EVENTS, MAX_SHAPES, MAX_VIEWS_PER_APP, MAX_VIEWS_PER_PERSON, NOT_YOURS};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

/// A camera the test adds.
#[derive(Clone, Debug)]
pub struct FakeCam {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub allowed: Vec<String>,
    pub source: SourceKind,
    pub ptz: bool,
    pub status: CamStatus,
    pub detail: String,
    pub recording: bool,
    pub motion_watch: bool,
}

impl FakeCam {
    pub fn new(id: &str, name: &str, owner: &str) -> FakeCam {
        FakeCam { id: id.into(), name: name.into(), owner: owner.into(), allowed: Vec::new(), source: SourceKind::Network, ptz: false, status: CamStatus::Live, detail: String::new(), recording: false, motion_watch: false }
    }
    pub fn allowing(mut self, people: &[&str]) -> FakeCam {
        self.allowed = people.iter().map(|p| p.to_string()).collect();
        self
    }
    pub fn with_ptz(mut self) -> FakeCam {
        self.ptz = true;
        self
    }
    pub fn recording(mut self) -> FakeCam {
        self.recording = true;
        self
    }

    fn visible_to(&self, user: &str) -> bool {
        self.owner == user || self.allowed.iter().any(|a| a == user)
    }
}

struct Pic {
    seq: u64,
    w: u32,
    h: u32,
    rgba: Arc<Vec<u8>>,
    jpeg: Arc<Vec<u8>>,
}

struct Cam {
    spec: FakeCam,
    pic: Option<Pic>,
    seq: u64,
    zones: Vec<OverlayShape>,
    marks: Vec<MotionMark>,
    ptz_log: Vec<PtzCommand>,
}

enum Pending {
    View,
    Snapshot(String),
    Ptz(String, PtzCommand),
}

struct View {
    user: String,
    app: String,
    cameras: Vec<String>,
    phase: ViewPhase,
    visible: bool,
    out: VecDeque<CamEvent>,
    pending: BTreeMap<String, Pending>,
    moved: Vec<String>,
}

#[derive(Default)]
struct State {
    cams: BTreeMap<String, Cam>,
    views: BTreeMap<String, View>,
    audit: Vec<String>,
    pulled: BTreeMap<String, u64>,
    snapshots: Vec<(String, String)>,
    next: u32,
    no_screen: bool,
    snapshot_fails: Option<String>,
    off: Vec<String>,
}

#[derive(Default)]
pub struct FakeCameras {
    st: Mutex<State>,
}

fn lock(m: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl FakeCameras {
    pub fn new() -> FakeCameras {
        FakeCameras::default()
    }

    pub fn add(&self, cam: FakeCam) {
        let id = cam.id.clone();
        lock(&self.st).cams.insert(id, Cam { spec: cam, pic: None, seq: 0, zones: Vec::new(), marks: Vec::new(), ptz_log: Vec::new() });
    }

    /// The camera shows a new picture (RGBA, width times height times 4 bytes); the sequence number grows. A fake JPEG (the marker bytes
    /// and the size) is made for the other format. Returns the new sequence number.
    pub fn push_frame(&self, camera: &str, w: u32, h: u32, rgba: Vec<u8>) -> u64 {
        assert_eq!(rgba.len(), (w * h * 4) as usize, "a frame is width x height x 4 bytes");
        let mut s = lock(&self.st);
        let c = s.cams.get_mut(camera).expect("a camera that was added");
        c.seq += 1;
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
        jpeg.extend_from_slice(&w.to_be_bytes());
        jpeg.extend_from_slice(&h.to_be_bytes());
        jpeg.push(rgba.first().copied().unwrap_or(0));
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        c.pic = Some(Pic { seq: c.seq, w, h, rgba: Arc::new(rgba), jpeg: Arc::new(jpeg) });
        c.seq
    }

    /// A flat picture of one colour.
    pub fn push_solid(&self, camera: &str, w: u32, h: u32, rgb: [u8; 3]) -> u64 {
        self.push_frame(camera, w, h, rgb.iter().copied().chain([255]).cycle().take((w * h * 4) as usize).collect())
    }

    pub fn set_status(&self, camera: &str, status: CamStatus, detail: &str) {
        let mut s = lock(&self.st);
        if let Some(c) = s.cams.get_mut(camera) {
            c.spec.status = status;
            c.spec.detail = detail.into();
        }
        let ids: Vec<String> = s.views.iter().filter(|(_, v)| v.cameras.iter().any(|c| c == camera) && v.phase == ViewPhase::Open).map(|(k, _)| k.clone()).collect();
        for id in ids {
            let v = s.views.get_mut(&id).unwrap();
            if v.out.len() < MAX_QUEUED_EVENTS {
                v.out.push_back(CamEvent::Status { camera: camera.into(), status, detail: detail.into() });
            }
        }
    }

    /// The zones and masks of the owner's motion detector, as the overlay shows them to the owner.
    pub fn set_zones(&self, camera: &str, zones: Vec<OverlayShape>) {
        if let Some(c) = lock(&self.st).cams.get_mut(camera) {
            c.zones = zones;
        }
    }

    /// The detector found motion: a mark for the overlay, and an event for every open, allowed view of the camera's owner.
    pub fn push_motion(&self, camera: &str, mark: MotionMark) {
        let mut s = lock(&self.st);
        let Some(c) = s.cams.get_mut(camera) else { return };
        c.marks.push(mark.clone());
        let owner = c.spec.owner.clone();
        for v in s.views.values_mut() {
            if v.user == owner && v.phase == ViewPhase::Open && v.cameras.iter().any(|x| x == camera) && v.out.len() < MAX_QUEUED_EVENTS {
                v.out.push_back(CamEvent::Motion { camera: camera.into(), mark: mark.clone() });
            }
        }
    }

    /// The machine cannot show a card (no screen, a remote session): every card is answered by the system with no.
    pub fn no_screen(&self, on: bool) {
        lock(&self.st).no_screen = on;
    }

    /// The next saved snapshot fails with this sentence.
    pub fn fail_snapshots(&self, why: Option<&str>) {
        lock(&self.st).snapshot_fails = why.map(str::to_string);
    }

    /// The audit lines the core wrote (names only).
    pub fn audit(&self) -> Vec<String> {
        lock(&self.st).audit.clone()
    }

    /// How many pictures were handed out for a camera, over all the views.
    pub fn pulled(&self, camera: &str) -> u64 {
        lock(&self.st).pulled.get(camera).copied().unwrap_or(0)
    }

    /// `(person, camera id)` for every snapshot the core was allowed to save. Nothing is written: this is the whole effect.
    pub fn snapshots(&self) -> Vec<(String, String)> {
        lock(&self.st).snapshots.clone()
    }

    /// The commands a camera received, in order.
    pub fn ptz_log(&self, camera: &str) -> Vec<PtzCommand> {
        lock(&self.st).cams.get(camera).map(|c| c.ptz_log.clone()).unwrap_or_default()
    }

    /// The names of the cameras shown right now to anyone (views that are allowed, open and visible): what the machine's sign reports.
    pub fn indicator_names(&self) -> Vec<String> {
        let s = lock(&self.st);
        let mut v: Vec<String> = s.views.values().filter(|v| v.phase == ViewPhase::Open && v.visible).flat_map(|v| v.cameras.iter().filter_map(|c| s.cams.get(c).map(|c| c.spec.name.clone()))).collect();
        v.sort();
        v.dedup();
        v
    }
}

fn info(c: &Cam) -> CameraInfo {
    CameraInfo { id: c.spec.id.clone(), name: clean_name(&c.spec.name), source: c.spec.source, status: c.spec.status, detail: clean_name(&c.spec.detail), ptz: c.spec.ptz, recording: c.spec.recording, motion_watch: c.spec.motion_watch, size: c.pic.as_ref().map(|p| (p.w, p.h)).unwrap_or((0, 0)) }
}

fn mine<'a>(s: &'a mut State, user: &str, view: &str) -> Result<&'a mut View, String> {
    match s.views.get_mut(view) {
        Some(v) if v.user == user && v.phase != ViewPhase::Closed => Ok(v),
        _ => Err("no such camera view".into()),
    }
}

impl State {
    fn call_id(&mut self) -> String {
        self.next += 1;
        format!("c{}", self.next)
    }

    fn names(&self, ids: &[String]) -> Vec<String> {
        ids.iter().filter_map(|i| self.cams.get(i).map(|c| c.spec.name.clone())).collect()
    }
}

impl Cameras for FakeCameras {
    fn list(&self, user: &str, _app: &str) -> Vec<CameraInfo> {
        lock(&self.st).cams.values().filter(|c| c.spec.visible_to(user)).map(info).collect()
    }

    fn open(&self, user: &str, app: &str, spec: &ViewSpec) -> Result<ViewInfo, String> {
        check_spec(app, spec)?;
        let mut s = lock(&self.st);
        if s.off.iter().any(|u| u == user) {
            return Err(SWITCHED_OFF.into());
        }
        let live = |v: &&View| v.phase != ViewPhase::Closed && !matches!(v.phase, ViewPhase::Denied { .. });
        if s.views.values().filter(live).filter(|v| v.user == user).count() >= MAX_VIEWS_PER_PERSON {
            return Err(format!("you have {MAX_VIEWS_PER_PERSON} camera views open; close one first"));
        }
        if s.views.values().filter(live).filter(|v| v.user == user && v.app == app).count() >= MAX_VIEWS_PER_APP {
            return Err(format!("this app may have {MAX_VIEWS_PER_APP} camera views open at once; close one first"));
        }
        for id in &spec.cameras {
            if !s.cams.get(id).is_some_and(|c| c.spec.visible_to(user)) {
                return Err(NOT_YOURS.into());
            }
        }
        let cams: Vec<CameraInfo> = spec.cameras.iter().filter_map(|i| s.cams.get(i)).map(info).collect();
        let id = format!("v{}", s.call_id());
        let call = s.call_id();
        let (summary, detail) = view_card(app, &spec.label, &s.names(&spec.cameras));
        let mut v = View { user: user.into(), app: app.into(), cameras: spec.cameras.clone(), phase: ViewPhase::AwaitingConsent { call: call.clone() }, visible: true, out: VecDeque::new(), pending: BTreeMap::new(), moved: Vec::new() };
        v.pending.insert(call.clone(), Pending::View);
        v.out.push_back(CamEvent::Approval { call: call.clone(), action: CamAction::View, camera: String::new(), summary, detail });
        let names = s.names(&spec.cameras).iter().map(|n| clean_name(n)).collect::<Vec<_>>().join(", ");
        s.audit.push(format!("cameras.open {app}: {names}"));
        let no_screen = s.no_screen;
        s.views.insert(id.clone(), v);
        drop(s);
        if no_screen {
            let _ = self.decide_as(user, &id, &call, false, "system");
        }
        Ok(ViewInfo { id, cameras: cams })
    }

    fn decide(&self, user: &str, view: &str, call: &str, approve: bool) -> Result<(), String> {
        self.decide_as(user, view, call, approve, "user")
    }

    fn state(&self, user: &str, view: &str) -> Result<ViewState, String> {
        let mut s = lock(&self.st);
        let (cams, phase, visible) = {
            let v = mine(&mut s, user, view)?;
            (v.cameras.clone(), v.phase.clone(), v.visible)
        };
        let cameras = cams.iter().filter_map(|i| s.cams.get(i)).map(info).collect();
        Ok(ViewState { indicator: phase == ViewPhase::Open && visible, phase, visible, cameras })
    }

    fn set_visible(&self, user: &str, view: &str, visible: bool) -> Result<(), String> {
        let mut s = lock(&self.st);
        mine(&mut s, user, view)?.visible = visible;
        Ok(())
    }

    fn frame(&self, user: &str, view: &str, camera: &str, after: u64, format: FrameFormat) -> Result<Option<CamFrame>, String> {
        let mut s = lock(&self.st);
        let v = mine(&mut s, user, view)?;
        if !v.cameras.iter().any(|c| c == camera) {
            return Err(NOT_YOURS.into());
        }
        if v.phase != ViewPhase::Open || !v.visible {
            return Ok(None);
        }
        let Some(p) = s.cams.get(camera).and_then(|c| c.pic.as_ref()).filter(|p| p.seq > after) else { return Ok(None) };
        let f = CamFrame { seq: p.seq, width: p.w, height: p.h, format, data: if format == FrameFormat::Rgba { p.rgba.clone() } else { p.jpeg.clone() } };
        *s.pulled.entry(camera.into()).or_default() += 1;
        Ok(Some(f))
    }

    fn overlay(&self, user: &str, view: &str, camera: &str) -> Result<Overlay, String> {
        let mut s = lock(&self.st);
        let v = mine(&mut s, user, view)?;
        if !v.cameras.iter().any(|c| c == camera) {
            return Err(NOT_YOURS.into());
        }
        let open = v.phase == ViewPhase::Open;
        let Some(c) = s.cams.get(camera) else { return Err(NOT_YOURS.into()) };
        if !open || c.spec.owner != user {
            return Ok(Overlay::default());
        }
        Ok(Overlay { zones: c.zones.iter().take(MAX_SHAPES).cloned().collect(), marks: c.marks.iter().rev().take(MAX_MARKS).rev().cloned().collect() })
    }

    fn snapshot(&self, user: &str, view: &str, camera: &str) -> Result<String, String> {
        let mut s = lock(&self.st);
        let v = mine(&mut s, user, view)?;
        if v.phase != ViewPhase::Open {
            return Err("the view is not open: the person has not allowed it".into());
        }
        if !v.cameras.iter().any(|c| c == camera) {
            return Err(NOT_YOURS.into());
        }
        let call = s.call_id();
        let name = s.cams.get(camera).map(|c| clean_name(&c.spec.name)).unwrap_or_default();
        let v = s.views.get_mut(view).expect("view");
        v.pending.insert(call.clone(), Pending::Snapshot(camera.into()));
        v.out.push_back(CamEvent::Approval { call: call.clone(), action: CamAction::Snapshot, camera: camera.into(), summary: format!("Save one picture of {name} in your Pictures folder"), detail: "One JPEG file. The view saves nothing else.".into() });
        let no_screen = s.no_screen;
        drop(s);
        if no_screen {
            let _ = self.decide_as(user, view, &call, false, "system");
        }
        Ok(call)
    }

    fn ptz(&self, user: &str, view: &str, camera: &str, cmd: PtzCommand) -> Result<Option<String>, String> {
        let cmd = cmd.checked()?;
        let mut s = lock(&self.st);
        let v = mine(&mut s, user, view)?;
        if v.phase != ViewPhase::Open {
            return Err("the view is not open: the person has not allowed it".into());
        }
        if !v.cameras.iter().any(|c| c == camera) {
            return Err(NOT_YOURS.into());
        }
        let moved = v.moved.iter().any(|c| c == camera);
        let Some(c) = s.cams.get_mut(camera) else { return Err(NOT_YOURS.into()) };
        if !c.spec.ptz {
            return Err("this camera has no pan, tilt or zoom control".into());
        }
        if moved {
            c.ptz_log.push(cmd);
            return Ok(None);
        }
        let name = clean_name(&c.spec.name);
        let call = s.call_id();
        let v = s.views.get_mut(view).expect("view");
        v.pending.insert(call.clone(), Pending::Ptz(camera.into(), cmd));
        v.out.push_back(CamEvent::Approval { call: call.clone(), action: CamAction::Ptz, camera: camera.into(), summary: format!("Let this view move the camera {name}"), detail: "Pan, tilt and zoom for as long as this view is open. Nothing is recorded.".into() });
        let no_screen = s.no_screen;
        drop(s);
        if no_screen {
            let _ = self.decide_as(user, view, &call, false, "system");
        }
        Ok(Some(call))
    }

    fn events(&self, user: &str, view: &str) -> Vec<CamEvent> {
        let mut s = lock(&self.st);
        match s.views.get_mut(view) {
            Some(v) if v.user == user => v.out.drain(..).collect(),
            _ => Vec::new(),
        }
    }

    fn enabled(&self, user: &str) -> bool {
        !lock(&self.st).off.iter().any(|u| u == user)
    }

    fn set_enabled(&self, user: &str, on: bool) -> Result<(), String> {
        let ids: Vec<String> = {
            let mut s = lock(&self.st);
            s.off.retain(|u| u != user);
            if on {
                s.audit.push(format!("cameras.switch {user} on"));
                return Ok(());
            }
            s.off.push(user.to_string());
            s.audit.push(format!("cameras.switch {user} off"));
            s.views.iter().filter(|(_, v)| v.user == user).map(|(k, _)| k.clone()).collect()
        };
        for id in ids {
            self.close(user, &id);
        }
        Ok(())
    }

    fn indicator_names(&self) -> Vec<String> {
        FakeCameras::indicator_names(self)
    }

    fn open_views(&self, user: &str) -> usize {
        lock(&self.st).views.values().filter(|v| v.user == user && !matches!(v.phase, ViewPhase::Closed | ViewPhase::Denied { .. })).count()
    }

    fn close(&self, user: &str, view: &str) {
        let mut s = lock(&self.st);
        let Some(v) = s.views.get_mut(view).filter(|v| v.user == user && v.phase != ViewPhase::Closed) else { return };
        let app = v.app.clone();
        v.pending.clear();
        v.phase = ViewPhase::Closed;
        v.out.clear();
        s.audit.push(format!("cameras.close {app}"));
    }
}

impl FakeCameras {
    fn decide_as(&self, user: &str, view: &str, call: &str, approve: bool, by: &str) -> Result<(), String> {
        let mut s = lock(&self.st);
        let v = mine(&mut s, user, view)?;
        let Some(p) = v.pending.remove(call) else { return Err("there is no such card waiting".into()) };
        let app = v.app.clone();
        let (action, camera) = match &p {
            Pending::View => (CamAction::View, String::new()),
            Pending::Snapshot(c) => (CamAction::Snapshot, c.clone()),
            Pending::Ptz(c, _) => (CamAction::Ptz, c.clone()),
        };
        v.out.push_back(CamEvent::Decision { call: call.into(), action, approved: approve, by: by.into() });
        let cname = s.cams.get(&camera).map(|c| clean_name(&c.spec.name)).unwrap_or_default();
        let fail = s.snapshot_fails.clone();
        match p {
            Pending::View => {
                s.audit.push(format!("cameras.consent {app} {}", if approve { "allowed" } else { "denied" }));
                let v = s.views.get_mut(view).expect("view");
                v.phase = if approve { ViewPhase::Open } else { ViewPhase::Denied { reason: if by == "system" { "there is no screen to ask on".into() } else { "you said no".into() } } };
            }
            Pending::Snapshot(c) => {
                if approve {
                    match fail {
                        Some(e) => s.views.get_mut(view).expect("view").out.push_back(CamEvent::Failed { camera: c, action: CamAction::Snapshot, error: e }),
                        None => {
                            s.snapshots.push((user.into(), c.clone()));
                            s.audit.push(format!("cameras.snapshot {app}: {cname}"));
                            s.views.get_mut(view).expect("view").out.push_back(CamEvent::Snapshot { camera: c, file: "camera-snapshot.jpg".into(), folder: "Pictures".into() });
                        }
                    }
                }
            }
            Pending::Ptz(c, cmd) => {
                if approve {
                    s.audit.push(format!("cameras.ptz {app}: {cname}"));
                    let v = s.views.get_mut(view).expect("view");
                    v.moved.push(c.clone());
                    if let Some(cam) = s.cams.get_mut(&c) {
                        cam.ptz_log.push(cmd);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(cams: &[&str]) -> ViewSpec {
        ViewSpec { cameras: cams.iter().map(|s| s.to_string()).collect(), ..ViewSpec::default() }
    }

    fn rig() -> FakeCameras {
        let f = FakeCameras::new();
        f.add(FakeCam::new("front", "Front door", "amy").allowing(&["bob"]).with_ptz());
        f.add(FakeCam::new("yard", "Yard", "amy"));
        f.push_solid("front", 4, 2, [10, 20, 30]);
        f
    }

    #[test]
    fn a_person_sees_only_the_cameras_they_own_or_are_listed_on_and_cannot_open_the_others() {
        let f = rig();
        assert_eq!(f.list("amy", "app").len(), 2);
        assert_eq!(f.list("bob", "app").iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["front"]);
        assert!(f.list("eve", "app").is_empty());
        assert_eq!(f.open("bob", "app", &spec(&["yard"])).unwrap_err(), NOT_YOURS);
        assert_eq!(f.open("bob", "app", &spec(&["nothing"])).unwrap_err(), NOT_YOURS, "the same words for a camera that is not there");
    }

    #[test]
    fn nothing_is_pulled_until_the_card_is_allowed_and_a_denied_card_shows_nothing() {
        let f = rig();
        let v = f.open("amy", "app", &spec(&["front"])).unwrap();
        assert!(matches!(f.state("amy", &v.id).unwrap().phase, ViewPhase::AwaitingConsent { .. }));
        assert!(f.frame("amy", &v.id, "front", 0, FrameFormat::Rgba).unwrap().is_none());
        let ev = f.events("amy", &v.id);
        let CamEvent::Approval { call, action, summary, .. } = &ev[0] else { panic!("{ev:?}") };
        assert_eq!(*action, CamAction::View);
        assert!(summary.contains("Front door"));
        f.decide("amy", &v.id, call, false).unwrap();
        assert!(matches!(f.state("amy", &v.id).unwrap().phase, ViewPhase::Denied { .. }));
        assert!(!f.state("amy", &v.id).unwrap().indicator);
        assert!(f.frame("amy", &v.id, "front", 0, FrameFormat::Rgba).unwrap().is_none());
        assert_eq!(f.pulled("front"), 0);
        assert!(f.audit().contains(&"cameras.consent app denied".to_string()));
    }

    #[test]
    fn an_allowed_view_hands_out_the_newest_picture_only_and_hidden_views_pull_nothing() {
        let f = rig();
        let v = f.open("amy", "app", &spec(&["front"])).unwrap();
        let CamEvent::Approval { call, .. } = f.events("amy", &v.id).remove(0) else { panic!() };
        f.decide("amy", &v.id, &call, true).unwrap();
        assert!(f.state("amy", &v.id).unwrap().indicator, "the sign is on while the view is open and visible");
        let a = f.frame("amy", &v.id, "front", 0, FrameFormat::Rgba).unwrap().unwrap();
        assert_eq!((a.width, a.height, a.data.len()), (4, 2, 32));
        assert!(f.frame("amy", &v.id, "front", a.seq, FrameFormat::Rgba).unwrap().is_none(), "nothing new");
        f.push_solid("front", 4, 2, [1, 2, 3]);
        f.push_solid("front", 4, 2, [4, 5, 6]);
        let b = f.frame("amy", &v.id, "front", a.seq, FrameFormat::Jpeg).unwrap().unwrap();
        assert!(b.seq > a.seq && b.data.starts_with(&[0xFF, 0xD8]), "the newest picture, as a JPEG");
        f.set_visible("amy", &v.id, false).unwrap();
        assert!(!f.state("amy", &v.id).unwrap().indicator);
        f.push_solid("front", 4, 2, [7, 8, 9]);
        let pulled = f.pulled("front");
        assert!(f.frame("amy", &v.id, "front", 0, FrameFormat::Rgba).unwrap().is_none());
        assert_eq!(f.pulled("front"), pulled, "hidden: nothing pulled");
        f.close("amy", &v.id);
        assert!(f.state("amy", &v.id).is_err() && f.frame("amy", &v.id, "front", 0, FrameFormat::Rgba).is_err());
    }

    #[test]
    fn limits_per_person_and_app_are_sentences_and_another_persons_view_is_unknown() {
        let f = rig();
        f.add(FakeCam::new("c3", "Three", "amy"));
        f.open("amy", "app", &spec(&["front"])).unwrap();
        f.open("amy", "app", &spec(&["yard"])).unwrap();
        assert!(f.open("amy", "app", &spec(&["c3"])).unwrap_err().contains("this app may have 2"));
        f.open("amy", "other", &spec(&["c3"])).unwrap();
        f.open("amy", "third", &spec(&["c3"])).unwrap();
        assert!(f.open("amy", "fourth", &spec(&["c3"])).unwrap_err().contains("4 camera views"));
        let v = f.open("bob", "app", &spec(&["front"])).unwrap();
        assert!(f.state("amy", &v.id).is_err(), "bob's view is unknown to amy");
        assert!(f.events("amy", &v.id).is_empty());
    }

    #[test]
    fn a_snapshot_asks_every_time_and_a_move_asks_once_per_view_and_camera() {
        let f = rig();
        let v = f.open("amy", "app", &spec(&["front", "yard"])).unwrap();
        let CamEvent::Approval { call, .. } = f.events("amy", &v.id).remove(0) else { panic!() };
        f.decide("amy", &v.id, &call, true).unwrap();
        f.events("amy", &v.id);
        let s1 = f.snapshot("amy", &v.id, "front").unwrap();
        assert!(f.snapshots().is_empty(), "nothing before the card is allowed");
        f.decide("amy", &v.id, &s1, true).unwrap();
        let s2 = f.snapshot("amy", &v.id, "front").unwrap();
        assert_ne!(s1, s2);
        f.decide("amy", &v.id, &s2, false).unwrap();
        assert_eq!(f.snapshots().len(), 1, "one saved, one denied");
        let ev = f.events("amy", &v.id);
        assert!(ev.iter().any(|e| matches!(e, CamEvent::Snapshot { folder, .. } if folder == "Pictures")));
        assert!(f.ptz("amy", &v.id, "yard", PtzCommand::Stop).unwrap_err().contains("no pan, tilt or zoom"));
        let call = f.ptz("amy", &v.id, "front", PtzCommand::Move { pan: 0.5, tilt: 0.0, zoom: 0.0 }).unwrap().expect("the first move asks");
        assert!(f.ptz_log("front").is_empty(), "the camera has not moved yet");
        f.decide("amy", &v.id, &call, true).unwrap();
        assert_eq!(f.ptz_log("front").len(), 1);
        assert_eq!(f.ptz("amy", &v.id, "front", PtzCommand::Stop).unwrap(), None, "the next move runs at once");
        assert_eq!(f.ptz_log("front").len(), 2);
        assert!(f.audit().iter().all(|l| !l.contains("rtsp") && !l.contains("http")), "{:?}", f.audit());
    }

    #[test]
    fn switching_camera_views_off_closes_the_persons_views_and_refuses_new_ones_in_words() {
        let f = rig();
        assert!(f.enabled("amy") && f.open_views("amy") == 0);
        let v = f.open("amy", "app", &spec(&["front"])).unwrap();
        f.open("bob", "app", &spec(&["front"])).unwrap();
        assert_eq!((f.open_views("amy"), f.open_views("bob")), (1, 1));
        f.set_enabled("amy", false).unwrap();
        assert!(!f.enabled("amy") && f.enabled("bob"), "the switch is the person's");
        assert_eq!((f.open_views("amy"), f.open_views("bob")), (0, 1));
        assert!(f.state("amy", &v.id).is_err());
        assert_eq!(f.open("amy", "app", &spec(&["front"])).unwrap_err(), iyakki_core_api::cameras::SWITCHED_OFF);
        f.set_enabled("amy", true).unwrap();
        assert!(f.open("amy", "app", &spec(&["front"])).is_ok());
    }

    #[test]
    fn no_screen_means_the_system_says_no_and_motion_marks_reach_the_owner_only() {
        let f = rig();
        f.no_screen(true);
        let v = f.open("amy", "app", &spec(&["front"])).unwrap();
        assert!(matches!(f.state("amy", &v.id).unwrap().phase, ViewPhase::Denied { .. }));
        assert!(f.events("amy", &v.id).iter().any(|e| matches!(e, CamEvent::Decision { by, approved: false, .. } if by == "system")));
        f.no_screen(false);
        let a = f.open("amy", "app", &spec(&["front"])).unwrap();
        let b = f.open("bob", "app", &spec(&["front"])).unwrap();
        for (u, v) in [("amy", &a.id), ("bob", &b.id)] {
            let CamEvent::Approval { call, .. } = f.events(u, v).remove(0) else { panic!() };
            f.decide(u, v, &call, true).unwrap();
        }
        f.set_zones("front", vec![OverlayShape { name: "door".into(), include: true, mask: false, points: vec![(0.1, 0.1), (0.9, 0.1), (0.5, 0.9)] }]);
        f.push_motion("front", MotionMark { id: 1, at_ms: 5, zone: "door".into(), score: 40, bbox: [0.1, 0.1, 0.5, 0.5], active: true });
        assert!(f.events("amy", &a.id).iter().any(|e| matches!(e, CamEvent::Motion { .. })));
        assert!(!f.events("bob", &b.id).iter().any(|e| matches!(e, CamEvent::Motion { .. })), "the owner's detector is the owner's");
        assert_eq!(f.overlay("amy", &a.id, "front").unwrap().zones.len(), 1);
        assert_eq!(f.overlay("bob", &b.id, "front").unwrap(), Overlay::default());
        assert_eq!(f.indicator_names(), vec!["Front door".to_string()]);
    }
}
