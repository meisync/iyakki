//! The media player an app may embed (docs/development/UI-SDK.md, "Embedding audio and video: the options", option A).
//!
//! An app (an SDK app, a WASM tile, a client) opens a source for the signed-in person, controls it (play, pause, seek, rate, volume, mute,
//! tracks) and reads events. The decoding happens in the core: the app never gets a byte of the file or the stream, and the sound goes
//! through the core's mixer, so the person's volume, the ducking of one sound under a more important one and the sound doctor apply to
//! it like to every other sound. Pictures (the video path, a later slice) reach the app as finished images, never as a stream.
//!
//! A source is one of: a file in the person's home folder (the home-folder policy of the Files view: only the person's own folders, no
//! link followed, names relative to the home), an http(s) stream (through the core's network guard) or a radio station of the person's
//! list. A remote source asks the person first (an approval card); a file of the home does not (the person picked it in the app).
//!
//! One session per person and app: opening again replaces the earlier session of that app. The limits below are the core's: a request
//! that breaks one is refused with a sentence, never cut silently. Audit lines carry names only (the app, the kind of source), never a
//! file name, an address or the content.

use serde::{Deserialize, Serialize};

/// The version of this facet (the types and the trait below). Additive changes only.
pub const MEDIA_FACET_VERSION: u32 = 1;

/// Sessions one person may hold at the same time (one per app, so this is also the number of apps that may play at once).
pub const MAX_MEDIA_SESSIONS: usize = 4;
pub const MAX_MEDIA_URL: usize = 2048;
pub const MAX_MEDIA_NAME: usize = 200;
/// Playback rate in thousandths (1000 is normal speed).
pub const MIN_RATE_PERMILLE: u32 = 500;
pub const MAX_RATE_PERMILLE: u32 = 2000;
/// A file of the home folder larger than this is refused (the core holds it in memory while it plays).
pub const MAX_MEDIA_FILE_BYTES: u64 = 64 << 20;
/// Names the home listing returns, at most.
pub const MAX_MEDIA_LIST: usize = 200;

/// Where the sound (and later the picture) comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MediaSource {
    /// A file in the person's home folder, by the name [`Media::list_home`] gave (relative, `/`-separated).
    Home { name: String },
    /// An http(s) address. The person is asked first; the core's network guard decides whether the address may be reached at all.
    Stream { url: String },
    /// A station of the person's radio list, by its name. The person is asked first.
    Station { name: String },
}

impl MediaSource {
    /// The word for the audit line: names only, never the file, address or station.
    pub fn audit_kind(&self) -> &'static str {
        match self {
            MediaSource::Home { .. } => "home",
            MediaSource::Stream { .. } => "stream",
            MediaSource::Station { .. } => "station",
        }
    }
    /// A remote source asks the person before anything is fetched.
    pub fn is_remote(&self) -> bool {
        !matches!(self, MediaSource::Home { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSpec {
    pub source: MediaSource,
    /// What the app shows while the real title is not known yet.
    pub title: String,
    /// Start playing as soon as the source is open (a remote source still waits for the person's approval).
    pub autoplay: bool,
    /// The starting volume in percent (`None`: 100).
    pub volume: Option<u32>,
}

impl MediaSpec {
    pub fn new(source: MediaSource) -> MediaSpec {
        MediaSpec { source, title: String::new(), autoplay: true, volume: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSession {
    pub id: String,
    /// The person must allow the source first: an [`MediaEvent::Approval`] follows, and [`Media::decide`] answers it.
    pub needs_approval: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaState {
    /// Waiting for the person's answer on a remote source.
    AwaitingApproval,
    /// Fetching and reading the start of the source.
    Opening,
    /// Open and not playing (before the first play, or after a stop).
    Ready,
    Playing,
    Paused,
    /// A live stream is waiting for data.
    Buffering,
    Ended,
    Failed,
}

impl MediaState {
    pub fn word(self) -> &'static str {
        match self {
            MediaState::AwaitingApproval => "waiting for your answer",
            MediaState::Opening => "opening",
            MediaState::Ready => "ready",
            MediaState::Playing => "playing",
            MediaState::Paused => "paused",
            MediaState::Buffering => "buffering",
            MediaState::Ended => "ended",
            MediaState::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Audio,
    Video,
    Caption,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackInfo {
    pub id: u32,
    pub kind: TrackKind,
    pub label: String,
    /// A language tag (`en`, `ta`) or empty.
    pub lang: String,
    pub selected: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaInfo {
    /// The title the file or stream gives (the file name without folders when it has none).
    pub title: String,
    /// `None` for a live stream.
    pub duration_ms: Option<u64>,
    pub seekable: bool,
    pub has_audio: bool,
    pub has_video: bool,
    pub sample_rate: u32,
    pub channels: u32,
    /// The plain name of the format (`MP3`, `WAV`, `Ogg Vorbis`, `Opus`).
    pub codec: String,
    pub tracks: Vec<TrackInfo>,
}

/// What a session tells the app, in order. [`Media::poll`] hands each event out once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MediaEvent {
    /// The person is asked to allow a remote source (shown as a card by the core's approval surface; the app may show `summary` too).
    Approval { summary: String },
    /// The source is open: what it is.
    Opened { info: MediaInfo },
    State { state: MediaState },
    /// The play position, about every 250 ms while playing, and after a seek.
    Position { ms: u64 },
    /// How loud the last moment was, 0 to 1000 (the level meter of the widget).
    Level { peak: u32 },
    /// A problem in a sentence the person can read. The state is `Failed` when playing cannot go on.
    Error { text: String },
}

/// The largest picture the core hands out, in pixels (RGBA, so under 1 MB): a larger one is scaled down in the core first.
pub const MAX_FRAME_PIXELS: u32 = 250_000;

/// One picture of a video session, already converted to RGBA by the core. `seq` grows with every new picture; the picture is the one whose
/// time has come on the sound's clock (late ones are dropped, never queued).
#[derive(Clone, Debug, PartialEq)]
pub struct MediaFrame {
    pub seq: u64,
    pub pts_ms: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: std::sync::Arc<Vec<u8>>,
}

/// One media file of the person's home folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaEntry {
    /// Relative to the home, `/`-separated.
    pub name: String,
    pub size: u64,
    pub video: bool,
}

/// The extensions the core knows as audio files and as video files.
pub const AUDIO_EXTENSIONS: [&str; 6] = ["mp3", "wav", "flac", "ogg", "oga", "opus"];
pub const VIDEO_EXTENSIONS: [&str; 5] = ["mp4", "webm", "mkv", "avi", "mov"];

/// `Some(is_video)` when the name ends in a media extension.
pub fn media_kind_of_name(name: &str) -> Option<bool> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    if AUDIO_EXTENSIONS.contains(&ext.as_str()) {
        Some(false)
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Some(true)
    } else {
        None
    }
}

pub fn check_app(app: &str) -> Result<(), String> {
    crate::assistant::check_app(app)
}

/// The reason a source is refused, as a sentence; used by every implementation so the words are the same.
pub fn check_spec(app: &str, spec: &MediaSpec) -> Result<(), String> {
    check_app(app)?;
    if spec.title.len() > 120 || spec.title.chars().any(char::is_control) {
        return Err("the title must be at most 120 characters without control characters".into());
    }
    if let Some(v) = spec.volume {
        if v > 100 {
            return Err("the volume is 0 to 100 percent".into());
        }
    }
    match &spec.source {
        MediaSource::Home { name } => {
            let p = std::path::Path::new(name);
            if name.is_empty() || name.len() > MAX_MEDIA_NAME || name.chars().any(char::is_control) || p.is_absolute() || p.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
                return Err("that is not a file name of your home folder".into());
            }
            if media_kind_of_name(name).is_none() {
                return Err("that file is not a sound or video file this player knows (MP3, WAV, FLAC, Ogg, Opus; MP4, WebM, MKV, AVI, MOV)".into());
            }
            Ok(())
        }
        MediaSource::Stream { url } => check_url(url),
        MediaSource::Station { name } => {
            if name.is_empty() || name.len() > MAX_MEDIA_NAME || name.chars().any(char::is_control) {
                return Err("the station name must be 1 to 200 characters without control characters".into());
            }
            Ok(())
        }
    }
}

/// An http(s) address with a host, no sign-in part, no spaces or control characters, at most 2048 long.
pub fn check_url(url: &str) -> Result<(), String> {
    if url.len() > MAX_MEDIA_URL || url.chars().any(|c| c.is_control() || c == ' ') {
        return Err("the address is too long or has spaces or control characters in it".into());
    }
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).ok_or("only http and https addresses can be played")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err("the address has no host".into());
    }
    if authority.contains('@') {
        return Err("an address with a user name or password in it is not played".into());
    }
    Ok(())
}

pub fn check_rate(permille: u32) -> Result<(), String> {
    if (MIN_RATE_PERMILLE..=MAX_RATE_PERMILLE).contains(&permille) {
        Ok(())
    } else {
        Err("the speed is 0.5x to 2x".into())
    }
}

pub fn check_volume(percent: u32) -> Result<(), String> {
    if percent <= 100 { Ok(()) } else { Err("the volume is 0 to 100 percent".into()) }
}

/// What a person is told when the switch is off.
pub const SWITCHED_OFF: &str = "apps may not play sound in their own windows for you: the switch is off";

/// What a core without a player says to every call that needs one (a client can tell it from a refusal by comparing).
pub const NO_MEDIA: &str = "this core has no media player";

/// The media player of the core, as an app embeds it. Every call names the signed-in person; a session belongs to the person who opened
/// it and to the app that named itself at `open`; another person's session id is refused like an unknown one.
pub trait Media: Send + Sync {
    /// Whether this person lets apps play sound in their own windows (the person's switch; a core with no player says false).
    fn enabled(&self, _user: &str) -> bool {
        false
    }
    /// The person's switch (the chat tool calls it); switching off stops and closes the person's sessions at once.
    fn set_enabled(&self, _user: &str, _on: bool) -> Result<(), String> {
        Err(NO_MEDIA.into())
    }
    /// How many sessions the person has open now (for the status line).
    fn open_sessions(&self, _user: &str) -> usize {
        0
    }
    /// The media files of the person's home folder (audio first), at most [`MAX_MEDIA_LIST`]. Empty: none, or this core has no player.
    fn list_home(&self, _user: &str) -> Vec<MediaEntry> {
        Vec::new()
    }
    /// Open `spec` for `user` in `app`; an earlier session of the same app is closed.
    fn open(&self, _user: &str, _app: &str, _spec: &MediaSpec) -> Result<MediaSession, String> {
        Err(NO_MEDIA.into())
    }
    /// The person's answer on a remote source (the app passes on the person's choice from its own card, or the core's approval surface).
    fn decide(&self, _user: &str, _session: &str, _allow: bool) -> Result<(), String> {
        Err(NO_MEDIA.into())
    }
    fn play(&self, _user: &str, _session: &str) -> Result<(), String> {
        Err(NO_MEDIA.into())
    }
    fn pause(&self, _user: &str, _session: &str) -> Result<(), String> {
        Err(NO_MEDIA.into())
    }
    /// Move to `ms` (past the end: the end). A live stream cannot be moved: a sentence says so.
    fn seek(&self, _user: &str, _session: &str, _ms: u64) -> Result<(), String> {
        Err(NO_MEDIA.into())
    }
    /// Speed in thousandths, [`MIN_RATE_PERMILLE`] to [`MAX_RATE_PERMILLE`]. The pitch follows the speed.
    fn set_rate(&self, _user: &str, _session: &str, _permille: u32) -> Result<(), String> {
        Err(NO_MEDIA.into())
    }
    /// This session's own volume in percent; the person's master volume applies on top.
    fn set_volume(&self, _user: &str, _session: &str, _percent: u32) -> Result<(), String> {
        Err(NO_MEDIA.into())
    }
    fn set_mute(&self, _user: &str, _session: &str, _mute: bool) -> Result<(), String> {
        Err(NO_MEDIA.into())
    }
    /// Choose the audio, video or caption track with this id (from [`MediaInfo::tracks`]).
    fn select_track(&self, _user: &str, _session: &str, _track: u32) -> Result<(), String> {
        Err(NO_MEDIA.into())
    }
    /// The events since the last call, in order.
    fn poll(&self, _user: &str, _session: &str) -> Vec<MediaEvent> {
        Vec::new()
    }
    /// The picture of a video session that is due now, when its `seq` is above `after`; `None`: nothing new (or the source has no picture).
    fn frame(&self, _user: &str, _session: &str, _after: u64) -> Result<Option<MediaFrame>, String> {
        Err(NO_MEDIA.into())
    }
    /// Close the session and stop its sound.
    fn close(&self, _user: &str, _session: &str) {}
}

/// The player of a core that has none: every call is refused with a sentence, the lists are empty.
pub struct NoMedia;

impl Media for NoMedia {}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(name: &str) -> MediaSpec {
        MediaSpec::new(MediaSource::Home { name: name.into() })
    }

    #[test]
    fn the_limits_refuse_with_a_sentence() {
        assert!(check_spec("player", &home("music/a.mp3")).is_ok());
        assert!(check_spec("Player", &home("a.mp3")).unwrap_err().contains("lower-case"));
        assert!(check_spec("player", &home("../a.mp3")).unwrap_err().contains("home folder"));
        assert!(check_spec("player", &home("/etc/a.mp3")).unwrap_err().contains("home folder"));
        assert!(check_spec("player", &home("notes.txt")).unwrap_err().contains("not a sound"));
        assert!(check_spec("player", &MediaSpec::new(MediaSource::Stream { url: "https://r.example.org/live.mp3".into() })).is_ok());
        for bad in ["ftp://x.org/a", "https://", "https://user:pw@x.org/a", "https://x.org/a b", "file:///etc/passwd"] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
        assert!(check_rate(1000).is_ok() && check_rate(400).is_err() && check_rate(2001).is_err());
        assert!(check_volume(100).is_ok() && check_volume(101).is_err());
        let mut s = home("a.mp3");
        s.volume = Some(150);
        assert!(check_spec("player", &s).is_err());
    }

    #[test]
    fn extensions_tell_audio_from_video() {
        assert_eq!(media_kind_of_name("a/b.MP3"), Some(false));
        assert_eq!(media_kind_of_name("clip.webm"), Some(true));
        assert_eq!(media_kind_of_name("readme"), None);
    }

    #[test]
    fn a_core_with_no_player_refuses_and_lists_nothing() {
        let n = NoMedia;
        assert!(n.list_home("u").is_empty());
        assert!(n.open("u", "player", &home("a.mp3")).unwrap_err().contains("no media player"));
        assert!(n.play("u", "s").is_err() && n.poll("u", "s").is_empty());
    }

    #[test]
    fn the_audit_kind_never_carries_the_name_and_remote_sources_ask() {
        assert_eq!(MediaSource::Stream { url: "https://x.org/".into() }.audit_kind(), "stream");
        assert!(MediaSource::Station { name: "Radio".into() }.is_remote() && !MediaSource::Home { name: "a.mp3".into() }.is_remote());
    }

    #[test]
    fn events_have_a_stable_wire_shape() {
        let e = MediaEvent::Position { ms: 1500 };
        let j = serde_json::to_string(&e).unwrap();
        assert_eq!(j, "{\"type\":\"position\",\"ms\":1500}");
        assert_eq!(serde_json::from_str::<MediaEvent>(&j).unwrap(), e);
        let s = MediaSource::Home { name: "a.mp3".into() };
        assert_eq!(serde_json::to_string(&s).unwrap(), "{\"kind\":\"home\",\"name\":\"a.mp3\"}");
    }
}
