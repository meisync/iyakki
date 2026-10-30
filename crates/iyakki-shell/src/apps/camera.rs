//! CAMERA: a USB webcam as a camera. A live picture (MJPEG from the camera when it offers it, else YUYV), a choice of
//! camera and picture size, photos saved as JPEG in the person's Pictures folder with a flash and a strip of recent shots,
//! video recorded as MJPEG in an AVI file in Videos (picture only, no sound), and a passport mode with guide lines for
//! the common sizes, a countdown, a 300 dpi crop and a printable sheet (PNG and PDF). A 3 or 10 second timer, a mirror
//! switch for the live picture, and brightness and contrast when the camera has them. Everything goes through
//! [`crate::camera`]; no picture leaves the machine.

use crate::safeclamp::SafeClamp;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

use super::{App, AppCx, Flow};
use crate::camera::live::Live;
use crate::camera::passport::{self, Paper, Spec, SPECS};
use crate::camera::specs;
use super::widgets::{Form, FormEvent};
use crate::camera::{pixels, v4l2, CamDevice, Control, Mode, Opened, Opener, Pix, Presence};
use crate::fx;
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, DIM, ERR, MUTED, R, TEXT};

pub const ACCENT: [u8; 4] = rgb(255, 99, 72);

/// What the shutter does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shot {
    Photo,
    Video,
    Passport,
}

impl Shot {
    fn label(self) -> &'static str {
        match self {
            Shot::Photo => "Photo",
            Shot::Video => "Video",
            Shot::Passport => "Passport",
        }
    }
}

enum Conn {
    /// No camera to show.
    None,
    Opening(Receiver<Result<Opened, String>>),
    Live(Live),
    Failed(String),
}

enum Overlay {
    None,
    Cameras,
    Sizes,
    Viewer(usize, Option<Pixmap>),
    PassportResult,
    /// The searchable list of countries: what was typed and the highlighted row (of the matches).
    Countries(String, usize),
    /// Width and height of the custom size.
    Custom(Form),
}

/// A recent photo or video in the strip.
pub struct Thumb {
    pub path: PathBuf,
    pub pm: Option<Pixmap>,
    pub video: bool,
}

/// The last passport photo, kept for the print sheet.
struct PassportPhoto {
    spec: Spec,
    digital: Option<PathBuf>,
    w: u32,
    h: u32,
    rgba: Vec<u8>,
    pm: Option<Pixmap>,
    path: PathBuf,
}

const TIMERS: [u8; 3] = [0, 3, 10];
const TICKS_PER_SEC: u64 = 30;
const PICK_BASE: usize = 7_200;
const OK_C: [u8; 4] = crate::ui::OK;
const THUMB_W: u32 = 160;
/// Under the person's home: the name of the camera they chose.
pub(crate) const CHOICE_FILE: &str = ".config/iyakki/camera";

pub(crate) fn load_choice(file: &Path) -> String {
    std::fs::read_to_string(file).map(|t| t.trim().to_string()).unwrap_or_default()
}

/// The camera to start with: the one the person chose last time when it is connected, else the built-in one, else the first.
pub(crate) fn preferred(cams: &[CamDevice], saved: &str) -> usize {
    cams.iter().position(|c| !saved.is_empty() && c.name == saved).or_else(|| cams.iter().position(|c| c.builtin)).unwrap_or(0)
}

pub struct CameraApp {
    presence: Presence,
    opener: Arc<dyn Opener>,
    cams: Vec<CamDevice>,
    cam: usize,
    conn: Conn,
    modes: Vec<Mode>,
    want: Option<Mode>,
    controls: Vec<Control>,
    shot: Shot,
    timer: u8,
    mirror: bool,
    countdown: Option<u64>,
    flash: u8,
    shown: u64,
    gallery: Vec<Thumb>,
    gallery_rx: Option<Receiver<Vec<Thumb>>>,
    pictures: PathBuf,
    videos: PathBuf,
    status: Option<(String, bool)>,
    spec: usize,
    /// The custom size (width, height in mm).
    custom_mm: (f32, f32),
    zoom: f32,
    center: (f32, f32),
    passport_photo: Option<PassportPhoto>,
    overlay: Overlay,
    ticks: u64,
    last_scan: u64,
    /// The size the person chose per camera (by name), kept while the app runs.
    wants: std::collections::HashMap<String, Mode>,
    /// Where the choice of camera is remembered (in the person's home).
    choice_file: PathBuf,
}

fn give_to_account(path: &Path) {
    crate::camera::own::give_saved(path);
}

fn now_stamp() -> String {
    crate::camera::stamp()
}

/// `dir/stem.ext`, or `dir/stem-2.ext` and so on: nothing is ever overwritten.
pub fn unique(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..10_000).map(|n| dir.join(format!("{stem}-{n}.{ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

pub fn human_size(b: u64) -> String {
    match b {
        0..=999 => format!("{b} B"),
        1_000..=999_999 => format!("{:.0} kB", b as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1} MB", b as f64 / 1e6),
        _ => format!("{:.2} GB", b as f64 / 1e9),
    }
}

pub fn clock_text(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

fn thumb_of(pm: &Pixmap, w: u32) -> Option<Pixmap> {
    let h = ((pm.height() as f32 * w as f32 / pm.width().max(1) as f32).round() as u32).max(1);
    let mut t = Pixmap::new(w, h)?;
    let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
    t.draw_pixmap(0, 0, pm.as_ref(), &paint, Transform::from_scale(w as f32 / pm.width() as f32, h as f32 / pm.height() as f32), None);
    Some(t)
}

/// A picture file as a pixmap (JPEG through the browser's decoder, PNG through tiny-skia).
fn load_picture(path: &Path) -> Option<Pixmap> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let b = iyakki_web::jpeg::decode(&bytes).ok()?;
        pixels::pixmap(b.width, b.height, b.rgba)
    } else {
        Pixmap::decode_png(&bytes).ok()
    }
}

/// The first frame of one of our AVI files (read from the start of the file only).
fn first_video_frame(path: &Path) -> Option<Pixmap> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path).ok()?.take(4 << 20).read_to_end(&mut buf).ok()?;
    let movi = buf.windows(4).position(|w| w == b"movi")?;
    let at = movi + 4;
    if buf.get(at..at + 4)? != b"00dc" {
        return None;
    }
    let len = u32::from_le_bytes(buf.get(at + 4..at + 8)?.try_into().ok()?) as usize;
    let b = crate::camera::jpeg::decode(buf.get(at + 8..at + 8 + len)?).ok()?;
    pixels::pixmap(b.width, b.height, b.rgba)
}

/// The newest photos, passport photos and videos this app saved, newest first, with small pictures.
pub fn scan_gallery(pictures: &Path, videos: &Path, max: usize) -> Vec<Thumb> {
    let mut found: Vec<(std::time::SystemTime, PathBuf, bool)> = Vec::new();
    for (dir, video) in [(pictures, false), (videos, true)] {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let ours = if video { name.starts_with("video-") && name.ends_with(".avi") } else { (name.starts_with("photo-") || name.starts_with("passport-")) && (name.ends_with(".jpg") || name.ends_with(".png")) };
            if ours {
                let t = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                found.push((t, e.path(), video));
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    found.truncate(max);
    found.into_iter().map(|(_, path, video)| {
        let pm = if video { first_video_frame(&path) } else { load_picture(&path) }.and_then(|p| thumb_of(&p, THUMB_W));
        Thumb { path, pm, video }
    }).collect()
}

impl CameraApp {
    pub fn new(cx: &AppCx) -> CameraApp {
        let home = PathBuf::from(format!("/home/{}", cx.session.user));
        CameraApp::with_parts(Presence::system(), Arc::new(v4l2::V4l2Opener), home.join("Pictures"), home.join("Videos"))
    }

    /// The app with its cameras and folders given (tests, screenshots).
    pub fn with_parts(mut presence: Presence, opener: Arc<dyn Opener>, pictures: PathBuf, videos: PathBuf) -> CameraApp {
        presence.scan();
        let cams = presence.cameras();
        let choice_file = pictures.parent().map(|h| h.join(CHOICE_FILE)).unwrap_or_else(|| PathBuf::from(CHOICE_FILE));
        let cam = preferred(&cams, &load_choice(&choice_file));
        let (tx, rx) = channel();
        let (p2, v2) = (pictures.clone(), videos.clone());
        std::thread::spawn(move || {
            let _ = tx.send(scan_gallery(&p2, &v2, 12));
        });
        let mut a = CameraApp {
            presence,
            opener,
            cams,
            cam,
            conn: Conn::None,
            modes: Vec::new(),
            want: None,
            controls: Vec::new(),
            shot: Shot::Photo,
            timer: 0,
            mirror: true,
            countdown: None,
            flash: 0,
            shown: 0,
            gallery: Vec::new(),
            gallery_rx: Some(rx),
            pictures,
            videos,
            status: None,
            spec: 0,
            custom_mm: (35.0, 45.0),
            zoom: 0.8,
            center: (0.5, 0.5),
            passport_photo: None,
            overlay: Overlay::None,
            ticks: 0,
            last_scan: 0,
            wants: Default::default(),
            choice_file,
        };
        a.connect();
        a
    }

    /// Open the chosen camera in the background.
    fn connect(&mut self) {
        if let Conn::Live(l) = &self.conn {
            if l.recording() {
                self.stop_recording();
            }
        }
        self.conn = Conn::None;
        self.modes.clear();
        let Some(dev) = self.cams.get(self.cam).cloned() else { return };
        let (tx, rx) = channel();
        let (op, want) = (self.opener.clone(), self.want);
        std::thread::spawn(move || {
            let _ = tx.send(op.open(&dev, want));
        });
        self.conn = Conn::Opening(rx);
    }

    pub fn camera_names(&self) -> Vec<String> {
        self.cams.iter().map(|c| c.name.clone()).collect()
    }
    pub fn live_mode(&self) -> Option<Mode> {
        match &self.conn {
            Conn::Live(l) => Some(l.mode),
            _ => None,
        }
    }
    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }
    pub fn gallery_paths(&self) -> Vec<PathBuf> {
        self.gallery.iter().map(|t| t.path.clone()).collect()
    }
    pub fn shot(&self) -> Shot {
        self.shot
    }
    pub fn is_recording(&self) -> bool {
        matches!(&self.conn, Conn::Live(l) if l.recording())
    }
    pub fn error_text(&self) -> Option<String> {
        match &self.conn {
            Conn::Failed(e) => Some(e.clone()),
            _ => None,
        }
    }

    fn live(&self) -> Option<&Live> {
        match &self.conn {
            Conn::Live(l) => Some(l),
            _ => None,
        }
    }

    fn say(&mut self, t: impl Into<String>, ok: bool) {
        let t: String = t.into();
        // a photo that cannot be saved because the boot drive is gone says that (iyakki_storage::loss)
        self.status = Some((if ok { t } else { iyakki_storage::loss::reword(&t) }, ok));
    }

    /// A folder the account may write to, made when missing.
    fn ready_dir(&mut self, dir: &Path, cx: &AppCx) -> bool {
        if !cx.session.policy.allows_write(&cx.session.user, &dir.display().to_string()) {
            self.say(format!("Your account may not save in {}.", dir.display()), false);
            return false;
        }
        let made = std::fs::create_dir_all(dir);
        give_to_account(dir);
        if let Err(e) = made {
            self.say(format!("Could not make {}: {e}", dir.display()), false);
            return false;
        }
        true
    }

    fn add_thumb(&mut self, path: PathBuf, pm: Option<Pixmap>, video: bool) {
        self.gallery.insert(0, Thumb { path, pm: pm.and_then(|p| thumb_of(&p, THUMB_W)), video });
        self.gallery.truncate(24);
    }

    fn current_preview_pm(&self) -> Option<Pixmap> {
        self.live().and_then(|l| l.preview()).map(|(_, p)| p.pm.clone())
    }

    /// Press the shutter: start the countdown when a timer is set, else act now. While recording it stops at once.
    fn shutter(&mut self, cx: &AppCx) {
        if self.is_recording() {
            self.stop_recording();
            return;
        }
        if self.countdown.is_some() {
            self.countdown = None;
            self.say("Timer cancelled.", true);
            return;
        }
        if self.live().is_none() {
            self.say("No camera is ready.", false);
            return;
        }
        if self.timer > 0 {
            self.countdown = Some(self.timer as u64 * TICKS_PER_SEC);
        } else {
            self.fire(cx);
        }
    }

    fn fire(&mut self, cx: &AppCx) {
        match self.shot {
            Shot::Photo => self.take_photo(cx),
            Shot::Video => self.start_recording(cx),
            Shot::Passport => self.take_passport(cx),
        }
    }

    pub fn take_photo(&mut self, cx: &AppCx) {
        let Some(frame) = self.live().and_then(|l| l.latest()) else {
            self.say("No picture from the camera yet.", false);
            return;
        };
        let dir = self.pictures.clone();
        if !self.ready_dir(&dir, cx) {
            return;
        }
        let jpeg = match frame.to_jpeg() {
            Ok(j) => j,
            Err(e) => return self.say(format!("Could not make the photo: {e}"), false),
        };
        let path = unique(&dir, &format!("photo-{}", now_stamp()), "jpg");
        let written = std::fs::write(&path, &jpeg);
        give_to_account(&path);
        match written {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "camera.photo", &path.display().to_string());
                self.flash = 8;
                let pm = self.current_preview_pm();
                self.add_thumb(path.clone(), pm, false);
                self.say(format!("Saved {} in Pictures ({}×{}, {}).", name_of(&path), frame.width, frame.height, human_size(jpeg.len() as u64)), true);
            }
            Err(e) => self.say(format!("Could not save the photo: {e}"), false),
        }
    }

    pub fn start_recording(&mut self, cx: &AppCx) {
        let dir = self.videos.clone();
        if self.live().is_none() || !self.ready_dir(&dir, cx) {
            return;
        }
        let path = unique(&dir, &format!("video-{}", now_stamp()), "avi");
        let r = self.live().map(|l| l.start_recording(&path));
        give_to_account(&path);
        match r {
            Some(Ok(())) => {
                cx.audit.record(&cx.session.user, "camera.record", &path.display().to_string());
                self.say(format!("Recording {} (picture only, no sound).", name_of(&path)), true);
            }
            Some(Err(e)) => self.say(format!("Could not start recording: {e}"), false),
            None => {}
        }
    }

    pub fn stop_recording(&mut self) {
        let Some(res) = self.live().and_then(|l| l.stop_recording()) else { return };
        match res {
            Ok((files, frames)) => {
                let size: u64 = files.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
                for f in &files {
                    give_to_account(f);
                }
                let first = files.first().cloned().unwrap_or_default();
                let pm = first_video_frame(&first).or_else(|| self.current_preview_pm());
                self.add_thumb(first.clone(), pm, true);
                let parts = if files.len() > 1 { format!(" in {} parts", files.len()) } else { String::new() };
                self.say(format!("Saved {} in Videos: {frames} frames, {}{parts}. Picture only, no sound.", name_of(&first), human_size(size)), true);
            }
            Err(e) => self.say(format!("The recording failed: {e}"), false),
        }
    }

    /// The passport rules in use (the custom entry with the person's own size).
    pub fn cur(&self) -> Spec {
        let s = SPECS[self.spec.min(SPECS.len() - 1)];
        if s.id == specs::CUSTOM_ID { specs::custom(self.custom_mm.0, self.custom_mm.1) } else { s }
    }

    /// Choose the passport rules by id (the picker, the tests).
    pub fn choose_spec(&mut self, id: &str) -> bool {
        match SPECS.iter().position(|s| s.id == id) {
            Some(i) => {
                self.spec = i;
                let s = self.cur();
                self.say(s.describe(), s.verified || s.id == specs::CUSTOM_ID);
                true
            }
            None => false,
        }
    }

    /// The part of the frame (in the frame's own, unmirrored pixels) that becomes the passport photo.
    pub fn passport_crop(&self, fw: u32, fh: u32) -> pixels::RectF {
        let s = self.cur();
        let r = passport::crop_rect(&s, fw, fh, self.zoom, self.center.0, self.center.1);
        if self.mirror {
            pixels::RectF { x: fw as f32 - r.x - r.w, ..r }
        } else {
            r
        }
    }

    pub fn take_passport(&mut self, cx: &AppCx) {
        let Some(frame) = self.live().and_then(|l| l.latest()) else {
            self.say("No picture from the camera yet.", false);
            return;
        };
        let (fw, fh, rgba) = match frame.to_rgba() {
            Ok(v) => v,
            Err(e) => return self.say(format!("Could not read the picture: {e}"), false),
        };
        let dir = self.pictures.clone();
        if !self.ready_dir(&dir, cx) {
            return;
        }
        let s = self.cur();
        let crop = self.passport_crop(fw, fh);
        let saved = match passport::save(&dir, &s, fw, fh, &rgba, crop, None, &now_stamp()) {
            Ok(v) => v,
            Err(e) => return self.say(format!("Could not make the photo: {e}"), false),
        };
        for f in [Some(&saved.photo), saved.digital.as_ref()].into_iter().flatten() {
            give_to_account(f);
        }
        cx.audit.record(&cx.session.user, "camera.passport", &saved.photo.display().to_string());
        let pm = pixels::pixmap(saved.w, saved.h, saved.rgba.clone());
        self.add_thumb(saved.photo.clone(), pm.clone(), false);
        self.flash = 8;
        let online = saved.digital.as_ref().map_or(String::new(), |d| format!(" and {} for online forms", name_of(d)));
        let warn = if saved.low_res { " — the camera picture is small for this size; choose a larger picture size or zoom out" } else if !s.verified && s.id != specs::CUSTOM_ID { " — these rules are UNVERIFIED, check them" } else { "" };
        self.say(format!("Saved {} ({}×{} px, 300 dpi){online}{warn}.", name_of(&saved.photo), saved.w, saved.h), warn.is_empty());
        self.passport_photo = Some(PassportPhoto { spec: s, digital: saved.digital, w: saved.w, h: saved.h, rgba: saved.rgba, pm, path: saved.photo });
        self.overlay = Overlay::PassportResult;
    }

    /// Lay the last passport photo out on a sheet and save it as PNG and PDF; the paths written.
    pub fn export_sheet(&mut self, paper: Paper, cx: &AppCx) -> Option<(PathBuf, PathBuf)> {
        let dir = self.pictures.clone();
        if !self.ready_dir(&dir, cx) {
            return None;
        }
        let p = self.passport_photo.as_ref()?;
        match passport::save_sheet(&dir, &p.spec, paper, p.w, p.h, &p.rgba, &now_stamp()) {
            Ok((png_path, pdf_path, copies)) => {
                give_to_account(&png_path);
                give_to_account(&pdf_path);
                cx.audit.record(&cx.session.user, "camera.sheet", &png_path.display().to_string());
                self.say(format!("Saved a {} sheet with {copies} copies: {} and {} in Pictures. Print at 100% (no scaling).", paper.name(), name_of(&png_path), name_of(&pdf_path)), true);
                Some((png_path, pdf_path))
            }
            Err(e) => {
                self.say(format!("Could not save the sheet: {e}"), false);
                None
            }
        }
    }

    fn set_shot(&mut self, s: Shot) {
        if self.is_recording() {
            self.say("Stop the recording first.", false);
            return;
        }
        self.shot = s;
        self.countdown = None;
        if s == Shot::Passport && self.timer == 0 {
            self.timer = 3;
        }
        if s == Shot::Passport {
            let sp = self.cur();
            self.say(format!("{}: stand before a plain light wall, even light, look straight at the camera. {}.", sp.name, sp.note), true);
        }
    }

    fn step_control(&mut self, id: u32, up: bool) {
        let Some(c) = self.controls.iter_mut().find(|c| c.id == id) else {
            self.status = Some(("This camera has no such setting.".into(), false));
            return;
        };
        c.value = c.stepped(up);
        let (v, name) = (c.value, c.name.clone());
        if let Some(l) = self.live() {
            l.set_control(id, v);
        }
        self.status = Some((format!("{name}: {v}"), true));
    }

    fn choose_camera(&mut self, i: usize) {
        if i < self.cams.len() && (i != self.cam || !matches!(self.conn, Conn::Live(_))) {
            if self.is_recording() {
                self.overlay = Overlay::None;
                return self.say("Stop the recording first.", false);
            }
            self.cam = i;
            self.want = self.wants.get(&self.cams[i].name).copied();
            let name = self.cams[i].name.clone();
            self.remember_choice(&name);
            self.connect();
        }
        self.overlay = Overlay::None;
    }

    /// Keep the choice of camera for next time (best effort; the file belongs to the person).
    fn remember_choice(&self, name: &str) {
        if let Some(dir) = self.choice_file.parent() {
            let _ = std::fs::create_dir_all(dir);
            crate::camera::own::give_saved(dir);
        }
        if std::fs::write(&self.choice_file, format!("{name}\n")).is_ok() {
            crate::camera::own::give_saved(&self.choice_file);
        }
    }

    /// Take the cameras the machine has now: keep the one in use when it is still there; when it is gone, fall back to the
    /// built-in camera (else the first) and say so.
    fn apply_cams(&mut self) {
        let current = self.cams.get(self.cam).cloned();
        self.cams = self.presence.cameras();
        match current {
            Some(c) => match self.cams.iter().position(|n| n.node == c.node && n.name == c.name) {
                Some(i) => self.cam = i,
                None => {
                    self.cam = preferred(&self.cams, "");
                    self.want = None;
                    if self.cams.is_empty() {
                        self.conn = Conn::None;
                        self.modes.clear();
                        self.say(format!("{} was disconnected. No camera is connected.", c.name), false);
                    } else {
                        let now = self.cams[self.cam].name.clone();
                        self.connect();
                        self.say(format!("{} was disconnected; using {now}.", c.name), false);
                    }
                }
            },
            None => {
                let saved = load_choice(&self.choice_file);
                self.cam = preferred(&self.cams, &saved);
                self.want = self.cams.get(self.cam).and_then(|c| self.wants.get(&c.name).copied());
                if !self.cams.is_empty() {
                    self.connect();
                }
            }
        }
    }

    /// The camera dialog's rows: the name, marked when it is the one in use or the built-in one.
    pub fn camera_rows(&self) -> Vec<String> {
        self.cams
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut tags = Vec::new();
                if c.builtin {
                    tags.push("built-in");
                }
                if i == self.cam {
                    tags.push("in use");
                }
                if tags.is_empty() { c.name.clone() } else { format!("{}  ({})", c.name, tags.join(", ")) }
            })
            .collect()
    }

    /// Look for cameras again now (the dialog does this when it opens and on its Refresh button).
    fn refresh_cameras(&mut self) {
        self.presence.scan();
        self.apply_cams();
    }

    /// The picture sizes offered for the format in use (MJPEG first), largest last.
    pub fn size_choices(&self) -> Vec<Mode> {
        // one entry per size: MJPEG for the bigger sizes (a USB camera sends more than a few frames a second only as MJPEG),
        // else the format in use
        let pix = self.live_mode().map(|m| m.pix);
        let mut v: Vec<Mode> = Vec::new();
        for m in &self.modes {
            let same = |o: &Mode| o.width == m.width && o.height == m.height;
            let mj = self.modes.iter().find(|o| same(o) && o.pix == Pix::Mjpeg).copied();
            let pick = match mj {
                Some(j) if m.width > 640 || pix.is_none_or(|p| p == Pix::Mjpeg) => j,
                _ => *self.modes.iter().find(|o| same(o) && Some(o.pix) == pix).unwrap_or(m),
            };
            if !v.contains(&pick) {
                v.push(pick);
            }
        }
        v.sort_by_key(|m| (m.width * m.height, m.pix != Pix::Mjpeg));
        v
    }

    fn choose_size(&mut self, i: usize) {
        if let Some(m) = self.size_choices().get(i).copied() {
            if Some(m) != self.live_mode() {
                if self.is_recording() {
                    self.overlay = Overlay::None;
                    return self.say("Stop the recording first.", false);
                }
                self.want = Some(m);
                if let Some(c) = self.cams.get(self.cam) {
                    self.wants.insert(c.name.clone(), m);
                }
                self.connect();
            }
        }
        self.overlay = Overlay::None;
    }

    fn on_tick(&mut self, cx: &AppCx) {
        self.ticks += 1;
        if let Some(rx) = &self.gallery_rx {
            if let Ok(found) = rx.try_recv() {
                for t in found {
                    if !self.gallery.iter().any(|g| g.path == t.path) {
                        self.gallery.push(t);
                    }
                }
                self.gallery_rx = None;
            }
        }
        if let Conn::Opening(rx) = &self.conn {
            match rx.try_recv() {
                Ok(Ok(o)) => {
                    self.modes = o.modes;
                    self.controls = o.controls;
                    self.shown = 0;
                    self.conn = Conn::Live(Live::start(o.cap));
                }
                Ok(Err(e)) => self.conn = Conn::Failed(e),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.conn = Conn::Failed("the camera did not answer".into()),
                Err(_) => {}
            }
        }
        if let Some(e) = self.live().and_then(|l| l.error()) {
            if self.is_recording() {
                self.stop_recording();
            }
            self.conn = Conn::Failed(e);
            self.countdown = None;
            // look at once whether the camera was pulled out, so the built-in one can take over
            self.last_scan = self.ticks.saturating_sub(60);
        }
        // cameras plugged in or pulled out
        if self.ticks - self.last_scan >= 60 {
            self.last_scan = self.ticks;
            if self.presence.scan() {
                self.apply_cams();
            } else if matches!(self.conn, Conn::None) && !self.cams.is_empty() {
                self.connect();
            }
        }
        if let Some(n) = self.countdown {
            if n <= 1 {
                self.countdown = None;
                self.fire(cx);
            } else {
                self.countdown = Some(n - 1);
            }
        }
        self.flash = self.flash.saturating_sub(1);
    }

    // ------------------------------------------------------------------------------------------------ drawing

    fn draw_preview(&mut self, g: &mut Gfx, r: R, s: f32) {
        fx::well(g, r, s);
        let preview = self.live().and_then(|l| l.preview());
        let Some((seq, p)) = preview else {
            let (title, hint) = match &self.conn {
                Conn::None => ("No camera", self.presence.hint().to_string()),
                Conn::Opening(_) => ("Starting the camera…", self.cams.get(self.cam).map(|c| c.name.clone()).unwrap_or_default()),
                Conn::Live(_) => ("Waiting for the first picture…", String::new()),
                Conn::Failed(e) => ("The camera stopped", format!("{e}. Press c to try again or connect another camera.")),
            };
            fx::empty_state(g, r, s, Icon::Camera, title, &hint, ACCENT);
            return;
        };
        self.shown = seq;
        let (fw, fh) = (p.width as f32, p.height as f32);
        let k = (r.w / fw).min(r.h / fh);
        let (dw, dh) = (fw * k, fh * k);
        let d = R::new(r.x + (r.w - dw) / 2.0, r.y + (r.h - dh) / 2.0, dw, dh);
        let (pw, ph) = (p.pm.width() as f32, p.pm.height() as f32);
        let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
        let t = if self.mirror { Transform::from_row(-dw / pw, 0.0, 0.0, dh / ph, d.x + dw, d.y) } else { Transform::from_row(dw / pw, 0.0, 0.0, dh / ph, d.x, d.y) };
        g.pm.draw_pixmap(0, 0, p.pm.as_ref(), &paint, t, None);
        if self.shot == Shot::Passport {
            self.draw_guides(g, d, p.width, p.height, s);
        }
        // what is in use, bottom left
        if let Some(l) = self.live() {
            let info = format!("{}×{} {} · {:.0} fps{}", p.width, p.height, l.mode.pix.name(), l.fps(), if self.mirror { " · mirrored view" } else { "" });
            let tw = g.text_width(12.0 * s, &info, Face::Sans) + 16.0 * s;
            g.rrect(d.x + 8.0 * s, d.y + dh - 30.0 * s, tw, 22.0 * s, 11.0 * s, [0, 0, 0, 150]);
            g.text(d.x + 16.0 * s, d.y + dh - 27.0 * s, 12.0 * s, TEXT, &info, Face::Sans);
        }
        if let Some(st) = self.live().and_then(|l| l.rec_status()) {
            let txt = format!("REC  {}  ·  {}", clock_text(st.secs), human_size(st.bytes));
            let tw = g.text_width(15.0 * s, &txt, Face::Bold) + 40.0 * s;
            let (x, y) = (d.x + 12.0 * s, d.y + 12.0 * s);
            g.rrect(x, y, tw, 30.0 * s, 15.0 * s, [40, 0, 6, 200]);
            if (self.ticks / 15) % 2 == 0 {
                g.circle(x + 16.0 * s, y + 15.0 * s, 6.0 * s, rgb(255, 60, 70));
            }
            g.text(x + 30.0 * s, y + 6.0 * s, 15.0 * s, TEXT, &txt, Face::Bold);
        }
        if let Some(n) = self.countdown {
            let secs = n.div_ceil(TICKS_PER_SEC).to_string();
            let sz = (dh * 0.35).min(160.0 * s);
            let tw = g.text_width(sz, &secs, Face::Bold);
            g.circle(d.x + dw / 2.0, d.y + dh / 2.0, sz * 0.75, [0, 0, 0, 120]);
            g.text(d.x + (dw - tw) / 2.0, d.y + dh / 2.0 - sz * 0.62, sz, TEXT, &secs, Face::Bold);
        }
        if self.flash > 0 {
            g.rect(d.x, d.y, dw, dh, [255, 255, 255, (self.flash as u32 * 28).min(230) as u8]);
        }
    }

    /// The passport frame: everything outside the photo darkened, the head oval, the crown line, the chin band, the eye line.
    fn draw_guides(&self, g: &mut Gfx, d: R, fw: u32, fh: u32, s: f32) {
        let sp = &self.cur();
        let c = passport::crop_rect(sp, fw, fh, self.zoom, self.center.0, self.center.1);
        let k = d.w / fw as f32;
        let r = R::new(d.x + c.x * k, d.y + c.y * k, c.w * k, c.h * k);
        let shade = [0, 0, 0, 150];
        g.rect(d.x, d.y, d.w, r.y - d.y, shade);
        g.rect(d.x, r.y + r.h, d.w, d.y + d.h - r.y - r.h, shade);
        g.rect(d.x, r.y, r.x - d.x, r.h, shade);
        g.rect(r.x + r.w, r.y, d.x + d.w - r.x - r.w, r.h, shade);
        g.rrect_outline(r.x, r.y, r.w, r.h, 2.0, 2.0 * s, rgb(255, 255, 255));
        let gd = passport::guides(sp);
        let yat = |f: f32| r.y + r.h * f;
        let guide = [255, 230, 120, 230];
        g.line(r.x, yat(gd.crown), r.x + r.w, yat(gd.crown), 1.5 * s, guide);
        g.rect(r.x, yat(gd.chin_min), r.w, yat(gd.chin_max) - yat(gd.chin_min), [255, 230, 120, 50]);
        g.line(r.x, yat(gd.chin_min), r.x + r.w, yat(gd.chin_min), 1.0 * s, guide);
        g.line(r.x, yat(gd.chin_max), r.x + r.w, yat(gd.chin_max), 1.0 * s, guide);
        g.line(r.x + r.w * 0.2, yat(gd.eye), r.x + r.w * 0.8, yat(gd.eye), 1.5 * s, [120, 230, 255, 230]);
        g.line(r.x + r.w / 2.0, r.y, r.x + r.w / 2.0, r.y + r.h, 1.0, [255, 255, 255, 90]);
        // the head oval: crown to the middle of the chin band
        let (top, bot) = (yat(gd.crown), yat((gd.chin_min + gd.chin_max) / 2.0));
        let (cx, cy, rx, ry) = (r.x + r.w / 2.0, (top + bot) / 2.0, r.w * gd.head_w / 2.0, (bot - top) / 2.0);
        let n = 48;
        for i in 0..n {
            let (a0, a1) = (i as f32 / n as f32 * std::f32::consts::TAU, (i + 1) as f32 / n as f32 * std::f32::consts::TAU);
            g.line(cx + rx * a0.cos(), cy + ry * a0.sin(), cx + rx * a1.cos(), cy + ry * a1.sin(), 2.0 * s, [255, 255, 255, 200]);
        }
        let label = format!("{} · crown, eyes, chin", sp.name);
        let lw = g.text_width(12.0 * s, &label, Face::Bold) + 14.0 * s;
        let ly = (r.y - 24.0 * s).max(d.y + 4.0 * s);
        g.rrect(r.x, ly, lw.min(d.w), 20.0 * s, 10.0 * s, [0, 0, 0, 170]);
        g.text(r.x + 7.0 * s, ly + 3.0 * s, 12.0 * s, rgb(255, 230, 120), &label, Face::Bold);
    }

    fn draw_strip(&self, g: &mut Gfx, r: R, s: f32) {
        if self.gallery.is_empty() {
            g.text(r.x + 4.0 * s, r.y + r.h / 2.0 - 8.0 * s, 13.0 * s, DIM, "Photos and videos you take appear here (Pictures and Videos folders).", Face::Sans);
            return;
        }
        let th = r.h;
        let tw = th * 4.0 / 3.0;
        let mut x = r.x;
        for (i, t) in self.gallery.iter().enumerate() {
            if x + tw > r.x + r.w {
                break;
            }
            let cell = R::new(x, r.y, tw, th);
            let hot = g.hovered(cell);
            g.rrect(cell.x, cell.y, cell.w, cell.h, 6.0 * s, rgb(10, 12, 22));
            if let Some(pm) = &t.pm {
                let k = (tw / pm.width() as f32).min(th / pm.height() as f32);
                let (w, h) = (pm.width() as f32 * k, pm.height() as f32 * k);
                let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
                g.pm.draw_pixmap(0, 0, pm.as_ref(), &paint, Transform::from_translate(x + (tw - w) / 2.0, r.y + (th - h) / 2.0).pre_scale(k, k), None);
            }
            if t.video {
                g.rrect(x + 4.0 * s, r.y + th - 20.0 * s, 34.0 * s, 16.0 * s, 8.0 * s, [0, 0, 0, 170]);
                g.text(x + 9.0 * s, r.y + th - 19.0 * s, 11.0 * s, TEXT, "AVI", Face::Bold);
            }
            g.rrect_outline(cell.x, cell.y, cell.w, cell.h, 6.0 * s, if hot { 2.0 * s } else { 1.0 }, if hot { ACCENT } else { rgb(52, 62, 96) });
            g.hit(cell, Event::Row(i), &format!("open {}", name_of(&t.path)));
            x += tw + 8.0 * s;
        }
    }

    fn small_button(g: &mut Gfx, x: f32, y: f32, s: f32, label: &str, ev: Event) -> f32 {
        g.button(x, y, 13.0 * s, label, fx::CYAN, ev)
    }

    fn draw_side(&mut self, g: &mut Gfx, r: R, s: f32) {
        fx::panel(g, r, s, ACCENT);
        let pad = 14.0 * s;
        let (x, w) = (r.x + pad, r.w - 2.0 * pad);
        let mut y = r.y + pad;
        let tabs: Vec<(String, Event)> = [Shot::Photo, Shot::Video, Shot::Passport].iter().enumerate().map(|(i, t)| (t.label().to_string(), Event::Key(Key::Char((b'1' + i as u8) as char)))).collect();
        let sel = [Shot::Photo, Shot::Video, Shot::Passport].iter().position(|t| *t == self.shot).unwrap_or(0);
        fx::tab_strip(g, x, y, s, w, &tabs, sel, ACCENT);
        y += 40.0 * s;
        // the shutter sits at the bottom; rows above stop where it starts
        let shutter_h = 52.0 * s;
        let bottom = r.y + r.h - pad - shutter_h - 8.0 * s;
        let row_h = 34.0 * s;
        let label = |g: &mut Gfx, y: f32, t: &str| {
            g.text(x, y + 7.0 * s, 13.0 * s, MUTED, t, Face::Sans);
        };
        let value_x = x + 92.0 * s;
        // camera
        if y + row_h <= bottom {
            label(g, y, "Camera");
            let name = self.cams.get(self.cam).map(|c| c.name.clone()).unwrap_or_else(|| "none".into());
            let bw = if !self.cams.is_empty() { g.text_width(13.0 * s, "Change", Face::Bold) + 13.0 * s * 1.7 + 6.0 * s } else { 0.0 };
            let n = g.fit(13.0 * s, &name, Face::Bold, (x + w - value_x - bw).max(10.0));
            g.text(value_x, y + 7.0 * s, 13.0 * s, TEXT, &n, Face::Bold);
            if !self.cams.is_empty() {
                Self::small_button(g, x + w - bw + 6.0 * s, y, s, "Change", Event::Key(Key::Char('c')));
            }
            y += row_h;
        }
        // picture size
        if y + row_h <= bottom {
            label(g, y, "Size");
            let m = self.live_mode().map(|m| format!("{}×{}", m.width, m.height)).unwrap_or_else(|| "—".into());
            let bw = g.button(value_x, y, 13.0 * s, &m, fx::CYAN, Event::Key(Key::Char('r')));
            if let Some(mm) = self.live_mode() {
                g.text(value_x + bw + 8.0 * s, y + 7.0 * s, 12.0 * s, DIM, mm.pix.name(), Face::Sans);
            }
            y += row_h;
        }
        // timer
        if y + row_h <= bottom {
            label(g, y, "Timer");
            let mut bx = value_x;
            for t in TIMERS {
                let txt = if t == 0 { "Off".to_string() } else { format!("{t} s") };
                let cw = fx::toggle_chip(g, bx, y, s * 0.85, &txt, self.timer == t, ACCENT);
                g.hit(R::new(bx, y, cw, 34.0 * s * 0.85), Event::Tab(PICK_BASE + 500 + t as usize), &format!("timer {txt}"));
                bx += cw + 6.0 * s;
            }
            y += row_h;
        }
        // mirror
        if y + row_h <= bottom {
            label(g, y, "Mirror view");
            let sw = R::new(value_x, y + 4.0 * s, 46.0 * s, 24.0 * s);
            fx::switch(g, sw, self.mirror, true, ACCENT, s);
            g.hit(sw, Event::Key(Key::Char('m')), "mirror");
            y += row_h;
        }
        // brightness and contrast
        let ctrls = self.controls.clone();
        for c in &ctrls {
            if y + row_h > bottom {
                break;
            }
            label(g, y, &c.name);
            let (down, up) = if c.id == v4l2::CID_BRIGHTNESS { ('[', ']') } else { (';', '\'') };
            let b1 = Self::small_button(g, value_x, y, s, "−", Event::Key(Key::Char(down)));
            let bar_x = value_x + b1 + 6.0 * s;
            let b2w = g.text_width(13.0 * s, "+", Face::Bold) + 13.0 * s * 1.7;
            let bar_w = (x + w - bar_x - b2w - 6.0 * s).max(10.0);
            fx::progress_bar(g, R::new(bar_x, y + 12.0 * s, bar_w, 6.0 * s), c.fraction(), ACCENT, s);
            Self::small_button(g, bar_x + bar_w + 6.0 * s, y, s, "+", Event::Key(Key::Char(up)));
            y += row_h;
        }
        if self.shot == Shot::Passport {
            if y + row_h <= bottom {
                label(g, y, "Photo size");
                let cur = self.cur();
                let n = g.fit(13.0 * s, cur.name, Face::Bold, x + w - value_x - 13.0 * s * 1.7);
                g.button(value_x, y, 13.0 * s, &n, fx::CYAN, Event::Key(Key::Char('p')));
                if !cur.verified && cur.id != specs::CUSTOM_ID {
                    y += row_h - 8.0 * s;
                    g.text(value_x, y, 12.0 * s, fx::AMBER, "UNVERIFIED rules: check before use", Face::Bold);
                }
                y += row_h;
            }
            if y + row_h <= bottom {
                label(g, y, "Frame");
                let mut bx = value_x;
                for (t, k) in [("Zoom −", '-'), ("Zoom +", '=')] {
                    bx += Self::small_button(g, bx, y, s, t, Event::Key(Key::Char(k))) + 6.0 * s;
                }
                y += row_h;
            }
            if y + 2.0 * 16.0 * s <= bottom {
                for line in g.wrap(12.0 * s, "Plain light background, even light, no glasses glare, neutral face. Arrows move the frame.", Face::Sans, w).into_iter().take(((bottom - y) / (16.0 * s)) as usize) {
                    g.text(x, y, 12.0 * s, DIM, &line, Face::Sans);
                    y += 16.0 * s;
                }
            }
        } else if self.shot == Shot::Video && y + 18.0 * s <= bottom {
            g.text(x, y + 4.0 * s, 12.0 * s, DIM, "MJPEG in AVI, picture only (no sound).", Face::Sans);
        }
        // the shutter
        let sr = R::new(x, r.y + r.h - pad - shutter_h, w, shutter_h);
        let recording = self.is_recording();
        let ready = self.live().is_some();
        let text = match (self.shot, recording, self.countdown.is_some()) {
            (_, _, true) => "Cancel timer".to_string(),
            (Shot::Video, true, _) => "Stop recording".to_string(),
            (Shot::Video, false, _) => "Record".to_string(),
            (Shot::Photo, ..) => "Take photo".to_string(),
            (Shot::Passport, ..) => "Take passport photo".to_string(),
        };
        let hot = g.hovered(sr);
        let col = if recording { ERR } else { ACCENT };
        let tc = fx::pill(g, sr, s, col, hot, ready);
        let dot = sr.h * 0.32;
        g.circle(sr.x + sr.h / 2.0 + 4.0 * s, sr.y + sr.h / 2.0, dot, if !ready { DIM } else if recording { rgb(255, 70, 80) } else { col });
        if recording {
            g.rect(sr.x + sr.h / 2.0 + 4.0 * s - dot * 0.45, sr.y + sr.h / 2.0 - dot * 0.45, dot * 0.9, dot * 0.9, TEXT);
        }
        let t = g.fit(16.0 * s, &text, Face::Bold, sr.w - sr.h - 16.0 * s);
        g.text(sr.x + sr.h + 8.0 * s, sr.y + (sr.h - g.line_height(16.0 * s, Face::Bold)) / 2.0, 16.0 * s, tc, &t, Face::Bold);
        g.hit(sr, Event::Key(Key::Char(' ')), "shutter");
    }

    fn draw_overlay(&mut self, g: &mut Gfx, area: R, s: f32) {
        match &self.overlay {
            Overlay::None => {}
            Overlay::Cameras | Overlay::Sizes => {
                let cams = matches!(self.overlay, Overlay::Cameras);
                let items: Vec<String> = if cams { self.camera_rows() } else { self.size_choices().iter().map(|m| m.label()).collect() };
                let current = if cams { Some(self.cam) } else { self.live_mode().and_then(|m| self.size_choices().iter().position(|c| *c == m)) };
                let title = if cams { "Choose a camera" } else { "Picture size" };
                let rh = 36.0 * s;
                // the camera dialog: rows, a line when there is no other camera, and a Refresh button
                let note = (cams && self.cams.len() <= 1).then(|| if self.cams.is_empty() { "No camera found. Plug in a USB camera." } else { "No other camera found. Plug one in; it appears here by itself." });
                let extra = if cams { 16.0 * s + if note.is_some() { 30.0 * s } else { 0.0 } + 44.0 * s } else { 0.0 };
                let p = fx::modal(g, area, s, 420.0 * s, 80.0 * s + rh * items.len().max(1) as f32 + extra, ACCENT, false);
                let mut y = fx::modal_title(g, p, s, title, ACCENT);
                if items.is_empty() && !cams {
                    g.text(p.x + 20.0 * s, y + 8.0 * s, 14.0 * s, MUTED, "Nothing to choose from yet.", Face::Sans);
                }
                for (i, it) in items.iter().enumerate() {
                    if y + rh > p.y + p.h - 8.0 * s {
                        break;
                    }
                    let row = R::new(p.x + 14.0 * s, y, p.w - 28.0 * s, rh - 4.0 * s);
                    let hot = g.hovered(row);
                    fx::list_row(g, row, s, current == Some(i), hot, ACCENT);
                    let t = g.fit(14.0 * s, it, Face::Sans, row.w - 20.0 * s);
                    g.text(row.x + 10.0 * s, row.y + 8.0 * s, 14.0 * s, TEXT, &t, Face::Sans);
                    g.hit(row, Event::Tab(PICK_BASE + i), it);
                    y += rh;
                }
                if cams {
                    if let Some(n) = note {
                        let t = g.fit(13.0 * s, n, Face::Sans, p.w - 40.0 * s);
                        g.text(p.x + 20.0 * s, y + 6.0 * s, 13.0 * s, MUTED, &t, Face::Sans);
                        y += 30.0 * s;
                    }
                    g.button(p.x + 20.0 * s, y + 8.0 * s, 13.0 * s, "Refresh", fx::CYAN, Event::Key(Key::Char('r')));
                }
            }
            Overlay::Viewer(i, pm) => {
                let Some(t) = self.gallery.get(*i) else { return };
                let p = fx::modal(g, area, s, area.w - 40.0 * s, area.h - 20.0 * s, ACCENT, false);
                let ty = fx::modal_title(g, p, s, &name_of(&t.path), ACCENT);
                let box_ = R::new(p.x + 16.0 * s, ty, p.w - 32.0 * s, p.y + p.h - ty - 44.0 * s);
                if let Some(pm) = pm.as_ref().or(t.pm.as_ref()) {
                    let k = (box_.w / pm.width() as f32).min(box_.h / pm.height() as f32);
                    let (w, h) = (pm.width() as f32 * k, pm.height() as f32 * k);
                    let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
                    g.pm.draw_pixmap(0, 0, pm.as_ref(), &paint, Transform::from_translate(box_.x + (box_.w - w) / 2.0, box_.y + (box_.h - h) / 2.0).pre_scale(k, k), None);
                }
                let note = if t.video { format!("{} · a video: play it from MEDIA (Videos) · ← → other shots · Esc close", t.path.display()) } else { format!("{} · ← → other shots · Esc close", t.path.display()) };
                let note = g.fit(13.0 * s, &note, Face::Sans, p.w - 32.0 * s);
                g.text(p.x + 16.0 * s, p.y + p.h - 32.0 * s, 13.0 * s, MUTED, &note, Face::Sans);
            }
            Overlay::PassportResult => {
                let Some(pp) = &self.passport_photo else { return };
                let p = fx::modal(g, area, s, 620.0 * s, 420.0 * s, ACCENT, false);
                let ty = fx::modal_title(g, p, s, "Passport photo", ACCENT);
                let ph = (p.y + p.h - ty - 70.0 * s).max(40.0);
                if let Some(pm) = &pp.pm {
                    let k = ph / pm.height() as f32;
                    let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
                    g.pm.draw_pixmap(0, 0, pm.as_ref(), &paint, Transform::from_translate(p.x + 20.0 * s, ty).pre_scale(k, k), None);
                }
                let sp = &pp.spec;
                let tx = p.x + 20.0 * s + ph * pp.w as f32 / pp.h as f32 + 20.0 * s;
                let tw = p.x + p.w - tx - 16.0 * s;
                let mut y = ty;
                let online = pp.digital.as_ref().map_or(String::new(), |d| format!("Online form file: {}", name_of(d)));
                let check = if sp.verified { format!("Source: {} (checked {})", sp.source, sp.checked) } else if sp.id == specs::CUSTOM_ID { "Custom size: check the rules yourself.".to_string() } else { "UNVERIFIED: no official page confirmed these rules; check before use.".to_string() };
                for (line, c) in [(sp.name.to_string(), TEXT), (format!("{}×{} px at 300 dpi · {}", pp.w, pp.h, name_of(&pp.path)), MUTED), (online, MUTED), (format!("Head {}–{} mm · {} background", sp.head_min_mm, sp.head_max_mm, sp.background), DIM), (sp.note.to_string(), DIM), (check, if sp.verified { DIM } else { fx::AMBER })] {
                    if line.is_empty() {
                        continue;
                    }
                    for l in g.wrap(13.0 * s, &line, Face::Sans, tw.max(40.0)).into_iter().take(3) {
                        g.text(tx, y, 13.0 * s, c, &l, Face::Sans);
                        y += 18.0 * s;
                    }
                    y += 4.0 * s;
                }
                let by = p.y + p.h - 50.0 * s;
                let mut bx = p.x + 20.0 * s;
                for (t, k) in [("Print sheet 4×6 in", '4'), ("Print sheet A4", 'a'), ("Retake", 'r')] {
                    bx += g.button(bx, by, 14.0 * s, t, if k == 'r' { MUTED } else { ACCENT }, Event::Key(Key::Char(k))) + 10.0 * s;
                }
            }
            Overlay::Countries(q, sel) => {
                let found = specs::search(q);
                let p = fx::modal(g, area, s, 620.0 * s, 560.0 * s, ACCENT, false);
                let mut y = fx::modal_title(g, p, s, "Passport or visa photo: country", ACCENT);
                let shown = if q.is_empty() { "type a country (india, usa visa, schengen…)".to_string() } else { format!("search: {q}") };
                g.text(p.x + 20.0 * s, y, 14.0 * s, if q.is_empty() { DIM } else { ACCENT }, &shown, Face::Sans);
                y += 26.0 * s;
                let rh = 32.0 * s;
                let rows = (((p.y + p.h - 16.0 * s - y) / rh) as usize).max(1);
                let first = sel.saturating_sub(rows - 1);
                if found.is_empty() {
                    g.text(p.x + 20.0 * s, y + 6.0 * s, 14.0 * s, MUTED, "No country matches; pick Custom size for any other size.", Face::Sans);
                }
                for (k, i) in found.iter().enumerate().skip(first).take(rows) {
                    let sp = &SPECS[*i];
                    let row = R::new(p.x + 14.0 * s, y, p.w - 28.0 * s, rh - 4.0 * s);
                    let hot = g.hovered(row);
                    fx::list_row(g, row, s, k == *sel, hot, ACCENT);
                    let tag = if sp.verified { format!("checked {}", sp.checked) } else if sp.id == specs::CUSTOM_ID { "your own size".to_string() } else { "unverified".to_string() };
                    let tw = g.text_width(12.0 * s, &tag, Face::Bold);
                    let name = g.fit(14.0 * s, sp.name, Face::Sans, row.w - tw - 36.0 * s);
                    g.text(row.x + 10.0 * s, row.y + 6.0 * s, 14.0 * s, TEXT, &name, Face::Sans);
                    g.text(row.x + row.w - tw - 10.0 * s, row.y + 8.0 * s, 12.0 * s, if sp.verified { OK_C } else { fx::AMBER }, &tag, Face::Bold);
                    g.hit(row, Event::Tab(PICK_BASE + *i), sp.name);
                    y += rh;
                }
            }
            Overlay::Custom(f) => f.render(g, area, s, ACCENT, super::blink(self.ticks)),
        }
    }

    /// A country chosen in the picker (the custom size asks for its width and height first).
    fn pick_country(&mut self, i: usize) {
        let Some(sp) = SPECS.get(i) else { return };
        if sp.id == specs::CUSTOM_ID {
            self.overlay = Overlay::Custom(Form::new("Custom photo size (mm)").text("Width", &format!("{}", self.custom_mm.0), "e.g. 35").text("Height", &format!("{}", self.custom_mm.1), "e.g. 45"));
            return;
        }
        self.overlay = Overlay::None;
        self.choose_spec(sp.id);
    }

    fn open_viewer(&mut self, i: usize) {
        if let Some(t) = self.gallery.get(i) {
            let pm = if t.video { first_video_frame(&t.path) } else { load_picture(&t.path) };
            self.overlay = Overlay::Viewer(i, pm);
        }
    }

    fn overlay_key(&mut self, k: &Key, cx: &AppCx) {
        let n = match &self.overlay {
            Overlay::Cameras => self.cams.len(),
            Overlay::Sizes => self.size_choices().len(),
            _ => 0,
        };
        if let Overlay::Countries(q, sel) = &mut self.overlay {
            let n = specs::search(q).len();
            match k {
                Key::Esc => self.overlay = Overlay::None,
                Key::Up => *sel = sel.saturating_sub(1),
                Key::Down => *sel = (*sel + 1).min(n.saturating_sub(1)),
                Key::Backspace => {
                    q.pop();
                    *sel = 0;
                }
                Key::Char(c) if !c.is_control() => {
                    q.push(*c);
                    *sel = 0;
                }
                Key::Enter => {
                    if let Some(i) = specs::search(q).get(*sel).copied() {
                        self.pick_country(i);
                    }
                }
                _ => {}
            }
            return;
        }
        if let Overlay::Custom(f) = &mut self.overlay {
            match f.handle(k) {
                FormEvent::Cancel => self.overlay = Overlay::None,
                FormEvent::Submit => {
                    let (w, h) = (f.value(0).trim().replace(',', ".").parse::<f32>(), f.value(1).trim().replace(',', ".").parse::<f32>());
                    match (w, h) {
                        (Ok(w), Ok(h)) if (20.0..=120.0).contains(&w) && (20.0..=120.0).contains(&h) => {
                            self.custom_mm = (w, h);
                            self.overlay = Overlay::None;
                            self.choose_spec(specs::CUSTOM_ID);
                        }
                        _ => f.error = Some("Give a width and a height between 20 and 120 mm.".into()),
                    }
                }
                _ => {}
            }
            return;
        }
        match (&self.overlay, k) {
            (_, Key::Esc) => self.overlay = Overlay::None,
            (Overlay::Viewer(i, _), Key::Left | Key::Right) => {
                let len = self.gallery.len().max(1);
                let j = if *k == Key::Left { (*i + len - 1) % len } else { (*i + 1) % len };
                self.open_viewer(j);
            }
            (Overlay::PassportResult, Key::Char('4')) => {
                self.export_sheet(Paper::Photo4x6, cx);
            }
            (Overlay::PassportResult, Key::Char('a')) => {
                self.export_sheet(Paper::A4, cx);
            }
            (Overlay::PassportResult, Key::Char('r')) => self.overlay = Overlay::None,
            (Overlay::Cameras, Key::Char('r')) => self.refresh_cameras(),
            (Overlay::Cameras | Overlay::Sizes, Key::Char(c)) if c.is_ascii_digit() && *c != '0' => {
                let i = (*c as u8 - b'1') as usize;
                if i < n {
                    if matches!(self.overlay, Overlay::Cameras) {
                        self.choose_camera(i)
                    } else {
                        self.choose_size(i)
                    }
                }
            }
            _ => {}
        }
    }
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

impl Drop for CameraApp {
    fn drop(&mut self) {
        // leaving the app finishes a recording properly (Live's own drop does it too, this keeps the thumbnail logic out)
        if let Conn::Live(l) = &self.conn {
            let _ = l.stop_recording();
        }
    }
}

impl App for CameraApp {
    fn title(&self) -> &'static str {
        "Camera"
    }
    fn icon(&self) -> Icon {
        Icon::Camera
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        "Space shoot · 1 2 3 photo/video/passport · t timer · m mirror · c camera · r size · Esc back".into()
    }
    fn help(&self) -> Vec<String> {
        [
            "Space or Enter · take a photo, start or stop recording, or take the passport photo (after the timer when one is set)",
            "1 · Photo (JPEG in Pictures)    2 · Video (MJPEG AVI in Videos, picture only)    3 · Passport",
            "t · timer off, 3 s or 10 s      m · mirror the live picture (saved pictures are never mirrored)",
            "c · choose the camera           r · choose the picture size",
            "[ and ] · brightness            ; and ' · contrast (when the camera has them)",
            "Passport: p · choose the country (type to search; Custom size for any other) · - and = · zoom the frame · arrows · move it",
            "After a passport photo: 4 · print sheet on 4×6 in paper · a · on A4 (PNG and PDF in Pictures) · r · retake",
            "Click a picture in the strip to look at it; ← → move through them",
            "Esc · cancel the timer, close a window, or go back to the apps (a recording is finished and kept)",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn wants_redraw(&mut self) -> bool {
        let newer = self.live().and_then(|l| l.preview()).is_some_and(|(n, _)| n != self.shown);
        newer || self.countdown.is_some() || self.flash > 0 || matches!(self.conn, Conn::Opening(_)) || (self.is_recording() && self.ticks % 10 == 0)
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if matches!(ev, Event::Tick) {
            self.on_tick(cx);
            return Flow::Stay;
        }
        if !matches!(self.overlay, Overlay::None) {
            match ev {
                Event::Key(k) => self.overlay_key(k, cx),
                Event::Paste(t) => {
                    for c in t.chars() {
                        self.overlay_key(&Key::Char(c), cx);
                    }
                }
                Event::Tab(n) if *n >= PICK_BASE && *n < PICK_BASE + 500 => {
                    let i = n - PICK_BASE;
                    if matches!(self.overlay, Overlay::Countries(..)) {
                        self.pick_country(i)
                    } else if matches!(self.overlay, Overlay::Cameras) {
                        self.choose_camera(i)
                    } else if matches!(self.overlay, Overlay::Sizes) {
                        self.choose_size(i)
                    }
                }
                _ => {}
            }
            return Flow::Stay;
        }
        match ev {
            Event::Key(Key::Esc) => {
                if self.countdown.take().is_some() {
                    self.say("Timer cancelled.", true);
                } else {
                    return Flow::Back;
                }
            }
            Event::Key(Key::Char(' ') | Key::Enter) => self.shutter(cx),
            Event::Key(Key::Char('1')) => self.set_shot(Shot::Photo),
            Event::Key(Key::Char('2')) => self.set_shot(Shot::Video),
            Event::Key(Key::Char('3')) => self.set_shot(Shot::Passport),
            Event::Key(Key::Tab) => {
                let next = match self.shot {
                    Shot::Photo => Shot::Video,
                    Shot::Video => Shot::Passport,
                    Shot::Passport => Shot::Photo,
                };
                self.set_shot(next);
            }
            Event::Key(Key::Char('t')) => {
                let i = TIMERS.iter().position(|t| *t == self.timer).unwrap_or(0);
                self.timer = TIMERS[(i + 1) % TIMERS.len()];
                self.say(if self.timer == 0 { "Timer off.".to_string() } else { format!("Timer: {} seconds.", self.timer) }, true);
            }
            Event::Tab(n) if *n >= PICK_BASE + 500 && *n <= PICK_BASE + 510 => self.timer = (*n - PICK_BASE - 500) as u8,
            Event::Key(Key::Char('m')) => self.mirror = !self.mirror,
            Event::Key(Key::Char('c')) => {
                if self.cams.len() > 1 || (self.cams.len() == 1 && !matches!(self.conn, Conn::Failed(_))) {
                    self.refresh_cameras();
                    self.overlay = Overlay::Cameras;
                } else if !self.cams.is_empty() {
                    self.want = None;
                    self.connect();
                    self.say("Starting the camera again.", true);
                } else {
                    let h = self.presence.hint().to_string();
                    self.say(h, false);
                }
            }
            Event::Key(Key::Char('r')) => {
                if self.live().is_some() {
                    self.overlay = Overlay::Sizes;
                }
            }
            Event::Key(Key::Char('[')) => self.step_control(v4l2::CID_BRIGHTNESS, false),
            Event::Key(Key::Char(']')) => self.step_control(v4l2::CID_BRIGHTNESS, true),
            Event::Key(Key::Char(';')) => self.step_control(v4l2::CID_CONTRAST, false),
            Event::Key(Key::Char('\'')) => self.step_control(v4l2::CID_CONTRAST, true),
            Event::Key(Key::Char('p')) if self.shot == Shot::Passport => self.overlay = Overlay::Countries(String::new(), 0),
            Event::Key(Key::Char('-')) if self.shot == Shot::Passport => self.zoom = (self.zoom + 0.05).min(1.0),
            Event::Key(Key::Char('=') | Key::Char('+')) if self.shot == Shot::Passport => self.zoom = (self.zoom - 0.05).max(0.3),
            Event::Key(k @ (Key::Left | Key::Right | Key::Up | Key::Down)) if self.shot == Shot::Passport => {
                let (dx, dy) = match k {
                    Key::Left => (-0.02, 0.0),
                    Key::Right => (0.02, 0.0),
                    Key::Up => (0.0, -0.02),
                    _ => (0.0, 0.02),
                };
                self.center = ((self.center.0 + dx).clamp(0.0, 1.0), (self.center.1 + dy).clamp(0.0, 1.0));
            }
            Event::Key(Key::Char('g')) => self.open_viewer(0),
            Event::Row(i) => self.open_viewer(*i),
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &AppCx, _tick: u64) {
        let pad = 14.0 * s;
        let side_w = (area.w * 0.28).clamp_safe(200.0 * s, 320.0 * s).min(area.w * 0.45);
        let strip_h = (area.h * 0.13).clamp_safe(36.0 * s, 84.0 * s);
        let status_h = 22.0 * s;
        let pv = R::new(area.x + pad, area.y + pad, (area.w - side_w - 3.0 * pad).max(40.0), (area.h - 2.0 * pad - strip_h - status_h - 2.0 * 8.0 * s).max(40.0));
        self.draw_preview(g, pv, s);
        let st = R::new(pv.x, pv.y + pv.h + 8.0 * s, pv.w, status_h);
        match &self.status {
            Some((t, ok)) => fx::status_line(g, st.x, st.y, st.w, s, t, *ok),
            None => {
                let t = match self.shot {
                    Shot::Photo => "Photos are saved as JPEG in your Pictures folder.",
                    Shot::Video => "Videos are saved as MJPEG AVI in your Videos folder (picture only, no sound).",
                    Shot::Passport => "Fit your head between the crown line and the chin band, eyes on the blue line.",
                };
                let t = g.fit(13.0 * s, t, Face::Sans, st.w);
                g.text(st.x, st.y, 13.0 * s, DIM, &t, Face::Sans);
            }
        }
        let strip = R::new(pv.x, st.y + status_h + 8.0 * s, pv.w, strip_h);
        self.draw_strip(g, strip, s);
        let side = R::new(pv.x + pv.w + pad, area.y + pad, side_w, area.h - 2.0 * pad);
        self.draw_side(g, side, s);
        self.draw_overlay(g, area, s);
    }
}

#[cfg(test)]
mod tests;
