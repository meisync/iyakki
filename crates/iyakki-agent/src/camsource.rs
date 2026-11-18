//! The camera hub as the CAMERAS tile's source (`iyakki_tile_api::cameras::CameraSource`): the tile asks, the installation's hub, recorder and
//! motion watcher answer for one person. Moved out of the shell (F-SPLIT-03, cameras cluster) so the UI holds no hub calls of its own.

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_tile_api::cameras as api;

use crate::cameras::Hub;
use crate::cameras::Who;

/// The hub and the person it answers for.
pub struct HubSource {
    pub hub: Arc<Hub>,
    pub who: Who,
}

fn motion_mode_to_api(m: iyakki_camdetect::settings::WatchMode) -> api::MotionMode {
    use iyakki_camdetect::settings::WatchMode;
    match m {
        WatchMode::Off => api::MotionMode::Off,
        WatchMode::Watched => api::MotionMode::Watched,
        WatchMode::Always => api::MotionMode::Always,
    }
}

fn status_to_api(s: crate::cameras::Status) -> api::Status {
    match s {
        crate::cameras::Status::Connecting => api::Status::Connecting,
        crate::cameras::Status::Live => api::Status::Live,
        crate::cameras::Status::Lost(m) => api::Status::Lost(m),
        crate::cameras::Status::NeedsAddon(m) => api::Status::NeedsAddon(m),
        crate::cameras::Status::Busy(m) => api::Status::Busy(m),
        crate::cameras::Status::NeedsPassword(m) => api::Status::NeedsPassword(m),
        crate::cameras::Status::Dark(m) => api::Status::Dark(m),
    }
}

impl api::CameraSource for HubSource {
    fn cameras(&self) -> Vec<api::CamInfo> {
        self.hub
            .cameras(&self.who)
            .into_iter()
            .map(|v| api::CamInfo {
                id: v.id,
                name: v.name,
                kind: if v.kind == crate::cameras::Kind::Usb { api::Kind::Usb } else { api::Kind::Network },
                address: v.address,
                user: v.user,
                status: status_to_api(v.status),
                fps: v.fps,
                size: v.size,
                manage: v.manage,
                owner: v.owner,
                allowed: v.allowed,
                has_password: v.has_password,
            })
            .collect()
    }
    fn view(&self, id: &str, big: bool) -> Option<api::Frame> {
        self.hub.view_on_screen(&self.who, id, big).ok().flatten().map(|f| api::Frame { seq: f.seq, width: f.width, height: f.height, rgba: f.rgba })
    }
    fn release_local(&self) {
        self.hub.release_usb();
    }
    fn release_local_except(&self, shown: &[String]) {
        self.hub.release_screen_except(shown);
    }
    fn snapshot(&self, id: &str) -> Result<String, String> {
        self.hub.snapshot(&self.who, id).map(|p: PathBuf| p.display().to_string())
    }
    fn save(&self, replacing: Option<&str>, cam: &api::NewCam, password: Option<&str>) -> Result<String, String> {
        self.hub.save(&self.who, replacing, &crate::cameras::NewSpec { name: cam.name.clone(), address: cam.address.clone(), user: cam.user.clone(), allowed: cam.allowed.clone() }, password)
    }
    fn remove(&self, id: &str) -> Result<(), String> {
        self.hub.remove(&self.who, id)
    }
    fn people(&self) -> Vec<String> {
        self.hub.people(&self.who)
    }

    fn recording_ids(&self) -> Vec<String> {
        crate::camrec::active_ids(&self.hub.data_dir())
    }
    fn rec_info(&self, id: &str) -> Option<api::RecInfo> {
        let rec = crate::camrec::for_hub(&self.hub);
        let st = rec.status(&self.who, id).ok()?;
        Some(api::RecInfo {
            mode: match st.settings.mode {
                crate::camrec::Mode::Off => api::RecMode::Off,
                crate::camrec::Mode::Continuous => api::RecMode::Continuous,
                crate::camrec::Mode::Schedule => api::RecMode::Schedule,
            },
            windows: st.settings.windows.iter().map(crate::camrec::Window::text).collect(),
            max_days: st.settings.max_days,
            max_mb: st.settings.max_mb,
            state: st.state.words(),
            recording: st.recording,
            problem: matches!(st.state, crate::camrec::RecState::Problem(_)),
            owner: st.owner,
            segments: st.segments,
            bytes: st.bytes,
            key_note: st.key_note.to_string(),
        })
    }
    fn rec_set(&self, id: &str, c: &api::RecChange) -> Result<(), String> {
        let rec = crate::camrec::for_hub(&self.hub);
        let windows = match &c.windows {
            Some(w) => Some(w.iter().map(|t| crate::camrec::Window::parse(t)).collect::<Result<Vec<_>, _>>()?),
            None => None,
        };
        let mode = c.mode.map(|m| match m {
            api::RecMode::Off => crate::camrec::Mode::Off,
            api::RecMode::Continuous => crate::camrec::Mode::Continuous,
            api::RecMode::Schedule => crate::camrec::Mode::Schedule,
        });
        rec.set(&self.who, id, &crate::camrec::Change { mode, windows, max_days: c.max_days, max_mb: c.max_mb }).map(|_| ())
    }
    fn rec_limits(&self) -> Option<api::RecLimits> {
        let p = crate::camrec::for_hub(&self.hub).policy();
        Some(api::RecLimits { total_mb: p.total_max_mb, min_free_mb: p.min_free_mb, min_floor_mb: crate::camrec::MIN_FLOOR_MB, can_change: self.who.manage })
    }
    fn rec_limits_set(&self, total_mb: Option<u64>, min_free_mb: Option<u64>) -> Result<(), String> {
        crate::camrec::for_hub(&self.hub).set_policy(&self.who, total_mb, min_free_mb).map(|_| ())
    }
    fn rec_days(&self, id: &str) -> Result<Vec<api::RecDay>, String> {
        let rec = crate::camrec::for_hub(&self.hub);
        rec.days(&self.who, id).map(|v| v.into_iter().map(|d| api::RecDay { label: d.label, day_start_ms: d.day_start_ms, spans: d.spans, frames: d.frames, bytes: d.bytes }).collect())
    }
    fn rec_play_start(&self, id: &str, at_ms: u64) -> Result<(), String> {
        crate::camrec::for_hub(&self.hub).play_start(&self.who, id, at_ms)
    }
    fn rec_frame(&self, id: &str, at_ms: u64, seek: api::RecSeek) -> Result<Option<api::PlayFrame>, String> {
        let seek = match seek {
            api::RecSeek::AtOrAfter => crate::camrec::Seek::AtOrAfter,
            api::RecSeek::After => crate::camrec::Seek::After,
            api::RecSeek::Before => crate::camrec::Seek::Before,
        };
        crate::camrec::for_hub(&self.hub).playback_frame(&self.who, id, at_ms, seek).map(|o| o.map(|f| api::PlayFrame { ts_ms: f.ts_ms, label: f.label, width: f.width, height: f.height, rgba: f.rgba, prev_ms: f.prev_ms, next_ms: f.next_ms }))
    }
    fn rec_export(&self, id: &str, from_ms: u64, to_ms: u64, confirmed: bool) -> Result<String, String> {
        crate::camrec::for_hub(&self.hub).export(&self.who, id, from_ms, to_ms, confirmed).map(|o| format!("{} ({} pictures){}", o.path.display(), o.frames, o.note))
    }
    fn motion_ids(&self) -> Vec<String> {
        crate::camdetect::watch::analysing_ids(&self.hub.data_dir())
    }
    fn motion_info(&self, id: &str) -> Option<api::MotionInfo> {
        let m = crate::camdetect::watch::for_hub(&self.hub);
        let st = m.status(&self.who, id).ok()?;
        let c = &st.settings;
        let shape = |z: &iyakki_camdetect::settings::ShapeRec| api::ShapeInfo { name: z.name.clone(), include: z.include, sensitivity: z.sensitivity, points: z.points.iter().map(|p| (p[0], p[1])).collect() };
        Some(api::MotionInfo {
            mode: motion_mode_to_api(c.mode),
            sensitivity: c.sensitivity,
            zones: c.zones.iter().map(shape).collect(),
            masks: c.masks.iter().map(shape).collect(),
            mask_storage: c.mask_storage,
            alerts_on: c.alerts.enabled,
            alerts_max_per_hour: c.alerts.max_per_hour,
            quiet: c.alerts.quiet.clone(),
            alerts_thumbnail: c.alerts.thumbnail,
            person: c.person,
            person_note: st.person_note,
            state: st.state,
            analysing: st.analysing,
            owner: st.owner,
            keep_days: st.keep_days,
        })
    }
    fn motion_set(&self, id: &str, c: &api::MotionChange) -> Result<(), String> {
        use iyakki_camdetect::settings::WatchMode;
        let m = crate::camdetect::watch::for_hub(&self.hub);
        let any_alert = c.alerts_on.is_some() || c.alerts_max_per_hour.is_some() || c.quiet.is_some() || c.alerts_thumbnail.is_some();
        let alerts = any_alert.then(|| crate::camdetect::watch::AlertChange { enabled: c.alerts_on, max_per_hour: c.alerts_max_per_hour, quiet: c.quiet.clone(), thumbnail: c.alerts_thumbnail, group_s: None });
        let mode = c.mode.map(|m| match m {
            api::MotionMode::Off => WatchMode::Off,
            api::MotionMode::Watched => WatchMode::Watched,
            api::MotionMode::Always => WatchMode::Always,
        });
        m.set(&self.who, id, &crate::camdetect::watch::Change { mode, sensitivity: c.sensitivity, mask_storage: c.mask_storage, person: None, alerts }).map(|_| ())
    }
    fn motion_shape_set(&self, id: &str, mask: bool, s: &api::ShapeInfo) -> Result<(), String> {
        use iyakki_camdetect::geom::Pt;
        let m = crate::camdetect::watch::for_hub(&self.hub);
        let pts: Vec<Pt> = s.points.iter().map(|p| Pt::new(p.0, p.1)).collect();
        let kind = if mask { crate::camdetect::watch::ShapeKind::Mask } else { crate::camdetect::watch::ShapeKind::Zone };
        m.shape_set(&self.who, id, kind, &s.name, s.include, s.sensitivity, &pts)
    }
    fn motion_shape_remove(&self, id: &str, mask: bool, name: &str) -> Result<(), String> {
        let m = crate::camdetect::watch::for_hub(&self.hub);
        m.shape_remove(&self.who, id, if mask { crate::camdetect::watch::ShapeKind::Mask } else { crate::camdetect::watch::ShapeKind::Zone }, name)
    }
    fn motion_events(&self, id: &str, from_ms: u64, to_ms: u64, limit: usize) -> Result<Vec<api::MotionRow>, String> {
        let m = crate::camdetect::watch::for_hub(&self.hub);
        m.events(&self.who, id, from_ms, to_ms, limit).map(|v| v.into_iter().map(|e| api::MotionRow { id: e.id, start_ms: e.start_ms, end_ms: e.end_ms, label: m.time_text(e.start_ms), zone: e.zone, score: e.peak.max(e.score), person: e.person, has_thumb: e.has_thumb }).collect())
    }
    fn motion_thumb(&self, id: &str, event: u64) -> Result<Option<api::Frame>, String> {
        let m = crate::camdetect::watch::for_hub(&self.hub);
        m.thumb(&self.who, id, event).map(|o| o.map(|(width, height, rgba)| api::Frame { seq: event, width, height, rgba: Arc::new(rgba) }))
    }

    fn rec_audit(&self, id: &str) -> Result<Vec<api::AuditRow>, String> {
        let rec = crate::camrec::for_hub(&self.hub);
        rec.audit(&self.who, id, 200).map(|v| v.into_iter().map(|l| api::AuditRow { time: rec.time_text(l.t * 1000), who: l.who, what: if l.detail.is_empty() { l.ev } else { format!("{}: {}", l.ev, l.detail) }, ok: l.ok }).collect())
    }
}


/// What the hub needs to know about a person who is not at the screen: their home and file rules, whether they manage cameras and may use this
/// computer's own. `policy` is `None` for a person who no longer exists: they get a guest's rights and nothing else.
pub fn who_of(policy: Option<iyakki_sec::policy::Policy>, person: &str, data: &std::path::Path) -> Who {
    let known = policy.is_some();
    let policy = policy.unwrap_or_else(|| iyakki_sec::policy::Role::Guest.template());
    let home = crate::scope::file_roots(&policy, person, data).home;
    let (p2, u2) = (policy.clone(), person.to_string());
    let may_write: Arc<dyn Fn(&std::path::Path) -> bool + Send + Sync> = Arc::new(move |dir: &std::path::Path| p2.allows_write(&u2, &dir.display().to_string()));
    Who { user: person.to_string(), manage: known && policy.manage_settings, usb_ok: known && policy.allows_app(iyakki_sec::policy::App::Camera), home, may_write }
}

/// What the red sign says: the cameras that record, and the cameras watched for motion all the time (a camera watched only while its
/// picture is on a screen or recorded needs no sign of its own: the picture or the REC sign is the indicator).
pub fn sign_names(data: &std::path::Path) -> Vec<String> {
    let mut v = crate::camrec::active_names(data);
    v.extend(crate::camdetect::watch::active_names(data).into_iter().map(|n| format!("{n} (motion watch)")));
    v
}

/// The click on the red sign: every recording and every all-the-time motion watch of the signed-in person's cameras stops.
pub fn stop_all_signs(data: &std::path::Path, user: &str) -> usize {
    crate::camrec::stop_all_for(data, user) + crate::camdetect::watch::stop_all_for(data, user)
}

/// The installation's motion watcher (F-CAM-04) over `hub`, with the inbox (and the vision pass) as the place its notices go.
pub fn motion_from(services: &Arc<crate::services::Services>, hub: &Arc<Hub>) -> Arc<crate::camdetect::watch::MotionService> {
    let m = crate::camdetect::watch::for_hub(hub);
    if !m.has_sink() {
        m.set_sink(Arc::new(crate::camvision::VisionSink { inner: Arc::new(crate::camdetect::watch::InboxSink(services.clone())), svc: services.clone() }));
    }
    m
}

/// At sign-in: start supervising recordings and the motion watcher over `hub`.
pub fn resume(services: &Arc<crate::services::Services>, hub: &Arc<Hub>) {
    crate::camrec::for_hub(hub);
    motion_from(services, hub);
}

/// The assistant's camera tools around `inner`, with the installation's recorder and motion watcher.
pub fn camera_tools(inner: Arc<dyn crate::agent::Tools>, hub: Arc<Hub>, who: Who, services: &Arc<crate::services::Services>) -> Arc<dyn crate::agent::Tools> {
    let (rec, motion) = (crate::camrec::for_hub(&hub), motion_from(services, &hub));
    Arc::new(crate::camerastools::CameraViewTools::new(inner, hub, who, services.clone()).with_recorder(rec).with_motion(motion))
}
