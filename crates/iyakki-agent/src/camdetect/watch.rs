//! The motion watcher: ties the detector to the camera hub, the owner's vault, the event store, the alert gate and the inbox.
//!
//! * One service per installation ([`for_hub`]). It takes a copy of the small decoded pictures the hub already makes for the live
//!   view (the hub's [`FrameTap`], at most 5 a second per camera) for the cameras whose owner turned watching on. It opens no
//!   connection of its own; in "always" mode it keeps the hub's stream alive.
//! * Rights: only the camera's owner turns watching on, draws zones and masks, changes alerts, reads events and thumbnails. An
//!   administrator may turn watching off. People the camera is shared with can see THAT it is watched, nothing else. Refusals of
//!   event reads are written to the camera's log.
//! * Nothing is analysed without the owner's vault key (the same key as recordings): when the vault closes (sign-out) the tick finds
//!   the key gone within a second or two and the detector stops; the first pictures after it opens start from a fresh background.
//! * Privacy masks are applied to a copy of every picture before the detector, the thumbnail and the person hook see it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use super::alerts::{AlertCfg, Gate, Notice, Quiet, MAX_PER_HOUR};
use super::events::{Event, EventStore, EVENTS_MAX_MB};
use super::geom::{self, Pt, MAX_SHAPES};
use super::motion::{Detector, MotionEvent, Params};
use super::person::{NotAvailable, PersonDetector, Shot};
use super::settings::{self, AlertRec, CamMotion, File, ShapeRec, WatchMode};
use crate::camrec::{self, key_name, log_event};
use crate::camrec_format::RecKey;
use crate::cameras::{FrameTap, Hub, Spec, Who};
use crate::clock::Zone;

/// Width of the stored thumbnail.
pub const THUMB_W: u32 = 160;
/// Pictures further apart than this start a new background (the camera was away, nobody was looking).
const GAP_RESET_MS: u64 = 10_000;
const PERSON_MIN_CONFIDENCE: f32 = 0.5;

// ------------------------------------------------------------------------------------------------ environment and seams

pub struct MotionEnv {
    pub now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub zone: Arc<dyn Fn() -> Zone + Send + Sync>,
    /// Whether the recorder is running for this camera.
    pub recording: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// Called with the names of the cameras that are watched in "always" mode, when that changes (the shell's red sign).
    pub on_active: Arc<dyn Fn(&[String]) + Send + Sync>,
    /// How often the vault key is looked at again.
    pub key_every: Duration,
    pub retention_every: Duration,
}

impl MotionEnv {
    pub fn real(data: PathBuf) -> MotionEnv {
        MotionEnv {
            now_ms: Arc::new(camrec::system_ms),
            zone: Arc::new(|| Zone::system_with(None)),
            recording: Arc::new(move |c| camrec::active_ids(&data).iter().any(|x| x == c)),
            on_active: Arc::new(|_| {}),
            key_every: Duration::from_secs(2),
            retention_every: Duration::from_secs(300),
        }
    }
}

/// Where notices go. The real sink is the person's inbox; the thumbnail is passed only when the person allowed it AND the sink can
/// carry a picture (the inbox cannot: its notice says the picture is in CAMERAS).
pub trait AlertSink: Send + Sync {
    fn notify(&self, user: &str, camera: &str, notice: &Notice, thumb: Option<&[u8]>);
}

pub struct InboxSink(pub Arc<crate::services::Services>);

impl AlertSink for InboxSink {
    fn notify(&self, user: &str, camera: &str, n: &Notice, _thumb: Option<&[u8]>) {
        let _ = self.0.notify(user, crate::inbox::Kind::Info, &n.title, &n.body, Some(crate::inbox::Link::new("cameras", camera)));
    }
}

// ------------------------------------------------------------------------------------------------ what a person asks for

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlertChange {
    pub enabled: Option<bool>,
    pub max_per_hour: Option<u32>,
    /// `Some("")` removes the quiet hours.
    pub quiet: Option<String>,
    pub thumbnail: Option<bool>,
    pub group_s: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    pub mode: Option<WatchMode>,
    pub sensitivity: Option<u8>,
    pub mask_storage: Option<bool>,
    pub person: Option<bool>,
    pub alerts: Option<AlertChange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Zone,
    Mask,
}

#[derive(Clone, Debug)]
pub struct MotionStatus {
    /// For a person the camera is only shared with, zones, masks and alert settings are left out.
    pub settings: CamMotion,
    pub owner: bool,
    /// A picture was analysed in the last few seconds.
    pub analysing: bool,
    pub state: String,
    pub person_note: String,
    pub event_bytes: u64,
    pub keep_days: u32,
}

// ------------------------------------------------------------------------------------------------ the service

struct Rt {
    name: String,
    owner: String,
    mode: WatchMode,
    masks: Vec<Vec<Pt>>,
    alerts: AlertCfg,
    person: bool,
    version: u64,
    det: Detector,
    gate: Gate,
    key: Option<RecKey>,
    key_at: Option<Instant>,
    key_err: String,
    last_frame_ms: u64,
    pictures: u64,
}

pub struct MotionService {
    hub: Arc<Hub>,
    env: MotionEnv,
    data: PathBuf,
    store: EventStore,
    cfg: Mutex<File>,
    file_lock: Mutex<()>,
    sink: Mutex<Option<Arc<dyn AlertSink>>>,
    person: Mutex<Arc<dyn PersonDetector>>,
    rt: Mutex<HashMap<String, Rt>>,
    last_retention: Mutex<Option<Instant>>,
    shown: Mutex<Vec<String>>,
    zone: Mutex<Option<(Instant, Arc<Zone>)>>,
}

struct Tap(Weak<MotionService>);

impl FrameTap for Tap {
    fn wants(&self, camera: &str) -> bool {
        self.0.upgrade().is_some_and(|s| s.wants(camera))
    }
    fn frame(&self, camera: &str, width: u32, height: u32, rgba: &[u8]) {
        if let Some(s) = self.0.upgrade() {
            s.process(camera, width, height, rgba);
        }
    }
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Arc<MotionService>>> {
    static R: OnceLock<Mutex<HashMap<PathBuf, Arc<MotionService>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// The installation's motion service (made with the first caller's environment). Its supervisor thread and its picture tap start with it.
pub fn shared(hub: Arc<Hub>, env: MotionEnv) -> Arc<MotionService> {
    let data = hub.data_dir();
    let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(x) = r.get(&data) {
        return x.clone();
    }
    let s = Arc::new(MotionService::new(hub.clone(), env));
    hub.set_tap(Arc::new(Tap(Arc::downgrade(&s))));
    s.start();
    r.insert(data, s.clone());
    s
}

pub fn for_hub(hub: &Arc<Hub>) -> Arc<MotionService> {
    shared(hub.clone(), MotionEnv::real(hub.data_dir()))
}

/// Names of the cameras watched all the time (nothing when no service exists yet): what the red sign says.
pub fn active_names(data: &std::path::Path) -> Vec<String> {
    let s = registry().lock().unwrap_or_else(|e| e.into_inner()).get(data).cloned();
    s.map(|s| s.always_names()).unwrap_or_default()
}

/// Ids of the cameras whose pictures are being analysed right now (the tile's MOTION mark).
pub fn analysing_ids(data: &std::path::Path) -> Vec<String> {
    let s = registry().lock().unwrap_or_else(|e| e.into_inner()).get(data).cloned();
    s.map(|s| s.analysing_ids()).unwrap_or_default()
}

/// The one-click stop behind the red sign: turns off "always" watching of every camera `user` owns.
pub fn stop_all_for(data: &std::path::Path, user: &str) -> usize {
    let s = registry().lock().unwrap_or_else(|e| e.into_inner()).get(data).cloned();
    s.map(|s| s.stop_all_for(user)).unwrap_or(0)
}

fn minute_of(l: &crate::clock::Local) -> u16 {
    (l.hour * 60 + l.minute) as u16
}

fn new_rt(c: &CamMotion, spec: &Spec) -> Rt {
    Rt {
        name: spec.name.clone(),
        owner: spec.owner.clone(),
        mode: c.mode,
        masks: c.masks_rt().into_iter().map(|m| m.points).collect(),
        alerts: c.alerts.cfg(),
        person: c.person,
        version: c.version,
        det: Detector::new(Params { sensitivity: c.sensitivity.clamp(1, 10), ..Params::default() }, c.zones_rt()),
        gate: Gate::new(),
        key: None,
        key_at: None,
        key_err: "the owner's vault has not been opened yet".into(),
        last_frame_ms: 0,
        pictures: 0,
    }
}

impl MotionService {
    pub fn new(hub: Arc<Hub>, env: MotionEnv) -> MotionService {
        let data = hub.data_dir();
        let cfg = settings::load(&data);
        MotionService {
            store: EventStore::new(&data),
            hub,
            env,
            data,
            cfg: Mutex::new(cfg),
            file_lock: Mutex::new(()),
            sink: Mutex::new(None),
            person: Mutex::new(Arc::new(NotAvailable)),
            rt: Mutex::new(HashMap::new()),
            last_retention: Mutex::new(None),
            shown: Mutex::new(vec![]),
            zone: Mutex::new(None),
        }
    }

    /// Supervise once a second: keys, keep-alive, quiet endings, alerts due, retention.
    pub fn start(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        std::thread::spawn(move || loop {
            match me.upgrade() {
                Some(s) => s.tick(),
                None => break,
            }
            std::thread::sleep(Duration::from_secs(1));
        });
    }

    pub fn set_sink(&self, sink: Arc<dyn AlertSink>) {
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
    }

    pub fn has_sink(&self) -> bool {
        self.sink.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    /// Replace the person detector (the default says "not available").
    pub fn set_person_detector(&self, d: Arc<dyn PersonDetector>) {
        *self.person.lock().unwrap_or_else(|e| e.into_inner()) = d;
    }

    pub fn person_availability(&self) -> Result<(), String> {
        self.person.lock().unwrap_or_else(|e| e.into_inner()).available()
    }

    fn zone(&self) -> Arc<Zone> {
        let mut z = self.zone.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, zone)) = z.as_ref() {
            if t.elapsed() < Duration::from_secs(60) {
                return zone.clone();
            }
        }
        let zone = Arc::new((self.env.zone)());
        *z = Some((Instant::now(), zone.clone()));
        zone
    }

    fn local(&self, ms: u64) -> crate::clock::Local {
        self.zone().local((ms / 1000) as i64)
    }

    /// `YYYY-MM-DD HH:MM:SS` in local time.
    pub fn time_text(&self, ms: u64) -> String {
        crate::clock::format(&self.zone(), (ms / 1000) as i64, "%Y-%m-%d %H:%M:%S")
    }

    /// The start and end of a local day written `YYYY-MM-DD`.
    pub fn day_range(&self, day: &str) -> Result<(u64, u64), String> {
        let bad = || "a day is written like 2026-11-08".to_string();
        let p: Vec<&str> = day.trim().split('-').collect();
        if p.len() != 3 {
            return Err(bad());
        }
        let (y, m, d): (i64, u32, u32) = (p[0].parse().map_err(|_| bad())?, p[1].parse().map_err(|_| bad())?, p[2].parse().map_err(|_| bad())?);
        if !(2000..=2200).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            return Err(bad());
        }
        let z = self.zone();
        let from = z.local_to_unix(y, m, d, 0, 0, 0).max(0) as u64 * 1000;
        Ok((from, from + 86_400_000 - 1))
    }

    pub fn now_ms(&self) -> u64 {
        (self.env.now_ms)()
    }

    fn spec_of(&self, cam: &str) -> Option<Spec> {
        self.hub.specs().into_iter().find(|s| s.id == cam)
    }

    /// The camera, when the person may know it exists (owner, on the list, or an administrator).
    fn visible(&self, who: &Who, cam: &str) -> Result<Spec, String> {
        if cam.starts_with("usb:") {
            return Err("motion watching is for network cameras; this computer's own camera is never watched in the background".into());
        }
        match self.spec_of(cam) {
            Some(s) if crate::cameras::can_view(&s, &who.user) || s.owner == who.user || who.manage => Ok(s),
            _ => Err("no such camera, or you may not see it".into()),
        }
    }

    fn owned(&self, who: &Who, cam: &str, ev: &str) -> Result<Spec, String> {
        let s = self.visible(who, cam)?;
        if s.owner != who.user {
            log_event(&self.data, cam, &who.user, ev, "refused: not the owner", false);
            return Err("only the person who added the camera can do that with its motion watching".into());
        }
        Ok(s)
    }

    fn cfg_of(&self, cam: &str) -> CamMotion {
        self.cfg.lock().unwrap_or_else(|e| e.into_inner()).cameras.iter().find(|c| c.camera == cam).cloned().unwrap_or_else(|| CamMotion::off(cam))
    }

    fn edit<T>(&self, cam: &str, f: impl FnOnce(&mut CamMotion) -> Result<T, String>) -> Result<T, String> {
        let _g = self.file_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut file = settings::load_strict(&self.data)?;
        let mut c = file.cameras.iter().find(|c| c.camera == cam).cloned().unwrap_or_else(|| CamMotion::off(cam));
        let r = f(&mut c)?;
        c.version += 1;
        c.valid()?;
        match file.cameras.iter_mut().find(|x| x.camera == cam) {
            Some(x) => *x = c,
            None => file.cameras.push(c),
        }
        settings::save(&self.data, &file)?;
        settings::forget_cache(&self.data);
        *self.cfg.lock().unwrap_or_else(|e| e.into_inner()) = file;
        Ok(r)
    }

    // ------------------------------------------------------------------------------ rights and settings

    /// What is set, for the tile and the assistant. Anyone who may see the camera learns whether it is watched; the owner (and an
    /// administrator, who manages the camera) sees the shapes and alert settings.
    pub fn status(&self, who: &Who, cam: &str) -> Result<MotionStatus, String> {
        let spec = self.visible(who, cam)?;
        let owner = spec.owner == who.user;
        let mut c = self.cfg_of(cam);
        if !owner && !who.manage {
            c.zones.clear();
            c.masks.clear();
            c.alerts = AlertRec::default();
            c.person = false;
        }
        let (state, analysing) = self.state_words(cam, &c);
        let person_note = match self.person_availability() {
            Ok(()) => "A person detector is available; it is asked about events when this is on.".to_string(),
            Err(e) => e,
        };
        Ok(MotionStatus { event_bytes: if owner { self.store.bytes(cam) } else { 0 }, keep_days: camrec::retention_days(&self.data, cam), settings: c, owner, analysing, state, person_note })
    }

    fn state_words(&self, cam: &str, c: &CamMotion) -> (String, bool) {
        let now = self.now_ms();
        let rts = self.rt.lock().unwrap_or_else(|e| e.into_inner());
        let rt = rts.get(cam);
        let analysing = rt.is_some_and(|r| r.key.is_some() && now.saturating_sub(r.last_frame_ms) < 4000 && r.last_frame_ms > 0);
        let s = match (c.mode, rt) {
            (WatchMode::Off, _) => "Motion watching is off.".to_string(),
            (_, Some(r)) if r.key.is_none() => format!("Waiting: {}. Nothing is analysed or stored until the owner's vault is open.", r.key_err),
            (WatchMode::Always, _) if analysing => "Watching all the time (a red sign shows on every screen).".to_string(),
            (WatchMode::Always, _) => "Watching all the time; waiting for pictures from the camera.".to_string(),
            (WatchMode::Watched, _) if analysing => "Looking now: the camera is on a screen or being recorded.".to_string(),
            (WatchMode::Watched, _) => "Watching only while the camera is on a screen or being recorded; not looking right now.".to_string(),
        };
        (s, analysing)
    }

    /// Change motion watching. Turning it on is the owner's alone; an administrator may only turn it off.
    pub fn set(self: &Arc<Self>, who: &Who, cam: &str, ch: &Change) -> Result<CamMotion, String> {
        let spec = self.visible(who, cam)?;
        let owner = spec.owner == who.user;
        let only_off = *ch == Change { mode: Some(WatchMode::Off), ..Default::default() };
        if !owner && !(who.manage && only_off) {
            log_event(&self.data, cam, &who.user, "motion.setting", "refused: not the owner", false);
            return Err("only the person who added the camera can change its motion watching (an administrator can turn it off)".into());
        }
        if let Some(s) = ch.sensitivity {
            if !(1..=10).contains(&s) {
                return Err("sensitivity is a number from 1 (least) to 10 (most)".into());
            }
        }
        let turns_on = ch.mode.is_some_and(|m| m != WatchMode::Off);
        if turns_on {
            camrec::ensure_camera_key(&*self.hub.env().secrets, &spec)?;
        }
        let a = ch.alerts.clone().unwrap_or_default();
        if let Some(n) = a.max_per_hour {
            if !(1..=MAX_PER_HOUR).contains(&n) {
                return Err(format!("alerts per hour are 1 to {MAX_PER_HOUR}"));
            }
        }
        if let Some(g) = a.group_s {
            if !(10..=86_400).contains(&g) {
                return Err("the grouping time is 10 seconds to 24 hours".into());
            }
        }
        if let Some(q) = a.quiet.as_deref().filter(|q| !q.trim().is_empty()) {
            Quiet::parse(q)?;
        }
        let now = self.edit(cam, |c| {
            if let Some(m) = ch.mode {
                c.mode = m;
            }
            if let Some(s) = ch.sensitivity {
                c.sensitivity = s;
            }
            if let Some(b) = ch.mask_storage {
                c.mask_storage = b;
            }
            if let Some(b) = ch.person {
                c.person = b;
            }
            if let Some(v) = a.enabled {
                c.alerts.enabled = v;
            }
            if let Some(v) = a.max_per_hour {
                c.alerts.max_per_hour = v;
            }
            if let Some(v) = &a.quiet {
                c.alerts.quiet = v.trim().to_string();
            }
            if let Some(v) = a.thumbnail {
                c.alerts.thumbnail = v;
            }
            if let Some(v) = a.group_s {
                c.alerts.group_s = v;
            }
            Ok(c.clone())
        })?;
        let detail = format!("mode {}, sensitivity {}, alerts {}, masks also in storage {}", now.mode.word(), now.sensitivity, if now.alerts.enabled { "on" } else { "off" }, if now.mask_storage { "yes" } else { "no" });
        log_event(&self.data, cam, &who.user, "motion.setting", &detail, true);
        (self.hub.env().audit)(&who.user, "camera.motion", &format!("{} {detail}", spec.name));
        self.tick();
        Ok(now)
    }

    /// Add or replace (by name) a zone or a privacy mask. Owner only. The shape is checked first; a bad one changes nothing.
    pub fn shape_set(self: &Arc<Self>, who: &Who, cam: &str, kind: ShapeKind, name: &str, include: bool, sensitivity: u8, points: &[Pt]) -> Result<(), String> {
        let spec = self.owned(who, cam, "motion.shape")?;
        let name = geom::clean_name(name)?;
        geom::validate(points)?;
        if !(1..=10).contains(&sensitivity) {
            return Err("sensitivity is a number from 1 (least) to 10 (most)".into());
        }
        self.edit(cam, |c| {
            let list = if kind == ShapeKind::Zone { &mut c.zones } else { &mut c.masks };
            let rec = ShapeRec { name: name.clone(), include, sensitivity, points: points.iter().map(|p| [p.x, p.y]).collect() };
            match list.iter().position(|s| s.name.eq_ignore_ascii_case(&name)) {
                Some(i) => list[i] = rec,
                None if list.len() >= MAX_SHAPES => return Err(format!("at most {MAX_SHAPES} zones and {MAX_SHAPES} masks per camera; remove one first")),
                None => list.push(rec),
            }
            Ok(())
        })?;
        let what = if kind == ShapeKind::Zone { if include { "zone" } else { "exclude zone" } } else { "privacy mask" };
        log_event(&self.data, cam, &who.user, "motion.shape", &format!("{what} “{name}” with {} corners", points.len()), true);
        (self.hub.env().audit)(&who.user, "camera.motion.shape", &format!("{} {what} {name}", spec.name));
        self.tick();
        Ok(())
    }

    pub fn shape_remove(self: &Arc<Self>, who: &Who, cam: &str, kind: ShapeKind, name: &str) -> Result<(), String> {
        let spec = self.owned(who, cam, "motion.shape")?;
        let found = self.edit(cam, |c| {
            let list = if kind == ShapeKind::Zone { &mut c.zones } else { &mut c.masks };
            let before = list.len();
            list.retain(|s| !s.name.eq_ignore_ascii_case(name.trim()));
            Ok(list.len() != before)
        })?;
        if !found {
            return Err(format!("no {} called “{}”", if kind == ShapeKind::Zone { "zone" } else { "mask" }, name.trim()));
        }
        log_event(&self.data, cam, &who.user, "motion.shape", &format!("removed “{}”", name.trim()), true);
        (self.hub.env().audit)(&who.user, "camera.motion.shape", &format!("{} removed {}", spec.name, name.trim()));
        self.tick();
        Ok(())
    }

    /// Turn off "always" watching of every camera `user` owns (the red sign's click). Returns how many.
    pub fn stop_all_for(self: &Arc<Self>, user: &str) -> usize {
        let mut n = 0;
        let ids: Vec<String> = self.cfg.lock().unwrap_or_else(|e| e.into_inner()).cameras.iter().filter(|c| c.mode == WatchMode::Always).map(|c| c.camera.clone()).collect();
        for id in ids {
            if self.spec_of(&id).is_some_and(|s| s.owner == user) {
                let who = Who { user: user.into(), manage: false, usb_ok: false, home: PathBuf::new(), may_write: Arc::new(|_| false) };
                if self.set(&who, &id, &Change { mode: Some(WatchMode::Off), ..Default::default() }).is_ok() {
                    n += 1;
                }
            }
        }
        n
    }

    // ------------------------------------------------------------------------------ events

    fn load_key(&self, spec: &Spec) -> Result<RecKey, String> {
        let t = self.hub.env().secrets.get(&spec.owner, &key_name(&spec.id)).map_err(|e| format!("the key for this camera's events cannot be read from the owner's vault ({e}); the owner signs in"))?;
        RecKey::from_hex(&t).ok_or_else(|| "the key in the vault is damaged".to_string())
    }

    /// Events that started between two times, newest first. Owner only.
    pub fn events(&self, who: &Who, cam: &str, from_ms: u64, to_ms: u64, limit: usize) -> Result<Vec<Event>, String> {
        let spec = self.owned(who, cam, "motion.events")?;
        let key = self.load_key(&spec)?;
        self.store.read(cam, &key, from_ms, to_ms, limit).map(|(v, _)| v)
    }

    /// An event's thumbnail as a decoded picture. Owner only.
    pub fn thumb(&self, who: &Who, cam: &str, id: u64) -> Result<Option<(u32, u32, Vec<u8>)>, String> {
        let spec = self.owned(who, cam, "motion.events")?;
        let key = self.load_key(&spec)?;
        let Some(j) = self.store.thumb(cam, &key, id)? else { return Ok(None) };
        let b = iyakki_web::jpeg::decode(&j).map_err(|e| format!("the thumbnail could not be read: {e}"))?;
        Ok(Some((b.width, b.height, b.rgba)))
    }

    // ------------------------------------------------------------------------------ the picture path

    /// Whether the picture tap should hand over the next picture of this camera.
    pub fn wants(&self, cam: &str) -> bool {
        let mode = {
            let rts = self.rt.lock().unwrap_or_else(|e| e.into_inner());
            match rts.get(cam) {
                Some(r) if r.key.is_some() => r.mode,
                _ => return false,
            }
        };
        match mode {
            WatchMode::Off => false,
            WatchMode::Always => true,
            WatchMode::Watched => self.hub.viewed_recently(cam) || (self.env.recording)(cam),
        }
    }

    /// One decoded picture of a camera (the hub's small picture). Public so tests can feed pictures without a camera.
    pub fn process(&self, cam: &str, w: u32, h: u32, rgba: &[u8]) {
        let now = self.now_ms();
        let mut rts = self.rt.lock().unwrap_or_else(|e| e.into_inner());
        let Some(rt) = rts.get_mut(cam) else { return };
        if rt.key.is_none() || rt.mode == WatchMode::Off {
            return;
        }
        if rt.last_frame_ms > 0 && now.saturating_sub(rt.last_frame_ms) > GAP_RESET_MS {
            self.close_open_event(cam, rt, now);
            rt.det.reset();
        }
        rt.last_frame_ms = now;
        rt.pictures += 1;
        // privacy masks first: the detector, the thumbnail and the person hook only ever see the masked copy
        let mut masked;
        let pic: &[u8] = if rt.masks.is_empty() {
            rgba
        } else {
            masked = rgba.to_vec();
            let refs: Vec<&[Pt]> = rt.masks.iter().map(|m| m.as_slice()).collect();
            if !geom::blacken(w, h, &mut masked, &refs) {
                return;
            }
            &masked
        };
        let events = rt.det.push(now, w, h, pic);
        for e in events {
            self.handle(cam, rt, now, w, h, pic, e);
        }
    }

    fn thumb_jpeg(&self, w: u32, h: u32, rgba: &[u8], max_w: u32) -> Option<Vec<u8>> {
        let (tw, th, data) = crate::cameras::shrink(w, h, rgba, max_w);
        let j = (self.hub.env().encode)(tw, th, &data);
        (j.len() >= 4).then_some(j)
    }

    #[allow(clippy::too_many_arguments)]
    fn handle(&self, cam: &str, rt: &mut Rt, now: u64, w: u32, h: u32, pic: &[u8], e: MotionEvent) {
        let Some(key) = rt.key.as_ref() else { return };
        match e {
            MotionEvent::Start { ts_ms, zone, score, bbox } => {
                let thumb = self.thumb_jpeg(w, h, pic, THUMB_W);
                let ev = Event { id: ts_ms, start_ms: ts_ms, end_ms: None, zone: zone.clone(), score, peak: score, bbox, person: None, has_thumb: thumb.is_some() };
                if let Err(m) = self.store.start(cam, key, &self.local(ts_ms), &ev, thumb.as_deref()) {
                    log_event(&self.data, cam, "system", "motion.store", &m, false);
                    return;
                }
                log_event(&self.data, cam, "system", "motion.start", &format!("zone {zone}, score {score}"), true);
                // the person hook: only for an event that already passed the motion detector, only the masked picture
                let mut person = None;
                if rt.person {
                    let det = self.person.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if det.available().is_ok() {
                        if let Some(j) = self.thumb_jpeg(w, h, pic, 480) {
                            let (sw, sh, _) = crate::cameras::shrink(w, h, pic, 480);
                            match det.detect(&Shot { width: sw, height: sh, jpeg: &j }, PERSON_MIN_CONFIDENCE) {
                                Ok(b) => {
                                    person = Some(!b.is_empty());
                                    let _ = self.store.person(cam, key, &self.local(ts_ms), ts_ms, ts_ms, !b.is_empty());
                                }
                                Err(m) => log_event(&self.data, cam, "system", "motion.person", &m, false),
                            }
                        }
                    }
                }
                let minute = minute_of(&self.local(now));
                let fmt = |ms: u64| crate::clock::format(&self.zone(), (ms / 1000) as i64, "%H:%M");
                let notices = rt.gate.event(&rt.alerts, now, minute, &rt.name, &zone, ts_ms, person, &fmt);
                self.deliver(cam, rt, &notices, thumb.as_deref());
            }
            MotionEvent::End { start_ms, end_ms, peak, zone } => {
                if let Err(m) = self.store.end(cam, key, &self.local(end_ms), start_ms, end_ms, peak) {
                    log_event(&self.data, cam, "system", "motion.store", &m, false);
                    return;
                }
                log_event(&self.data, cam, "system", "motion.end", &format!("zone {zone}, {} s, peak {peak}", end_ms.saturating_sub(start_ms) / 1000), true);
            }
        }
    }

    fn deliver(&self, cam: &str, rt: &Rt, notices: &[Notice], thumb: Option<&[u8]>) {
        if notices.is_empty() {
            return;
        }
        let sink = self.sink.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for n in notices {
            log_event(&self.data, cam, "system", "motion.alert", &format!("{} ({} event{})", n.title, n.count, if n.count == 1 { "" } else { "s" }), sink.is_some());
            if let Some(s) = &sink {
                s.notify(&rt.owner, cam, n, if n.thumbnail { thumb } else { None });
            }
        }
    }

    fn close_open_event(&self, cam: &str, rt: &mut Rt, now: u64) {
        if let Some(MotionEvent::End { start_ms, end_ms, peak, .. }) = rt.det.finish(now) {
            if let Some(k) = rt.key.as_ref() {
                let _ = self.store.end(cam, k, &self.local(end_ms), start_ms, end_ms, peak);
            }
        }
    }

    // ------------------------------------------------------------------------------ supervision

    /// Bring the running state in line with the settings and the cameras that exist.
    fn sync(&self) {
        let cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner()).cameras.clone();
        let specs = self.hub.specs();
        let mut rts = self.rt.lock().unwrap_or_else(|e| e.into_inner());
        let now = self.now_ms();
        let gone: Vec<String> = rts.keys().filter(|id| !cfg.iter().any(|c| &c.camera == *id && c.mode != WatchMode::Off && specs.iter().any(|s| &s.id == *id))).cloned().collect();
        for id in gone {
            if let Some(mut rt) = rts.remove(&id) {
                self.close_open_event(&id, &mut rt, now);
            }
            self.store.close(&id);
        }
        for c in cfg.iter().filter(|c| c.mode != WatchMode::Off) {
            let Some(spec) = specs.iter().find(|s| s.id == c.camera) else { continue };
            match rts.get_mut(&c.camera) {
                Some(rt) if rt.version == c.version => {}
                Some(rt) => {
                    // settings changed: close what is open, start from a fresh background with the new shapes
                    self.close_open_event(&c.camera, rt, now);
                    let (key, key_at, key_err, gate_last) = (rt.key.take(), rt.key_at, std::mem::take(&mut rt.key_err), std::mem::take(&mut rt.gate));
                    let mut fresh = new_rt(c, spec);
                    fresh.key = key;
                    fresh.key_at = key_at;
                    fresh.key_err = key_err;
                    fresh.gate = gate_last;
                    *rt = fresh;
                }
                None => {
                    rts.insert(c.camera.clone(), new_rt(c, spec));
                }
            }
        }
    }

    /// One supervision step (the thread calls it every second; tests call it directly).
    pub fn tick(self: &Arc<Self>) {
        self.sync();
        let now = self.now_ms();
        let specs = self.hub.specs();
        {
            let mut rts = self.rt.lock().unwrap_or_else(|e| e.into_inner());
            for (id, rt) in rts.iter_mut() {
                let Some(spec) = specs.iter().find(|s| &s.id == id) else { continue };
                if rt.key_at.is_none_or(|t| t.elapsed() >= self.env.key_every) {
                    rt.key_at = Some(Instant::now());
                    match self.load_key(spec) {
                        Ok(k) => {
                            if rt.key.is_none() {
                                // the vault opened again: the old background is stale
                                rt.det.reset();
                            }
                            rt.key = Some(k);
                            rt.key_err.clear();
                        }
                        Err(e) => {
                            if rt.key.is_some() {
                                // the vault closed (sign-out): nothing can be stored any more, so nothing is analysed
                                let _ = rt.det.finish(now);
                                rt.det.reset();
                                self.store.close(id);
                            }
                            rt.key = None;
                            rt.key_err = e;
                        }
                    }
                }
                if rt.key.is_some() && (rt.mode == WatchMode::Always || (rt.mode == WatchMode::Watched && (self.env.recording)(id))) {
                    let _ = self.hub.keep_alive(id);
                }
                // an event with no pictures for a while (the camera was lost, nobody looks any more) is closed
                if rt.det.active() && now.saturating_sub(rt.last_frame_ms) > 15_000 {
                    self.close_open_event(id, rt, now);
                }
                let minute = minute_of(&self.local(now));
                let fmt = |ms: u64| crate::clock::format(&self.zone(), (ms / 1000) as i64, "%H:%M");
                let notices = rt.gate.tick(&rt.alerts, now, minute, &rt.name, &fmt);
                self.deliver(id, rt, &notices, None);
            }
        }
        let due = self.last_retention.lock().unwrap_or_else(|e| e.into_inner()).is_none_or(|t| t.elapsed() >= self.env.retention_every);
        if due {
            *self.last_retention.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
            self.enforce();
        }
        let names = self.always_names();
        let changed = {
            let mut s = self.shown.lock().unwrap_or_else(|e| e.into_inner());
            let c = *s != names;
            *s = names.clone();
            c
        };
        if changed {
            (self.env.on_active)(&names);
        }
    }

    /// Retention of the events: the camera's recording days, and a size cap per camera. Also cleans cameras that were removed.
    pub fn enforce(&self) -> (usize, u64) {
        let now = self.now_ms();
        let (mut n, mut bytes) = (0, 0);
        for cam in self.store.cameras() {
            let (a, b) = self.store.enforce(&cam, now, camrec::retention_days(&self.data, &cam), EVENTS_MAX_MB * 1024 * 1024);
            n += a;
            bytes += b;
            if a > 0 {
                log_event(&self.data, &cam, "system", "motion.retention", &format!("{a} event files, {b} bytes"), true);
            }
        }
        (n, bytes)
    }

    /// Close the event files (shutdown, tests).
    pub fn shutdown(&self) {
        let now = self.now_ms();
        let mut rts = self.rt.lock().unwrap_or_else(|e| e.into_inner());
        for (id, rt) in rts.iter_mut() {
            self.close_open_event(id, rt, now);
        }
        rts.clear();
        self.store.close_all();
    }

    /// Names of the cameras watched all the time.
    pub fn always_names(&self) -> Vec<String> {
        let cfg = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        let specs = self.hub.specs();
        let mut v: Vec<String> = cfg.cameras.iter().filter(|c| c.mode == WatchMode::Always).filter_map(|c| specs.iter().find(|s| s.id == c.camera).map(|s| s.name.clone())).collect();
        v.sort();
        v
    }

    /// Ids of the cameras that had a picture analysed in the last few seconds.
    pub fn analysing_ids(&self) -> Vec<String> {
        let now = self.now_ms();
        let rts = self.rt.lock().unwrap_or_else(|e| e.into_inner());
        let mut v: Vec<String> = rts.iter().filter(|(_, r)| r.key.is_some() && r.last_frame_ms > 0 && now.saturating_sub(r.last_frame_ms) < 4000).map(|(k, _)| k.clone()).collect();
        v.sort();
        v
    }

    /// Ids of the cameras with watching switched on (any mode).
    pub fn watched_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.cfg.lock().unwrap_or_else(|e| e.into_inner()).cameras.iter().filter(|c| c.mode != WatchMode::Off).map(|c| c.camera.clone()).collect();
        v.sort();
        v
    }
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod tests;
