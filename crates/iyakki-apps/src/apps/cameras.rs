//! CAMERAS: the tile lives in the `iyakki-tile-cameras` crate (it does not depend on the agent). This file is the shell's side: the
//! installation's `cameras::Hub` (saved cameras, streams, access rules, the owner's vault) behind the tile's `CameraSource`, this
//! computer's USB cameras through the shared camera module (`crate::camera`, V4L2) as a `UsbProvider`, the person's file rules for
//! snapshots, and the adapter that makes the tile crate's `App` an `App` of the shell.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iyakki_shared::cameras as agent;
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
pub fn hub_with(cx: &AppCx, usb: Arc<dyn agent::UsbProvider>) -> Arc<dyn std::any::Any + Send + Sync> {
    hub_from(&cx.b4(), &cx.audit, &cx.store, usb)
}

fn hub_from(b4: &Arc<dyn iyakki_agent_api::ServicesFacetB4>, audit: &Arc<crate::accounts::AuditLog>, store: &Arc<Mutex<crate::accounts::UserStore>>, usb: Arc<dyn agent::UsbProvider>) -> Arc<dyn std::any::Any + Send + Sync> {
    let (audit, store) = (audit.clone(), store.clone());
    b4.camera_hub(
            Box::new(usb),
            Arc::new(move || store.lock().map(|s| s.list().iter().map(|u| u.name.clone()).collect()).unwrap_or_default()),
            Arc::new(|w, h, d| camera::jpeg::encode_rgba(w, h, d, 88)),
            Arc::new(|p| camera::own::give_saved(p)),
            Arc::new(move |u, e, d| audit.record(u, e, d)),
            &crate::hostname::current("iyakki"),
        )
        .expect("the agent core serves the cameras")
}

pub fn hub_for(cx: &AppCx) -> Arc<dyn std::any::Any + Send + Sync> {
    hub_with(cx, system_usb())
}

/// The cameras of this machine as frames for a client (EXPERIMENTAL, `clientmedia`): the installation's hub, and for each person the
/// rights their account has (their home, the CAMERA app being allowed). A person who no longer exists sees nothing.
pub fn client_feed(b4: &Arc<dyn iyakki_agent_api::ServicesFacetB4>, data: &std::path::Path, audit: &Arc<crate::accounts::AuditLog>, store: &Arc<Mutex<crate::accounts::UserStore>>) -> Arc<dyn iyakki_shared::clientmedia::CameraFeed> {
    let hub = hub_from(b4, audit, store, system_usb());
    let (store, data) = (store.clone(), data.to_path_buf());
    let who = Arc::new(move |person: &str| client_who(&store, &data, person));
    let who: Arc<dyn Fn(&str) -> Box<dyn std::any::Any + Send> + Send + Sync> = Arc::new(move |p: &str| Box::new(who(p)) as Box<dyn std::any::Any + Send>);
    crate::agent().helpers_b6().camera_feed(hub, who).expect("the agent core serves the camera feed")
}

/// What the hub needs to know about a person who is not at the screen (a client session).
pub fn client_who(store: &Arc<Mutex<crate::accounts::UserStore>>, data: &std::path::Path, person: &str) -> agent::Who {
    let policy = store.lock().ok().and_then(|s| s.get(person).map(|u| u.policy()));
    // a person who no longer exists gets a guest's rights and nothing else
    let known = policy.is_some();
    let policy = policy.unwrap_or_else(|| iyakki_sec::policy::Role::Guest.template());
    let home = crate::file_scope().roots(&policy, person, data).home;
    let (p2, u2) = (policy.clone(), person.to_string());
    let may_write: Arc<dyn Fn(&std::path::Path) -> bool + Send + Sync> = Arc::new(move |dir: &std::path::Path| p2.allows_write(&u2, &dir.display().to_string()));
    agent::Who { user: person.to_string(), manage: known && policy.manage_settings, usb_ok: known && policy.allows_app(iyakki_sec::policy::App::Camera), home, may_write }
}

/// At sign-in: start supervising recordings (those switched on start again by themselves) and the retention checks, and the motion
/// watcher (watching that the owner switched on starts again once their vault is open).
pub fn resume_recording(b4: &Arc<dyn iyakki_agent_api::ServicesFacetB4>, audit: &Arc<crate::accounts::AuditLog>, store: &Arc<Mutex<crate::accounts::UserStore>>) {
    let hub = hub_from(b4, audit, store, system_usb());
    b4.camera_resume(hub);
}

/// The signed-in person as the hub sees them: their home and file rules decide where a snapshot may go.
pub fn who_for(cx: &AppCx) -> agent::Who {
    let (user, policy, data) = (cx.session.user.clone(), cx.session.policy.clone(), cx.data_dir().to_path_buf());
    let home = cx.core.file_scope().roots(&policy, &user, &data).home;
    let (p2, u2) = (policy.clone(), user.clone());
    let may_write: Arc<dyn Fn(&std::path::Path) -> bool + Send + Sync> = Arc::new(move |dir: &std::path::Path| p2.allows_write(&u2, &dir.display().to_string()));
    agent::Who { user, manage: policy.manage_settings, usb_ok: policy.allows_app(iyakki_sec::policy::App::Camera), home, may_write }
}

/// Whether the red sign was on at the last look, per data folder (one machine has one folder; tests that run side by side each have their own, so one
/// test's redraw never makes another test's sign look unchanged).
static REC_SIGN: std::sync::Mutex<Vec<(std::path::PathBuf, bool)>> = std::sync::Mutex::new(Vec::new());

/// The cameras that record or are watched for motion all the time (the red sign).
pub fn sign_names(data: &std::path::Path) -> Vec<String> {
    crate::agent().helpers_b4().camera_sign_names(data)
}

/// The click on the red sign: every recording and motion watch of `user` stops.
pub fn stop_all_signs(data: &std::path::Path, user: &str) -> usize {
    crate::agent().helpers_b4().camera_stop_all_signs(data, user)
}

/// What the red sign says, in one line. Recordings: "REC: a, b. Click to stop." A client's camera or microphone that streams to this
/// machine (EXPERIMENTAL, `clientmedia::indicator`) is added to the same sign, never shown in a second one: "REC: a. X is in use by this
/// machine. Click to stop." (the click stops recordings; a client device stops from the client or by revoking it in Settings).
pub fn sign_text(names: &[String], client: Option<&str>) -> String {
    let rec = (!names.is_empty()).then(|| format!("REC: {}.", names.join(", ")));
    match (rec, client) {
        (Some(r), Some(c)) => format!("{r} {c}. Click to stop."),
        (Some(r), None) => format!("{r} Click to stop."),
        (None, Some(c)) => format!("{c}."),
        (None, None) => String::new(),
    }
}

/// Whether the red REC sign has to appear or go away since the last call (the shell redraws when it does).
pub fn rec_sign_changed(data: &std::path::Path) -> bool {
    let on = !sign_names(data).is_empty() || crate::agent().client_media().indicator().active();
    let mut seen = REC_SIGN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match seen.iter_mut().find(|(d, _)| d == data) {
        Some((_, was)) => std::mem::replace(was, on) != on,
        None => {
            seen.push((data.to_path_buf(), on));
            on
        }
    }
}

/// The assistant's camera tools around `inner`: the installation's recorder (F-CAM-03, whose recording the owner's sign-in resumes, see
/// [`resume_recording`]) and motion watcher (F-CAM-04, with the inbox as the place its notices go) belong to them.
pub fn tools_for(cx: &AppCx, inner: Arc<dyn iyakki_shared::agent::Tools>) -> Arc<dyn iyakki_shared::agent::Tools> {
    cx.b4().camera_tools(inner, hub_for(cx), Box::new(who_for(cx)))
}

// ------------------------------------------------------------------------------------------------ the tile's source


/// The tile crate's app, as an app of the shell.
pub struct CamerasApp(iyakki_tile_cameras::CamerasApp);

impl CamerasApp {
    pub fn new(_cx: &AppCx) -> CamerasApp {
        CamerasApp(iyakki_tile_cameras::CamerasApp::new())
    }

    fn with<T>(cx: &AppCx, f: impl FnOnce(&TileCx) -> T) -> T {
        let src = cx.b4().camera_source(hub_for(cx), Box::new(who_for(cx))).expect("the agent core serves the cameras");
        f(&TileCx { user: cx.session.user.as_str(), manage_settings: cx.session.policy.manage_settings, usage: &super::usage::ShellUsage::of(cx), calendar: &iyakki_tile_api::calendar::NoCalendar, audit: &iyakki_tile_api::NoAudit, remote: &iyakki_tile_api::remote::NoRemote, cameras: &*src, radio: &iyakki_tile_api::radio::NoRadio })
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
