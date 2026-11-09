//! Recording cameras (F-CAM-03): opt-in recording per camera, encrypted segment files, retention, playback, export and a log of who
//! looked at what.
//!
//! * **Opt-in, visible, one click.** Nothing records until the camera's owner turns it on (mode `continuous`, or `schedule` with
//!   weekly windows). It is off by default for every camera. While a camera records, [`Recorder::recording`] says so (the tile shows a
//!   red REC mark on it, the shell a red sign on every screen) and one call, [`Recorder::set`] with `Mode::Off`, stops it.
//! * **No re-encoding.** The recorder opens its own connection to the camera (so the live view is not disturbed; cameras that allow
//!   one connection only will not record while somebody watches, and the page says "lost") and stores what arrives: Motion-JPEG
//!   frames as they are, H.264 access units in Annex-B form with the parameter sets at the start of every segment. Playing H.264
//!   back needs the video add-on; exporting it does not.
//! * **Where.** `<data>/cameras/recordings/<camera>/YYYY/MM/DD/HHMMSS.irec` (local time of the day), 60 s per file, private (0600).
//! * **Encrypted.** Each file is encrypted and authenticated with a key made from the camera's recording key, which lives in the
//!   OWNER's vault (`camrec.<id>`). See [`crate::camrec_format`]. Losing the vault loses the recordings; nothing else can open them.
//! * **Retention.** By age, by size per camera and in total, oldest first; checked at start and every few minutes. A free-space
//!   floor stops recording with a message instead of filling the disk.
//! * **Who may.** Only the owner turns recording on, plays it back and exports it. Administrators may turn it OFF and change the
//!   limits, but are never given a picture. People the camera is shared with can see that it records, nothing more.
//! * **Audit.** `<camera>/audit.log` records live views, snapshots, playback, exports, start/stop, setting changes and retention
//!   deletions (who, what, when, outcome; never pictures, passwords or keys) and the owner can read it.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::camrec_format::{self as fmt, ReadErr, RecKey, RecKind, Segment, SegmentWriter};
use crate::cameras::{CamSecrets, FeedErr, Hub, Spec, Who};
use crate::clock::{Local, Zone};
use iyakki_rtsp as rtsp;
use iyakki_rtsp::url::scrub;

/// The vault entry that holds a camera's recording key.
pub fn key_name(camera: &str) -> String {
    format!("camrec.{camera}")
}

/// Shown on the recordings page and in the assistant's tool text.
pub const KEY_NOTE: &str = "Recordings are encrypted with a key kept in your vault. If you lose your vault (for example you forget your password and have no recovery key), the recordings cannot be opened by anybody, including us.";

const MIB: u64 = 1024 * 1024;
/// The least the free-space floor may be set to.
pub const MIN_FLOOR_MB: u64 = 256;
pub const MAX_WINDOWS: usize = 14;
const SEGMENT_MAX_BYTES: u64 = 64 * MIB;
const AUDIT_ROTATE: u64 = MIB;
/// An export stops at this many pictures or bytes and says so.
pub const EXPORT_MAX_FRAMES: usize = 20_000;
pub const EXPORT_MAX_BYTES: u64 = 2 * 1024 * MIB;
const EXPORT_FLOOR: u64 = 512 * MIB;

// ------------------------------------------------------------------------------------------------ settings

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Off,
    Continuous,
    Schedule,
}

impl Mode {
    pub fn word(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Continuous => "continuous",
            Mode::Schedule => "schedule",
        }
    }
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Mode::Off),
            "continuous" | "always" => Some(Mode::Continuous),
            "schedule" | "scheduled" => Some(Mode::Schedule),
            _ => None,
        }
    }
}

/// A weekly window: the days (bit 0 = Monday ... bit 6 = Sunday) and minutes of the day. `to` before `from` runs past midnight into
/// the next day. (The flow scheduler's cron lines are single instants, not windows, so recording has this small model of its own.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub days: u8,
    pub from_min: u16,
    pub to_min: u16,
}

const DAY_NAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

fn hhmm(s: &str) -> Option<u16> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u16, u16) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    (m < 60 && (h < 24 || (h == 24 && m == 0))).then_some(h * 60 + m)
}

impl Window {
    /// `mon-fri 08:00-18:00`, `daily 22:00-06:00`, `sat,sun 00:00-24:00`, `weekdays 09:00-17:00`, `weekends 00:00-24:00`.
    pub fn parse(text: &str) -> Result<Window, String> {
        let t = text.trim().to_ascii_lowercase();
        let bad = || format!("“{}” is not a window; write for example mon-fri 08:00-18:00 or daily 22:00-06:00", scrub(text).chars().take(60).collect::<String>());
        let (days_t, time_t) = t.split_once(' ').ok_or_else(bad)?;
        let (a, b) = time_t.split_once('-').ok_or_else(bad)?;
        let (from_min, to_min) = (hhmm(a).ok_or_else(bad)?, hhmm(b).ok_or_else(bad)?);
        if from_min == to_min {
            return Err("a window cannot start and end at the same minute; for the whole day write 00:00-24:00".into());
        }
        let idx = |n: &str| DAY_NAMES.iter().position(|d| n.trim().starts_with(d) && n.trim().len() >= 3);
        let mut days = 0u8;
        match days_t {
            "daily" | "every" | "all" => days = 0x7f,
            "weekdays" => days = 0x1f,
            "weekends" => days = 0x60,
            list => {
                for part in list.split(',') {
                    match part.split_once('-') {
                        Some((x, y)) => {
                            let (x, y) = (idx(x).ok_or_else(bad)?, idx(y).ok_or_else(bad)?);
                            let mut i = x;
                            loop {
                                days |= 1 << i;
                                if i == y {
                                    break;
                                }
                                i = (i + 1) % 7;
                            }
                        }
                        None => days |= 1 << idx(part).ok_or_else(bad)?,
                    }
                }
            }
        }
        if days == 0 {
            return Err(bad());
        }
        Ok(Window { days, from_min, to_min })
    }

    pub fn text(&self) -> String {
        let days = if self.days == 0x7f {
            "daily".to_string()
        } else if self.days == 0x1f {
            "weekdays".to_string()
        } else if self.days == 0x60 {
            "weekends".to_string()
        } else {
            (0..7).filter(|i| self.days & (1 << i) != 0).map(|i| DAY_NAMES[i]).collect::<Vec<_>>().join(",")
        };
        format!("{days} {:02}:{:02}-{:02}:{:02}", self.from_min / 60, self.from_min % 60, self.to_min / 60, self.to_min % 60)
    }

    pub fn contains(&self, l: &Local) -> bool {
        let today = (l.weekday as usize + 6) % 7; // Local: 0 = Sunday
        let yesterday = (today + 6) % 7;
        let m = (l.hour * 60 + l.minute) as u16;
        let on = |d: usize| self.days & (1 << d) != 0;
        if self.from_min < self.to_min {
            on(today) && m >= self.from_min && m < self.to_min
        } else {
            (on(today) && m >= self.from_min) || (on(yesterday) && m < self.to_min)
        }
    }
}

fn d_days() -> u32 {
    14
}
fn d_mb() -> u64 {
    20_000
}

/// One camera's recording setting. The default is off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecSettings {
    pub camera: String,
    pub mode: Mode,
    #[serde(default)]
    pub windows: Vec<Window>,
    /// Recordings older than this many days are deleted (0: no age limit).
    #[serde(default = "d_days")]
    pub max_days: u32,
    /// The most this camera's recordings may use, in MiB (0: no limit of its own).
    #[serde(default = "d_mb")]
    pub max_mb: u64,
}

impl RecSettings {
    pub fn off(camera: &str) -> RecSettings {
        RecSettings { camera: camera.into(), mode: Mode::Off, windows: vec![], max_days: d_days(), max_mb: d_mb() }
    }
}

/// Limits for all cameras together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// All recordings together (MiB, 0: no limit).
    pub total_max_mb: u64,
    /// Recording stops when the disk has less than this free (MiB).
    pub min_free_mb: u64,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy { total_max_mb: 100_000, min_free_mb: 5_000 }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    cameras: Vec<RecSettings>,
    #[serde(default)]
    policy: Policy,
}

/// What a person asks to change; `None` keeps the current value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub mode: Option<Mode>,
    pub windows: Option<Vec<Window>>,
    pub max_days: Option<u32>,
    pub max_mb: Option<u64>,
}

// ------------------------------------------------------------------------------------------------ the camera side

/// What the camera sent.
pub enum Unit {
    Jpeg(Vec<u8>),
    H264 { annex_b: Vec<u8>, key: bool },
}

/// A connection to a camera that hands out frames as they arrived.
pub trait RecFeed: Send {
    fn next(&mut self) -> Result<Unit, FeedErr>;
    fn kind(&self) -> RecKind;
    /// H.264: SPS and PPS in Annex-B form (empty for Motion-JPEG or when the camera sends them in the stream).
    fn params(&self) -> Vec<u8> {
        vec![]
    }
}

pub trait RecSource: Send + Sync {
    fn open(&self, spec: &Spec) -> Result<Box<dyn RecFeed>, FeedErr>;
}

/// The real source: the in-house RTSP and HTTP clients.
pub struct NetSource(pub Arc<dyn CamSecrets>);

struct RtspRaw {
    client: rtsp::client::Client<std::net::TcpStream>,
    kind: RecKind,
    params: Vec<u8>,
}

impl RecFeed for RtspRaw {
    fn next(&mut self) -> Result<Unit, FeedErr> {
        match self.client.next_media().map_err(crate::cameras::map_err)? {
            rtsp::client::Media::Jpeg(j) => Ok(Unit::Jpeg(j)),
            rtsp::client::Media::H264(au) => Ok(Unit::H264 { key: au.is_keyframe(), annex_b: au.annex_b() }),
        }
    }
    fn kind(&self) -> RecKind {
        self.kind
    }
    fn params(&self) -> Vec<u8> {
        self.params.clone()
    }
}

struct HttpRaw {
    h: rtsp::httpmjpeg::HttpMjpeg<std::net::TcpStream>,
    last: Option<Instant>,
}

impl RecFeed for HttpRaw {
    fn next(&mut self) -> Result<Unit, FeedErr> {
        if self.h.is_snapshot() {
            if let Some(t) = self.last {
                let gap = Duration::from_millis(500);
                if t.elapsed() < gap {
                    std::thread::sleep(gap - t.elapsed());
                }
            }
            self.last = Some(Instant::now());
        }
        self.h.next_jpeg().map(Unit::Jpeg).map_err(crate::cameras::map_err)
    }
    fn kind(&self) -> RecKind {
        RecKind::Jpeg
    }
}

impl RecSource for NetSource {
    fn open(&self, spec: &Spec) -> Result<Box<dyn RecFeed>, FeedErr> {
        let url = rtsp::url::parse(&spec.address).map_err(|e| FeedErr::Lost(scrub(&e)))?;
        let creds = if spec.secret.is_empty() {
            if spec.user.is_empty() { None } else { Some((spec.user.clone(), String::new())) }
        } else {
            let pw = self.0.get(&spec.owner, &spec.secret).map_err(|e| FeedErr::Auth(format!("{e}. The owner signs in so the vault opens.")))?;
            Some((spec.user.clone(), pw.to_string()))
        };
        match url.scheme {
            rtsp::url::Scheme::Rtsp => {
                let t = crate::cameras::tcp(&url.host, url.port)?;
                let client = rtsp::client::Client::open(t, &url, creds).map_err(crate::cameras::map_err)?;
                let (kind, params) = match &client.codec {
                    rtsp::sdp::Codec::H264 { sps, pps, .. } => {
                        let mut p = Vec::new();
                        for n in [sps, pps] {
                            if !n.is_empty() {
                                p.extend_from_slice(&[0, 0, 0, 1]);
                                p.extend_from_slice(n);
                            }
                        }
                        (RecKind::H264, p)
                    }
                    rtsp::sdp::Codec::Mjpeg => (RecKind::Jpeg, vec![]),
                    rtsp::sdp::Codec::Other(n) => return Err(FeedErr::Lost(format!("the camera sends {n}, which cannot be recorded"))),
                };
                Ok(Box::new(RtspRaw { client, kind, params }))
            }
            rtsp::url::Scheme::Http => {
                let (host, port) = (url.host.clone(), url.port);
                let h = rtsp::httpmjpeg::HttpMjpeg::open(Box::new(move || crate::cameras::tcp(&host, port).map_err(|e| rtsp::Error::Io(format!("{e:?}")))), &url, creds).map_err(crate::cameras::map_err)?;
                Ok(Box::new(HttpRaw { h, last: None }))
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ environment

pub struct RecEnv {
    pub source: Arc<dyn RecSource>,
    /// Free bytes on the volume of a path.
    pub free_space: Arc<dyn Fn(&Path) -> Option<u64> + Send + Sync>,
    pub now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub zone: Arc<dyn Fn() -> Zone + Send + Sync>,
    /// Called with the names of the cameras that record now, when that changes (the shell's red sign).
    pub on_active: Arc<dyn Fn(&[String]) + Send + Sync>,
    pub segment_ms: u64,
    pub retention_every: Duration,
    /// First pause before a lost camera is tried again (doubles up to 30 s).
    pub backoff_ms: u64,
    /// A recorder stopped by a problem is tried again after this long, once the disk has room.
    pub retry_after: Duration,
}

impl RecEnv {
    pub fn real(secrets: Arc<dyn CamSecrets>, on_active: Arc<dyn Fn(&[String]) + Send + Sync>) -> RecEnv {
        RecEnv {
            source: Arc::new(NetSource(secrets)),
            free_space: Arc::new(crate::sharedsite::statvfs_free),
            now_ms: Arc::new(system_ms),
            zone: Arc::new(|| Zone::system_with(None)),
            on_active,
            segment_ms: 60_000,
            retention_every: Duration::from_secs(300),
            backoff_ms: 1000,
            retry_after: Duration::from_secs(60),
        }
    }
}

fn system_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

// ------------------------------------------------------------------------------------------------ files, index, audit

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn root_of(data: &Path) -> PathBuf {
    data.join("cameras").join("recordings")
}

fn private_dir(p: &Path) {
    use std::os::unix::fs::DirBuilderExt;
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(p);
}

fn clean(s: &str, max: usize) -> String {
    scrub(s).chars().map(|c| if c.is_control() { ' ' } else { c }).take(max).collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditLine {
    pub t: u64,
    pub who: String,
    pub ev: String,
    pub detail: String,
    pub ok: bool,
}

/// Add a line to a camera's own log (`recordings/<camera>/audit.log`, mode 0600, rotated at 1 MiB). Never fails the caller.
pub fn log_event(data: &Path, camera: &str, who: &str, ev: &str, detail: &str, ok: bool) {
    log_event_at(data, camera, who, ev, detail, ok, system_ms() / 1000)
}

pub(crate) fn log_event_at(data: &Path, camera: &str, who: &str, ev: &str, detail: &str, ok: bool, t: u64) {
    use std::os::unix::fs::OpenOptionsExt;
    if !safe_id(camera) {
        return;
    }
    let dir = root_of(data).join(camera);
    private_dir(&dir);
    let f = dir.join("audit.log");
    if std::fs::metadata(&f).map(|m| m.len() > AUDIT_ROTATE).unwrap_or(false) {
        let _ = std::fs::rename(&f, dir.join("audit.log.1"));
    }
    let line = AuditLine { t, who: clean(who, 80), ev: clean(ev, 40), detail: clean(detail, 300), ok };
    let Ok(mut text) = serde_json::to_string(&line) else { return };
    text.push('\n');
    if let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).mode(0o600).open(&f) {
        let _ = file.write_all(text.as_bytes());
    }
}

fn read_audit(dir: &Path) -> Vec<AuditLine> {
    let mut out = Vec::new();
    for name in ["audit.log.1", "audit.log"] {
        if let Ok(t) = std::fs::read_to_string(dir.join(name)) {
            out.extend(t.lines().filter_map(|l| serde_json::from_str::<AuditLine>(l).ok()));
        }
    }
    out
}

/// One closed, indexed segment file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegEntry {
    /// Relative to the camera's folder, `YYYY/MM/DD/HHMMSS.irec`.
    pub rel: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub frames: u32,
    /// The size of the file on the disk.
    pub bytes: u64,
    pub kind: RecKind,
}

fn entry_line(e: &SegEntry) -> String {
    format!("{}\t{}\t{}\t{}\t{}\t{}\n", e.rel, e.start_ms, e.end_ms, e.frames, e.bytes, e.kind as u8)
}

fn parse_entry(l: &str) -> Option<SegEntry> {
    let mut p = l.split('\t');
    let rel = p.next()?.to_string();
    if rel.contains("..") || rel.starts_with('/') || !rel.ends_with(".irec") {
        return None;
    }
    let (start_ms, end_ms, frames, bytes, kind) = (p.next()?.parse().ok()?, p.next()?.parse().ok()?, p.next()?.parse().ok()?, p.next()?.parse().ok()?, p.next()?.parse::<u8>().ok()?);
    Some(SegEntry { rel, start_ms, end_ms, frames, bytes, kind: if kind == 2 { RecKind::H264 } else { RecKind::Jpeg } })
}

fn segment_files(cam_dir: &Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let sub = |p: &Path| -> Vec<PathBuf> { std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.path()).collect() };
    for y in sub(cam_dir).into_iter().filter(|p| p.is_dir()) {
        for m in sub(&y).into_iter().filter(|p| p.is_dir()) {
            for d in sub(&m).into_iter().filter(|p| p.is_dir()) {
                for f in sub(&d) {
                    if f.extension().is_some_and(|x| x == "irec") {
                        if let (Ok(rel), Ok(md)) = (f.strip_prefix(cam_dir), std::fs::metadata(&f)) {
                            out.push((rel.to_string_lossy().replace('\\', "/"), md.len()));
                        }
                    }
                }
            }
        }
    }
    out
}

fn scan_entry(cam_dir: &Path, rel: &str, len: u64) -> Option<SegEntry> {
    let s = fmt::scan(&cam_dir.join(rel)).ok()?;
    Some(SegEntry { rel: rel.to_string(), start_ms: s.start_ms, end_ms: s.end_ms(), frames: s.frames.len() as u32, bytes: len, kind: s.kind })
}

// ------------------------------------------------------------------------------------------------ state

/// What a camera's recorder is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecState {
    Off,
    /// Switched on, not recording at the moment (outside the schedule, starting, or the camera cannot be reached; the text says which).
    Waiting(String),
    Recording { since_ms: u64, frames: u64, bytes: u64 },
    /// Stopped by a problem that needs a person (disk nearly full, vault locked ...).
    Problem(String),
}

impl RecState {
    pub fn words(&self) -> String {
        match self {
            RecState::Off => "not recording".into(),
            RecState::Waiting(m) => format!("waiting: {m}"),
            RecState::Recording { frames, bytes, .. } => format!("recording ({frames} pictures, {} MB this run)", bytes / MIB),
            RecState::Problem(m) => format!("stopped: {m}"),
        }
    }
}

struct WorkInfo {
    state: RecState,
    active: Option<PathBuf>,
}

struct Worker {
    name: String,
    stop: Arc<AtomicBool>,
    info: Arc<Mutex<WorkInfo>>,
    handle: std::thread::JoinHandle<()>,
}

#[derive(Default)]
struct State {
    workers: HashMap<String, Worker>,
    /// Cameras stopped for lack of disk space; they start again when there is room.
    disk_blocked: HashMap<String, (String, Instant)>,
    last_retention: Option<Instant>,
    shown: Vec<String>,
}

/// One camera as the recordings page shows it.
#[derive(Clone, Debug)]
pub struct RecStatus {
    pub camera: String,
    pub name: String,
    pub settings: RecSettings,
    pub state: RecState,
    pub recording: bool,
    pub segments: usize,
    pub bytes: u64,
    pub oldest_ms: Option<u64>,
    pub newest_ms: Option<u64>,
    /// The person is the owner: may turn recording on, play it back, export and read the log.
    pub owner: bool,
    pub key_note: &'static str,
}

#[derive(Clone, Debug)]
pub struct DayInfo {
    pub label: String,
    pub day_start_ms: u64,
    /// Recorded stretches (start, end) in milliseconds, merged.
    pub spans: Vec<(u64, u64)>,
    pub frames: u64,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seek {
    AtOrAfter,
    After,
    Before,
}

#[derive(Clone, Debug)]
pub struct PlayFrame {
    pub ts_ms: u64,
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
    pub prev_ms: Option<u64>,
    pub next_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Exported {
    pub path: PathBuf,
    pub frames: usize,
    pub kind: RecKind,
    pub note: String,
}

#[derive(Clone, Debug, Default)]
pub struct RetentionReport {
    /// (camera, segments, bytes, why)
    pub deleted: Vec<(String, usize, u64, String)>,
}

pub struct Recorder {
    hub: Arc<Hub>,
    env: RecEnv,
    root: PathBuf,
    st: Mutex<State>,
    file_lock: Mutex<()>,
    idx: Mutex<HashMap<String, Vec<SegEntry>>>,
    play: Mutex<Option<(PathBuf, zeroize::Zeroizing<String>, Arc<Segment>)>>,
    zone: Mutex<Option<(Instant, Arc<Zone>)>>,
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Arc<Recorder>>> {
    static R: OnceLock<Mutex<HashMap<PathBuf, Arc<Recorder>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// The installation's recorder (made with the first caller's environment; its supervisor thread starts with it).
pub fn shared(hub: Arc<Hub>, env: RecEnv) -> Arc<Recorder> {
    let data = hub.env().data.clone();
    let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(x) = r.get(&data) {
        return x.clone();
    }
    let rec = Arc::new(Recorder::new(hub, env));
    rec.start();
    r.insert(data, rec.clone());
    rec
}

/// The installation's recorder for this hub (made the first time, with the real environment; the supervisor starts with it).
pub fn for_hub(hub: &Arc<Hub>) -> Arc<Recorder> {
    let secrets = hub.env().secrets.clone();
    shared(hub.clone(), RecEnv::real(secrets, Arc::new(|_| {})))
}

/// Ids of the cameras recording now (nothing when no recorder exists yet); the tile's red REC marks.
pub fn active_ids(data: &Path) -> Vec<String> {
    let r = registry().lock().unwrap_or_else(|e| e.into_inner());
    r.get(data).map(|x| x.recording_ids()).unwrap_or_default()
}

/// Names of the cameras recording now, for the red sign (nothing when no recorder exists).
pub fn active_names(data: &Path) -> Vec<String> {
    let r = registry().lock().unwrap_or_else(|e| e.into_inner());
    r.get(data).map(|x| x.recording_names()).unwrap_or_default()
}

/// The one-click stop behind the red sign: turns off every recording of `user`'s cameras.
pub fn stop_all_for(data: &Path, user: &str) -> usize {
    let rec = registry().lock().unwrap_or_else(|e| e.into_inner()).get(data).cloned();
    rec.map(|r| r.stop_all_for(user)).unwrap_or(0)
}

impl Recorder {
    pub fn new(hub: Arc<Hub>, env: RecEnv) -> Recorder {
        let root = root_of(&hub.env().data);
        Recorder { hub, env, root, st: Mutex::new(State::default()), file_lock: Mutex::new(()), idx: Mutex::new(HashMap::new()), play: Mutex::new(None), zone: Mutex::new(None) }
    }

    /// Check the files and the limits now, then supervise: recordings that are switched on (re)start by themselves, windows open and
    /// close, retention runs every few minutes.
    pub fn start(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        std::thread::spawn(move || loop {
            // the first pass can read many files (an index that has to be rebuilt): it never holds up the screen
            match me.upgrade() {
                Some(r) => r.tick(),
                None => break,
            }
            std::thread::sleep(Duration::from_secs(2));
        });
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
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

    fn local(&self, ms: u64) -> Local {
        self.zone().local((ms / 1000) as i64)
    }

    fn cam_dir(&self, cam: &str) -> PathBuf {
        self.root.join(cam)
    }

    // ------------------------------------------------------------------------------ settings

    fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    fn load(&self) -> Store {
        std::fs::read(self.settings_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self, s: &Store) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        private_dir(&self.root);
        let f = self.settings_file();
        let tmp = f.with_extension("json.tmp");
        {
            let mut t = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| format!("could not save the recording settings: {e}"))?;
            t.write_all(&serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let _ = t.sync_all();
        }
        std::fs::rename(&tmp, &f).map_err(|e| format!("could not save the recording settings: {e}"))
    }

    pub fn policy(&self) -> Policy {
        self.load().policy
    }

    /// Change the limits for all cameras together (administrators).
    pub fn set_policy(&self, who: &Who, total_max_mb: Option<u64>, min_free_mb: Option<u64>) -> Result<Policy, String> {
        if !who.manage {
            return Err("only an administrator can change the limits for all cameras".into());
        }
        let _g = self.file_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = self.load();
        if let Some(t) = total_max_mb {
            s.policy.total_max_mb = t;
        }
        if let Some(f) = min_free_mb {
            if f < MIN_FLOOR_MB {
                return Err(format!("the free-space floor cannot be below {MIN_FLOOR_MB} MB: recording must never fill the disk"));
            }
            s.policy.min_free_mb = f;
        }
        self.save(&s)?;
        for c in self.hub.specs() {
            log_event(&self.hub.env().data, &c.id, &who.user, "recording.limits", &format!("all cameras: total {} MB, free-space floor {} MB", s.policy.total_max_mb, s.policy.min_free_mb), true);
        }
        Ok(s.policy.clone())
    }

    fn spec_of(&self, cam: &str) -> Option<Spec> {
        self.hub.specs().into_iter().find(|s| s.id == cam)
    }

    /// The camera and the person's right over its recordings: `Err` when they may not even know it exists.
    fn visible(&self, who: &Who, cam: &str) -> Result<Spec, String> {
        match self.spec_of(cam) {
            Some(s) if crate::cameras::can_view(&s, &who.user) || s.owner == who.user || who.manage => Ok(s),
            _ => Err("no such camera, or you may not see it".into()),
        }
    }

    fn owned(&self, who: &Who, cam: &str, ev: &str) -> Result<Spec, String> {
        let s = self.visible(who, cam)?;
        if s.owner != who.user {
            log_event(&self.hub.env().data, cam, &who.user, ev, "refused: not the owner", false);
            return Err("only the person who added the camera can open its recordings".into());
        }
        Ok(s)
    }

    pub fn settings_of(&self, cam: &str) -> RecSettings {
        self.load().cameras.into_iter().find(|c| c.camera == cam).unwrap_or_else(|| RecSettings::off(cam))
    }

    /// Change a camera's recording setting. Turning recording ON is the owner's alone and needs the owner's vault (it holds the key);
    /// turning it OFF and changing limits is also open to administrators.
    pub fn set(self: &Arc<Self>, who: &Who, cam: &str, ch: &Change) -> Result<RecSettings, String> {
        if cam.starts_with("usb:") {
            return Err("recording is for network cameras; a camera plugged into this computer is not recorded".into());
        }
        let spec = self.visible(who, cam)?;
        let owner = spec.owner == who.user;
        let turns_on = ch.mode.is_some_and(|m| m != Mode::Off);
        if turns_on && !owner {
            log_event(&self.hub.env().data, cam, &who.user, "recording.setting", "refused: only the owner turns recording on", false);
            return Err("only the person who added the camera can turn its recording on".into());
        }
        if !owner && !who.manage {
            return Err("only the person who added the camera, or an administrator, can change its recording".into());
        }
        let _g = self.file_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = self.load();
        let before = store.cameras.iter().find(|c| c.camera == cam).cloned().unwrap_or_else(|| RecSettings::off(cam));
        let mut now = before.clone();
        if let Some(m) = ch.mode {
            now.mode = m;
        }
        if let Some(w) = &ch.windows {
            if w.len() > MAX_WINDOWS {
                return Err(format!("at most {MAX_WINDOWS} windows"));
            }
            now.windows = w.clone();
        }
        if let Some(d) = ch.max_days {
            now.max_days = d;
        }
        if let Some(m) = ch.max_mb {
            now.max_mb = m;
        }
        if now.mode == Mode::Schedule && now.windows.is_empty() {
            return Err("a schedule needs at least one window, for example mon-fri 08:00-18:00".into());
        }
        if turns_on {
            self.ensure_key(&spec)?;
        }
        match store.cameras.iter_mut().find(|c| c.camera == cam) {
            Some(c) => *c = now.clone(),
            None => store.cameras.push(now.clone()),
        }
        self.save(&store)?;
        drop(_g);
        let windows = if now.mode == Mode::Schedule { format!(" ({})", now.windows.iter().map(Window::text).collect::<Vec<_>>().join("; ")) } else { String::new() };
        let detail = format!("mode {}{windows}, keep {} days, at most {} MB", now.mode.word(), now.max_days, now.max_mb);
        log_event(&self.hub.env().data, cam, &who.user, "recording.setting", &detail, true);
        (self.hub.env().audit)(&who.user, "camera.recording", &format!("{} {}", spec.name, scrub(&detail)));
        self.tick();
        Ok(now)
    }

    /// Turn off every recording of the cameras `user` owns (the red sign's click).
    pub fn stop_all_for(self: &Arc<Self>, user: &str) -> usize {
        let mut n = 0;
        for c in self.load().cameras.into_iter().filter(|c| c.mode != Mode::Off) {
            if self.spec_of(&c.camera).is_some_and(|s| s.owner == user) {
                let who = Who { user: user.into(), manage: false, usb_ok: false, home: PathBuf::new(), may_write: Arc::new(|_| false) };
                if self.set(&who, &c.camera, &Change { mode: Some(Mode::Off), ..Default::default() }).is_ok() {
                    n += 1;
                }
            }
        }
        n
    }

    fn ensure_key(&self, spec: &Spec) -> Result<(), String> {
        let name = key_name(&spec.id);
        let secrets = &self.hub.env().secrets;
        if secrets.has(&spec.owner, &name) {
            // a key that is there but cannot be read (the vault is locked) is a refusal, not a reason to make another key
            return secrets.get(&spec.owner, &name).map(|_| ()).map_err(|e| format!("the recording key cannot be read: {e}"));
        }
        let k = RecKey::generate()?;
        secrets.set(&spec.owner, &name, &k.to_hex()).map_err(|e| format!("the recording key could not be stored in your vault, so recording was not turned on: {e}"))
    }

    fn load_key(&self, spec: &Spec) -> Result<RecKey, String> {
        let t = self.hub.env().secrets.get(&spec.owner, &key_name(&spec.id)).map_err(|e| format!("the recording key cannot be read from the owner's vault ({e}); the owner signs in"))?;
        RecKey::from_hex(&t).ok_or_else(|| "the recording key in the vault is damaged".to_string())
    }

    // ------------------------------------------------------------------------------ index

    fn reconcile(&self, cam: &str) -> Vec<SegEntry> {
        let dir = self.cam_dir(cam);
        let active: Option<PathBuf> = self.st().workers.get(cam).and_then(|w| w.info.lock().ok().and_then(|i| i.active.clone()));
        let mut known: HashMap<String, SegEntry> = std::fs::read_to_string(dir.join("index.txt")).unwrap_or_default().lines().filter_map(parse_entry).map(|e| (e.rel.clone(), e)).collect();
        let mut out = Vec::new();
        let mut changed = false;
        let files = segment_files(&dir);
        for (rel, len) in &files {
            if active.as_ref().is_some_and(|a| a == &dir.join(rel)) {
                continue;
            }
            match known.remove(rel) {
                Some(e) if e.bytes == *len => out.push(e),
                _ => {
                    changed = true;
                    match scan_entry(&dir, rel, *len) {
                        Some(e) if e.frames > 0 => out.push(e),
                        Some(_) => {
                            let _ = std::fs::remove_file(dir.join(rel)); // a header and nothing else: a start that never got a picture
                        }
                        None => {}
                    }
                }
            }
        }
        if !known.is_empty() {
            changed = true; // files that are gone
        }
        out.sort_by_key(|e| e.start_ms);
        if changed {
            self.write_index(cam, &out);
        }
        out
    }

    fn write_index(&self, cam: &str, v: &[SegEntry]) {
        use std::os::unix::fs::OpenOptionsExt;
        let dir = self.cam_dir(cam);
        private_dir(&dir);
        let tmp = dir.join("index.txt.tmp");
        let text: String = v.iter().map(entry_line).collect();
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp) {
            if f.write_all(text.as_bytes()).is_ok() {
                let _ = std::fs::rename(&tmp, dir.join("index.txt"));
            }
        }
    }

    /// The closed segments of a camera, oldest first (the file listing is checked against the saved index the first time).
    pub fn entries(&self, cam: &str) -> Vec<SegEntry> {
        if !safe_id(cam) {
            return vec![];
        }
        if let Some(v) = self.idx.lock().unwrap_or_else(|e| e.into_inner()).get(cam) {
            return v.clone();
        }
        let v = self.reconcile(cam);
        self.idx.lock().unwrap_or_else(|e| e.into_inner()).insert(cam.to_string(), v.clone());
        v
    }

    /// Throw the saved index away and read every file again (after a crash, a copy of the folder, a damaged index file).
    pub fn rebuild_index(&self, cam: &str) -> usize {
        if !safe_id(cam) {
            return 0;
        }
        let _ = std::fs::remove_file(self.cam_dir(cam).join("index.txt"));
        self.idx.lock().unwrap_or_else(|e| e.into_inner()).remove(cam);
        self.entries(cam).len()
    }

    fn index_add(&self, cam: &str, e: SegEntry) {
        let mut m = self.idx.lock().unwrap_or_else(|e| e.into_inner());
        if !m.contains_key(cam) {
            drop(m);
            self.entries(cam); // reads the folder, including the file just closed
            return;
        }
        let v = m.get_mut(cam).expect("checked");
        v.retain(|x| x.rel != e.rel);
        v.push(e);
        v.sort_by_key(|x| x.start_ms);
        let copy = v.clone();
        drop(m);
        self.write_index(cam, &copy);
    }

    // ------------------------------------------------------------------------------ retention

    fn limits_of(&self, store: &Store, cam: &str) -> (u32, u64) {
        store.cameras.iter().find(|c| c.camera == cam).map(|c| (c.max_days, c.max_mb)).unwrap_or((d_days(), d_mb()))
    }

    fn cameras_on_disk(&self, store: &Store) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.root).into_iter().flatten().flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().to_str().map(str::to_string)).filter(|n| safe_id(n)).collect();
        for c in &store.cameras {
            if !v.contains(&c.camera) {
                v.push(c.camera.clone());
            }
        }
        v.sort();
        v
    }

    fn delete_segments(&self, cam: &str, rels: &[String]) -> u64 {
        let dir = self.cam_dir(cam);
        let mut bytes = 0;
        let mut m = self.idx.lock().unwrap_or_else(|e| e.into_inner());
        let v = m.entry(cam.to_string()).or_default();
        for rel in rels {
            if let Some(i) = v.iter().position(|e| &e.rel == rel) {
                let e = v.remove(i);
                if std::fs::remove_file(dir.join(&e.rel)).is_ok() {
                    bytes += e.bytes;
                    // empty day, month and year folders go too
                    let mut p = dir.join(&e.rel);
                    for _ in 0..3 {
                        p.pop();
                        if std::fs::remove_dir(&p).is_err() {
                            break;
                        }
                    }
                } else {
                    v.push(e); // could not be deleted: still there, still counted
                }
            }
        }
        v.sort_by_key(|x| x.start_ms);
        let copy = v.clone();
        drop(m);
        self.write_index(cam, &copy);
        *self.play.lock().unwrap_or_else(|e| e.into_inner()) = None;
        bytes
    }

    /// Delete by age, by size per camera and in total, oldest first. Every pass is written to the cameras' logs.
    pub fn enforce(&self) -> RetentionReport {
        let store = self.load();
        let now = (self.env.now_ms)();
        let mut report = RetentionReport::default();
        let cams = self.cameras_on_disk(&store);
        let note = |cam: &str, rels: Vec<String>, bytes: u64, why: String, rep: &mut RetentionReport| {
            if !rels.is_empty() {
                log_event(&self.hub.env().data, cam, "system", "retention.delete", &format!("{} recording files, {} MB: {why}", rels.len(), bytes / MIB), true);
                (self.hub.env().audit)("system", "camera.retention", &format!("{cam}: {} files, {} MB, {why}", rels.len(), bytes / MIB));
                rep.deleted.push((cam.to_string(), rels.len(), bytes, why));
            }
        };
        for cam in &cams {
            let (days, max_mb) = self.limits_of(&store, cam);
            // age
            if days > 0 {
                let cutoff = now.saturating_sub(days as u64 * 86_400_000);
                let old: Vec<String> = self.entries(cam).into_iter().filter(|e| e.end_ms < cutoff).map(|e| e.rel).collect();
                let b = self.delete_segments(cam, &old);
                note(cam, old, b, format!("older than {days} days"), &mut report);
            }
            // size of this camera
            if max_mb > 0 {
                let mut total: u64 = self.entries(cam).iter().map(|e| e.bytes).sum();
                let mut rels = Vec::new();
                for e in self.entries(cam) {
                    if total <= max_mb * MIB {
                        break;
                    }
                    total = total.saturating_sub(e.bytes);
                    rels.push(e.rel);
                }
                let b = self.delete_segments(cam, &rels);
                note(cam, rels, b, format!("over this camera's limit of {max_mb} MB"), &mut report);
            }
        }
        // all cameras together
        if store.policy.total_max_mb > 0 {
            let cap = store.policy.total_max_mb * MIB;
            let mut all: Vec<(String, SegEntry)> = cams.iter().flat_map(|c| self.entries(c).into_iter().map(|e| (c.clone(), e)).collect::<Vec<_>>()).collect();
            all.sort_by_key(|(_, e)| e.start_ms);
            let mut total: u64 = all.iter().map(|(_, e)| e.bytes).sum();
            let mut per: HashMap<String, Vec<String>> = HashMap::new();
            for (c, e) in all {
                if total <= cap {
                    break;
                }
                total = total.saturating_sub(e.bytes);
                per.entry(c).or_default().push(e.rel);
            }
            let mut per: Vec<_> = per.into_iter().collect();
            per.sort();
            for (c, rels) in per {
                let b = self.delete_segments(&c, &rels);
                note(&c, rels, b, format!("over the limit of {} MB for all cameras", store.policy.total_max_mb), &mut report);
            }
        }
        self.st().last_retention = Some(Instant::now());
        report
    }

    fn floor_bytes(&self) -> u64 {
        self.load().policy.min_free_mb.max(MIN_FLOOR_MB) * MIB
    }

    /// `Err(words)` when the disk is below the free-space floor.
    pub fn room(&self) -> Result<(), String> {
        private_dir(&self.root);
        let floor = self.floor_bytes();
        match (self.env.free_space)(&self.root) {
            Some(f) if f < floor => Err(format!("the disk has only {} MB free, below the floor of {} MB; recording stopped so the disk does not fill up", f / MIB, floor / MIB)),
            _ => Ok(()),
        }
    }

    // ------------------------------------------------------------------------------ supervision

    /// One supervision step (also called by the thread every two seconds): start and stop recorders, retention.
    pub fn tick(self: &Arc<Self>) {
        let store = self.load();
        let local = self.local((self.env.now_ms)());
        let specs = self.hub.specs();
        let due = self.st().last_retention.is_none_or(|t| t.elapsed() >= self.env.retention_every);
        if due {
            self.enforce();
        }
        let mut start: Vec<(Spec, RecSettings)> = Vec::new();
        let mut stop: Vec<(String, &'static str)> = Vec::new();
        {
            let mut st = self.st();
            // finished workers: remember why a stop needs a person
            let done: Vec<String> = st.workers.iter().filter(|(_, w)| w.handle.is_finished()).map(|(k, _)| k.clone()).collect();
            for k in done {
                if let Some(w) = st.workers.remove(&k) {
                    let state = w.info.lock().map(|i| i.state.clone()).unwrap_or(RecState::Off);
                    if let RecState::Problem(m) = state {
                        st.disk_blocked.insert(k.clone(), (m.clone(), Instant::now()));
                        log_event(&self.hub.env().data, &k, "system", "recording.stop", &m, false);
                        (self.hub.env().audit)("system", "camera.recording.stop", &format!("{k}: {m}"));
                    }
                }
            }
            for s in &store.cameras {
                let spec = specs.iter().find(|x| x.id == s.camera);
                let want = spec.is_some()
                    && match s.mode {
                        Mode::Off => false,
                        Mode::Continuous => true,
                        Mode::Schedule => s.windows.iter().any(|w| w.contains(&local)),
                    };
                let running = st.workers.contains_key(&s.camera);
                if spec.is_none() && running {
                    stop.push((s.camera.clone(), "the camera was removed"));
                } else if !want && running {
                    stop.push((s.camera.clone(), if s.mode == Mode::Off { "turned off" } else { "the schedule window ended" }));
                } else if want && !running {
                    if let Some((_, since)) = st.disk_blocked.get(&s.camera) {
                        let floor = self.floor_bytes();
                        if since.elapsed() >= self.env.retry_after && (self.env.free_space)(&self.root).is_some_and(|f| f >= floor + floor / 10) {
                            st.disk_blocked.remove(&s.camera);
                        } else {
                            continue;
                        }
                    }
                    if let Some(sp) = spec {
                        start.push((sp.clone(), s.clone()));
                    }
                }
                if s.mode == Mode::Off {
                    st.disk_blocked.remove(&s.camera);
                }
            }
            for (cam, why) in &stop {
                if let Some(w) = st.workers.get(cam) {
                    w.stop.store(true, Ordering::SeqCst);
                }
                let _ = why;
            }
        }
        for (cam, why) in stop {
            // the thread closes its file on its own; the worker is reaped on a later tick
            let w = self.st().workers.remove(&cam);
            if let Some(w) = w {
                let _ = w.handle.join();
            }
            log_event(&self.hub.env().data, &cam, "system", "recording.stop", why, true);
            (self.hub.env().audit)("system", "camera.recording.stop", &format!("{cam}: {why}"));
        }
        for (spec, s) in start {
            if let Err(m) = self.room() {
                self.st().disk_blocked.insert(spec.id.clone(), (m.clone(), Instant::now()));
                log_event(&self.hub.env().data, &spec.id, "system", "recording.stop", &m, false);
                continue;
            }
            let stop = Arc::new(AtomicBool::new(false));
            let info = Arc::new(Mutex::new(WorkInfo { state: RecState::Waiting("starting".into()), active: None }));
            let (me, sp, st2, in2) = (self.clone(), spec.clone(), stop.clone(), info.clone());
            let handle = std::thread::spawn(move || me.run_worker(sp, st2, in2));
            self.st().workers.insert(spec.id.clone(), Worker { name: spec.name.clone(), stop, info, handle });
            log_event(&self.hub.env().data, &spec.id, &spec.owner, "recording.start", s.mode.word(), true);
            (self.hub.env().audit)(&spec.owner, "camera.recording.start", &format!("{} ({})", spec.name, s.mode.word()));
        }
        let names = self.recording_names();
        let changed = {
            let mut st = self.st();
            let c = st.shown != names;
            st.shown = names.clone();
            c
        };
        if changed {
            (self.env.on_active)(&names);
        }
    }

    /// Names of the cameras whose recorder is switched on and running (also while it is connecting): what the red sign says.
    pub fn recording_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.st().workers.values().map(|w| w.name.clone()).collect();
        v.sort();
        v
    }

    pub fn recording_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.st().workers.keys().cloned().collect();
        v.sort();
        v
    }

    /// Whether this camera's recorder is running (the REC mark on the tile).
    pub fn recording(&self, cam: &str) -> bool {
        self.st().workers.contains_key(cam)
    }

    /// Stop every worker and wait for them (shutdown, tests).
    pub fn shutdown(&self) {
        let ws: Vec<Worker> = self.st().workers.drain().map(|(_, w)| w).collect();
        for w in &ws {
            w.stop.store(true, Ordering::SeqCst);
        }
        for w in ws {
            let _ = w.handle.join();
        }
    }

    // ------------------------------------------------------------------------------ the recorder thread

    fn run_worker(self: Arc<Self>, spec: Spec, stop: Arc<AtomicBool>, info: Arc<Mutex<WorkInfo>>) {
        let set = |s: RecState| {
            if let Ok(mut i) = info.lock() {
                i.state = s;
            }
        };
        let base = Duration::from_millis(self.env.backoff_ms.max(5));
        let mut pause = base;
        let nap = |d: Duration| {
            let end = Instant::now() + d;
            while Instant::now() < end && !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(20).min(end.saturating_duration_since(Instant::now())));
            }
        };
        let (mut frames_total, mut bytes_total) = (0u64, 0u64);
        let since = (self.env.now_ms)();
        'outer: while !stop.load(Ordering::SeqCst) {
            let key = match self.load_key(&spec) {
                Ok(k) => k,
                Err(e) => {
                    set(RecState::Waiting(e));
                    nap(Duration::from_secs(2).max(pause));
                    continue;
                }
            };
            set(RecState::Waiting("connecting to the camera".into()));
            let mut feed = match self.env.source.open(&spec) {
                Ok(f) => f,
                Err(e) => {
                    let m = match e {
                        FeedErr::Lost(m) | FeedErr::Auth(m) | FeedErr::Busy(m) | FeedErr::Addon(m) => m,
                        FeedErr::Timeout => "no answer".into(),
                    };
                    set(RecState::Waiting(format!("the camera cannot be reached ({})", scrub(&m))));
                    nap(pause);
                    pause = (pause * 2).min(Duration::from_secs(30));
                    continue;
                }
            };
            pause = base;
            let kind = feed.kind();
            let params = feed.params();
            let mut cur: Option<(SegmentWriter, PathBuf, String, Instant)> = None;
            let mut last_ts = 0u64;
            let mut last_check = Instant::now();
            let why_lost: Option<String>;
            loop {
                if stop.load(Ordering::SeqCst) {
                    self.close_segment(&spec.id, &info, cur.take());
                    break 'outer;
                }
                // rotate on time, check the disk and the key now and then
                if let Some((w, _, _, t0)) = &cur {
                    if t0.elapsed() >= Duration::from_millis(self.env.segment_ms) || w.bytes >= SEGMENT_MAX_BYTES {
                        self.close_segment(&spec.id, &info, cur.take());
                    }
                }
                if last_check.elapsed() >= Duration::from_secs(10).min(Duration::from_millis(self.env.segment_ms)) {
                    last_check = Instant::now();
                    if let Err(m) = self.room() {
                        self.enforce();
                        if let Err(m2) = self.room() {
                            self.close_segment(&spec.id, &info, cur.take());
                            let _ = m;
                            set(RecState::Problem(m2));
                            return;
                        }
                    }
                    if self.load_key(&spec).is_err() {
                        self.close_segment(&spec.id, &info, cur.take());
                        why_lost = Some("the vault that holds the recording key was locked".into());
                        break;
                    }
                }
                match feed.next() {
                    Ok(unit) => {
                        let ts = (self.env.now_ms)().max(last_ts);
                        last_ts = ts;
                        let (data, flags) = match unit {
                            Unit::Jpeg(j) => (j, fmt::F_KEY),
                            Unit::H264 { annex_b, key } => {
                                if cur.is_none() && !key {
                                    continue; // a segment of H.264 starts at a key frame
                                }
                                (annex_b, if key { fmt::F_KEY } else { 0 })
                            }
                        };
                        if cur.is_none() {
                            match self.open_segment(&spec, &key, kind, ts, &params) {
                                Ok((w, path, rel)) => {
                                    if let Ok(mut i) = info.lock() {
                                        i.active = Some(path.clone());
                                    }
                                    cur = Some((w, path, rel, Instant::now()));
                                }
                                Err(e) => {
                                    set(RecState::Problem(e));
                                    return;
                                }
                            }
                        }
                        if let Some((w, ..)) = cur.as_mut() {
                            match w.append(ts, flags, &data) {
                                Ok(()) => {
                                    frames_total += 1;
                                    bytes_total += data.len() as u64;
                                    set(RecState::Recording { since_ms: since, frames: frames_total, bytes: bytes_total });
                                }
                                Err(e) if e.contains("unusable size") => {}
                                Err(e) => {
                                    self.close_segment(&spec.id, &info, cur.take());
                                    set(RecState::Problem(e));
                                    return;
                                }
                            }
                        }
                    }
                    Err(FeedErr::Timeout) => {}
                    Err(FeedErr::Lost(m)) | Err(FeedErr::Auth(m)) | Err(FeedErr::Busy(m)) | Err(FeedErr::Addon(m)) => {
                        self.close_segment(&spec.id, &info, cur.take());
                        why_lost = Some(format!("the camera stopped sending ({})", scrub(&m)));
                        break;
                    }
                }
            }
            set(RecState::Waiting(why_lost.unwrap_or_else(|| "reconnecting".into())));
            nap(pause);
            pause = (pause * 2).min(Duration::from_secs(30));
        }
        self.close_segment(&spec.id, &info, None);
        set(RecState::Off);
    }

    fn open_segment(&self, spec: &Spec, key: &RecKey, kind: RecKind, ts: u64, params: &[u8]) -> Result<(SegmentWriter, PathBuf, String), String> {
        let l = self.local(ts);
        let rel_dir = format!("{:04}/{:02}/{:02}", l.year, l.month, l.day);
        let stem = format!("{:02}{:02}{:02}", l.hour, l.minute, l.second);
        let dir = self.cam_dir(&spec.id).join(&rel_dir);
        for n in 0..50 {
            let name = if n == 0 { format!("{stem}.irec") } else { format!("{stem}-{n}.irec") };
            let path = dir.join(&name);
            if path.exists() {
                continue;
            }
            let mut w = SegmentWriter::create(&path, key, kind, ts)?;
            if kind == RecKind::H264 && !params.is_empty() {
                w.append(ts, fmt::F_PARAMS, params)?;
            }
            return Ok((w, path, format!("{rel_dir}/{name}")));
        }
        Err("could not find a free file name".into())
    }

    fn close_segment(&self, cam: &str, info: &Arc<Mutex<WorkInfo>>, seg: Option<(SegmentWriter, PathBuf, String, Instant)>) {
        let Some((w, path, rel, _)) = seg else { return };
        drop(w);
        if let Ok(mut i) = info.lock() {
            i.active = None;
        }
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        match scan_entry(&self.cam_dir(cam), &rel, len) {
            Some(e) if e.frames > 0 => self.index_add(cam, e),
            _ => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    // ------------------------------------------------------------------------------ status

    pub fn status(&self, who: &Who, cam: &str) -> Result<RecStatus, String> {
        let spec = self.visible(who, cam)?;
        let owner = spec.owner == who.user;
        let settings = self.settings_of(cam);
        let (running, info_state) = {
            let st = self.st();
            (st.workers.contains_key(cam), st.workers.get(cam).and_then(|w| w.info.lock().ok().map(|i| i.state.clone())))
        };
        let blocked = self.st().disk_blocked.get(cam).map(|b| b.0.clone());
        let state = if let Some(s) = info_state.filter(|_| running) {
            s
        } else if let Some(b) = blocked.filter(|_| settings.mode != Mode::Off) {
            RecState::Problem(b)
        } else if settings.mode == Mode::Off {
            RecState::Off
        } else {
            RecState::Waiting(if settings.mode == Mode::Schedule { "outside the schedule".into() } else { "starting".into() })
        };
        let e = if owner { self.entries(cam) } else { vec![] };
        Ok(RecStatus {
            camera: cam.to_string(),
            name: spec.name.clone(),
            settings,
            recording: running,
            state,
            segments: e.len(),
            bytes: e.iter().map(|x| x.bytes).sum(),
            oldest_ms: e.first().map(|x| x.start_ms),
            newest_ms: e.last().map(|x| x.end_ms),
            owner,
            key_note: KEY_NOTE,
        })
    }

    // ------------------------------------------------------------------------------ playback

    fn day_label(&self, ms: u64) -> String {
        let l = self.local(ms);
        format!("{:04}-{:02}-{:02}", l.year, l.month, l.day)
    }

    fn clock_label(&self, ms: u64) -> String {
        let l = self.local(ms);
        format!("{:02}:{:02}:{:02}", l.hour, l.minute, l.second)
    }

    /// `YYYY-MM-DD HH:MM[:SS]` (or a date alone: the start of that day) in the computer's local time, as milliseconds.
    pub fn parse_time(&self, text: &str) -> Result<u64, String> {
        let bad = || format!("“{}” is not a date and time; write for example 2026-11-08 14:30", clean(text, 40));
        let t = text.trim().replace('T', " ");
        let (d, h) = match t.split_once(' ') {
            Some((d, h)) => (d, h.trim()),
            None => (t.as_str(), "00:00"),
        };
        let dp: Vec<&str> = d.split('-').collect();
        let hp: Vec<&str> = h.split(':').collect();
        if dp.len() != 3 || !(2..=3).contains(&hp.len()) {
            return Err(bad());
        }
        let (y, m, dd): (i64, u32, u32) = (dp[0].parse().map_err(|_| bad())?, dp[1].parse().map_err(|_| bad())?, dp[2].parse().map_err(|_| bad())?);
        let (hh, mm, ss): (u32, u32, u32) = (hp[0].parse().map_err(|_| bad())?, hp[1].parse().map_err(|_| bad())?, hp.get(2).map_or(Ok(0), |x| x.parse()).map_err(|_| bad())?);
        if !(2000..=2200).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&dd) || hh > 23 || mm > 59 || ss > 59 {
            return Err(bad());
        }
        Ok(self.zone().local_to_unix(y, m, dd, hh, mm, ss).max(0) as u64 * 1000)
    }

    /// `YYYY-MM-DD HH:MM:SS` in local time.
    pub fn time_text(&self, ms: u64) -> String {
        format!("{} {}", self.day_label(ms), self.clock_label(ms))
    }

    /// The days that have recordings, newest first. Owner only.
    pub fn days(&self, who: &Who, cam: &str) -> Result<Vec<DayInfo>, String> {
        self.owned(who, cam, "recordings.list")?;
        let mut days: Vec<DayInfo> = Vec::new();
        for e in self.entries(cam) {
            let l = self.local(e.start_ms);
            let label = format!("{:04}-{:02}-{:02}", l.year, l.month, l.day);
            if days.last().is_none_or(|d| d.label != label) {
                let start = self.zone().local_to_unix(l.year, l.month, l.day, 0, 0, 0).max(0) as u64 * 1000;
                days.push(DayInfo { label, day_start_ms: start, spans: vec![], frames: 0, bytes: 0 });
            }
            let d = days.last_mut().expect("pushed");
            d.frames += e.frames as u64;
            d.bytes += e.bytes;
            match d.spans.last_mut() {
                Some(last) if e.start_ms <= last.1 + 3_000 => last.1 = last.1.max(e.end_ms),
                _ => d.spans.push((e.start_ms, e.end_ms)),
            }
        }
        days.reverse();
        Ok(days)
    }

    fn segment(&self, cam: &str, spec: &Spec, rel: &str) -> Result<Arc<Segment>, String> {
        let path = self.cam_dir(cam).join(rel);
        // the key is read from the vault on every call: a locked or emptied vault stops playback at once, whatever was cached
        let key = self.load_key(spec).map_err(|e| format!("{e}. Without the key the recordings cannot be opened."))?;
        let hex = key.to_hex();
        if let Some((p, h, s)) = self.play.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if p == &path && **h == **hex {
                return Ok(s.clone());
            }
        }
        let seg = Arc::new(Segment::open(&path, &key).map_err(|e| e.to_string())?);
        *self.play.lock().unwrap_or_else(|e| e.into_inner()) = Some((path, hex, seg.clone()));
        Ok(seg)
    }

    /// Record that the person started to watch recordings of a camera (once per visit to the page). Owner only.
    pub fn play_start(&self, who: &Who, cam: &str, at_ms: u64) -> Result<(), String> {
        let spec = self.owned(who, cam, "playback.start")?;
        let detail = format!("{} from {} {}", spec.name, self.day_label(at_ms), self.clock_label(at_ms));
        log_event(&self.hub.env().data, cam, &who.user, "playback.start", &detail, true);
        (self.hub.env().audit)(&who.user, "camera.playback", &detail);
        Ok(())
    }

    /// One recorded picture, decoded for the screen: the first at or after `at_ms`, the first after it, or the last before it.
    /// `Ok(None)`: there is none in that direction.
    pub fn playback_frame(&self, who: &Who, cam: &str, at_ms: u64, seek: Seek) -> Result<Option<PlayFrame>, String> {
        let spec = self.owned(who, cam, "playback.frame")?;
        let entries = self.entries(cam);
        // the segment: for AtOrAfter / After the first one that ends late enough, for Before the last one that starts early enough
        let mut order: Vec<usize> = (0..entries.len()).collect();
        if seek == Seek::Before {
            order.reverse();
        }
        for i in order {
            let e = &entries[i];
            let relevant = match seek {
                Seek::AtOrAfter => e.end_ms >= at_ms,
                Seek::After => e.end_ms > at_ms,
                Seek::Before => e.start_ms < at_ms,
            };
            if !relevant {
                continue;
            }
            let seg = match self.segment(cam, &spec, &e.rel) {
                Ok(s) => s,
                Err(m) => {
                    log_event(&self.hub.env().data, cam, &who.user, "playback.error", &m, false);
                    return Err(m);
                }
            };
            if seg.scan.kind == RecKind::H264 {
                return Err("This camera was recorded as H.264. Playing it needs the audio and video add-on (Store, Setup), which you download yourself. You can export the clip and open it in a player of your own.".into());
            }
            let fr = &seg.scan.frames;
            let pick = match seek {
                Seek::AtOrAfter => fr.iter().position(|f| f.ts_ms >= at_ms),
                Seek::After => fr.iter().position(|f| f.ts_ms > at_ms),
                Seek::Before => fr.iter().rposition(|f| f.ts_ms < at_ms),
            };
            let Some(k) = pick else { continue };
            let frame = match seg.frame(k) {
                Ok(f) => f,
                Err(e @ ReadErr::Tampered(_)) => {
                    log_event(&self.hub.env().data, cam, &who.user, "playback.error", &e.to_string(), false);
                    return Err(e.to_string());
                }
                Err(e) => return Err(e.to_string()),
            };
            let img = iyakki_web::jpeg::decode(&frame.data).map_err(|_| "this picture could not be decoded".to_string())?;
            let (w, h, rgba) = crate::cameras::shrink(img.width, img.height, &img.rgba, crate::cameras::BIG_W);
            let prev_ms = if k > 0 { Some(fr[k - 1].ts_ms) } else { entries.get(i.wrapping_sub(1)).filter(|_| i > 0).map(|p| p.end_ms) };
            let next_ms = if k + 1 < fr.len() { Some(fr[k + 1].ts_ms) } else { entries.get(i + 1).map(|n| n.start_ms) };
            return Ok(Some(PlayFrame { ts_ms: frame.ts_ms, label: format!("{} {}", self.day_label(frame.ts_ms), self.clock_label(frame.ts_ms)), width: w, height: h, rgba: Arc::new(rgba), prev_ms, next_ms }));
        }
        Ok(None)
    }

    // ------------------------------------------------------------------------------ export

    /// Write the pictures between `from_ms` and `to_ms` (decrypted) into the person's own files. `confirmed` must be true: the caller
    /// shows the person what will be written (the tile's confirmation, the assistant's approval card) before calling.
    pub fn export(&self, who: &Who, cam: &str, from_ms: u64, to_ms: u64, confirmed: bool) -> Result<Exported, String> {
        let spec = self.owned(who, cam, "export")?;
        let range = format!("{} {} to {} {}", self.day_label(from_ms), self.clock_label(from_ms), self.day_label(to_ms), self.clock_label(to_ms));
        let fail = |m: String| {
            log_event(&self.hub.env().data, cam, &who.user, "export", &format!("{range}: {m}"), false);
            Err(m)
        };
        if !confirmed {
            return fail("the export was not confirmed".into());
        }
        if to_ms <= from_ms {
            return fail("the end of the clip must be after its start".into());
        }
        let entries: Vec<SegEntry> = self.entries(cam).into_iter().filter(|e| e.end_ms >= from_ms && e.start_ms <= to_ms).collect();
        if entries.is_empty() {
            return fail("nothing was recorded in that time".into());
        }
        let kind = entries[0].kind;
        let stamp = {
            let l = self.local(from_ms);
            format!("{:04}{:02}{:02}-{:02}{:02}{:02}", l.year, l.month, l.day, l.hour, l.minute, l.second)
        };
        let base = format!("camera-{}-{stamp}", crate::cameras::slug(&spec.name).chars().collect::<String>());
        let parent = who.home.join(if kind == RecKind::H264 { "Videos" } else { "Pictures" });
        if !(who.may_write)(&parent) {
            return fail(format!("your account may not save in {}", parent.display()));
        }
        if (self.env.free_space)(&parent).is_some_and(|f| f < EXPORT_FLOOR) {
            return fail("there is not enough free space to export".into());
        }
        std::fs::create_dir_all(&parent).map_err(|e| format!("could not make {}: {e}", parent.display()))?;
        let key = self.load_key(&spec).map_err(|e| format!("{e}. Without the key the recordings cannot be opened."))?;
        use std::os::unix::fs::OpenOptionsExt;
        let (mut frames, mut bytes, mut note) = (0usize, 0u64, String::new());
        let out_path;
        match kind {
            RecKind::Jpeg => {
                let mut dir = parent.join(&base);
                for n in 2..1000 {
                    if !dir.exists() {
                        break;
                    }
                    dir = parent.join(format!("{base}-{n}"));
                }
                {
                    use std::os::unix::fs::DirBuilderExt;
                    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
                }
                'all: for e in &entries {
                    let seg = Segment::open(&self.cam_dir(cam).join(&e.rel), &key).map_err(|e| e.to_string())?;
                    for k in 0..seg.len() {
                        let ts = seg.scan.frames[k].ts_ms;
                        if ts < from_ms || ts > to_ms {
                            continue;
                        }
                        if frames >= EXPORT_MAX_FRAMES || bytes >= EXPORT_MAX_BYTES {
                            note = format!(" Stopped at the limit of {EXPORT_MAX_FRAMES} pictures or {} GB; export the rest as a second clip.", EXPORT_MAX_BYTES / 1024 / MIB);
                            break 'all;
                        }
                        let f = match seg.frame(k) {
                            Ok(f) => f,
                            Err(err) => return fail(format!("{err}; {frames} pictures were written before it")),
                        };
                        let p = dir.join(format!("frame-{:06}.jpg", frames + 1));
                        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&p).map_err(|e| format!("could not write {}: {e}", p.display()))?;
                        file.write_all(&f.data).map_err(|e| format!("could not write {}: {e}", p.display()))?;
                        (self.hub.env().after_save)(&p);
                        frames += 1;
                        bytes += f.data.len() as u64;
                    }
                }
                out_path = dir;
            }
            RecKind::H264 => {
                let mut p = parent.join(format!("{base}.h264"));
                for n in 2..1000 {
                    if !p.exists() {
                        break;
                    }
                    p = parent.join(format!("{base}-{n}.h264"));
                }
                let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&p).map_err(|e| format!("could not write {}: {e}", p.display()))?;
                let mut begun = false;
                'h: for e in &entries {
                    let seg = Segment::open(&self.cam_dir(cam).join(&e.rel), &key).map_err(|e| e.to_string())?;
                    let fr = &seg.scan.frames;
                    let is_key = |k: usize| fr[k].flags & fmt::F_KEY != 0 && fr[k].flags & fmt::F_PARAMS == 0;
                    // the first segment starts at the last key frame at or before `from` (a decoder needs one to begin)
                    let start_k = if begun { 0 } else { (0..fr.len()).rev().find(|k| is_key(*k) && fr[*k].ts_ms <= from_ms).or_else(|| (0..fr.len()).find(|k| is_key(*k))).unwrap_or(0) };
                    for k in 0..fr.len() {
                        let params = fr[k].flags & fmt::F_PARAMS != 0;
                        if !params && k < start_k {
                            continue;
                        }
                        if !params && fr[k].ts_ms > to_ms {
                            break 'h;
                        }
                        if bytes >= EXPORT_MAX_BYTES {
                            note = format!(" Stopped at the limit of {} GB.", EXPORT_MAX_BYTES / 1024 / MIB);
                            break 'h;
                        }
                        let f = match seg.frame(k) {
                            Ok(f) => f,
                            Err(err) => return fail(format!("{err}; {frames} units were written before it")),
                        };
                        file.write_all(&f.data).map_err(|e| format!("could not write {}: {e}", p.display()))?;
                        bytes += f.data.len() as u64;
                        if !params {
                            frames += 1;
                        }
                    }
                    begun = true;
                }
                (self.hub.env().after_save)(&p);
                note.push_str(" This is raw H.264 (Annex-B); open it with a video player of your own (for example VLC or ffplay).");
                out_path = p;
            }
        }
        if frames == 0 {
            return fail("nothing was recorded in that time".into());
        }
        log_event(&self.hub.env().data, cam, &who.user, "export", &format!("{range}: {frames} pictures to {}", out_path.display()), true);
        (self.hub.env().audit)(&who.user, "camera.export", &format!("{} {range} -> {}", spec.name, out_path.display()));
        Ok(Exported { path: out_path, frames, kind, note })
    }

    // ------------------------------------------------------------------------------ the log

    /// The camera's log, newest first. Owner only (the log says who watched).
    pub fn audit(&self, who: &Who, cam: &str, limit: usize) -> Result<Vec<AuditLine>, String> {
        self.owned(who, cam, "audit.read")?;
        let mut v = read_audit(&self.cam_dir(cam));
        v.reverse();
        v.truncate(limit.clamp(1, 500));
        Ok(v)
    }

    /// Cameras that have recordings or a setting, for people who own them (the assistant's listing).
    pub fn cameras_with_recordings(&self, who: &Who) -> Vec<String> {
        let store = self.load();
        let mine: HashSet<String> = self.hub.specs().into_iter().filter(|s| s.owner == who.user).map(|s| s.id).collect();
        let mut v: Vec<String> = self.cameras_on_disk(&store).into_iter().filter(|c| mine.contains(c)).collect();
        v.sort();
        v
    }
}

#[cfg(test)]
#[path = "camrec_tests.rs"]
mod tests;
