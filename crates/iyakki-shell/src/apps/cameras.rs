//! CAMERAS: the tile lives in the `iyakki-tile-cameras` crate (it does not depend on the agent). This file is the shell's side: the
//! installation's `cameras::Hub` (saved cameras, streams, access rules, the owner's vault) behind the tile's `CameraSource`, this
//! computer's USB cameras through the shared camera module (`crate::camera`, V4L2) as a `UsbProvider`, the person's file rules for
//! snapshots, and the adapter that makes the tile crate's `App` an `App` of the shell.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iyakki_agent::camrec as rec_agent;
use iyakki_agent::cameras as agent;
use iyakki_tile_api::cameras as api;
use iyakki_tile_api::{App as TileApp, TileCx};

use super::{App, AppCx, Flow};
use crate::camera::{self, CamDevice, Capture, Opener, Pix, Presence};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub const ACCENT: [u8; 4] = iyakki_tile_cameras::ACCENT;

// ------------------------------------------------------------------------------------------------ this computer's USB cameras

/// The cameras plugged into this computer, found and opened through the shared camera module.
pub struct ShellUsb {
    presence: Mutex<Presence>,
    opener: Arc<dyn Opener>,
    cache: Mutex<Option<(Instant, Vec<(CamDevice, agent::UsbCam)>)>>,
}

impl ShellUsb {
    pub fn system() -> ShellUsb {
        ShellUsb::with(Presence::system(), Arc::new(camera::v4l2::V4l2Opener))
    }

    pub fn with(presence: Presence, opener: Arc<dyn Opener>) -> ShellUsb {
        ShellUsb { presence: Mutex::new(presence), opener, cache: Mutex::new(None) }
    }

    /// One entry per physical camera, looked up at most once a second (the tile asks about every frame).
    fn devices(&self) -> Vec<(CamDevice, agent::UsbCam)> {
        let mut c = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((t, v)) = c.as_ref() {
            if t.elapsed() < Duration::from_secs(1) {
                return v.clone();
            }
        }
        let mut p = self.presence.lock().unwrap_or_else(|e| e.into_inner());
        p.scan();
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        for d in p.cameras() {
            if seen.contains(&d.physical) {
                continue;
            }
            seen.push(d.physical.clone());
            out.push((d.clone(), agent::UsbCam { id: d.node.clone(), name: d.name.clone(), builtin: d.builtin }));
        }
        *c = Some((Instant::now(), out.clone()));
        out
    }
}

struct UsbFeed(Box<dyn Capture>);

impl agent::Feed for UsbFeed {
    fn next(&mut self) -> Result<agent::Picture, agent::FeedErr> {
        match self.0.next(500) {
            Ok(Some(f)) => match f.pix {
                Pix::Mjpeg => match camera::jpeg::with_huffman(&f.data) {
                    Some(j) => Ok(agent::Picture::Jpeg(j.into_owned())),
                    None => Err(agent::FeedErr::Timeout),
                },
                Pix::Yuyv => f.to_rgba().map(|(width, height, data)| agent::Picture::Rgba { width, height, data }).map_err(agent::FeedErr::Lost),
            },
            Ok(None) => Err(agent::FeedErr::Timeout),
            Err(e) => Err(agent::FeedErr::Lost(e)),
        }
    }
}

impl agent::UsbProvider for ShellUsb {
    fn list(&self) -> Vec<agent::UsbCam> {
        self.devices().into_iter().map(|(_, c)| c).collect()
    }

    fn open(&self, id: &str) -> Result<Box<dyn agent::Feed>, agent::FeedErr> {
        let dev = self.devices().into_iter().find(|(_, c)| c.id == id).map(|(d, _)| d).ok_or_else(|| agent::FeedErr::Lost("the camera was unplugged".into()))?;
        match self.opener.open(&dev, None) {
            Ok(o) => Ok(Box::new(UsbFeed(o.cap))),
            Err(e) if e.contains("busy") || e.contains("EBUSY") => Err(agent::FeedErr::Busy("the camera is in use by another app (is the CAMERA app open?)".into())),
            Err(e) => Err(agent::FeedErr::Lost(e)),
        }
    }
}

fn system_usb() -> Arc<ShellUsb> {
    static U: OnceLock<Arc<ShellUsb>> = OnceLock::new();
    U.get_or_init(|| Arc::new(ShellUsb::system())).clone()
}

// ------------------------------------------------------------------------------------------------ the hub and the person

/// The installation's hub, made with `usb` the first time (the real wiring uses [`hub_for`]).
pub fn hub_with(cx: &AppCx, usb: Arc<dyn agent::UsbProvider>) -> Arc<agent::Hub> {
    hub_from(&cx.services, &cx.audit, &cx.store, usb)
}

fn hub_from(services: &Arc<iyakki_agent::services::Services>, audit: &Arc<crate::accounts::AuditLog>, store: &Arc<Mutex<crate::accounts::UserStore>>, usb: Arc<dyn agent::UsbProvider>) -> Arc<agent::Hub> {
    let (svc, audit, store) = (services.clone(), audit.clone(), store.clone());
    agent::shared(agent::Env {
        data: services.dir().to_path_buf(),
        secrets: Arc::new(agent::VaultCamSecrets(svc)),
        people: Arc::new(move || store.lock().map(|s| s.list().iter().map(|u| u.name.clone()).collect()).unwrap_or_default()),
        usb,
        h264: Arc::new(agent::NoH264),
        encode: Arc::new(|w, h, d| camera::jpeg::encode_rgba(w, h, d, 88)),
        after_save: Arc::new(|p| camera::own::give_saved(p)),
        audit: Arc::new(move |u, e, d| audit.record(u, e, d)),
        backoff_ms: 1000,
        idle: agent::IDLE_STOP,
    })
}

pub fn hub_for(cx: &AppCx) -> Arc<agent::Hub> {
    hub_with(cx, system_usb())
}

/// The installation's recorder (F-CAM-03). Recording that its owner switched on resumes when the owner signs in (the key is in their
/// vault), see [`resume_recording`].
pub fn recorder_for(cx: &AppCx) -> Arc<iyakki_agent::camrec::Recorder> {
    iyakki_agent::camrec::for_hub(&hub_for(cx))
}

/// At sign-in: start supervising recordings (those switched on start again by themselves) and the retention checks.
pub fn resume_recording(services: &Arc<iyakki_agent::services::Services>, audit: &Arc<crate::accounts::AuditLog>, store: &Arc<Mutex<crate::accounts::UserStore>>) {
    iyakki_agent::camrec::for_hub(&hub_from(services, audit, store, system_usb()));
}

/// The signed-in person as the hub sees them: their home and file rules decide where a snapshot may go.
pub fn who_for(cx: &AppCx) -> agent::Who {
    let (user, policy, data) = (cx.session.user.clone(), cx.session.policy.clone(), cx.services.dir().to_path_buf());
    let home = iyakki_agent::scope::FileRoots::new(&policy, &user, &data).home;
    let (p2, u2) = (policy.clone(), user.clone());
    let may_write: Arc<dyn Fn(&std::path::Path) -> bool + Send + Sync> = Arc::new(move |dir: &std::path::Path| p2.allows_write(&u2, &dir.display().to_string()));
    agent::Who { user, manage: policy.manage_settings, usb_ok: policy.allows_app(iyakki_sec::policy::App::Camera), home, may_write }
}

static REC_SIGN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the red REC sign has to appear or go away since the last call (the shell redraws when it does).
pub fn rec_sign_changed(data: &std::path::Path) -> bool {
    let on = !iyakki_agent::camrec::active_names(data).is_empty();
    on != REC_SIGN.swap(on, std::sync::atomic::Ordering::SeqCst)
}

/// The assistant's camera tools around `inner`.
pub fn tools_for(cx: &AppCx, inner: Arc<dyn iyakki_agent::agent::Tools>) -> Arc<dyn iyakki_agent::agent::Tools> {
    Arc::new(iyakki_agent::camerastools::CameraViewTools::new(inner, hub_for(cx), who_for(cx), cx.services.clone()).with_recorder(recorder_for(cx)))
}

// ------------------------------------------------------------------------------------------------ the tile's source

pub struct ShellCameras {
    pub hub: Arc<agent::Hub>,
    pub who: agent::Who,
}

fn status_to_api(s: agent::Status) -> api::Status {
    match s {
        agent::Status::Connecting => api::Status::Connecting,
        agent::Status::Live => api::Status::Live,
        agent::Status::Lost(m) => api::Status::Lost(m),
        agent::Status::NeedsAddon(m) => api::Status::NeedsAddon(m),
        agent::Status::Busy(m) => api::Status::Busy(m),
        agent::Status::NeedsPassword(m) => api::Status::NeedsPassword(m),
    }
}

impl api::CameraSource for ShellCameras {
    fn cameras(&self) -> Vec<api::CamInfo> {
        self.hub
            .cameras(&self.who)
            .into_iter()
            .map(|v| api::CamInfo {
                id: v.id,
                name: v.name,
                kind: if v.kind == agent::Kind::Usb { api::Kind::Usb } else { api::Kind::Network },
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
        self.hub.view(&self.who, id, big).ok().flatten().map(|f| api::Frame { seq: f.seq, width: f.width, height: f.height, rgba: f.rgba })
    }
    fn snapshot(&self, id: &str) -> Result<String, String> {
        self.hub.snapshot(&self.who, id).map(|p: PathBuf| p.display().to_string())
    }
    fn save(&self, replacing: Option<&str>, cam: &api::NewCam, password: Option<&str>) -> Result<String, String> {
        self.hub.save(&self.who, replacing, &agent::NewSpec { name: cam.name.clone(), address: cam.address.clone(), user: cam.user.clone(), allowed: cam.allowed.clone() }, password)
    }
    fn remove(&self, id: &str) -> Result<(), String> {
        self.hub.remove(&self.who, id)
    }
    fn people(&self) -> Vec<String> {
        self.hub.people(&self.who)
    }

    fn recording_ids(&self) -> Vec<String> {
        iyakki_agent::camrec::active_ids(&self.hub.data_dir())
    }
    fn rec_info(&self, id: &str) -> Option<api::RecInfo> {
        let rec = iyakki_agent::camrec::for_hub(&self.hub);
        let st = rec.status(&self.who, id).ok()?;
        Some(api::RecInfo {
            mode: match st.settings.mode {
                rec_agent::Mode::Off => api::RecMode::Off,
                rec_agent::Mode::Continuous => api::RecMode::Continuous,
                rec_agent::Mode::Schedule => api::RecMode::Schedule,
            },
            windows: st.settings.windows.iter().map(rec_agent::Window::text).collect(),
            max_days: st.settings.max_days,
            max_mb: st.settings.max_mb,
            state: st.state.words(),
            recording: st.recording,
            problem: matches!(st.state, rec_agent::RecState::Problem(_)),
            owner: st.owner,
            segments: st.segments,
            bytes: st.bytes,
            key_note: st.key_note.to_string(),
        })
    }
    fn rec_set(&self, id: &str, c: &api::RecChange) -> Result<(), String> {
        let rec = iyakki_agent::camrec::for_hub(&self.hub);
        let windows = match &c.windows {
            Some(w) => Some(w.iter().map(|t| rec_agent::Window::parse(t)).collect::<Result<Vec<_>, _>>()?),
            None => None,
        };
        let mode = c.mode.map(|m| match m {
            api::RecMode::Off => rec_agent::Mode::Off,
            api::RecMode::Continuous => rec_agent::Mode::Continuous,
            api::RecMode::Schedule => rec_agent::Mode::Schedule,
        });
        rec.set(&self.who, id, &rec_agent::Change { mode, windows, max_days: c.max_days, max_mb: c.max_mb }).map(|_| ())
    }
    fn rec_days(&self, id: &str) -> Result<Vec<api::RecDay>, String> {
        let rec = iyakki_agent::camrec::for_hub(&self.hub);
        rec.days(&self.who, id).map(|v| v.into_iter().map(|d| api::RecDay { label: d.label, day_start_ms: d.day_start_ms, spans: d.spans, frames: d.frames, bytes: d.bytes }).collect())
    }
    fn rec_play_start(&self, id: &str, at_ms: u64) -> Result<(), String> {
        iyakki_agent::camrec::for_hub(&self.hub).play_start(&self.who, id, at_ms)
    }
    fn rec_frame(&self, id: &str, at_ms: u64, seek: api::RecSeek) -> Result<Option<api::PlayFrame>, String> {
        let seek = match seek {
            api::RecSeek::AtOrAfter => rec_agent::Seek::AtOrAfter,
            api::RecSeek::After => rec_agent::Seek::After,
            api::RecSeek::Before => rec_agent::Seek::Before,
        };
        iyakki_agent::camrec::for_hub(&self.hub).playback_frame(&self.who, id, at_ms, seek).map(|o| o.map(|f| api::PlayFrame { ts_ms: f.ts_ms, label: f.label, width: f.width, height: f.height, rgba: f.rgba, prev_ms: f.prev_ms, next_ms: f.next_ms }))
    }
    fn rec_export(&self, id: &str, from_ms: u64, to_ms: u64, confirmed: bool) -> Result<String, String> {
        iyakki_agent::camrec::for_hub(&self.hub).export(&self.who, id, from_ms, to_ms, confirmed).map(|o| format!("{} ({} pictures){}", o.path.display(), o.frames, o.note))
    }
    fn rec_audit(&self, id: &str) -> Result<Vec<api::AuditRow>, String> {
        let rec = iyakki_agent::camrec::for_hub(&self.hub);
        rec.audit(&self.who, id, 200).map(|v| v.into_iter().map(|l| api::AuditRow { time: rec.time_text(l.t * 1000), who: l.who, what: if l.detail.is_empty() { l.ev } else { format!("{}: {}", l.ev, l.detail) }, ok: l.ok }).collect())
    }
}

/// The tile crate's app, as an app of the shell.
pub struct CamerasApp(iyakki_tile_cameras::CamerasApp);

impl CamerasApp {
    pub fn new(_cx: &AppCx) -> CamerasApp {
        CamerasApp(iyakki_tile_cameras::CamerasApp::new())
    }

    fn with<T>(cx: &AppCx, f: impl FnOnce(&TileCx) -> T) -> T {
        let src = ShellCameras { hub: hub_for(cx), who: who_for(cx) };
        f(&TileCx { user: cx.session.user.as_str(), manage_settings: cx.session.policy.manage_settings, usage: &super::usage::ShellUsage(&cx.services), calendar: &iyakki_tile_api::calendar::NoCalendar, audit: &iyakki_tile_api::NoAudit, remote: &iyakki_tile_api::remote::NoRemote, cameras: &src })
    }

    /// What the tile did on its last frame (tests).
    pub fn inner(&self) -> &iyakki_tile_cameras::CamerasApp {
        &self.0
    }
}

impl App for CamerasApp {
    fn title(&self) -> &'static str {
        self.0.title()
    }
    fn icon(&self) -> Icon {
        self.0.icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.0.accent()
    }
    fn hint(&self) -> String {
        self.0.hint()
    }
    fn help(&self) -> Vec<String> {
        self.0.help()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match Self::with(cx, |t| self.0.handle(ev, t)) {
            iyakki_tile_api::Flow::Stay => Flow::Stay,
            iyakki_tile_api::Flow::Back => Flow::Back,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        Self::with(cx, |t| self.0.render(g, area, s, t, tick));
    }
}

#[cfg(test)]
#[path = "cameras_tests.rs"]
mod tests;
