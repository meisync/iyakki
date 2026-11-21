//! The device core's embedded camera views (F-SDK-161): `iyakki_core_api::cameras::Cameras` over the machine's camera hub (`cameras::Hub`,
//! F-CAM-02/16), the motion watcher (`camdetect::watch`, read only) and the recorder (an indicator only). One per installation
//! ([`shared`]): the shell builds a few `AgentCore` handles over the same services and a view must be found through any of them.
//!
//! What this adds to the hub, and nothing else:
//!
//! * **Consent.** `open` makes a view that pulls nothing; the person's card (`CamEvent::Approval`) starts it. No standing grant.
//! * **Visibility.** The list holds the cameras the hub lets the person watch (owner, people on the list, this computer's own cameras for an
//!   account that may use them). An administrator who may only manage a camera does not see it here. A camera that is not the person's
//!   is refused with the same sentence as one that does not exist.
//! * **No writes by a view.** Frames are handed on and kept nowhere. A snapshot is a separate card each time and goes through
//!   `Hub::snapshot` (the person's own Pictures folder, the account's write rule, the masks the owner asked for in stored pictures).
//! * **Pan, tilt and zoom** are refused with a sentence: the hub's sources (RTSP over TCP, Motion-JPEG, V4L2) have no control channel
//!   (the in-house ONVIF client `onvif.rs`, F-SDK-167), so `CameraInfo::ptz` is true once a camera was asked and answered that it moves.
//! * **Overlays** are read from the motion watcher for the camera's owner only: zones, masks and the events of the last half minute.
//! * **Audit, by names.** `cameras.open app: Front door, Yard`, `cameras.consent app allowed`, `cameras.snapshot app: Front door`,
//!   `cameras.close app`, through the hub's audit sink (credentials scrubbed there as everywhere).
//! * **Streams** run only while a view is allowed, open and visible *and* asks: every `frame` is a look of the hub (`Hub::view`), the hub
//!   stops a stream 8 s after the last look, so a hidden or closed view lets the camera go (the computer's own camera's light goes out
//!   then). The hub's own pacing (5 / 10 pictures a second) applies.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use iyakki_core_api::cameras::{
    check_spec, clean_name, view_card, CamAction, CamEvent, CamFrame, CamStatus, CameraInfo, Cameras, FrameFormat, FrameSize, MotionMark, Overlay, OverlayShape, PtzCommand, SourceKind, ViewInfo, ViewPhase, ViewSpec, ViewState, MAX_MARKS,
    MAX_QUEUED_EVENTS, MAX_SHAPES, MAX_VIEWS_PER_APP, MAX_VIEWS_PER_PERSON, NOT_YOURS,
};

use crate::camdetect::watch::MotionService;
use crate::camrec::Recorder;
use crate::cameras::{Frame, Hub, Kind, Status, Who};

/// How far back the overlay's motion marks reach, and how often a view asks the detector for new events.
const MARK_WINDOW_MS: u64 = 30_000;
const EVENT_POLL: std::time::Duration = std::time::Duration::from_millis(1000);

type WhoFn = Arc<dyn Fn(&str) -> Who + Send + Sync>;

enum Pending {
    View,
    Snapshot(String),
    Ptz(String, PtzCommand),
}

struct View {
    user: String,
    app: String,
    cameras: Vec<String>,
    large: bool,
    phase: ViewPhase,
    visible: bool,
    out: VecDeque<CamEvent>,
    pending: BTreeMap<String, Pending>,
    last_status: HashMap<String, (CamStatus, String)>,
    /// The highest motion event id handed out per camera, and when the detector was last asked.
    seen_motion: HashMap<String, u64>,
    opened_ms: u64,
    polled: Option<std::time::Instant>,
    /// The cameras this view may already move (the person allowed the first command).
    ptz_ok: std::collections::BTreeSet<String>,
}

#[derive(Default)]
struct State {
    views: BTreeMap<String, View>,
    next: u64,
}

/// What the views stand on: the hub, how a person's rights are worked out, and the optional motion watcher and recorder.
pub struct Parts {
    pub hub: Arc<Hub>,
    pub who_of: WhoFn,
    pub motion: Option<Arc<MotionService>>,
    pub rec: Option<Arc<Recorder>>,
}

/// How a camera is asked whether it can move: camera id, stream address, user name, password. Tests give their own; the real one speaks ONVIF over HTTP.
pub type PtzProbe = Arc<dyn Fn(&str, &str, &str, &str) -> Result<Option<crate::onvif::PtzCamera>, String> + Send + Sync>;
type PtzBook = Arc<Mutex<HashMap<String, Option<Arc<crate::onvif::PtzCamera>>>>>;

pub(crate) fn real_probe(data: PathBuf) -> PtzProbe {
    Arc::new(move |id, addr, user, pass| {
        // the camera answers on one of the usual ONVIF ports; a port that does not answer is skipped, a camera that answers and has nothing to move ends it
        let mut last = Err("the camera did not answer on any of the usual control ports".to_string());
        for (tls, port) in crate::onvif::candidate_endpoints(addr) {
            let Some(url) = crate::onvif::device_url_for(addr, tls, port) else { return Ok(None) };
            // an encrypted control address goes through the camera's pinned certificate (the same card as for its stream)
            let transport = if tls { crate::onvif::HttpTransport::with_tls(data.clone(), id) } else { crate::onvif::HttpTransport::default() };
            let client = crate::onvif::Onvif::new(Box::new(transport), Box::new(crate::onvif::SystemSource), user, pass);
            match crate::onvif::PtzCamera::probe(client, &url) {
                Ok(found) => return Ok(found),
                // a certificate that waits for the person's yes is the answer; the other ports would only repeat it
                Err(e) if tls && e.contains("camera_trust_set") => return Err(e),
                Err(e) => last = Err(e),
            }
        }
        last
    })
}

pub struct AgentCameras {
    /// The cameras that were asked whether they can move: `Some` can, `None` cannot (asked once per run of the computer).
    ptz: PtzBook,
    ptz_probe: Mutex<PtzProbe>,
    /// Whether a list that shows a network camera nobody asked yet starts the question in the background, so the buttons are there at the next
    /// look (on in the running machine, off until a test asks for it), the cameras asked this run, and the questions still running.
    probe_list: std::sync::atomic::AtomicBool,
    tried: Mutex<std::collections::HashSet<String>>,
    probe_jobs: Mutex<Vec<std::thread::JoinHandle<()>>>,
    /// Made on first use: building the hub starts the motion watcher's and the recorder's supervisors, which a machine that never shows an
    /// embedded view has no reason to do at start.
    data: PathBuf,
    parts: OnceLock<Parts>,
    init: Arc<dyn Fn() -> Option<Parts> + Send + Sync>,
    st: Mutex<State>,
}

fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Arc<AgentCameras>>> {
    static R: OnceLock<Mutex<HashMap<PathBuf, Arc<AgentCameras>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// The installation's embedded camera views (made with the first caller's seams, shared afterwards).
pub fn shared(data: &Path, init: Arc<dyn Fn() -> Option<Parts> + Send + Sync>) -> Arc<AgentCameras> {
    lk(registry()).entry(data.to_path_buf()).or_insert_with(|| Arc::new(AgentCameras::lazy(data, init))).clone()
}

/// The installation's views over what the machine already runs: the camera hub, the motion watcher and the recorder that the sign-in
/// started (`resume_recording` in the shell), found when a person first asks. The rights of a person are the account's: their home, the
/// CAMERA app being allowed for this computer's own cameras, the write rule for a snapshot, managing for an administrator.
pub fn from_running(data: &Path, policy_of: Arc<dyn Fn(&str) -> Option<iyakki_sec::policy::Policy> + Send + Sync>) -> Arc<AgentCameras> {
    let d = data.to_path_buf();
    shared(
        data,
        Arc::new(move || {
            let hub = crate::cameras::existing(&d)?;
            let (policy_of, d2) = (policy_of.clone(), d.clone());
            let who_of: WhoFn = Arc::new(move |person: &str| {
                let policy = policy_of(person);
                let known = policy.is_some();
                let policy = policy.unwrap_or_else(|| iyakki_sec::policy::Role::Guest.template());
                let home = crate::scope::file_roots(&policy, person, &d2).home;
                let (p2, u2) = (policy.clone(), person.to_string());
                let may_write: Arc<dyn Fn(&Path) -> bool + Send + Sync> = Arc::new(move |dir: &Path| p2.allows_write(&u2, &dir.display().to_string()));
                Who { user: person.to_string(), manage: known && policy.manage_settings, usb_ok: known && policy.allows_app(iyakki_sec::policy::App::Camera), home, may_write }
            });
            Some(Parts { hub, who_of, motion: crate::camdetect::watch::existing(&d), rec: crate::camrec::existing(&d) })
        }),
    )
}

/// The names of the cameras shown to somebody right now through an embedded view of the installation at `data` (allowed, open, visible):
/// what the machine's sign reports besides the recordings. Empty when no view was ever made.
pub fn viewing_names(data: &Path) -> Vec<String> {
    let c = lk(registry()).get(data).cloned();
    c.map(|c| c.indicator_names()).unwrap_or_default()
}

fn status_of(s: &Status) -> (CamStatus, String) {
    match s {
        Status::Connecting => (CamStatus::Connecting, String::new()),
        Status::Live => (CamStatus::Live, String::new()),
        Status::Lost(m) => (CamStatus::Lost, m.clone()),
        Status::NeedsAddon(m) => (CamStatus::NeedsAddon, m.clone()),
        Status::Busy(m) => (CamStatus::Busy, m.clone()),
        Status::NeedsPassword(m) => (CamStatus::NeedsPassword, m.clone()),
        Status::Dark(m) => (CamStatus::Dark, m.clone()),
    }
}

/// The motion mark of a detector event (the detector's `bbox` is already 0.0 to 1.0).
pub fn mark_of(e: &iyakki_camdetect::events::Event) -> MotionMark {
    MotionMark { id: e.id, at_ms: e.start_ms, zone: clean_name(&e.zone), score: e.score, bbox: e.bbox, active: e.end_ms.is_none() }
}

impl AgentCameras {
    pub fn new(hub: Arc<Hub>, who_of: WhoFn, motion: Option<Arc<MotionService>>, rec: Option<Arc<Recorder>>) -> AgentCameras {
        let parts = OnceLock::new();
        let data = hub.data_dir();
        let _ = parts.set(Parts { hub, who_of, motion, rec });
        AgentCameras { ptz: Default::default(), ptz_probe: Mutex::new(real_probe(data.clone())), probe_list: Default::default(), tried: Default::default(), probe_jobs: Default::default(), data, parts, init: Arc::new(|| None), st: Mutex::new(State::default()) }
    }

    /// The parts are found by `init` when a person first lists or opens; `None` (the hub is not running yet) is asked again next time.
    pub fn lazy(data: &Path, init: Arc<dyn Fn() -> Option<Parts> + Send + Sync>) -> AgentCameras {
        AgentCameras { ptz: Default::default(), ptz_probe: Mutex::new(real_probe(data.to_path_buf())), probe_list: std::sync::atomic::AtomicBool::new(true), tried: Default::default(), probe_jobs: Default::default(), data: data.to_path_buf(), parts: OnceLock::new(), init, st: Mutex::new(State::default()) }
    }

    /// Use another way to ask a camera whether it can move (a test's fake camera).
    pub fn set_ptz_probe(&self, probe: PtzProbe) {
        *lk(&self.ptz_probe) = probe;
    }

    /// Let a list that shows a network camera nobody asked yet ask it in the background (F-SDK-167: the buttons are there at the next look, not
    /// only after the first command). The running machine has it on; a test switches it on with its fake probe.
    pub fn probe_on_list(&self, on: bool) {
        self.probe_list.store(on, std::sync::atomic::Ordering::SeqCst);
    }

    /// Wait for the questions the last lists started (tests; the running machine never waits).
    pub fn wait_probes(&self) {
        let jobs: Vec<_> = lk(&self.probe_jobs).drain(..).collect();
        for j in jobs {
            let _ = j.join();
        }
    }

    /// Start the question for each listed network camera that was never asked this run. One thread each; a camera that does not answer is
    /// not asked again until the next run (a command still asks for itself), and the login is read from the owner's vault here, never kept.
    fn probe_listed(&self, user: &str, listed: &[CameraInfo]) {
        if !self.probe_list.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        for c in listed.iter().filter(|c| c.source == SourceKind::Network && !c.ptz) {
            if lk(&self.ptz).contains_key(&c.id) || !lk(&self.tried).insert(c.id.clone()) {
                continue;
            }
            let Ok((addr, name, pw)) = self.p().hub.control_login(&self.who(user), &c.id) else { continue };
            let (probe, book, id) = (lk(&self.ptz_probe).clone(), self.ptz.clone(), c.id.clone());
            let job = std::thread::Builder::new().name("iyakki-ptz-probe".into()).spawn(move || {
                if let Ok(found) = probe(&id, &addr, &name, &pw) {
                    lk(&book).insert(id, found.map(Arc::new));
                }
            });
            if let Ok(j) = job {
                let mut jobs = lk(&self.probe_jobs);
                jobs.retain(|j| !j.is_finished());
                jobs.push(j);
            }
        }
    }

    /// Ask one network camera whether it can pan, tilt or zoom and remember the answer (a failure is remembered as "cannot": the next run asks again).
    /// Returns the camera when it can.
    pub fn probe_ptz(&self, user: &str, camera: &str) -> Option<Arc<crate::onvif::PtzCamera>> {
        if let Some(known) = lk(&self.ptz).get(camera) {
            return known.clone();
        }
        let found = self.p().hub.control_login(&self.who(user), camera).ok().and_then(|(addr, name, pw)| {
            let probe = lk(&self.ptz_probe).clone();
            probe(camera, &addr, &name, &pw).ok().flatten().map(Arc::new)
        });
        lk(&self.ptz).insert(camera.to_string(), found.clone());
        found
    }

    fn run_ptz(&self, user: &str, view: &str, camera: &str, cmd: PtzCommand) -> Result<(), String> {
        let cam = self.probe_ptz(user, camera).ok_or("this camera has no pan, tilt or zoom control that this system can use")?;
        cam.command(cmd)?;
        // the log says that a view moved a camera, never where to or with what login
        let name = self.name_of(&self.who(user), camera);
        self.audit(user, "cameras.ptz", &format!("{}: {name}", self.app_of(view)));
        Ok(())
    }

    fn switch_path(&self, user: &str) -> Option<PathBuf> {
        let ok = !user.is_empty() && user.len() <= 48 && user.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
        ok.then(|| self.data.join("cameras").join(format!("embed-{user}.json")))
    }

    /// Whether the hub is there. Every call that can make or show something checks it first; a view exists only after it was true.
    fn ready(&self) -> bool {
        if self.parts.get().is_some() {
            return true;
        }
        match (self.init)() {
            Some(p) => {
                let _ = self.parts.set(p);
                true
            }
            None => false,
        }
    }

    fn p(&self) -> &Parts {
        self.parts.get().expect("checked by ready()")
    }

    fn who(&self, user: &str) -> Who {
        (self.p().who_of)(user)
    }

    fn audit(&self, user: &str, event: &str, detail: &str) {
        match self.parts.get() {
            Some(p) => (p.hub.env().audit)(user, event, detail),
            None => log::info!("{event} {user} {detail}"),
        }
    }

    /// The cameras this person may watch (not the ones they may only manage).
    fn watchable(&self, who: &Who) -> Vec<CameraInfo> {
        self.p()
            .hub
            .cameras(who)
            .into_iter()
            .filter(|v| v.kind == Kind::Usb || v.owner == who.user || v.allowed.iter().any(|a| a == &who.user))
            .map(|v| {
                let (status, detail) = status_of(&v.status);
                let motion_watch = self.p().motion.as_ref().is_some_and(|m| m.analysing_ids().iter().any(|i| *i == v.id));
                CameraInfo {
                    recording: self.p().rec.as_ref().is_some_and(|r| r.recording(&v.id)),
                    ptz: lk(&self.ptz).get(&v.id).is_some_and(|p| p.is_some()),
                    id: v.id,
                    name: clean_name(&v.name),
                    source: if v.kind == Kind::Usb { SourceKind::Usb } else { SourceKind::Network },
                    status,
                    detail: clean_name(&iyakki_rtsp::url::scrub(&detail)),
                    motion_watch,
                    size: v.size,
                }
            })
            .collect()
    }

    /// The names of the cameras shown right now (see [`viewing_names`]).
    pub fn indicator_names(&self) -> Vec<String> {
        let s = lk(&self.st);
        let mut names: Vec<String> = Vec::new();
        for v in s.views.values().filter(|v| v.phase == ViewPhase::Open && v.visible) {
            let who = self.who(&v.user);
            let all = self.watchable(&who);
            names.extend(v.cameras.iter().filter_map(|c| all.iter().find(|i| i.id == *c).map(|i| i.name.clone())));
        }
        names.sort();
        names.dedup();
        names
    }

    fn with_view<T>(&self, user: &str, view: &str, f: impl FnOnce(&mut View) -> Result<T, String>) -> Result<T, String> {
        let mut s = lk(&self.st);
        match s.views.get_mut(view) {
            Some(v) if v.user == user && v.phase != ViewPhase::Closed => f(v),
            _ => Err("no such camera view".into()),
        }
    }

    fn push(v: &mut View, e: CamEvent) {
        if v.out.len() < MAX_QUEUED_EVENTS {
            v.out.push_back(e);
        }
    }

    fn ask(&self, user: &str, view: &str, action: CamAction, camera: &str, pending: Pending, summary: String, detail: String) -> Result<String, String> {
        let mut s = lk(&self.st);
        s.next += 1;
        let call = format!("c{}", s.next);
        let v = s.views.get_mut(view).filter(|v| v.user == user).ok_or("no such camera view")?;
        v.pending.insert(call.clone(), pending);
        Self::push(v, CamEvent::Approval { call: call.clone(), action, camera: camera.into(), summary, detail });
        Ok(call)
    }

    fn name_of(&self, who: &Who, id: &str) -> String {
        self.watchable(who).into_iter().find(|c| c.id == id).map(|c| c.name).unwrap_or_default()
    }

    fn do_snapshot(&self, user: &str, view: &str, camera: &str) {
        let who = self.who(user);
        let name = self.name_of(&who, camera);
        let r = self.p().hub.snapshot(&who, camera);
        let ev = match r {
            Ok(p) => {
                self.audit(user, "cameras.snapshot", &format!("{}: {name}", self.app_of(view)));
                CamEvent::Snapshot { camera: camera.into(), file: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), folder: p.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Pictures".into()) }
            }
            Err(e) => CamEvent::Failed { camera: camera.into(), action: CamAction::Snapshot, error: iyakki_rtsp::url::scrub(&e) },
        };
        let _ = self.with_view(user, view, |v| {
            Self::push(v, ev);
            Ok(())
        });
    }

    fn app_of(&self, view: &str) -> String {
        lk(&self.st).views.get(view).map(|v| v.app.clone()).unwrap_or_default()
    }

    fn poll_changes(&self, user: &str, view: &str) {
        let who = self.who(user);
        let all = self.watchable(&who);
        let motion = self.p().motion.clone();
        let _ = self.with_view(user, view, |v| {
            if v.phase != ViewPhase::Open {
                return Ok(());
            }
            for c in v.cameras.clone() {
                let Some(i) = all.iter().find(|i| i.id == c) else { continue };
                let now = (i.status, i.detail.clone());
                if v.last_status.get(&c) != Some(&now) {
                    v.last_status.insert(c.clone(), now.clone());
                    Self::push(v, CamEvent::Status { camera: c.clone(), status: now.0, detail: now.1 });
                }
            }
            if v.polled.is_some_and(|t| t.elapsed() < EVENT_POLL) {
                return Ok(());
            }
            v.polled = Some(std::time::Instant::now());
            let Some(m) = &motion else { return Ok(()) };
            let to = m.now_ms();
            for c in v.cameras.clone() {
                // the detector answers the camera's owner only; anybody else gets an error that is not shown
                let Ok(evs) = m.events(&who, &c, v.opened_ms, to, MAX_MARKS) else { continue };
                let seen = v.seen_motion.get(&c).copied().unwrap_or(0);
                for e in evs.iter().rev().filter(|e| e.id > seen) {
                    v.seen_motion.insert(c.clone(), e.id);
                    Self::push(v, CamEvent::Motion { camera: c.clone(), mark: mark_of(e) });
                }
            }
            Ok(())
        });
    }
}

fn frame_out(f: &Frame, format: FrameFormat, encode: &(dyn Fn(u32, u32, &[u8]) -> Vec<u8> + Send + Sync)) -> Result<CamFrame, String> {
    match format {
        FrameFormat::Rgba => Ok(CamFrame { seq: f.seq, width: f.width, height: f.height, format, data: f.rgba.clone() }),
        FrameFormat::Jpeg => {
            let j = encode(f.width, f.height, &f.rgba);
            if j.len() < 4 {
                return Err("the picture could not be encoded".into());
            }
            Ok(CamFrame { seq: f.seq, width: f.width, height: f.height, format, data: Arc::new(j) })
        }
    }
}

impl Cameras for AgentCameras {
    fn list(&self, user: &str, _app: &str) -> Vec<CameraInfo> {
        if !self.ready() {
            return Vec::new();
        }
        let listed = self.watchable(&self.who(user));
        self.probe_listed(user, &listed);
        listed
    }

    fn open(&self, user: &str, app: &str, spec: &ViewSpec) -> Result<ViewInfo, String> {
        check_spec(app, spec)?;
        if !self.enabled(user) {
            return Err(iyakki_core_api::cameras::SWITCHED_OFF.into());
        }
        if !self.ready() {
            return Err("the cameras are not running yet; try again in a moment".into());
        }
        let who = self.who(user);
        let all = self.watchable(&who);
        let mut cams = Vec::new();
        for id in &spec.cameras {
            cams.push(all.iter().find(|c| c.id == *id).cloned().ok_or_else(|| NOT_YOURS.to_string())?);
        }
        let names: Vec<String> = cams.iter().map(|c| c.name.clone()).collect();
        let id = {
            let mut s = lk(&self.st);
            let live: Vec<&View> = s.views.values().filter(|v| v.user == user && v.phase != ViewPhase::Closed && !matches!(v.phase, ViewPhase::Denied { .. })).collect();
            if live.len() >= MAX_VIEWS_PER_PERSON {
                return Err(format!("you have {MAX_VIEWS_PER_PERSON} camera views open; close one first"));
            }
            if live.iter().filter(|v| v.app == app).count() >= MAX_VIEWS_PER_APP {
                return Err(format!("this app may have {MAX_VIEWS_PER_APP} camera views open at once; close one first"));
            }
            s.next += 1;
            let id = format!("v{}", s.next);
            let opened_ms = self.p().motion.as_ref().map(|m| m.now_ms()).unwrap_or(0);
            s.views.insert(id.clone(), View { user: user.into(), app: app.into(), cameras: spec.cameras.clone(), large: spec.size == FrameSize::Large, phase: ViewPhase::AwaitingConsent { call: String::new() }, visible: true, out: VecDeque::new(), pending: BTreeMap::new(), last_status: HashMap::new(), seen_motion: HashMap::new(), opened_ms, polled: None, ptz_ok: Default::default() });
            id
        };
        let (summary, detail) = view_card(app, &spec.label, &names);
        let call = self.ask(user, &id, CamAction::View, "", Pending::View, summary, detail)?;
        let _ = self.with_view(user, &id, |v| {
            v.phase = ViewPhase::AwaitingConsent { call };
            Ok(())
        });
        // a view that was never allowed is not "open" in the log's sense, but the request itself is worth a line
        self.audit(user, "cameras.open", &format!("{app}: {}", names.join(", ")));
        Ok(ViewInfo { id, cameras: cams })
    }

    fn decide(&self, user: &str, view: &str, call: &str, approve: bool) -> Result<(), String> {
        let (p, app) = self.with_view(user, view, |v| match v.pending.remove(call) {
            Some(p) => Ok((p, v.app.clone())),
            None => Err("there is no such card waiting".to_string()),
        })?;
        let (action, camera) = match &p {
            Pending::View => (CamAction::View, String::new()),
            Pending::Snapshot(c) => (CamAction::Snapshot, c.clone()),
            Pending::Ptz(c, _) => (CamAction::Ptz, c.clone()),
        };
        let _ = self.with_view(user, view, |v| {
            Self::push(v, CamEvent::Decision { call: call.into(), action, approved: approve, by: "user".into() });
            Ok(())
        });
        match p {
            Pending::View => {
                self.audit(user, "cameras.consent", &format!("{app} {}", if approve { "allowed" } else { "denied" }));
                let _ = self.with_view(user, view, |v| {
                    v.phase = if approve { ViewPhase::Open } else { ViewPhase::Denied { reason: "you said no".into() } };
                    Ok(())
                });
            }
            Pending::Snapshot(_) if approve => self.do_snapshot(user, view, &camera),
            Pending::Snapshot(_) => {}
            Pending::Ptz(c, cmd) if approve => {
                let _ = self.with_view(user, view, |v| {
                    v.ptz_ok.insert(c.clone());
                    Ok(())
                });
                if let Err(e) = self.run_ptz(user, view, &c, cmd) {
                    let _ = self.with_view(user, view, |v| {
                        Self::push(v, CamEvent::Failed { camera: c.clone(), action: CamAction::Ptz, error: iyakki_rtsp::url::scrub(&e) });
                        Ok(())
                    });
                }
            }
            Pending::Ptz(..) => {}
        }
        Ok(())
    }

    fn state(&self, user: &str, view: &str) -> Result<ViewState, String> {
        let (cams, phase, visible) = self.with_view(user, view, |v| Ok((v.cameras.clone(), v.phase.clone(), v.visible)))?;
        let all = self.watchable(&self.who(user));
        let cameras = cams.iter().filter_map(|c| all.iter().find(|i| i.id == *c).cloned()).collect();
        Ok(ViewState { indicator: phase == ViewPhase::Open && visible, phase, visible, cameras })
    }

    fn set_visible(&self, user: &str, view: &str, visible: bool) -> Result<(), String> {
        self.with_view(user, view, |v| {
            v.visible = visible;
            Ok(())
        })
    }

    fn frame(&self, user: &str, view: &str, camera: &str, after: u64, format: FrameFormat) -> Result<Option<CamFrame>, String> {
        let (open, large) = self.with_view(user, view, |v| {
            if !v.cameras.iter().any(|c| c == camera) {
                return Err(NOT_YOURS.to_string());
            }
            Ok((v.phase == ViewPhase::Open && v.visible, v.large))
        })?;
        if !open {
            return Ok(None);
        }
        let who = self.who(user);
        // every look is the hub's: it checks the right again (a camera taken off the person's list stops here), asks the broker for this
        // computer's own cameras, starts the stream and keeps it alive for the idle time
        let Some(f) = self.p().hub.view(&who, camera, large)? else { return Ok(None) };
        if f.seq <= after {
            return Ok(None);
        }
        frame_out(&f, format, &*self.p().hub.env().encode).map(Some)
    }

    fn listen(&self, user: &str, view: &str, camera: &str) -> Result<Option<iyakki_core_api::cameras::CamSound>, String> {
        self.with_view(user, view, |v| if v.cameras.iter().any(|c| c == camera) { Ok(()) } else { Err(NOT_YOURS.to_string()) })?;
        Err(iyakki_core_api::cameras::NO_VIEW_SOUND.into())
    }

    fn overlay(&self, user: &str, view: &str, camera: &str) -> Result<Overlay, String> {
        let open = self.with_view(user, view, |v| {
            if !v.cameras.iter().any(|c| c == camera) {
                return Err(NOT_YOURS.to_string());
            }
            Ok(v.phase == ViewPhase::Open)
        })?;
        let Some(m) = self.p().motion.as_ref().filter(|_| open) else { return Ok(Overlay::default()) };
        let who = self.who(user);
        // zones, masks and events are the camera's owner's: for anybody else the detector refuses and the overlay is empty
        let Ok(st) = m.status(&who, camera) else { return Ok(Overlay::default()) };
        if !st.owner {
            return Ok(Overlay::default());
        }
        let shape = |s: &iyakki_camdetect::settings::ShapeRec, mask: bool| OverlayShape { name: clean_name(&s.name), include: s.include, mask, points: s.points.iter().map(|p| (p[0], p[1])).collect() };
        let mut zones: Vec<OverlayShape> = st.settings.zones.iter().map(|s| shape(s, false)).chain(st.settings.masks.iter().map(|s| shape(s, true))).collect();
        zones.truncate(MAX_SHAPES);
        let to = m.now_ms();
        let marks = m.events(&who, camera, to.saturating_sub(MARK_WINDOW_MS), to, MAX_MARKS).map(|v| v.iter().rev().map(mark_of).collect()).unwrap_or_default();
        Ok(Overlay { zones, marks })
    }

    fn snapshot(&self, user: &str, view: &str, camera: &str) -> Result<String, String> {
        self.with_view(user, view, |v| {
            if v.phase != ViewPhase::Open {
                return Err("the view is not open: the person has not allowed it".to_string());
            }
            if !v.cameras.iter().any(|c| c == camera) {
                return Err(NOT_YOURS.to_string());
            }
            Ok(())
        })?;
        let name = self.name_of(&self.who(user), camera);
        self.ask(user, view, CamAction::Snapshot, camera, Pending::Snapshot(camera.into()), format!("Save one picture of {name} in your Pictures folder"), "One JPEG file. The view saves nothing else.".into())
    }

    fn ptz(&self, user: &str, view: &str, camera: &str, cmd: PtzCommand) -> Result<Option<String>, String> {
        cmd.checked()?;
        self.with_view(user, view, |v| {
            if v.phase != ViewPhase::Open {
                return Err("the view is not open: the person has not allowed it".to_string());
            }
            if !v.cameras.iter().any(|c| c == camera) {
                return Err(NOT_YOURS.to_string());
            }
            Ok(())
        })?;
        let who = self.who(user);
        const NONE: &str = "this camera has no pan, tilt or zoom control that this system can use";
        if !self.watchable(&who).iter().any(|c| c.id == camera && c.source == SourceKind::Network) || self.probe_ptz(user, camera).is_none() {
            return Err(NONE.into());
        }
        if self.with_view(user, view, |v| Ok(v.ptz_ok.contains(camera)))? {
            self.run_ptz(user, view, camera, cmd)?;
            return Ok(None);
        }
        // the first command of a view for a camera asks: a camera that moves is a thing in a room
        let name = self.name_of(&who, camera);
        let app = self.app_of(view);
        let call = self.ask(user, view, CamAction::Ptz, camera, Pending::Ptz(camera.into(), cmd), format!("Let {app} turn and zoom {name}"), "The camera moves while the view is open. Stop is always allowed. Nothing is recorded by this.".into())?;
        Ok(Some(call))
    }

    fn events(&self, user: &str, view: &str) -> Vec<CamEvent> {
        self.poll_changes(user, view);
        let mut s = lk(&self.st);
        match s.views.get_mut(view) {
            Some(v) if v.user == user => v.out.drain(..).collect(),
            _ => Vec::new(),
        }
    }

    fn enabled(&self, user: &str) -> bool {
        let Some(p) = self.switch_path(user) else { return false };
        std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["enabled"].as_bool()).unwrap_or(true)
    }

    fn set_enabled(&self, user: &str, on: bool) -> Result<(), String> {
        let p = self.switch_path(user).ok_or("that person name is not allowed")?;
        std::fs::create_dir_all(p.parent().ok_or("no folder")?).map_err(|e| e.to_string())?;
        std::fs::write(&p, serde_json::json!({ "enabled": on }).to_string()).map_err(|e| e.to_string())?;
        self.audit(user, "cameras.switch", if on { "on" } else { "off" });
        if !on {
            let ids: Vec<String> = lk(&self.st).views.iter().filter(|(_, v)| v.user == user).map(|(k, _)| k.clone()).collect();
            for id in ids {
                self.close(user, &id);
            }
        }
        Ok(())
    }

    fn indicator_names(&self) -> Vec<String> {
        AgentCameras::indicator_names(self)
    }

    fn open_views(&self, user: &str) -> usize {
        lk(&self.st).views.values().filter(|v| v.user == user && !matches!(v.phase, ViewPhase::Closed | ViewPhase::Denied { .. })).count()
    }

    fn close(&self, user: &str, view: &str) {
        let app = {
            let mut s = lk(&self.st);
            match s.views.get(view).filter(|v| v.user == user) {
                Some(v) => {
                    let app = v.app.clone();
                    s.views.remove(view);
                    app
                }
                None => return,
            }
        };
        self.audit(user, "cameras.close", &app);
    }
}

#[cfg(test)]
#[path = "embedcameras_tests.rs"]
mod tests;
