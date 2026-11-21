//! The playback backend of `iyakki.media` and `iyakki.audio` (SDK 1.2, F-SDK-14): what an app starts, pauses and stops is played by the
//! host, through the same player and the same sound path as everything else, so the person's volume and mute always apply and a busy speaker
//! answers `Busy` with the name of the holder.
//!
//! * `media.*` goes to the core's player ([`iyakki_core_api::Media`], the one the MEDIA tile and the embedded audio view use): a file of the
//!   person's home folder by the name `media.library` gave. The app never sees a path, a picture or a sample.
//! * `audio.*` goes to a [`SoundOut`]: short sounds an app brings (a file inside the app, or a WAV it sends), one stream at a time per app.
//!   [`MixerSound`] is the real one over the mixer (the sound path with ducking and the person's volume); tests use [`FakeSound`].
//!
//! The backend keeps no sound and no path: it keeps small numbers (an id for each playback this app started) and nothing else.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use iyakki_core_api::media::{check_spec, media_kind_of_name, MediaEvent, MediaSource, MediaSpec, MediaState};
use iyakki_core_api::Media;
use serde_json::{json, Value};

use super::sdkv2_backends::{Backend, Who};
use super::tilesdk::{code, SdkError};

/// Most items waiting in an app's play queue.
pub const MAX_QUEUE: usize = 50;

/// Why a sound could not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoundError {
    /// Someone else holds the speaker; `holder` is a name the person can read.
    Busy { holder: String },
    /// Not possible here, in words.
    Unavailable(String),
    Bad(String),
}

/// What an app asks to hear.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundSpec {
    pub app: String,
    /// The person whose app plays it (their client hears it in the per-person sound mode).
    pub user: String,
    /// WAV bytes (a file inside the app, or what the app sent).
    pub wav: Vec<u8>,
    pub volume: f64,
    pub looped: bool,
    pub channel: String,
}

/// The sound path as the playback backend needs it. Ids are the backend's own.
pub trait SoundOut: Send + Sync {
    fn start(&self, spec: &SoundSpec) -> Result<u32, SoundError>;
    fn stop(&self, id: u32);
    fn set_paused(&self, id: u32, paused: bool) -> bool;
    fn set_volume(&self, id: u32, volume: f64) -> bool;
    /// The outputs this computer has: (name, active).
    fn outputs(&self) -> Vec<(String, bool)>;
}

/// Where an app's own files are: the bytes of `file` inside the app, or `None`.
pub type AppFile = Arc<dyn Fn(&str, &str) -> Option<Vec<u8>> + Send + Sync>;

#[derive(Default)]
struct State {
    next: u32,
    /// (user, app) -> playback id -> the core's session id.
    sessions: BTreeMap<(String, String), BTreeMap<u32, String>>,
    queue: BTreeMap<(String, String), Vec<String>>,
    /// The sound stream of each app: the backend's id -> the sound path's id.
    sounds: BTreeMap<(String, String), BTreeMap<u32, u32>>,
    /// The last state and position (seconds) seen of each app's playback, so a look that finds no new event (another reader took them) still answers truly.
    last: BTreeMap<(String, String), (String, u64)>,
}

pub struct PlaybackBackend {
    media: Arc<dyn Media>,
    sound: Arc<dyn SoundOut>,
    files: AppFile,
    st: Mutex<State>,
}

fn unavailable(what: &str, fix: &str) -> SdkError {
    SdkError::new(code::UNAVAILABLE, what.to_string()).with("fix", fix)
}

fn failed(e: impl Into<String>) -> SdkError {
    SdkError::new(code::FAILED, e)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn s<'a>(a: &'a Value, k: &str) -> &'a str {
    a.get(k).and_then(Value::as_str).unwrap_or("")
}

impl PlaybackBackend {
    pub fn new(media: Arc<dyn Media>, sound: Arc<dyn SoundOut>, files: AppFile) -> PlaybackBackend {
        PlaybackBackend { media, sound, files, st: Mutex::new(State::default()) }
    }

    fn key(who: &Who) -> (String, String) {
        (who.user.clone(), who.app.clone())
    }

    fn items(&self, who: &Who, kind: Option<&str>) -> Vec<Value> {
        self.media
            .list_home(&who.user)
            .into_iter()
            .filter(|e| match kind {
                Some("audio") => !e.video,
                Some("video") => e.video,
                _ => true,
            })
            .map(|e| {
                let title = e.name.rsplit('/').next().unwrap_or(&e.name).rsplit_once('.').map(|(t, _)| t.to_string()).unwrap_or_else(|| e.name.clone());
                json!({"id": e.name, "title": title, "artist": "", "secs": 0, "kind": if e.video { "video" } else { "audio" }})
            })
            .collect()
    }

    fn open(&self, who: &Who, name: &str, start: u64) -> Result<Value, SdkError> {
        let spec = MediaSpec::new(MediaSource::Home { name: name.to_string() });
        check_spec(&who.app, &spec).map_err(|e| SdkError::bad("file", &e))?;
        let ses = self.media.open(&who.user, &who.app, &spec).map_err(|e| unavailable(&e, "Play it from the MEDIA tile."))?;
        if ses.needs_approval {
            // a home file never needs the person's answer; if the core asks anyway, nothing plays
            self.media.close(&who.user, &ses.id);
            return Err(SdkError::new(code::DENIED, "the player asked for an answer the app cannot give"));
        }
        self.media.play(&who.user, &ses.id).map_err(failed)?;
        if start > 0 {
            // a source that cannot be moved plays from the start; that is not an error for the app
            let _ = self.media.seek(&who.user, &ses.id, start.saturating_mul(1000));
        }
        let mut st = lock(&self.st);
        st.next += 1;
        let id = st.next;
        st.last.remove(&Self::key(who));
        // an app has one playback at a time, like one audio stream: the earlier one is closed
        let old: Vec<String> = st.sessions.entry(Self::key(who)).or_default().values().cloned().collect();
        st.sessions.entry(Self::key(who)).or_default().clear();
        st.sessions.entry(Self::key(who)).or_default().insert(id, ses.id);
        drop(st);
        for o in old {
            self.media.close(&who.user, &o);
        }
        Ok(json!({"id": id}))
    }

    fn session(&self, who: &Who, id: u64) -> Result<String, SdkError> {
        lock(&self.st).sessions.get(&Self::key(who)).and_then(|m| m.get(&(id as u32)).cloned()).ok_or_else(|| SdkError::bad("id", "not a playback this app started"))
    }

    /// The state and position of a session, from the events since the last look; an ended playback moves on to the queue.
    fn state_of(&self, who: &Who, id: Option<u64>) -> Result<Value, SdkError> {
        let (pid, ses) = match id {
            Some(i) => (i as u32, self.session(who, i)?),
            None => match lock(&self.st).sessions.get(&Self::key(who)).and_then(|m| m.iter().next_back().map(|(a, b)| (*a, b.clone()))) {
                Some(x) => x,
                None => return Ok(json!({"state": "stopped", "position": 0})),
            },
        };
        let (mut state, mut pos) = match lock(&self.st).last.get(&Self::key(who)) {
            Some((st, p)) => (match st.as_str() { "playing" => "playing", "paused" => "paused", "ended" => "ended", "failed" => "failed", _ => "ready" }, *p * 1000),
            None => ("ready", 0u64),
        };
        for e in self.media.poll(&who.user, &ses) {
            match e {
                MediaEvent::State { state: st } => {
                    state = match st {
                        MediaState::Playing | MediaState::Buffering => "playing",
                        MediaState::Paused => "paused",
                        MediaState::Ended => "ended",
                        MediaState::Failed => "failed",
                        _ => "ready",
                    }
                }
                MediaEvent::Position { ms } => pos = ms,
                _ => {}
            }
        }
        if state == "ended" {
            let next = {
                let mut st = lock(&self.st);
                let q = st.queue.entry(Self::key(who)).or_default();
                if q.is_empty() { None } else { Some(q.remove(0)) }
            };
            if let Some(n) = next {
                let r = self.open(who, &n, 0)?;
                return Ok(json!({"state": "playing", "position": 0, "id": r["id"]}));
            }
        }
        let _ = pid;
        lock(&self.st).last.insert(Self::key(who), (state.to_string(), pos / 1000));
        Ok(json!({"state": state, "position": pos / 1000}))
    }

    fn audio_play(&self, who: &Who, a: &Value) -> Result<Value, SdkError> {
        let src = a.get("src").cloned().unwrap_or(Value::Null);
        let wav = if let Some(f) = src.get("file").and_then(Value::as_str) {
            (self.files)(&who.app, f).ok_or_else(|| SdkError::bad("file", "a file inside the app"))?
        } else if let Some(w) = src.get("wav").and_then(Value::as_str) {
            iyakki_shared::b64::b64_decode(w).ok_or_else(|| SdkError::bad("wav", "base64 text of a WAV file"))?
        } else {
            return Err(SdkError::bad("file", "a sound"));
        };
        // an .mp3 (from the app's own files or sent by it) is decoded here with the in-house decoder; the sound path itself only takes WAV
        let wav = if wav.starts_with(b"RIFF") { wav } else { mp3_to_wav(&wav).map_err(|e| SdkError::bad("src", &e))? };
        let spec = SoundSpec { app: who.app.clone(), user: who.user.clone(), wav, volume: a.get("volume").and_then(Value::as_f64).unwrap_or(1.0), looped: a.get("loop").and_then(Value::as_bool).unwrap_or(false), channel: s(a, "channel").to_string() };
        let sid = self.sound.start(&spec).map_err(|e| match e {
            SoundError::Busy { holder } => SdkError::new(code::BUSY, format!("the speaker is in use by {holder}")).with("holder", holder),
            SoundError::Unavailable(w) => unavailable(&w, "Check Settings -> Sound."),
            SoundError::Bad(w) => SdkError::bad("wav", &w),
        })?;
        let mut st = lock(&self.st);
        st.next += 1;
        let id = st.next;
        st.sounds.entry(Self::key(who)).or_default().insert(id, sid);
        Ok(json!({"id": id}))
    }

    fn sound_id(&self, who: &Who, id: u64) -> Result<u32, SdkError> {
        lock(&self.st).sounds.get(&Self::key(who)).and_then(|m| m.get(&(id as u32)).copied()).ok_or_else(|| SdkError::bad("id", "not a sound this app started"))
    }
}

impl Backend for PlaybackBackend {
    fn call(&self, who: &Who, op: &str, a: &Value) -> Result<Value, SdkError> {
        match op {
            "media.library" => Ok(Value::Array(self.items(who, a.get("kind").and_then(Value::as_str)))),
            "media.in_folders" => {
                let f = s(a, "file");
                let inside = !f.is_empty() && self.media.list_home(&who.user).iter().any(|e| e.name == f);
                Ok(json!({"inside": inside}))
            }
            "media.play" => {
                let name = if a.get("file").is_some_and(|v| !v.is_null()) {
                    s(a, "file").to_string()
                } else {
                    s(a, "id").to_string()
                };
                if name.is_empty() || media_kind_of_name(&name).is_none() {
                    return Err(SdkError::bad("id", "a name from media.library"));
                }
                // an id must be one the person's own list gave; a file outside the media folders was already asked about by the bridge
                self.open(who, &name, a.get("start").and_then(Value::as_u64).unwrap_or(0))
            }
            "media.pause" | "media.resume" | "media.stop" => {
                let id = a.get("id").and_then(Value::as_u64).unwrap_or(0);
                let ses = self.session(who, id)?;
                match op {
                    "media.pause" => self.media.pause(&who.user, &ses).map_err(failed)?,
                    "media.resume" => self.media.play(&who.user, &ses).map_err(failed)?,
                    _ => {
                        self.media.close(&who.user, &ses);
                        if let Some(m) = lock(&self.st).sessions.get_mut(&Self::key(who)) {
                            m.remove(&(id as u32));
                        }
                    }
                }
                Ok(json!({"state": match op { "media.pause" => "paused", "media.resume" => "playing", _ => "stopped" }}))
            }
            "media.state" => self.state_of(who, a.get("id").and_then(Value::as_u64)),
            "media.queue.add" => {
                let id = s(a, "id").to_string();
                if !self.media.list_home(&who.user).iter().any(|e| e.name == id) {
                    return Err(SdkError::bad("id", "a name from media.library"));
                }
                let mut st = lock(&self.st);
                let q = st.queue.entry(Self::key(who)).or_default();
                if q.len() >= MAX_QUEUE {
                    return Err(SdkError::new(code::QUOTA, "the play queue is full"));
                }
                q.push(id);
                Ok(json!({"queued": q.len()}))
            }
            "media.queue.list" => {
                let q = lock(&self.st).queue.get(&Self::key(who)).cloned().unwrap_or_default();
                Ok(Value::Array(q.into_iter().map(|n| json!({"id": n, "title": n.rsplit('/').next().unwrap_or(&n), "artist": "", "secs": 0, "kind": "audio"})).collect()))
            }
            "audio.play" => self.audio_play(who, a),
            "audio.stop" => {
                let id = a.get("id").and_then(Value::as_u64).unwrap_or(0);
                let sid = self.sound_id(who, id)?;
                self.sound.stop(sid);
                if let Some(m) = lock(&self.st).sounds.get_mut(&Self::key(who)) {
                    m.remove(&(id as u32));
                }
                Ok(json!({}))
            }
            "audio.pause" | "audio.resume" => {
                let sid = self.sound_id(who, a.get("id").and_then(Value::as_u64).unwrap_or(0))?;
                if self.sound.set_paused(sid, op == "audio.pause") { Ok(json!({})) } else { Err(failed("that sound has ended")) }
            }
            "audio.setVolume" => {
                let sid = self.sound_id(who, a.get("id").and_then(Value::as_u64).unwrap_or(0))?;
                let v = a.get("volume").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.0, 1.0);
                if self.sound.set_volume(sid, v) { Ok(json!({})) } else { Err(failed("that sound has ended")) }
            }
            "audio.outputs" => Ok(Value::Array(self.sound.outputs().into_iter().map(|(n, on)| json!({"name": n, "active": on})).collect())),
            other => Err(SdkError::new(code::UNSUPPORTED, format!("{other} is not a playback call"))),
        }
    }
}

// ---- the real sound path

/// [`SoundOut`] over the mixer: each stream is a source of the mixer, fed from a thread that paces itself by the speaker; the person's volume and
/// mute are the mixer's own, the stream's volume is its gain.
pub struct MixerSound {
    streams: Mutex<BTreeMap<u32, crate::mixer::SourceWriter>>,
    next: Mutex<u32>,
}

impl MixerSound {
    pub fn new() -> MixerSound {
        MixerSound { streams: Mutex::new(BTreeMap::new()), next: Mutex::new(0) }
    }
}

impl Default for MixerSound {
    fn default() -> Self {
        MixerSound::new()
    }
}

impl SoundOut for MixerSound {
    fn start(&self, spec: &SoundSpec) -> Result<u32, SoundError> {
        use crate::mixer::SourceKind;
        let mixer = crate::audiosink::mixer_if_on().ok_or_else(|| SoundError::Unavailable("the sound mixer is off".into()))?;
        let w16 = crate::audioout::to_pcm16_wav(&spec.wav);
        let w = crate::voice::parse_wav(w16.as_deref().unwrap_or(&spec.wav)).map_err(SoundError::Bad)?;
        let kind = match spec.channel.as_str() {
            "cue" => SourceKind::Notification,
            "speech" => SourceKind::Speech,
            _ => SourceKind::Media,
        };
        let label = format!("the app {}", spec.app);
        let src = mixer.open_source_with(kind, w.rate, w.channels as usize, &label, false).map_err(|e| match e {
            crate::mixer::MixError::Busy { holder } => SoundError::Busy { holder },
            other => SoundError::Unavailable(other.to_string()),
        })?;
        src.set_owner(&spec.user);
        src.set_gain(spec.volume.clamp(0.0, 1.0) as f32);
        let id = {
            let mut n = lock(&self.next);
            *n += 1;
            *n
        };
        lock(&self.streams).insert(id, src.clone_handle());
        let (samples, looped) = (w.samples.clone(), spec.looped);
        std::thread::spawn(move || {
            loop {
                if src.write_i16_wait(&samples, std::time::Duration::from_secs(30)).is_err() || !looped || src.is_done() {
                    break;
                }
            }
            src.finish();
        });
        Ok(id)
    }
    fn stop(&self, id: u32) {
        if let Some(w) = lock(&self.streams).remove(&id) {
            w.flush();
            w.finish();
        }
    }
    fn set_paused(&self, id: u32, paused: bool) -> bool {
        lock(&self.streams).get(&id).map(|w| w.set_paused(paused)).is_some()
    }
    fn set_volume(&self, id: u32, volume: f64) -> bool {
        lock(&self.streams).get(&id).map(|w| w.set_gain(volume as f32)).is_some()
    }
    fn outputs(&self) -> Vec<(String, bool)> {
        match crate::audiosink::mixer_if_on() {
            Some(m) => {
                let st = m.stats();
                vec![(if st.sink.is_empty() { "Speakers".to_string() } else { st.sink }, true)]
            }
            None => Vec::new(),
        }
    }
}

// ---- the fake

/// A sound path for tests: records what was started, can be told the speaker is busy.
#[derive(Default)]
pub struct FakeSound {
    pub started: Mutex<Vec<SoundSpec>>,
    pub busy: Mutex<Option<String>>,
    pub log: Mutex<Vec<String>>,
}

impl SoundOut for FakeSound {
    fn start(&self, spec: &SoundSpec) -> Result<u32, SoundError> {
        if let Some(h) = lock(&self.busy).clone() {
            return Err(SoundError::Busy { holder: h });
        }
        let mut v = lock(&self.started);
        v.push(spec.clone());
        Ok(v.len() as u32)
    }
    fn stop(&self, id: u32) {
        lock(&self.log).push(format!("stop {id}"));
    }
    fn set_paused(&self, id: u32, paused: bool) -> bool {
        lock(&self.log).push(format!("{} {id}", if paused { "pause" } else { "resume" }));
        true
    }
    fn set_volume(&self, id: u32, volume: f64) -> bool {
        lock(&self.log).push(format!("volume {id} {volume}"));
        true
    }
    fn outputs(&self) -> Vec<(String, bool)> {
        vec![("Speakers".into(), true)]
    }
}

// ---- wiring

static PLAYER: OnceLock<Arc<dyn Media>> = OnceLock::new();

/// The shell hands the core's player over once; apps started afterwards play through it. Without it the playback calls stay `Unavailable`.
pub fn install_player(media: Arc<dyn Media>) -> bool {
    PLAYER.set(media).is_ok()
}

/// The installed player, if any.
pub fn installed_player() -> Option<Arc<dyn Media>> {
    PLAYER.get().cloned()
}

/// The playback backend of the running system, or `None` when no player was installed (the calls then answer `Unavailable` with their fix).
/// `files` gives an app's own files (the installed bundle): a name that leaves the app is refused here, whatever `files` would answer.
pub fn playback_of_system(files: AppFile) -> Option<Arc<dyn Backend>> {
    Some(playback_over(installed_player()?, Arc::new(MixerSound::new()), files))
}

/// The same over a given player and sound path (the system's two, or a test's).
pub fn playback_over(media: Arc<dyn Media>, sound: Arc<dyn SoundOut>, files: AppFile) -> Arc<dyn Backend> {
    let safe: AppFile = Arc::new(move |app, file| {
        if app.is_empty() || app.contains(['/', '\\']) || file.split('/').any(|p| p == ".." || p.is_empty()) || file.starts_with('/') {
            return None;
        }
        files(app, file).filter(|b| b.len() <= 2 * 1024 * 1024)
    });
    Arc::new(PlaybackBackend::new(media, sound, safe))
}

/// The longest sound an app may play from an `.mp3`, in seconds of decoded sound (a 2 MB file is far longer at a low bit rate).
const MP3_MAX_SECONDS: usize = 120;

/// An MP3 as a 16-bit WAV, decoded with the in-house decoder (`nanomp3`): the tag at the start is skipped, a damaged frame is skipped,
/// the sound stops at [`MP3_MAX_SECONDS`]. A text that holds no frame at all is refused with a sentence.
pub fn mp3_to_wav(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let skip = if bytes.len() >= 10 && bytes.starts_with(b"ID3") { 10 + ((usize::from(bytes[6]) & 0x7f) << 21 | (usize::from(bytes[7]) & 0x7f) << 14 | (usize::from(bytes[8]) & 0x7f) << 7 | (usize::from(bytes[9]) & 0x7f)) } else { 0 };
    let mut dec = nanomp3::Decoder::new();
    let mut frame = vec![0i16; nanomp3::MAX_SAMPLES_PER_FRAME];
    let (mut pos, mut rate, mut channels) = (skip.min(bytes.len()), 0u32, 0u16);
    let mut pcm: Vec<i16> = Vec::new();
    while pos < bytes.len() {
        let (used, got) = dec.decode(&bytes[pos..], &mut frame);
        pos += used.max(1);
        if let Ok(f) = got {
            if rate == 0 {
                (rate, channels) = (f.sample_rate, f.channels.num() as u16);
            }
            if f.sample_rate != rate || f.channels.num() as u16 != channels {
                break;
            }
            pcm.extend_from_slice(&frame[..f.samples_produced * usize::from(f.channels.num())]);
            if pcm.len() >= MP3_MAX_SECONDS * rate as usize * usize::from(channels) {
                break;
            }
        }
    }
    if pcm.is_empty() || rate == 0 || channels == 0 {
        return Err("a sound must be a WAV or an MP3 file".into());
    }
    let data_len = (pcm.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + pcm.len() * 2);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    w.extend_from_slice(&(channels * 2).to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for v in pcm {
        w.extend_from_slice(&v.to_le_bytes());
    }
    Ok(w)
}

/// An app's files from its installed bundle: only sound files are kept (`.wav` and `.mp3`, at most 2 MB each).
pub fn files_of_bundle(app: &str, bundle: &super::tile::Bundle) -> AppFile {
    let mut kept: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (name, data) in &bundle.files {
        if let super::tile::FileData::Bytes(b) = data {
            let lower = name.to_ascii_lowercase();
            if (lower.ends_with(".wav") || lower.ends_with(".mp3")) && b.len() <= 2 * 1024 * 1024 {
                kept.insert(name.clone(), b.clone());
            }
        }
    }
    let app = app.to_string();
    Arc::new(move |who, file| if who == app { kept.get(file).cloned() } else { None })
}

#[cfg(test)]
#[path = "sdkv2_playback_tests.rs"]
mod tests;
