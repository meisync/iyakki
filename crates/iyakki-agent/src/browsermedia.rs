//! The browser as the hardware of a session: clipboard (images and rich text), sound out, microphone and webcam.
//!
//! When a person opens a cloud tenant (or a desktop served to browsers) in a web browser, the machine behind it has no speakers,
//! microphone or camera. The page can supply them with HTML5 (`navigator.clipboard`, WebAudio, `getUserMedia`) and this module is
//! the server side of that: the framing, the limits, the policy and the audit lines. It does no I/O of its own except an optional
//! folder for the latest camera frame; the web server (`iyakki-remote`) feeds it and the shell's tiles read from it.
//!
//! Rules (the page and this module both keep them):
//! * the microphone, the camera and reading the clipboard are off until the person switches each one on in the page;
//! * the policy can forbid a kind ([`Policy`]; the tenant setting `media: allow|deny`, default deny for tenants), and a forbidden
//!   switch is answered with a denial and nothing is stored;
//! * a source that is off drops what arrives (a late chunk after a stop is never kept);
//! * the microphone stops by itself after five minutes of sound (the VOICE dictation limit);
//! * everything is audited by kind and time only (`browser.media.on`, `.off`, `.limit`, `.denied`, `browser.clip.in`), never content.
//!
//! Wire format (binary WebSocket messages; the first byte is the channel, everything else is little-endian):
//! page to server: `0x01` microphone, 16-bit signed PCM, mono, 16000 Hz (the page resamples); `0x02` one JPEG camera frame;
//! `0x03` a clipboard image (PNG); `0x04` clipboard HTML (UTF-8).
//! server to page: `0x10` sound: `[0x10, channels u8, rate u32, seq u32, 16-bit signed PCM...]`; `0x11` a clipboard image (PNG);
//! `0x12` clipboard HTML. Picture messages of the screen start with `0x02` too, but only in the server-to-page direction.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub const CH_MIC: u8 = 0x01;
pub const CH_CAMERA: u8 = 0x02;
pub const CH_CLIP_IMAGE: u8 = 0x03;
pub const CH_CLIP_HTML: u8 = 0x04;
pub const OUT_AUDIO: u8 = 0x10;
pub const OUT_CLIP_IMAGE: u8 = 0x11;
pub const OUT_CLIP_HTML: u8 = 0x12;

/// The microphone format the page sends.
pub const MIC_RATE: u32 = 16_000;
/// Bytes of microphone sound per second (16-bit mono).
pub const MIC_BYTES_PER_SEC: usize = 32_000;
/// The same five minutes as the VOICE tile's dictation (F-VOICE-18).
pub const MIC_LIMIT_SECS: usize = 300;
/// One microphone chunk: about a second.
pub const MAX_MIC_CHUNK: usize = 64 * 1024;
/// Sound the consumer has not taken yet; older sound is dropped first.
pub const MIC_BUFFER: usize = 8 * MIC_BYTES_PER_SEC;
pub const MAX_CAMERA_FRAME: usize = 256 * 1024;
/// A camera is a low-rate source: faster frames are dropped (about six a second).
pub const MIN_CAMERA_GAP: Duration = Duration::from_millis(150);
pub const MAX_CLIP_IMAGE: usize = 4 * 1024 * 1024;
pub const MAX_CLIP_HTML: usize = 1024 * 1024;
/// Items kept on the session's rich clipboard history, and their total size.
pub const CLIP_ITEMS: usize = 10;
pub const CLIP_TOTAL: usize = 8 * 1024 * 1024;
/// Sound waiting for the page; older sound is dropped first (about two seconds at 48 kHz stereo).
pub const AUDIO_QUEUE_BYTES: usize = 48_000 * 2 * 2 * 2;
pub const AUDIO_CHUNK_BYTES: usize = 16 * 1024;

/// What can be switched on from the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Mic,
    Camera,
    /// Reading the browser's clipboard (`navigator.clipboard.read`) for the shell.
    ClipRead,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Mic => "mic",
            Kind::Camera => "camera",
            Kind::ClipRead => "clipread",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "mic" => Kind::Mic,
            "camera" => Kind::Camera,
            "clipread" => Kind::ClipRead,
            _ => return None,
        })
    }
    fn index(self) -> usize {
        self as usize
    }
}

/// The tenant setting `media: allow|deny` (default deny).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Allow {
    Allow,
    #[default]
    Deny,
}

/// What the role or tenant policy lets the page switch on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub mic: bool,
    pub camera: bool,
    pub clip_read: bool,
    /// Sound out and rich clipboard items do not need a switch, but the policy can still refuse them.
    pub audio: bool,
}

impl Policy {
    pub fn allow_all() -> Policy {
        Policy { mic: true, camera: true, clip_read: true, audio: true }
    }
    pub fn deny_all() -> Policy {
        Policy { mic: false, camera: false, clip_read: false, audio: false }
    }
    pub fn from_allow(a: Allow) -> Policy {
        if a == Allow::Allow {
            Policy::allow_all()
        } else {
            Policy::deny_all()
        }
    }
    /// `IYAKKI_BROWSER_MEDIA=allow|deny`; `default` when it is unset or says something else.
    pub fn from_env_value(v: Option<&str>, default: Policy) -> Policy {
        match v.map(str::trim) {
            Some("allow") => Policy::allow_all(),
            Some("deny") => Policy::deny_all(),
            _ => default,
        }
    }
    /// The policy of one person: the tenant switch, their role and any refinement of theirs (`iyakki-sec`, default deny).
    pub fn from_sec(p: &iyakki_sec::media::MediaPolicy, user: &str, role: iyakki_sec::policy::Role) -> Policy {
        use iyakki_sec::media::Device;
        Policy { mic: p.allows(user, role, Device::Mic), camera: p.allows(user, role, Device::Camera), clip_read: p.allows(user, role, Device::ClipRead), audio: p.allows(user, role, Device::Audio) }
    }
    /// The policy of one person in a tenant: the tenant's `media.policy` configuration decides when the tenant has one
    /// (`IYAKKI_TENANT_DATA_DIR` names its folder); otherwise `fallback` (the machine setting) stays.
    pub fn for_tenant(fallback: Policy, user: &str, role: iyakki_sec::policy::Role) -> Policy {
        match crate::tenantconfig::enforce::data_dir_from_env() {
            Some(dir) => crate::tenantconfig::enforce::media_policy_for(&dir, None, user, role, fallback),
            None => fallback,
        }
    }
    pub fn allows(&self, k: Kind) -> bool {
        match k {
            Kind::Mic => self.mic,
            Kind::Camera => self.camera,
            Kind::ClipRead => self.clip_read,
        }
    }
}

/// Where the session's sound is played while a page has its sound on (the page's "Play sound on" choice, kept for the session only).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SinkMode {
    /// This computer, the browser's: the Mac's own output is silenced for as long as this page's sound is on.
    #[default]
    Browser,
    /// The browser and the machine's own speakers.
    Both,
    /// The machine's speakers only: nothing is sent to the page.
    Mac,
}

impl SinkMode {
    pub fn name(self) -> &'static str {
        match self {
            SinkMode::Browser => "browser",
            SinkMode::Both => "both",
            SinkMode::Mac => "mac",
        }
    }
    pub fn parse(s: &str) -> Option<SinkMode> {
        [SinkMode::Browser, SinkMode::Both, SinkMode::Mac].into_iter().find(|m| m.name() == s)
    }
}

/// A page that has not asked for sound for this long is taken to be gone (a hung or crashed connection): its choice then stops
/// silencing the machine, whatever state it was left in.
pub const SINK_ALIVE: Duration = Duration::from_secs(4);

/// Which browser sources are on right now (what the tiles and cloud mode look at).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    pub mic: bool,
    pub camera: bool,
    /// A page is connected and has started its sound (after a click: browsers require it).
    pub audio_out: bool,
    pub clip_read: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Denied {
    Policy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipKind {
    Image,
    Html,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipItem {
    pub kind: ClipKind,
    pub bytes: Vec<u8>,
}

/// The answer to [`BrowserMedia::request`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requested {
    Already,
    /// The page was asked to show its Allow button; the person has to click it.
    Asked,
    Denied,
}

/// What a binary message from the page turned into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Accepted {
    Mic(usize),
    Camera,
    Clip(ClipKind),
    /// The microphone reached its five minutes and was switched off.
    MicLimit,
    /// Dropped: the source is off, too big, too fast or malformed (the reason is for tests and logs).
    Dropped(&'static str),
}

type AuditFn = Arc<dyn Fn(&str, &str) + Send + Sync>;

#[derive(Default)]
struct Inner {
    on: [bool; 3],
    audio_out: bool,
    mic: VecDeque<u8>,
    mic_bytes_this_run: usize,
    camera: Option<Vec<u8>>,
    camera_frames: u64,
    camera_last: Option<Instant>,
    clips: VecDeque<ClipItem>,
    audio: VecDeque<u8>,
    audio_rate: u32,
    audio_channels: u8,
    audio_seq: u32,
    audio_dropped: u64,
    sink: SinkMode,
    /// When the page started its sound, and when the connection last asked for sound (a liveness mark).
    audio_on_at: Option<Instant>,
    last_pull: Option<Instant>,
    /// Notices for the page, JSON text (limit reached...).
    notices: Vec<String>,
    /// Changed since the page was last told.
    dirty: bool,
}

/// One person's browser hardware. Shared between the web connection (which feeds it) and the shell (which reads from it).
pub struct BrowserMedia {
    inner: Mutex<Inner>,
    policy: Policy,
    audit: AuditFn,
    dir: Option<PathBuf>,
}

impl BrowserMedia {
    pub fn new(policy: Policy, audit: AuditFn, dir: Option<PathBuf>) -> Arc<BrowserMedia> {
        Arc::new(BrowserMedia { inner: Mutex::new(Inner::default()), policy, audit, dir })
    }

    /// With no audit sink (tests of other crates).
    pub fn silent(policy: Policy) -> Arc<BrowserMedia> {
        BrowserMedia::new(policy, Arc::new(|_, _| {}), None)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// The person switches a kind on or off in the page.
    pub fn set(&self, kind: Kind, on: bool) -> Result<(), Denied> {
        if on && !self.policy.allows(kind) {
            (self.audit)("browser.media.denied", kind.name());
            return Err(Denied::Policy);
        }
        let mut g = self.lock();
        if g.on[kind.index()] == on {
            return Ok(());
        }
        g.on[kind.index()] = on;
        g.dirty = true;
        match kind {
            Kind::Mic => {
                g.mic.clear();
                g.mic_bytes_this_run = 0;
            }
            Kind::Camera if !on => {
                g.camera = None;
                g.camera_last = None;
            }
            _ => {}
        }
        drop(g);
        if kind == Kind::Camera && !on {
            self.export_camera(None);
        }
        (self.audit)(if on { "browser.media.on" } else { "browser.media.off" }, kind.name());
        self.write_sources();
        Ok(())
    }

    /// The page connected (`true`) or went away (`false`): everything it supplied is switched off with it.
    pub fn connection(&self, connected: bool) {
        if connected {
            self.lock().dirty = true;
            return;
        }
        for k in [Kind::Mic, Kind::Camera, Kind::ClipRead] {
            let _ = self.set(k, false);
        }
        self.set_audio_out(false);
    }

    /// The page started its sound (a click was needed) or stopped it.
    pub fn set_audio_out(&self, on: bool) {
        let allowed = on && self.policy.audio;
        let mut g = self.lock();
        if g.audio_out != allowed {
            g.audio_out = allowed;
            g.dirty = true;
            if !allowed {
                g.audio.clear();
                g.audio_on_at = None;
            } else {
                let now = Instant::now();
                g.audio_on_at = Some(now);
                g.last_pull = Some(now);
            }
        }
        drop(g);
        if on && !self.policy.audio {
            (self.audit)("browser.media.denied", "audio");
        }
        self.write_sources();
    }

    /// The person chose where the session's sound plays. Only this session's own setting; nothing is stored beyond the session.
    pub fn set_sink_mode(&self, mode: SinkMode) {
        let mut g = self.lock();
        if g.sink == mode {
            return;
        }
        g.sink = mode;
        g.dirty = true;
        if mode == SinkMode::Mac {
            g.audio.clear(); // nothing goes to the page now
        }
        drop(g);
        (self.audit)("browser.media.route", mode.name());
    }

    pub fn sink_mode(&self) -> SinkMode {
        self.lock().sink
    }

    /// Does this page get a copy of the session's sound? (Its sound is on and it did not choose "The Mac only".)
    pub fn wants_audio(&self) -> bool {
        let g = self.lock();
        g.audio_out && g.sink != SinkMode::Mac
    }

    /// Is this page's connection alive and playing at `now`? A page that stopped asking is not.
    fn playing_at(&self, now: Instant) -> Option<Instant> {
        let g = self.lock();
        match (g.audio_out, g.audio_on_at, g.last_pull) {
            (true, Some(on), Some(pull)) if now.saturating_duration_since(pull) <= SINK_ALIVE => Some(on),
            _ => None,
        }
    }

    pub fn sources(&self) -> Sources {
        let g = self.lock();
        Sources { mic: g.on[0], camera: g.on[1], audio_out: g.audio_out, clip_read: g.on[2] }
    }

    pub fn is_on(&self, k: Kind) -> bool {
        self.lock().on[k.index()]
    }

    /// The names of what is recording now, for the shell's indicator ("browser microphone", "browser camera").
    pub fn indicator(&self) -> Vec<&'static str> {
        let s = self.sources();
        let mut v = Vec::new();
        if s.mic {
            v.push("Browser microphone is recording");
        }
        if s.camera {
            v.push("Browser camera is on");
        }
        v
    }

    /// One line for the sign the shell draws on every screen while the browser's microphone or camera is live (`None`: neither is).
    pub fn indicator_text(&self) -> Option<String> {
        let s = self.sources();
        match (s.mic, s.camera) {
            (true, true) => Some("Your browser's microphone and camera are on. Click to stop.".into()),
            (true, false) => Some("Your browser's microphone is recording. Click to stop.".into()),
            (false, true) => Some("Your browser's camera is on. Click to stop.".into()),
            (false, false) => None,
        }
    }

    /// The sign was clicked: the microphone and the camera are switched off here (the page is told and stops its devices), and the
    /// stop is audited. Returns whether anything was on.
    pub fn stop_capture(&self) -> bool {
        let was = self.indicator_text().is_some();
        for k in [Kind::Mic, Kind::Camera] {
            let _ = self.set(k, false);
        }
        if was {
            (self.audit)("browser.media.stopped", "shell");
        }
        was
    }

    /// The `media_state` message for the page (what is allowed and what is on).
    pub fn state_json(&self) -> String {
        let s = self.sources();
        let p = self.policy;
        serde_json::json!({
            "t": "media_state",
            "sink": self.sink_mode().name(),
            "mic": s.mic, "camera": s.camera, "clipread": s.clip_read,
            "allowed": { "mic": p.mic, "camera": p.camera, "clipread": p.clip_read, "audio": p.audio },
            "mic_limit_s": MIC_LIMIT_SECS,
            "max_camera": MAX_CAMERA_FRAME, "min_camera_gap_ms": MIN_CAMERA_GAP.as_millis() as u64,
            "max_clip_image": MAX_CLIP_IMAGE, "max_clip_html": MAX_CLIP_HTML,
        })
        .to_string()
    }

    /// The state message when something changed since the page was last told, and any notices (the microphone limit).
    pub fn take_messages(&self) -> Vec<String> {
        let (dirty, notices) = {
            let mut g = self.lock();
            (std::mem::take(&mut g.dirty), std::mem::take(&mut g.notices))
        };
        let mut v = Vec::new();
        if dirty {
            v.push(self.state_json());
        }
        v.extend(notices);
        v
    }

    /// A tile (VOICE, CAMERA) wants a browser source that is off. A source is only ever switched on by the person's own click in the
    /// page (browsers require it, and it is the permission prompt), so the page is asked to show its "Allow" button.
    pub fn request(&self, kind: Kind) -> Requested {
        if self.is_on(kind) {
            return Requested::Already;
        }
        if !self.policy.allows(kind) {
            (self.audit)("browser.media.denied", kind.name());
            return Requested::Denied;
        }
        self.lock().notices.push(serde_json::json!({ "t": "media_ask", "kind": kind.name() }).to_string());
        (self.audit)("browser.media.asked", kind.name());
        Requested::Asked
    }

    /// A denial to tell the page about.
    pub fn denial_json(kind: Kind) -> String {
        serde_json::json!({ "t": "media_denied", "kind": kind.name(), "why": "This is not allowed for this session." }).to_string()
    }

    // ---- from the page

    /// One binary message from the page (the first byte is the channel).
    pub fn binary(&self, msg: &[u8]) -> Accepted {
        let Some((&ch, body)) = msg.split_first() else { return Accepted::Dropped("empty") };
        match ch {
            CH_MIC => self.mic_chunk(body),
            CH_CAMERA => self.camera_frame(body),
            CH_CLIP_IMAGE => self.clip_in(ClipKind::Image, body),
            CH_CLIP_HTML => self.clip_in(ClipKind::Html, body),
            _ => Accepted::Dropped("unknown channel"),
        }
    }

    fn mic_chunk(&self, pcm: &[u8]) -> Accepted {
        if pcm.is_empty() || pcm.len() % 2 != 0 || pcm.len() > MAX_MIC_CHUNK {
            return Accepted::Dropped("bad microphone chunk");
        }
        let mut g = self.lock();
        if !g.on[Kind::Mic.index()] {
            return Accepted::Dropped("microphone is off");
        }
        let room = MIC_LIMIT_SECS * MIC_BYTES_PER_SEC - g.mic_bytes_this_run.min(MIC_LIMIT_SECS * MIC_BYTES_PER_SEC);
        let take = pcm.len().min(room) & !1;
        g.mic.extend(&pcm[..take]);
        g.mic_bytes_this_run += take;
        while g.mic.len() > MIC_BUFFER {
            g.mic.pop_front();
        }
        if take < pcm.len() || g.mic_bytes_this_run >= MIC_LIMIT_SECS * MIC_BYTES_PER_SEC {
            g.on[Kind::Mic.index()] = false;
            g.dirty = true;
            g.notices.push(serde_json::json!({ "t": "media_limit", "kind": "mic", "secs": MIC_LIMIT_SECS }).to_string());
            drop(g);
            (self.audit)("browser.media.limit", "mic");
            (self.audit)("browser.media.off", "mic");
            self.write_sources();
            return Accepted::MicLimit;
        }
        Accepted::Mic(take)
    }

    fn camera_frame(&self, jpeg: &[u8]) -> Accepted {
        if jpeg.len() < 4 || jpeg.len() > MAX_CAMERA_FRAME || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
            return Accepted::Dropped("not a JPEG frame");
        }
        let mut g = self.lock();
        if !g.on[Kind::Camera.index()] {
            return Accepted::Dropped("camera is off");
        }
        let now = Instant::now();
        if g.camera_last.is_some_and(|t| now.duration_since(t) < MIN_CAMERA_GAP) {
            return Accepted::Dropped("too fast");
        }
        g.camera_last = Some(now);
        g.camera_frames += 1;
        g.camera = Some(jpeg.to_vec());
        drop(g);
        self.export_camera(Some(jpeg));
        Accepted::Camera
    }

    fn clip_in(&self, kind: ClipKind, body: &[u8]) -> Accepted {
        if !self.policy.clip_read {
            (self.audit)("browser.media.denied", "clipboard");
            return Accepted::Dropped("clipboard is not allowed");
        }
        match kind {
            ClipKind::Image if body.len() > MAX_CLIP_IMAGE || !body.starts_with(b"\x89PNG\r\n\x1a\n") => return Accepted::Dropped("not a PNG or too big"),
            ClipKind::Html if body.is_empty() || body.len() > MAX_CLIP_HTML || std::str::from_utf8(body).is_err() => return Accepted::Dropped("bad HTML"),
            _ => {}
        }
        let mut g = self.lock();
        g.clips.push_back(ClipItem { kind: kind.clone(), bytes: body.to_vec() });
        while g.clips.len() > CLIP_ITEMS || g.clips.iter().map(|c| c.bytes.len()).sum::<usize>() > CLIP_TOTAL {
            g.clips.pop_front();
        }
        drop(g);
        (self.audit)("browser.clip.in", if kind == ClipKind::Image { "image" } else { "html" });
        Accepted::Clip(kind)
    }

    // ---- for the shell and the jobs

    /// Take the microphone sound waiting (16-bit mono 16 kHz), oldest first.
    pub fn take_mic(&self) -> Vec<u8> {
        self.lock().mic.drain(..).collect()
    }

    /// The newest camera frame (JPEG), kept until the camera is switched off.
    pub fn camera(&self) -> Option<Vec<u8>> {
        self.lock().camera.clone()
    }

    pub fn camera_frames(&self) -> u64 {
        self.lock().camera_frames
    }

    /// The rich clipboard history, newest last.
    pub fn clips(&self) -> Vec<ClipItem> {
        self.lock().clips.iter().cloned().collect()
    }

    /// The plain text of the newest HTML item (tags dropped), for the shell's text clipboard.
    pub fn html_as_text(html: &str) -> String {
        let mut out = String::new();
        let (mut tag, mut skip_until): (bool, Option<&str>) = (false, None);
        let mut tag_at = 0;
        let lower = html.to_ascii_lowercase();
        let mut i = 0;
        while i < html.len() {
            let rest = &lower[i..];
            if let Some(end) = skip_until {
                match rest.find(end) {
                    Some(p) => {
                        i += p + end.len();
                        skip_until = None;
                    }
                    None => break,
                }
                continue;
            }
            if rest.starts_with("<script") {
                skip_until = Some("</script>");
                continue;
            }
            if rest.starts_with("<style") {
                skip_until = Some("</style>");
                continue;
            }
            let c = html[i..].chars().next().unwrap_or(' ');
            match c {
                '<' => {
                    tag = true;
                    tag_at = i;
                }
                '>' if tag => {
                    tag = false;
                    let t = &lower[tag_at..];
                    if !out.ends_with('\n') && (t.starts_with("</p") || t.starts_with("<br") || t.starts_with("</div") || t.starts_with("</li") || t.starts_with("</h")) {
                        out.push('\n');
                    }
                }
                _ if !tag => out.push(c),
                _ => {}
            }
            i += c.len_utf8();
        }
        out.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&").trim().to_string()
    }

    // ---- sound out

    /// Queue 16-bit PCM for the page (interleaved). Returns false when no page is listening (nothing is queued then).
    pub fn push_audio(&self, rate: u32, channels: u8, pcm: &[i16]) -> bool {
        let mut g = self.lock();
        if !g.audio_out || g.sink == SinkMode::Mac || !(8_000..=96_000).contains(&rate) || !(1..=2).contains(&channels) {
            return false;
        }
        if (g.audio_rate, g.audio_channels) != (rate, channels) {
            g.audio.clear();
            g.audio_rate = rate;
            g.audio_channels = channels;
        }
        for s in pcm {
            g.audio.extend(s.to_le_bytes());
        }
        while g.audio.len() > AUDIO_QUEUE_BYTES {
            let drop_n = g.audio.len() - AUDIO_QUEUE_BYTES;
            g.audio.drain(..drop_n);
            g.audio_dropped += drop_n as u64;
        }
        true
    }

    /// Sound messages for the page, each at most [`AUDIO_CHUNK_BYTES`] of sound.
    pub fn take_audio(&self) -> Vec<Vec<u8>> {
        let mut g = self.lock();
        g.last_pull = Some(Instant::now());
        let mut out = Vec::new();
        while !g.audio.is_empty() {
            let n = g.audio.len().min(AUDIO_CHUNK_BYTES) & !(2 * g.audio_channels.max(1) as usize - 1);
            if n == 0 {
                break;
            }
            let mut m = Vec::with_capacity(10 + n);
            m.push(OUT_AUDIO);
            m.push(g.audio_channels);
            m.extend(g.audio_rate.to_le_bytes());
            m.extend(g.audio_seq.to_le_bytes());
            g.audio_seq = g.audio_seq.wrapping_add(1);
            m.extend(g.audio.drain(..n));
            out.push(m);
        }
        out
    }

    pub fn audio_dropped(&self) -> u64 {
        self.lock().audio_dropped
    }

    /// A clipboard item for the page to put on the browser's clipboard (`None` when it is too big).
    pub fn clip_out(kind: ClipKind, bytes: &[u8]) -> Option<Vec<u8>> {
        let (tag, max) = if kind == ClipKind::Image { (OUT_CLIP_IMAGE, MAX_CLIP_IMAGE) } else { (OUT_CLIP_HTML, MAX_CLIP_HTML) };
        if bytes.is_empty() || bytes.len() > max {
            return None;
        }
        let mut m = Vec::with_capacity(1 + bytes.len());
        m.push(tag);
        m.extend_from_slice(bytes);
        Some(m)
    }

    // ---- files for other processes

    fn export_camera(&self, jpeg: Option<&[u8]>) {
        let Some(dir) = &self.dir else { return };
        let f = dir.join("camera.jpg");
        match jpeg {
            Some(b) => {
                let tmp = dir.join("camera.jpg.tmp");
                if std::fs::create_dir_all(dir).is_ok() && std::fs::write(&tmp, b).is_ok() {
                    let _ = std::fs::rename(&tmp, &f);
                }
            }
            None => {
                let _ = std::fs::remove_file(&f);
            }
        }
    }

    /// `sources.json` next to the camera frame: which browser sources are on (read by jobs that are not in this process).
    fn write_sources(&self) {
        let Some(dir) = &self.dir else { return };
        let s = self.sources();
        if std::fs::create_dir_all(dir).is_ok() {
            let _ = std::fs::write(dir.join("sources.json"), serde_json::json!({ "mic": s.mic, "camera": s.camera, "audio_out": s.audio_out }).to_string());
        }
    }
}

/// The signed-in people whose browser media is open, by user name (the shell finds its person's here).
fn registry() -> &'static Mutex<Vec<(String, Arc<BrowserMedia>)>> {
    static R: std::sync::OnceLock<Mutex<Vec<(String, Arc<BrowserMedia>)>>> = std::sync::OnceLock::new();
    R.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn register(user: &str, m: Arc<BrowserMedia>) {
    let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
    r.retain(|(u, _)| u != user);
    r.push((user.to_string(), m));
}

pub fn unregister(user: &str) {
    registry().lock().unwrap_or_else(|e| e.into_inner()).retain(|(u, _)| u != user);
}

pub fn for_user(user: &str) -> Option<Arc<BrowserMedia>> {
    registry().lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(u, _)| u == user).map(|(_, m)| m.clone())
}

/// Does any connected page listen to the session's sound? (The mixer sends nothing, and keeps nothing, when this is false.)
pub fn audio_listeners() -> bool {
    registry().lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(_, m)| m.wants_audio())
}

/// Must the machine's own speakers stay silent right now? Only the controlling session decides: the live page (its connection
/// asked for sound a moment ago) whose sound was switched on last. It silences the machine only with "This computer (the browser)".
/// "Both", "The Mac only", a page that went away, switched off or hung, or no page at all: the machine plays as usual.
pub fn local_muted() -> bool {
    local_muted_at(Instant::now())
}

pub fn local_muted_at(now: Instant) -> bool {
    let all: Vec<Arc<BrowserMedia>> = registry().lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(_, m)| m.clone()).collect();
    muted_by(&all, now)
}

/// The rule of [`local_muted`] over a given set of sessions.
pub fn muted_by(sessions: &[Arc<BrowserMedia>], now: Instant) -> bool {
    sessions.iter().filter_map(|m| m.playing_at(now).map(|on| (on, m))).max_by_key(|(on, _)| *on).is_some_and(|(_, m)| m.sink_mode() == SinkMode::Browser)
}

/// The browser media whose camera the desktop should use: a registered page that may use the camera, one that has it on first, else
/// the one that connected last.
pub fn camera_session() -> Option<Arc<BrowserMedia>> {
    let all: Vec<Arc<BrowserMedia>> = registry().lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(_, m)| m.clone()).filter(|m| m.policy().camera).collect();
    all.iter().rev().find(|m| m.is_on(Kind::Camera)).or(all.last()).cloned()
}

/// Offer the mixer's output (16-bit interleaved PCM) to every page that started its sound. True when at least one took it.
pub fn broadcast_audio(rate: u32, channels: u8, pcm: &[i16]) -> bool {
    let all: Vec<Arc<BrowserMedia>> = registry().lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(_, m)| m.clone()).collect();
    let mut any = false;
    for m in all {
        any |= m.push_audio(rate, channels, pcm);
    }
    any
}

/// Where a session's browser media files go: `<runtime dir>/iyakki-browser-media/<user>`.
pub fn session_dir(runtime: &std::path::Path, user: &str) -> PathBuf {
    let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).take(64).collect();
    runtime.join("iyakki-browser-media").join(if safe.is_empty() { "user".to_string() } else { safe })
}

#[cfg(test)]
#[path = "browsermedia_tests.rs"]
mod tests;
