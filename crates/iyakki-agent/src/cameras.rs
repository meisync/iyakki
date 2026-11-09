//! Live camera view (F-CAM-02): the cameras of the Cameras tile and of the assistant's `cameras_*` tools.
//!
//! * **Sources.** USB cameras of this computer (through a [`UsbProvider`] the shell supplies, which uses the shared V4L2 module) and
//!   network cameras saved by a person: an `rtsp://` address (the in-house client in `iyakki-rtsp`: TCP interleaved, Basic / Digest,
//!   Motion-JPEG and H.264 packets) or an `http://` Motion-JPEG / snapshot address.
//! * **Secrets.** A camera's saved record (`<data>/cameras/cameras.json`, mode 0600) holds the name, the address *without* a user name
//!   or password, the user name, the owner and the list of allowed people, and the *name* of a vault entry. The password lives in the
//!   owner's vault. An address that arrives with `user:password@` is split: the password goes to the vault and the record keeps the
//!   clean address. Displays, audit lines and error messages go through [`iyakki_rtsp::url::scrub`].
//! * **Access.** The owner and the people on the camera's list may watch; administrators may manage (change the list, remove) but are
//!   not given the picture for that. Every call checks it, in the tile and in the tools alike.
//! * **Streams run only while somebody looks.** Every [`Hub::view`] keeps a camera's stream alive for [`IDLE_STOP`]; after that the
//!   connection (or the USB device) is released. A lost stream is retried with a growing pause. Pictures are decoded at most
//!   [`SMALL_FPS`] / [`BIG_FPS`] times a second, whatever the camera sends.
//! * **H.264** needs a decoder this system does not ship. The packets are read and the camera shows "needs the audio and video add-on"
//!   (the person-clicked download); an [`H264Support`] with a decoder plugs in without changing anything else.
//! * Nothing here records. A snapshot is a person's click or an approved tool call and writes one JPEG into their Pictures.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use iyakki_rtsp as rtsp;
use iyakki_rtsp::rtp::AccessUnit;
use iyakki_rtsp::url::scrub;

pub const MAX_CAMERAS: usize = 64;
pub const MAX_PEOPLE: usize = 32;
/// A stream stops this long after the last [`Hub::view`].
pub const IDLE_STOP: Duration = Duration::from_secs(8);
pub const SMALL_FPS: u32 = 5;
pub const BIG_FPS: u32 = 10;
/// Width of the picture handed to a grid cell and to the enlarged view.
pub const SMALL_W: u32 = 480;
pub const BIG_W: u32 = 1280;

// ------------------------------------------------------------------------------------------------ records

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spec {
    pub id: String,
    pub name: String,
    /// `rtsp://host:port/path` or `http://...`; never a user name or password.
    pub address: String,
    pub user: String,
    pub owner: String,
    #[serde(default)]
    pub allowed: Vec<String>,
    /// The owner's vault entry that holds the password (empty: none).
    #[serde(default)]
    pub secret: String,
}

/// What a person enters.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct NewSpec {
    pub name: String,
    pub address: String,
    pub user: String,
    pub allowed: Vec<String>,
}

pub fn can_view(s: &Spec, user: &str) -> bool {
    s.owner == user || s.allowed.iter().any(|a| a == user)
}

/// The person using the hub.
#[derive(Clone)]
pub struct Who {
    pub user: String,
    /// May manage cameras of others (administrators).
    pub manage: bool,
    /// May use this computer's USB cameras (the CAMERA app is allowed for the account).
    pub usb_ok: bool,
    pub home: PathBuf,
    pub may_write: Arc<dyn Fn(&Path) -> bool + Send + Sync>,
}

impl Who {
    fn can_manage(&self, s: &Spec) -> bool {
        s.owner == self.user || self.manage
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Usb,
    Network,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Live,
    Lost(String),
    NeedsAddon(String),
    Busy(String),
    NeedsPassword(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub address: String,
    pub user: String,
    pub status: Status,
    pub fps: f32,
    pub size: (u32, u32),
    pub manage: bool,
    pub owner: String,
    pub allowed: Vec<String>,
    pub has_password: bool,
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

// ------------------------------------------------------------------------------------------------ seams

pub trait CamSecrets: Send + Sync {
    fn get(&self, owner: &str, name: &str) -> Result<Zeroizing<String>, String>;
    fn set(&self, owner: &str, name: &str, value: &str) -> Result<(), String>;
    fn delete(&self, owner: &str, name: &str) -> Result<(), String>;
    fn has(&self, owner: &str, name: &str) -> bool;
}

/// The owner's vault, through the services.
pub struct VaultCamSecrets(pub Arc<crate::services::Services>);

impl CamSecrets for VaultCamSecrets {
    fn get(&self, owner: &str, name: &str) -> Result<Zeroizing<String>, String> {
        self.0.user_get(owner, name).map_err(|e| format!("could not read the camera's password from {owner}'s vault: {e}"))
    }
    fn set(&self, owner: &str, name: &str, value: &str) -> Result<(), String> {
        self.0.user_set(owner, name, value).map_err(|e| format!("could not save the camera's password in your vault: {e}"))
    }
    fn delete(&self, owner: &str, name: &str) -> Result<(), String> {
        self.0.user_delete(owner, name).map_err(|e| e.to_string())
    }
    fn has(&self, owner: &str, name: &str) -> bool {
        self.0.user_names(owner).map(|n| n.iter().any(|x| x == name)).unwrap_or(false)
    }
}

pub enum Picture {
    /// A complete JPEG file.
    Jpeg(Vec<u8>),
    Rgba { width: u32, height: u32, data: Vec<u8> },
}

#[derive(Debug, PartialEq, Eq)]
pub enum FeedErr {
    /// Nothing arrived yet; ask again.
    Timeout,
    Lost(String),
    Addon(String),
    Auth(String),
    Busy(String),
}

/// One open source of pictures.
pub trait Feed: Send {
    fn next(&mut self) -> Result<Picture, FeedErr>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbCam {
    pub id: String,
    pub name: String,
    pub builtin: bool,
}

/// The cameras plugged into this computer. The shell implements it on top of its shared V4L2 module.
pub trait UsbProvider: Send + Sync {
    fn list(&self) -> Vec<UsbCam>;
    fn open(&self, id: &str) -> Result<Box<dyn Feed>, FeedErr>;
}

pub struct NoUsb;
impl UsbProvider for NoUsb {
    fn list(&self) -> Vec<UsbCam> {
        vec![]
    }
    fn open(&self, _: &str) -> Result<Box<dyn Feed>, FeedErr> {
        Err(FeedErr::Lost("no USB camera".into()))
    }
}

pub trait H264Decoder: Send {
    /// A picture when this access unit completes one (width, height, RGBA).
    fn decode(&mut self, au: &AccessUnit) -> Result<Option<(u32, u32, Vec<u8>)>, String>;
}

pub trait H264Support: Send + Sync {
    fn decoder(&self) -> Option<Box<dyn H264Decoder>>;
    /// What to tell the person when there is none.
    fn hint(&self) -> String;
}

/// No decoder: H.264 cameras are recognised and the person is pointed to the add-on.
pub struct NoH264;
impl H264Support for NoH264 {
    fn decoder(&self) -> Option<Box<dyn H264Decoder>> {
        None
    }
    fn hint(&self) -> String {
        "This camera sends H.264 video. Showing it needs the audio and video add-on (Store, Setup), which you download yourself. Until then, switch the camera to Motion-JPEG or use its snapshot address.".into()
    }
}

/// Something that wants a copy of the small decoded pictures of some cameras (motion watching, F-CAM-04). It is called on the stream's
/// own thread at most [`SMALL_FPS`] times a second per camera, and only for cameras it says it `wants`.
pub trait FrameTap: Send + Sync {
    fn wants(&self, camera: &str) -> bool;
    fn frame(&self, camera: &str, width: u32, height: u32, rgba: &[u8]);
}

pub struct Env {
    pub data: PathBuf,
    pub secrets: Arc<dyn CamSecrets>,
    /// Everybody with an account.
    pub people: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    pub usb: Arc<dyn UsbProvider>,
    pub h264: Arc<dyn H264Support>,
    /// RGBA to a JPEG file (for snapshots of pictures that did not arrive as JPEG).
    pub encode: Arc<dyn Fn(u32, u32, &[u8]) -> Vec<u8> + Send + Sync>,
    /// Called with every file a snapshot saved (the shell gives it to the account).
    pub after_save: Arc<dyn Fn(&Path) + Send + Sync>,
    /// (user, event, detail)
    pub audit: Arc<dyn Fn(&str, &str, &str) + Send + Sync>,
    /// First pause before a lost stream is tried again (doubles up to 30 s).
    pub backoff_ms: u64,
    /// How long a stream runs after the last look ([`IDLE_STOP`] in the real wiring).
    pub idle: Duration,
}

// ------------------------------------------------------------------------------------------------ the hub

struct St {
    status: Status,
    seq: u64,
    small: Option<Frame>,
    big: Option<Frame>,
    latest: Option<Arc<Picture>>,
    fps: f32,
    size: (u32, u32),
    touched: Instant,
    want_big: bool,
    last_small: Option<Instant>,
    last_big: Option<Instant>,
    /// The last time a person (not the motion watcher) asked for a picture.
    viewed: Option<Instant>,
}

struct Shared {
    st: Mutex<St>,
    stop: AtomicBool,
}

impl Shared {
    fn st(&self) -> std::sync::MutexGuard<'_, St> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }
}

type TapSlot = Arc<Mutex<Option<Arc<dyn FrameTap>>>>;

pub struct Hub {
    env: Env,
    streams: Mutex<HashMap<String, Arc<Shared>>>,
    last_view: Mutex<HashMap<(String, String), Instant>>,
    tap: TapSlot,
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Arc<Hub>>> {
    static R: OnceLock<Mutex<HashMap<PathBuf, Arc<Hub>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// The installation's hub (made with the first caller's environment, shared afterwards).
pub fn shared(env: Env) -> Arc<Hub> {
    let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
    r.entry(env.data.clone()).or_insert_with(|| Arc::new(Hub::new(env))).clone()
}

fn clean_text(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}

pub(crate) fn slug(name: &str) -> String {
    let s: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    s.chars().take(24).collect::<String>().trim_matches('-').to_string()
}

/// Shrink by a whole factor (box average) so the width is at most `max_w`.
pub fn shrink(w: u32, h: u32, rgba: &[u8], max_w: u32) -> (u32, u32, Vec<u8>) {
    if w <= max_w || w == 0 || h == 0 || rgba.len() < w as usize * h as usize * 4 {
        return (w, h, rgba.to_vec());
    }
    let k = w.div_ceil(max_w) as usize;
    let (nw, nh) = (w as usize / k, h as usize / k);
    let mut out = vec![255u8; nw * nh * 4];
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0u32; 3];
            for dy in 0..k {
                for dx in 0..k {
                    let i = ((y * k + dy) * w as usize + x * k + dx) * 4;
                    acc[0] += rgba[i] as u32;
                    acc[1] += rgba[i + 1] as u32;
                    acc[2] += rgba[i + 2] as u32;
                }
            }
            let n = (k * k) as u32;
            let o = (y * nw + x) * 4;
            out[o] = (acc[0] / n) as u8;
            out[o + 1] = (acc[1] / n) as u8;
            out[o + 2] = (acc[2] / n) as u8;
        }
    }
    (nw as u32, nh as u32, out)
}

impl Hub {
    pub fn new(env: Env) -> Hub {
        Hub { env, streams: Mutex::new(HashMap::new()), last_view: Mutex::new(HashMap::new()), tap: Arc::new(Mutex::new(None)) }
    }

    fn file(&self) -> PathBuf {
        self.env.data.join("cameras").join("cameras.json")
    }

    pub fn specs(&self) -> Vec<Spec> {
        std::fs::read(self.file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn write_specs(&self, v: &[Spec]) -> Result<(), String> {
        let f = self.file();
        std::fs::create_dir_all(f.parent().ok_or("no folder")?).map_err(|e| format!("could not save the cameras: {e}"))?;
        let tmp = f.with_extension("json.tmp");
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut t = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| format!("could not save the cameras: {e}"))?;
            t.write_all(&serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &f).map_err(|e| format!("could not save the cameras: {e}"))
    }

    fn audit(&self, who: &str, ev: &str, detail: &str) {
        (self.env.audit)(who, ev, &scrub(detail));
    }

    /// The installation's data folder.
    pub fn data_dir(&self) -> PathBuf {
        self.env.data.clone()
    }

    /// The environment (the recorder, `camrec`, shares the vault, the audit sink and the file hand-over).
    pub(crate) fn env(&self) -> &Env {
        &self.env
    }

    fn streams(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Shared>>> {
        self.streams.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ------------------------------------------------------------------------------ listing

    /// The cameras this person may watch: USB cameras of this computer first, then saved cameras they own or are allowed on.
    pub fn cameras(&self, who: &Who) -> Vec<View> {
        let mut out = Vec::new();
        if who.usb_ok {
            for u in self.env.usb.list() {
                let id = format!("usb:{}", u.id);
                let (status, fps, size) = self.state_of(&id);
                out.push(View { id, name: u.name, kind: Kind::Usb, address: String::new(), user: String::new(), status, fps, size, manage: false, owner: String::new(), allowed: vec![], has_password: false });
            }
        }
        for s in self.specs() {
            if !can_view(&s, &who.user) && !who.can_manage(&s) {
                continue;
            }
            let visible = can_view(&s, &who.user);
            let (mut status, fps, size) = self.state_of(&s.id);
            if !visible {
                status = Status::Connecting; // a manager who may not watch sees the record only
            }
            let has_password = !s.secret.is_empty() && self.env.secrets.has(&s.owner, &s.secret);
            out.push(View { id: s.id.clone(), name: s.name.clone(), kind: Kind::Network, address: s.address.clone(), user: s.user.clone(), status, fps, size, manage: who.can_manage(&s), owner: s.owner.clone(), allowed: s.allowed.clone(), has_password });
        }
        out
    }

    fn state_of(&self, id: &str) -> (Status, f32, (u32, u32)) {
        match self.streams().get(id) {
            Some(s) => {
                let st = s.st();
                (st.status.clone(), st.fps, st.size)
            }
            None => (Status::Connecting, 0.0, (0, 0)),
        }
    }

    /// Whether the person may watch camera `id`; the name to show on failure is generic on purpose.
    fn watchable(&self, who: &Who, id: &str) -> Result<Option<Spec>, String> {
        if let Some(u) = id.strip_prefix("usb:") {
            if !who.usb_ok {
                return Err("your account may not use this computer's cameras".into());
            }
            if !self.env.usb.list().iter().any(|c| c.id == u) {
                return Err("that camera is not connected".into());
            }
            return Ok(None);
        }
        match self.specs().into_iter().find(|s| s.id == id) {
            Some(s) if can_view(&s, &who.user) => Ok(Some(s)),
            _ => Err("no such camera, or you are not on its list of people who may watch".into()),
        }
    }

    /// A camera by id or name among those the person may watch; a name that fits two is refused.
    pub fn resolve(&self, who: &Who, key: &str) -> Result<View, String> {
        let key = key.trim();
        let all = self.cameras(who);
        if let Some(v) = all.iter().find(|v| v.id == key) {
            return Ok(v.clone());
        }
        let hits: Vec<&View> = all.iter().filter(|v| v.name.eq_ignore_ascii_case(key)).collect();
        match hits.len() {
            1 => Ok(hits[0].clone()),
            0 => Err(format!("no camera called “{}” that you may watch (cameras_list shows them)", clean_text(key, 60))),
            _ => Err(format!("more than one camera is called “{}”; use its id", clean_text(key, 60))),
        }
    }

    // ------------------------------------------------------------------------------ viewing

    /// The newest picture of a camera. Asking starts the stream when none runs and keeps it alive for [`IDLE_STOP`].
    pub fn view(&self, who: &Who, id: &str, big: bool) -> Result<Option<Frame>, String> {
        let spec = self.watchable(who, id)?;
        let sh = self.stream_for(id, spec, big);
        sh.st().viewed = Some(Instant::now());
        let mut st = sh.st();
        st.touched = Instant::now();
        st.want_big = big;
        let f = if big { st.big.clone().or_else(|| st.small.clone()) } else { st.small.clone() };
        drop(st);
        let mut lv = self.last_view.lock().unwrap_or_else(|e| e.into_inner());
        let key = (who.user.clone(), id.to_string());
        if lv.get(&key).is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
            lv.insert(key, Instant::now());
            let name = self.name_of(id);
            self.audit(&who.user, "camera.view", &name);
            crate::camrec::log_event(&self.env.data, id, &who.user, "live.view", "", true);
        }
        Ok(f)
    }

    fn stream_for(&self, id: &str, spec: Option<Spec>, big: bool) -> Arc<Shared> {
        let mut m = self.streams();
        m.retain(|_, s| !s.stop.load(Ordering::SeqCst));
        match m.get(id) {
            Some(s) => s.clone(),
            None => {
                let s = Arc::new(Shared {
                    st: Mutex::new(St { status: Status::Connecting, seq: 0, small: None, big: None, latest: None, fps: 0.0, size: (0, 0), touched: Instant::now(), want_big: big, last_small: None, last_big: None, viewed: None }),
                    stop: AtomicBool::new(false),
                });
                m.insert(id.to_string(), s.clone());
                let (sh2, env) = (s.clone(), self.stream_env());
                let id2 = id.to_string();
                std::thread::spawn(move || run_stream(sh2, env, id2, spec));
                s
            }
        }
    }

    /// Keep a network camera's stream going without a person looking (the motion watcher, the recorder's neighbour). It does not count
    /// as being watched, writes no log line, and never starts a USB camera.
    pub fn keep_alive(&self, id: &str) -> Result<(), String> {
        if id.starts_with("usb:") {
            return Err("this computer's own camera is never kept running in the background".into());
        }
        let spec = self.specs().into_iter().find(|s| s.id == id).ok_or("no such camera")?;
        let sh = self.stream_for(id, Some(spec), false);
        sh.st().touched = Instant::now();
        Ok(())
    }

    /// Whether a person asked for this camera's picture within the idle time (it is on a screen).
    pub fn viewed_recently(&self, id: &str) -> bool {
        self.streams().get(id).is_some_and(|s| s.st().viewed.is_some_and(|t| t.elapsed() <= self.env.idle))
    }

    /// Whether the camera's stream is up and delivering pictures right now.
    pub fn is_live(&self, id: &str) -> bool {
        self.streams().get(id).is_some_and(|s| s.st().status == Status::Live)
    }

    /// Install the one picture tap (motion watching).
    pub fn set_tap(&self, tap: Arc<dyn FrameTap>) {
        *self.tap.lock().unwrap_or_else(|e| e.into_inner()) = Some(tap);
    }

    fn name_of(&self, id: &str) -> String {
        if let Some(u) = id.strip_prefix("usb:") {
            return self.env.usb.list().into_iter().find(|c| c.id == u).map(|c| c.name).unwrap_or_else(|| id.to_string());
        }
        self.specs().into_iter().find(|s| s.id == id).map(|s| s.name).unwrap_or_else(|| id.to_string())
    }

    fn stream_env(&self) -> StreamEnv {
        StreamEnv { secrets: self.env.secrets.clone(), usb: self.env.usb.clone(), h264: self.env.h264.clone(), backoff_ms: self.env.backoff_ms, idle: self.env.idle, tap: self.tap.clone() }
    }

    /// Stop every stream now (sign-out, shutdown, tests).
    pub fn stop_all(&self) {
        for (_, s) in self.streams().drain() {
            s.stop.store(true, Ordering::SeqCst);
        }
    }

    pub fn running(&self) -> usize {
        self.streams().values().filter(|s| !s.stop.load(Ordering::SeqCst)).count()
    }

    // ------------------------------------------------------------------------------ snapshot

    /// Save the newest picture as a JPEG in the person's Pictures. One click, one file; the caller shows the indicator.
    pub fn snapshot(&self, who: &Who, id: &str) -> Result<PathBuf, String> {
        let r = self.snapshot_inner(who, id);
        // the camera's own log (shown to its owner) records the outcome, whoever asked
        if self.specs().iter().any(|s| s.id == id) {
            match &r {
                Ok(p) => crate::camrec::log_event(&self.env.data, id, &who.user, "live.snapshot", &p.display().to_string(), true),
                Err(e) => crate::camrec::log_event(&self.env.data, id, &who.user, "live.snapshot", e, false),
            }
        }
        r
    }

    fn snapshot_inner(&self, who: &Who, id: &str) -> Result<PathBuf, String> {
        self.watchable(who, id)?;
        let pic = {
            let m = self.streams();
            m.get(id).and_then(|s| s.st().latest.clone())
        };
        let Some(pic) = pic else { return Err("this camera has not shown a picture yet; wait until it is live".into()) };
        // privacy masks the person asked to apply to stored pictures are black before the snapshot is written; a picture that cannot be
        // masked is not saved
        let bytes = match &*pic {
            Picture::Jpeg(j) => crate::camdetect::mask_jpeg_for_storage(&self.env.data, id, j.clone(), &*self.env.encode)?,
            Picture::Rgba { width, height, data } => {
                let mut d = data.clone();
                crate::camdetect::mask_rgba_for_storage(&self.env.data, id, *width, *height, &mut d)?;
                (self.env.encode)(*width, *height, &d)
            }
        };
        if bytes.len() < 4 {
            return Err("the picture could not be encoded".into());
        }
        let dir = who.home.join("Pictures");
        if !(who.may_write)(&dir) {
            return Err(format!("your account may not save in {}", dir.display()));
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
        let name = slug(&self.name_of(id));
        let stamp = crate::clock::format(&crate::clock::Zone::system_with(None), crate::clock::now_unix(), "%Y%m%d-%H%M%S");
        let stem = format!("camera-{}-{stamp}", if name.is_empty() { "view" } else { &name });
        let mut path = dir.join(format!("{stem}.jpg"));
        for n in 2..1000 {
            if !path.exists() {
                break;
            }
            path = dir.join(format!("{stem}-{n}.jpg"));
        }
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(&path).map_err(|e| format!("could not save the snapshot: {e}"))?;
            f.write_all(&bytes).map_err(|e| format!("could not save the snapshot: {e}"))?;
        }
        (self.env.after_save)(&path);
        self.audit(&who.user, "camera.snapshot", &format!("{} -> {}", self.name_of(id), path.display()));
        Ok(path)
    }

    // ------------------------------------------------------------------------------ saving

    pub fn people(&self, who: &Who) -> Vec<String> {
        let mut p = (self.env.people)();
        p.retain(|n| *n != who.user);
        p.sort();
        p
    }

    /// Add (`replacing: None`) or change a network camera. `password`: `None` keeps what is stored, `Some("")` removes it, `Some(text)`
    /// stores it in the owner's vault (only the owner can; an administrator can change the list of people). Returns the id.
    pub fn save(&self, who: &Who, replacing: Option<&str>, new: &NewSpec, password: Option<&str>) -> Result<String, String> {
        let name = clean_text(&new.name, 40);
        if name.is_empty() {
            return Err("give the camera a name".into());
        }
        let mut url = rtsp::url::parse(&new.address).map_err(|e| scrub(&e))?;
        // credentials typed into the address are moved out of it
        let mut password = password.map(str::to_string);
        if let Some(p) = url.password.take() {
            if password.is_none() {
                password = Some(p);
            }
        }
        let mut user = clean_text(&new.user, 80);
        if let Some(u) = url.user.take() {
            if user.is_empty() {
                user = u;
            }
        }
        let address = url.display();
        let mut all = self.specs();
        let existing = match replacing {
            Some(id) => Some(all.iter().position(|s| s.id == id).ok_or("that camera no longer exists")?),
            None => None,
        };
        if let Some(i) = existing {
            if !who.can_manage(&all[i]) {
                return Err("only the person who added the camera, or an administrator, can change it".into());
            }
        } else if all.len() >= MAX_CAMERAS {
            return Err(format!("at most {MAX_CAMERAS} cameras"));
        }
        let owner = existing.map(|i| all[i].owner.clone()).unwrap_or_else(|| who.user.clone());
        if all.iter().enumerate().any(|(i, s)| s.owner == owner && s.name.eq_ignore_ascii_case(&name) && Some(i) != existing) {
            return Err(format!("a camera called “{name}” already exists"));
        }
        let people = (self.env.people)();
        let mut allowed: Vec<String> = Vec::new();
        for a in new.allowed.iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()) {
            if a == owner {
                continue;
            }
            if !people.contains(&a) {
                return Err(format!("there is no account called “{}”", clean_text(&a, 40)));
            }
            if !allowed.contains(&a) {
                allowed.push(a);
            }
        }
        if allowed.len() > MAX_PEOPLE {
            return Err(format!("at most {MAX_PEOPLE} people"));
        }
        let id = match existing {
            Some(i) => all[i].id.clone(),
            None => {
                let seed = format!("{name}{}{}", crate::clock::now_unix(), all.len());
                let h = rtsp::hash::md5_hex(seed.as_bytes());
                format!("{}-{}", if slug(&name).is_empty() { "cam".to_string() } else { slug(&name) }, &h[..6])
            }
        };
        let mut secret = existing.map(|i| all[i].secret.clone()).unwrap_or_default();
        match password.as_deref() {
            None => {}
            Some("") => {
                if !secret.is_empty() {
                    if who.user != owner {
                        return Err("only the person who added the camera can remove its password".into());
                    }
                    let _ = self.env.secrets.delete(&owner, &secret);
                    secret.clear();
                }
            }
            Some(p) => {
                if who.user != owner {
                    return Err("only the person who added the camera can change its password".into());
                }
                if secret.is_empty() {
                    secret = format!("cam.{id}");
                }
                self.env.secrets.set(&owner, &secret, p)?;
            }
        }
        let spec = Spec { id: id.clone(), name: name.clone(), address: address.clone(), user, owner, allowed, secret };
        let changed_stream = existing.is_some_and(|i| all[i].address != spec.address || all[i].user != spec.user || password.is_some());
        match existing {
            Some(i) => all[i] = spec,
            None => all.push(spec),
        }
        self.write_specs(&all)?;
        if changed_stream {
            if let Some(s) = self.streams().remove(&id) {
                s.stop.store(true, Ordering::SeqCst);
            }
        }
        self.audit(&who.user, if existing.is_some() { "camera.edit" } else { "camera.add" }, &format!("{name} {address}"));
        Ok(id)
    }

    /// Record the vault entry a person's masked form stored the password under (the assistant's `cameras_add` route).
    pub fn set_secret_name(&self, who: &Who, id: &str, vault_name: &str) -> Result<(), String> {
        let mut all = self.specs();
        let s = all.iter_mut().find(|s| s.id == id).ok_or("that camera no longer exists")?;
        if s.owner != who.user {
            return Err("only the person who added the camera can set its password".into());
        }
        s.secret = vault_name.to_string();
        self.write_specs(&all)?;
        if let Some(st) = self.streams().remove(id) {
            st.stop.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    pub fn remove(&self, who: &Who, id: &str) -> Result<(), String> {
        let mut all = self.specs();
        let i = all.iter().position(|s| s.id == id).ok_or("no such camera")?;
        if !who.can_manage(&all[i]) {
            return Err("only the person who added the camera, or an administrator, can remove it".into());
        }
        let s = all.remove(i);
        self.write_specs(&all)?;
        if !s.secret.is_empty() {
            let _ = self.env.secrets.delete(&s.owner, &s.secret);
        }
        if let Some(st) = self.streams().remove(id) {
            st.stop.store(true, Ordering::SeqCst);
        }
        self.audit(&who.user, "camera.remove", &format!("{} {}", s.name, s.address));
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------ streams

struct StreamEnv {
    secrets: Arc<dyn CamSecrets>,
    usb: Arc<dyn UsbProvider>,
    h264: Arc<dyn H264Support>,
    backoff_ms: u64,
    idle: Duration,
    tap: TapSlot,
}

pub(crate) fn tcp(host: &str, port: u16) -> Result<std::net::TcpStream, FeedErr> {
    use std::net::ToSocketAddrs;
    let addrs: Vec<_> = (host, port).to_socket_addrs().map_err(|_| FeedErr::Lost(format!("cannot find {host}")))?.collect();
    let mut last = String::from("no address");
    for a in addrs.into_iter().take(4) {
        match std::net::TcpStream::connect_timeout(&a, Duration::from_secs(5)) {
            Ok(s) => {
                let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(FeedErr::Lost(format!("cannot reach {host}:{port}: {last}")))
}

pub(crate) fn map_err(e: rtsp::Error) -> FeedErr {
    match e {
        rtsp::Error::Timeout => FeedErr::Timeout,
        rtsp::Error::Auth(m) => FeedErr::Auth(m),
        rtsp::Error::Unsupported(m) => FeedErr::Lost(scrub(&m)),
        e => FeedErr::Lost(scrub(&e.to_string())),
    }
}

struct RtspFeed {
    client: rtsp::client::Client<std::net::TcpStream>,
    dec: Option<Box<dyn H264Decoder>>,
}

impl Feed for RtspFeed {
    fn next(&mut self) -> Result<Picture, FeedErr> {
        loop {
            match self.client.next_media().map_err(map_err)? {
                rtsp::client::Media::Jpeg(j) => return Ok(Picture::Jpeg(j)),
                rtsp::client::Media::H264(au) => {
                    let d = self.dec.as_mut().ok_or_else(|| FeedErr::Addon(String::new()))?;
                    if let Some((w, h, rgba)) = d.decode(&au).map_err(|e| FeedErr::Lost(scrub(&e)))? {
                        return Ok(Picture::Rgba { width: w, height: h, data: rgba });
                    }
                }
            }
        }
    }
}

struct HttpFeed {
    h: rtsp::httpmjpeg::HttpMjpeg<std::net::TcpStream>,
    last: Option<Instant>,
}

impl Feed for HttpFeed {
    fn next(&mut self) -> Result<Picture, FeedErr> {
        if self.h.is_snapshot() {
            // a snapshot address is asked again for each picture; not faster than twice a second
            if let Some(t) = self.last {
                let gap = Duration::from_millis(500);
                if t.elapsed() < gap {
                    std::thread::sleep(gap - t.elapsed());
                }
            }
            self.last = Some(Instant::now());
        }
        self.h.next_jpeg().map(Picture::Jpeg).map_err(map_err)
    }
}

fn connect(env: &StreamEnv, id: &str, spec: Option<&Spec>) -> Result<Box<dyn Feed>, FeedErr> {
    if let Some(u) = id.strip_prefix("usb:") {
        return env.usb.open(u);
    }
    let spec = spec.ok_or_else(|| FeedErr::Lost("the camera was removed".into()))?;
    let url = rtsp::url::parse(&spec.address).map_err(|e| FeedErr::Lost(scrub(&e)))?;
    let creds = if spec.secret.is_empty() {
        if spec.user.is_empty() { None } else { Some((spec.user.clone(), String::new())) }
    } else {
        let pw = env.secrets.get(&spec.owner, &spec.secret).map_err(|e| FeedErr::Auth(format!("{e}. The owner signs in so the vault opens, or enters the password again.")))?;
        Some((spec.user.clone(), pw.to_string()))
    };
    match url.scheme {
        rtsp::url::Scheme::Rtsp => {
            let t = tcp(&url.host, url.port)?;
            let client = rtsp::client::Client::open(t, &url, creds).map_err(map_err)?;
            let dec = match &client.codec {
                rtsp::sdp::Codec::H264 { .. } => match env.h264.decoder() {
                    Some(d) => Some(d),
                    None => return Err(FeedErr::Addon(env.h264.hint())),
                },
                _ => None,
            };
            Ok(Box::new(RtspFeed { client, dec }))
        }
        rtsp::url::Scheme::Http => {
            let (host, port) = (url.host.clone(), url.port);
            let h = rtsp::httpmjpeg::HttpMjpeg::open(Box::new(move || tcp(&host, port).map_err(|e| rtsp::Error::Io(format!("{e:?}")))), &url, creds).map_err(map_err)?;
            Ok(Box::new(HttpFeed { h, last: None }))
        }
    }
}

fn sleep_unless_stopped(sh: &Shared, idle: Duration, total: Duration) {
    let end = Instant::now() + total;
    while Instant::now() < end {
        if sh.stop.load(Ordering::SeqCst) || sh.st().touched.elapsed() > idle {
            return;
        }
        std::thread::sleep(Duration::from_millis(50).min(end.saturating_duration_since(Instant::now())));
    }
}

fn set_status(sh: &Shared, s: Status) {
    sh.st().status = s;
}

fn convert(p: &Picture, max_w: u32) -> Option<(u32, u32, Vec<u8>)> {
    let (w, h, rgba) = match p {
        Picture::Jpeg(j) => {
            let b = iyakki_web::jpeg::decode(j).ok()?;
            (b.width, b.height, b.rgba)
        }
        Picture::Rgba { width, height, data } => (*width, *height, data.clone()),
    };
    Some(shrink(w, h, &rgba, max_w))
}

fn run_stream(sh: Arc<Shared>, env: StreamEnv, id: String, spec: Option<Spec>) {
    let base = Duration::from_millis(env.backoff_ms.max(10));
    let mut pause = base;
    'outer: loop {
        if sh.stop.load(Ordering::SeqCst) || sh.st().touched.elapsed() > env.idle {
            break;
        }
        set_status(&sh, Status::Connecting);
        let mut feed = match connect(&env, &id, spec.as_ref()) {
            Ok(f) => f,
            Err(e) => {
                let wait = match e {
                    FeedErr::Addon(m) => {
                        set_status(&sh, Status::NeedsAddon(m));
                        Duration::from_secs(60).max(pause)
                    }
                    FeedErr::Auth(m) => {
                        set_status(&sh, if m.contains("vault") || m.contains("needs a user name") { Status::NeedsPassword(m) } else { Status::Lost(m) });
                        pause.max(Duration::from_secs(5)).min(Duration::from_secs(30))
                    }
                    FeedErr::Busy(m) => {
                        set_status(&sh, Status::Busy(m));
                        pause
                    }
                    FeedErr::Lost(m) => {
                        set_status(&sh, Status::Lost(m));
                        pause
                    }
                    FeedErr::Timeout => {
                        set_status(&sh, Status::Lost("no answer".into()));
                        pause
                    }
                };
                sleep_unless_stopped(&sh, env.idle, wait);
                pause = (pause * 2).min(Duration::from_secs(30));
                continue;
            }
        };
        let (mut count, mut window) = (0u32, Instant::now());
        loop {
            if sh.stop.load(Ordering::SeqCst) || sh.st().touched.elapsed() > env.idle {
                break 'outer;
            }
            match feed.next() {
                Ok(pic) => {
                    pause = base;
                    count += 1;
                    let pic = Arc::new(pic);
                    let (small_due, big_due) = {
                        let mut st = sh.st();
                        st.status = Status::Live;
                        st.latest = Some(pic.clone());
                        if window.elapsed() >= Duration::from_secs(2) {
                            st.fps = count as f32 / window.elapsed().as_secs_f32();
                            count = 0;
                            window = Instant::now();
                        }
                        let due = |last: Option<Instant>, fps: u32| last.is_none_or(|t| t.elapsed() >= Duration::from_millis(1000 / fps as u64));
                        (due(st.last_small, SMALL_FPS), st.want_big && due(st.last_big, BIG_FPS))
                    };
                    let mut tap_frame: Option<Frame> = None;
                    if small_due || big_due {
                        let small = convert(&pic, if big_due && !small_due { BIG_W } else { SMALL_W });
                        let big = if big_due && small_due { convert(&pic, BIG_W) } else { None };
                        let mut st = sh.st();
                        st.seq += 1;
                        let seq = st.seq;
                        let mk = |t: (u32, u32, Vec<u8>)| Frame { seq, width: t.0, height: t.1, rgba: Arc::new(t.2) };
                        if big_due {
                            let b = big.or_else(|| small.clone());
                            if let Some(b) = b {
                                st.size = (b.0, b.1);
                                st.big = Some(mk(b));
                                st.last_big = Some(Instant::now());
                            }
                        }
                        if small_due {
                            if let Some(s) = small.map(|s| if big_due && s.0 > SMALL_W { shrink(s.0, s.1, &s.2, SMALL_W) } else { s }) {
                                if !big_due {
                                    st.size = (s.0, s.1);
                                }
                                let f = mk(s);
                                tap_frame = Some(f.clone());
                                st.small = Some(f);
                                st.last_small = Some(Instant::now());
                            }
                        }
                    }
                    if let Some(f) = tap_frame {
                        let tap = env.tap.lock().unwrap_or_else(|e| e.into_inner()).clone();
                        if let Some(t) = tap.filter(|t| t.wants(&id)) {
                            t.frame(&id, f.width, f.height, &f.rgba);
                        }
                    }
                }
                Err(FeedErr::Timeout) => {}
                Err(FeedErr::Lost(m)) => {
                    set_status(&sh, Status::Lost(m));
                    break;
                }
                Err(FeedErr::Addon(m)) => {
                    set_status(&sh, Status::NeedsAddon(m));
                    break;
                }
                Err(FeedErr::Auth(m)) => {
                    set_status(&sh, Status::Lost(m));
                    break;
                }
                Err(FeedErr::Busy(m)) => {
                    set_status(&sh, Status::Busy(m));
                    break;
                }
            }
        }
        sleep_unless_stopped(&sh, env.idle, pause);
        pause = (pause * 2).min(Duration::from_secs(30));
    }
    // the connection (or the USB device) is released when `feed` drops
    sh.stop.store(true, Ordering::SeqCst);
}

#[cfg(test)]
#[path = "cameras_tests.rs"]
mod tests;
