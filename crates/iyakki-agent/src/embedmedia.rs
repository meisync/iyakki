//! The media player an app embeds, on the device (docs/development/UI-SDK.md, "Embedding audio and video"): `iyakki_core_api::Media` over
//! the agent's own decoders and the core's mixer. A session is one person's player in one app. The file or stream is read and decoded
//! here, in the core; the app only ever gets events (state, position, level). The sound goes into the mixer as a `Media` source, so the
//! person's volume, the ducking under speech and alarms and the sound doctor apply to it like to the radio.
//!
//! Decoders today: WAV (PCM, 8, 16 and 24 bit) and MP3 in this process (the radio's `nanomp3`), Ogg Vorbis, Opus and FLAC through the helper
//! `iyakki-audiodec` (own code, a separate program, `aacpipe`; the user-downloaded add-on is the fallback). Video is not decoded here yet (a video file is
//! refused with a sentence); see the row F-SDK-141.

use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_core_api::media::{MediaFrame, SWITCHED_OFF, check_rate, check_spec, check_volume, media_kind_of_name, Media, MediaEntry, MediaEvent, MediaInfo, MediaSession, MediaSource, MediaSpec, MediaState, TrackInfo, TrackKind, MAX_MEDIA_FILE_BYTES, MAX_MEDIA_SESSIONS};
use iyakki_core_api::AuditSink;

use crate::audiofmt::{AacDecoder, Codec, Pcm};
use crate::mixer::{Mixer, SourceKind, SourceWriter};

/// What the player needs from the machine.
pub trait MediaHost: Send + Sync {
    /// The root of the person's own home folder.
    fn home_root(&self, user: &str) -> PathBuf;
    /// The mixer; `None` while it is off.
    fn mixer(&self) -> Option<&Mixer>;
    /// The address of a station of the person's list.
    fn station_url(&self, user: &str, name: &str) -> Option<String>;
    /// Open an http(s) address through the core's network guard (the same guard as the radio and the web); the error is a sentence.
    fn open_remote(&self, user: &str, url: &str) -> Result<Box<dyn Read + Send>, String>;
    /// The decoder program for Ogg Vorbis (the helper, or the add-on).
    fn ogg_decoder(&self) -> Result<Box<dyn AacDecoder>, String> {
        crate::aacpipe::ogg_decoder()
    }
    /// Whether this person lets apps play sound in their own windows (on until the person says otherwise).
    fn enabled(&self, _user: &str) -> bool {
        true
    }
    /// Keep the person's switch.
    fn set_enabled(&self, _user: &str, _on: bool) -> Result<(), String> {
        Ok(())
    }
    /// The decoder program for FLAC (the helper, or the add-on).
    fn flac_decoder(&self) -> Result<Box<dyn AacDecoder>, String> {
        crate::aacpipe::ogg_decoder()
    }
    /// The decoder program for Opus (the helper, or the add-on).
    fn opus_decoder(&self) -> Result<Box<dyn AacDecoder>, String> {
        crate::aacpipe::opus_decoder()
    }
}

pub const NO_VIDEO: &str = "Video files in this format cannot be played yet: Motion-JPEG AVI files can (H.264, VP8, VP9 and AV1 need a decoder that is not here yet).";
const MAX_SESSIONS_TOTAL: usize = 32;
const POSITION_EVERY: Duration = Duration::from_millis(250);
const CHUNK: usize = 16 * 1024;

// ---------------------------------------------------------------------------------------------------------------- decoders

enum Pull {
    /// Samples were appended.
    Data,
    /// Nothing yet: feed more bytes.
    Pending,
    /// Everything fed has come out.
    End,
}

trait Dec: Send {
    fn push(&mut self, bytes: &[u8]) -> Result<(), String>;
    fn finish(&mut self);
    fn pull(&mut self, out: &mut Vec<i16>) -> Result<Pull, String>;
    /// Rate and channels, known after the first samples.
    fn format(&self) -> Option<(u32, usize)>;
}

#[derive(Default)]
struct WavDec {
    buf: Vec<u8>,
    fmt: Option<(u32, usize, u16)>,
    data_left: Option<u64>,
    finished: bool,
    header_done: bool,
}

impl WavDec {
    /// Looks for the `fmt ` and `data` chunks at the start of the buffered bytes.
    fn header(&mut self) -> Result<bool, String> {
        let b = &self.buf;
        if b.len() < 12 {
            return Ok(false);
        }
        if &b[..4] != b"RIFF" || &b[8..12] != b"WAVE" {
            return Err("this is not a WAV file".into());
        }
        let mut p = 12;
        loop {
            if b.len() < p + 8 {
                return Ok(false);
            }
            let id = [b[p], b[p + 1], b[p + 2], b[p + 3]];
            let size = u32::from_le_bytes([b[p + 4], b[p + 5], b[p + 6], b[p + 7]]) as usize;
            if &id == b"data" {
                let Some((_, _, _)) = self.fmt else { return Err("this WAV file has no format chunk".into()) };
                self.data_left = Some(size as u64);
                self.buf.drain(..p + 8);
                self.header_done = true;
                return Ok(true);
            }
            if &id == b"fmt " {
                if b.len() < p + 8 + 16 {
                    return Ok(false);
                }
                let f = &b[p + 8..];
                let tag = u16::from_le_bytes([f[0], f[1]]);
                let ch = u16::from_le_bytes([f[2], f[3]]) as usize;
                let rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
                let bits = u16::from_le_bytes([f[14], f[15]]);
                if tag != 1 && tag != 0xFFFE {
                    return Err("this WAV file is not plain PCM (compressed WAV files are not played)".into());
                }
                if !matches!(bits, 8 | 16 | 24) {
                    return Err(format!("this WAV file has {bits} bits per sample; 8, 16 and 24 are played"));
                }
                if !(1..=2).contains(&ch) || !(8_000..=192_000).contains(&rate) {
                    return Err("this WAV file has a sound format the player does not take (1 or 2 channels, 8 to 192 kHz)".into());
                }
                self.fmt = Some((rate, ch, bits));
            }
            p += 8 + size + (size & 1);
            if p > 1 << 20 {
                return Err("this WAV file has no sound data near its start".into());
            }
        }
    }
}

impl Dec for WavDec {
    fn push(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.buf.extend_from_slice(bytes);
        if !self.header_done {
            self.header()?;
        }
        Ok(())
    }
    fn finish(&mut self) {
        self.finished = true;
    }
    fn pull(&mut self, out: &mut Vec<i16>) -> Result<Pull, String> {
        if !self.header_done {
            return if self.finished { Err("this WAV file ends before its sound data".into()) } else { Ok(Pull::Pending) };
        }
        let (_, ch, bits) = self.fmt.unwrap_or((44_100, 2, 16));
        let width = (bits / 8) as usize;
        let frame = width * ch;
        let avail = self.buf.len() / frame * frame;
        let avail = avail.min(self.data_left.map(|d| d as usize / frame * frame).unwrap_or(avail)).min(CHUNK * 2);
        if avail == 0 {
            let over = self.data_left == Some(0) || self.data_left.map(|d| (d as usize) < frame).unwrap_or(false);
            return Ok(if over || self.finished { Pull::End } else { Pull::Pending });
        }
        for s in self.buf[..avail].chunks_exact(width) {
            out.push(match width {
                1 => ((s[0] as i16) - 128) << 8,
                2 => i16::from_le_bytes([s[0], s[1]]),
                _ => i16::from_le_bytes([s[1], s[2]]),
            });
        }
        self.buf.drain(..avail);
        if let Some(d) = self.data_left.as_mut() {
            *d -= avail as u64;
        }
        Ok(Pull::Data)
    }
    fn format(&self) -> Option<(u32, usize)> {
        self.fmt.map(|(r, c, _)| (r, c))
    }
}

struct Mp3Dec {
    dec: nanomp3::Decoder,
    buf: Vec<u8>,
    finished: bool,
    fmt: Option<(u32, usize)>,
    garbage: usize,
}

impl Mp3Dec {
    fn new() -> Mp3Dec {
        Mp3Dec { dec: nanomp3::Decoder::new(), buf: Vec::new(), finished: false, fmt: None, garbage: 0 }
    }
}

impl Dec for Mp3Dec {
    fn push(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.buf.extend_from_slice(bytes);
        Ok(())
    }
    fn finish(&mut self) {
        self.finished = true;
    }
    fn pull(&mut self, out: &mut Vec<i16>) -> Result<Pull, String> {
        let mut pcm = vec![0i16; nanomp3::MAX_SAMPLES_PER_FRAME];
        let mut got = false;
        // a frame is trusted once the next one is seen: wait for a little data unless the stream is over
        while (self.buf.len() >= 4096 || (self.finished && !self.buf.is_empty())) && out.len() < CHUNK {
            let (used, frame) = self.dec.decode(&self.buf, &mut pcm);
            self.buf.drain(..used.max(1).min(self.buf.len()));
            match frame {
                Ok(info) if info.samples_produced > 0 => {
                    self.garbage = 0;
                    let ch = usize::from(info.channels.num());
                    self.fmt = Some((info.sample_rate as u32, ch));
                    out.extend_from_slice(&pcm[..info.samples_produced * ch]);
                    got = true;
                }
                Ok(_) => {}
                Err(_) => {
                    self.garbage += used;
                    if self.garbage > 256 * 1024 {
                        return Err("this does not look like an MP3 file".into());
                    }
                }
            }
        }
        if got {
            Ok(Pull::Data)
        } else if self.finished && self.buf.is_empty() {
            Ok(Pull::End)
        } else {
            Ok(Pull::Pending)
        }
    }
    fn format(&self) -> Option<(u32, usize)> {
        self.fmt
    }
}

struct PipeDec {
    inner: Box<dyn AacDecoder>,
    held: Option<Vec<u8>>,
    fmt: Option<(u32, usize)>,
    finished: bool,
}

impl Dec for PipeDec {
    fn push(&mut self, bytes: &[u8]) -> Result<(), String> {
        // the program takes a bounded amount: wait for room, reading what it has said meanwhile is the caller's next `pull`
        match self.held.as_mut() {
            Some(h) => h.extend_from_slice(bytes),
            None => self.held = Some(bytes.to_vec()),
        }
        self.flush_held()
    }
    fn finish(&mut self) {
        self.finished = true;
    }
    fn pull(&mut self, out: &mut Vec<i16>) -> Result<Pull, String> {
        self.flush_held()?;
        if self.finished && self.held.is_none() {
            self.inner.finish();
        }
        let mut buf = vec![0i16; 8192];
        match self.inner.read(&mut buf, Duration::from_millis(20))? {
            Pcm::Samples(n) => {
                let (r, c) = self.inner.format();
                self.fmt = Some((r, c.max(1)));
                let n = n - n % c.max(1);
                out.extend_from_slice(&buf[..n]);
                Ok(Pull::Data)
            }
            Pcm::Pending => Ok(Pull::Pending),
            Pcm::End => Ok(Pull::End),
        }
    }
    fn format(&self) -> Option<(u32, usize)> {
        self.fmt
    }
}

impl PipeDec {
    fn flush_held(&mut self) -> Result<(), String> {
        if let Some(h) = self.held.take() {
            if !self.inner.feed(&h)? {
                self.held = Some(h);
            }
        }
        Ok(())
    }
}

/// The sound of an AVI file as a decoder: the PCM the container held, or silence as long as the video when it has none (so the
/// same clock drives the pictures).
struct AviSound {
    avi: Arc<crate::avimjpeg::Avi>,
    pos: usize,
}

const SILENCE_RATE: u32 = 8_000;

impl Dec for AviSound {
    fn push(&mut self, _bytes: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn finish(&mut self) {}
    fn pull(&mut self, out: &mut Vec<i16>) -> Result<Pull, String> {
        match &self.avi.audio {
            Some(a) => {
                if self.pos >= a.pcm.len() {
                    return Ok(Pull::End);
                }
                let end = (self.pos + CHUNK * 2).min(a.pcm.len());
                out.extend_from_slice(&a.pcm[self.pos..end]);
                self.pos = end;
            }
            None => {
                let total = (self.avi.duration_ms * u64::from(SILENCE_RATE) / 1000) as usize;
                if self.pos >= total {
                    return Ok(Pull::End);
                }
                let end = (self.pos + CHUNK * 2).min(total);
                out.extend(std::iter::repeat(0).take(end - self.pos));
                self.pos = end;
            }
        }
        Ok(Pull::Data)
    }
    fn format(&self) -> Option<(u32, usize)> {
        Some(self.avi.audio.as_ref().map_or((SILENCE_RATE, 1), |a| (a.rate, a.channels)))
    }
}

enum Fmt {
    /// Motion-JPEG in an AVI file: the sound is decoded from the container (`avimjpeg`), the pictures are asked for by `frame`.
    Avi(Arc<crate::avimjpeg::Avi>),
    Wav,
    Mp3,
    Ogg,
    Opus,
    Flac,
}

fn sniff(head: &[u8]) -> Result<(Fmt, &'static str), String> {
    if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WAVE" {
        return Ok((Fmt::Wav, "WAV"));
    }
    if head.starts_with(b"fLaC") {
        return Ok((Fmt::Flac, "FLAC"));
    }
    match crate::audiofmt::sniff(head) {
        Some(Codec::Mp3) => Ok((Fmt::Mp3, "MP3")),
        Some(Codec::Ogg) => Ok((Fmt::Ogg, "Ogg Vorbis")),
        Some(Codec::Opus) => Ok((Fmt::Opus, "Opus")),
        Some(Codec::Aac) => Err("AAC streams cannot be played here yet: they need the audio add-on, which the radio uses but this player does not.".into()),
        None => Err("this is not a sound the player knows (WAV, MP3, FLAC, Ogg Vorbis and Opus can be played)".into()),
    }
}

fn make_dec(host: &dyn MediaHost, f: &Fmt) -> Result<Box<dyn Dec>, String> {
    Ok(match f {
        Fmt::Avi(a) => Box::new(AviSound { avi: a.clone(), pos: 0 }),
        Fmt::Wav => Box::new(WavDec::default()),
        Fmt::Mp3 => Box::new(Mp3Dec::new()),
        Fmt::Ogg => Box::new(PipeDec { inner: host.ogg_decoder()?, held: None, fmt: None, finished: false }),
        Fmt::Flac => Box::new(PipeDec { inner: host.flac_decoder()?, held: None, fmt: None, finished: false }),
        Fmt::Opus => Box::new(PipeDec { inner: host.opus_decoder()?, held: None, fmt: None, finished: false }),
    })
}

// ---------------------------------------------------------------------------------------------------------------- the bytes

enum Feed {
    Local { data: Arc<Vec<u8>>, pos: usize },
    Remote { rx: Receiver<Result<Vec<u8>, String>>, eof: bool },
}

enum Fed {
    Bytes(Vec<u8>),
    Pending,
    Eof,
    Failed(String),
}

impl Feed {
    fn next(&mut self, wait: Duration) -> Fed {
        match self {
            Feed::Local { data, pos } => {
                if *pos >= data.len() {
                    return Fed::Eof;
                }
                let end = (*pos + CHUNK).min(data.len());
                let b = data[*pos..end].to_vec();
                *pos = end;
                Fed::Bytes(b)
            }
            Feed::Remote { rx, eof } => {
                if *eof {
                    return Fed::Eof;
                }
                match rx.recv_timeout(wait) {
                    Ok(Ok(b)) => Fed::Bytes(b),
                    Ok(Err(e)) => Fed::Failed(e),
                    Err(RecvTimeoutError::Timeout) => Fed::Pending,
                    Err(RecvTimeoutError::Disconnected) => {
                        *eof = true;
                        Fed::Eof
                    }
                }
            }
        }
    }
    fn restart(&mut self) -> bool {
        if let Feed::Local { pos, .. } = self {
            *pos = 0;
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------- the session

struct Ctrl {
    want_play: AtomicBool,
    stop: AtomicBool,
    seek: Mutex<Option<u64>>,
    rate: AtomicU32,
    volume: AtomicU32,
    muted: AtomicBool,
    approved: AtomicBool,
}

struct Shared {
    ctrl: Ctrl,
    events: Mutex<VecDeque<MediaEvent>>,
    state: Mutex<MediaState>,
    info: Mutex<Option<MediaInfo>>,
    seekable: AtomicBool,
    /// The pictures of a Motion-JPEG video and the pacer that picks the one that is due.
    video: Mutex<Option<Video>>,
    /// Where the sound is now (what the mixer says was heard), for the pacer.
    pos_ms: std::sync::atomic::AtomicU64,
    /// How many times the player's thread has refreshed `pos_ms` (tests wait for it to move before they look at a picture).
    turns: std::sync::atomic::AtomicU64,
}

struct Video {
    avi: Arc<crate::avimjpeg::Avi>,
    data: Arc<Vec<u8>>,
    pacer: crate::avimjpeg::Pacer,
    cache: Option<(usize, MediaFrame)>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Shared {
    fn emit(&self, e: MediaEvent) {
        let mut q = lock(&self.events);
        // a slow app does not make the queue grow: older position and level events give way
        if q.len() >= 256 {
            if let Some(i) = q.iter().position(|e| matches!(e, MediaEvent::Position { .. } | MediaEvent::Level { .. })) {
                q.remove(i);
            }
        }
        q.push_back(e);
    }
    fn set_state(&self, s: MediaState) {
        let mut g = lock(&self.state);
        if *g != s {
            *g = s;
            drop(g);
            self.emit(MediaEvent::State { state: s });
        }
    }
    fn state(&self) -> MediaState {
        *lock(&self.state)
    }
    fn fail(&self, why: &str) {
        self.emit(MediaEvent::Error { text: why.to_string() });
        self.set_state(MediaState::Failed);
    }
}

struct Sess {
    user: String,
    app: String,
    shared: Arc<Shared>,
    /// What the thread needs to start once the person has allowed a remote source.
    pending: Mutex<Option<Start>>,
}

struct Start {
    source: Source,
    title: String,
}

enum Source {
    Local { path: PathBuf, data: Arc<Vec<u8>> },
    Remote { url: String },
}

pub struct AgentMedia {
    host: Arc<dyn MediaHost>,
    audit: Arc<dyn AuditSink>,
    sessions: Mutex<BTreeMap<String, Arc<Sess>>>,
    next: std::sync::atomic::AtomicU64,
}

impl AgentMedia {
    /// Tests: how many times the session's thread has refreshed where the sound is.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn turns(&self, user: &str, session: &str) -> u64 {
        self.mine(user, session).map_or(0, |s| s.shared.turns.load(Ordering::SeqCst))
    }

    pub fn new(host: Arc<dyn MediaHost>, audit: Arc<dyn AuditSink>) -> AgentMedia {
        AgentMedia { host, audit, sessions: Mutex::new(BTreeMap::new()), next: std::sync::atomic::AtomicU64::new(1) }
    }

    /// Stops every session (tests and shutdown).
    pub fn close_all(&self) {
        for s in lock(&self.sessions).values() {
            s.shared.ctrl.stop.store(true, Ordering::SeqCst);
        }
    }

    fn mine(&self, user: &str, id: &str) -> Result<Arc<Sess>, String> {
        match lock(&self.sessions).get(id) {
            Some(s) if s.user == user => Ok(s.clone()),
            _ => Err("that media session is not open".into()),
        }
    }

    fn launch(&self, s: &Arc<Sess>) {
        let Some(start) = lock(&s.pending).take() else { return };
        let host = self.host.clone();
        let (shared, user) = (s.shared.clone(), s.user.clone());
        let app = s.app.clone();
        std::thread::spawn(move || run(host, shared, user, app, start));
    }
}

impl Drop for AgentMedia {
    /// The core going away stops every sound it was playing.
    fn drop(&mut self) {
        for s in lock(&self.sessions).values() {
            s.shared.ctrl.stop.store(true, Ordering::SeqCst);
        }
    }
}

impl Media for AgentMedia {
    fn enabled(&self, user: &str) -> bool {
        self.host.enabled(user)
    }

    fn set_enabled(&self, user: &str, on: bool) -> Result<(), String> {
        self.host.set_enabled(user, on)?;
        if !on {
            let mine: Vec<String> = lock(&self.sessions).iter().filter(|(_, s)| s.user == user).map(|(k, _)| k.clone()).collect();
            for id in mine {
                self.close(user, &id);
            }
        }
        self.audit.record(user, "media.switch", if on { "on" } else { "off" });
        Ok(())
    }

    fn open_sessions(&self, user: &str) -> usize {
        lock(&self.sessions).values().filter(|s| s.user == user).count()
    }

    fn list_home(&self, user: &str) -> Vec<MediaEntry> {
        iyakki_core_local::homefiles::list_media(&self.host.home_root(user)).into_iter().map(|f| MediaEntry { name: f.name, size: f.size, video: f.video }).collect()
    }

    fn open(&self, user: &str, app: &str, spec: &MediaSpec) -> Result<MediaSession, String> {
        check_spec(app, spec)?;
        if !self.host.enabled(user) {
            return Err(SWITCHED_OFF.into());
        }
        if self.host.mixer().is_none() {
            return Err("the sound mixer is off (Settings -> Sound), so nothing can be played".into());
        }
        let source = match &spec.source {
            MediaSource::Home { name } => {
                if media_kind_of_name(name) == Some(true) && !name.to_ascii_lowercase().ends_with(".avi") {
                    return Err(NO_VIDEO.into());
                }
                let path = iyakki_core_local::homefiles::resolve_media(&self.host.home_root(user), name, MAX_MEDIA_FILE_BYTES)?;
                let data = std::fs::read(&path).map_err(|_| "that file cannot be read".to_string())?;
                Source::Local { path, data: Arc::new(data) }
            }
            MediaSource::Stream { url } => Source::Remote { url: url.clone() },
            MediaSource::Station { name } => Source::Remote { url: self.host.station_url(user, name).ok_or("that station is not in your list")? },
        };
        // one session per person and app; the earlier one stops
        let old: Vec<(String, Arc<Sess>)> = lock(&self.sessions).iter().filter(|(_, s)| s.user == user && s.app == app).map(|(k, s)| (k.clone(), s.clone())).collect();
        for (k, s) in old {
            s.shared.ctrl.stop.store(true, Ordering::SeqCst);
            lock(&self.sessions).remove(&k);
        }
        {
            let g = lock(&self.sessions);
            if g.values().filter(|s| s.user == user).count() >= MAX_MEDIA_SESSIONS {
                return Err(format!("at most {MAX_MEDIA_SESSIONS} apps may play at the same time"));
            }
            if g.len() >= MAX_SESSIONS_TOTAL {
                return Err("too many players are open on this machine".into());
            }
        }
        let remote = spec.source.is_remote();
        let shared = Arc::new(Shared {
            ctrl: Ctrl { want_play: AtomicBool::new(spec.autoplay), stop: AtomicBool::new(false), seek: Mutex::new(None), rate: AtomicU32::new(1000), volume: AtomicU32::new(spec.volume.unwrap_or(100)), muted: AtomicBool::new(false), approved: AtomicBool::new(!remote) },
            events: Mutex::new(VecDeque::new()),
            state: Mutex::new(MediaState::Opening),
            info: Mutex::new(None),
            seekable: AtomicBool::new(false),
            video: Mutex::new(None),
            pos_ms: std::sync::atomic::AtomicU64::new(0),
            turns: std::sync::atomic::AtomicU64::new(0),
        });
        let id = format!("media-{}", self.next.fetch_add(1, Ordering::SeqCst));
        let sess = Arc::new(Sess { user: user.into(), app: app.into(), shared: shared.clone(), pending: Mutex::new(Some(Start { source, title: spec.title.clone() })) });
        self.audit.record(user, "media.open", &format!("{app} {}", spec.source.audit_kind()));
        lock(&self.sessions).insert(id.clone(), sess.clone());
        if remote {
            let summary = match &spec.source {
                MediaSource::Stream { url } => format!("Play {url} from the internet"),
                MediaSource::Station { name } => format!("Play the radio station {name}"),
                MediaSource::Home { .. } => String::new(),
            };
            shared.set_state(MediaState::AwaitingApproval);
            shared.emit(MediaEvent::Approval { summary });
        } else {
            shared.set_state(MediaState::Opening);
            self.launch(&sess);
        }
        Ok(MediaSession { id, needs_approval: remote })
    }

    fn decide(&self, user: &str, session: &str, allow: bool) -> Result<(), String> {
        let s = self.mine(user, session)?;
        if s.shared.state() != MediaState::AwaitingApproval {
            return Err("nothing is waiting for your answer".into());
        }
        self.audit.record(user, "media.decide", &format!("{} {}", s.app, if allow { "approved" } else { "denied" }));
        if allow {
            s.shared.ctrl.approved.store(true, Ordering::SeqCst);
            s.shared.set_state(MediaState::Opening);
            self.launch(&s);
        } else {
            lock(&s.pending).take();
            s.shared.fail("you did not allow that source");
        }
        Ok(())
    }

    fn play(&self, user: &str, session: &str) -> Result<(), String> {
        let s = self.mine(user, session)?;
        match s.shared.state() {
            MediaState::AwaitingApproval => return Err("allow the source first".into()),
            MediaState::Failed => return Err("this cannot be played now".into()),
            MediaState::Ended => *lock(&s.shared.ctrl.seek) = Some(0),
            _ => {}
        }
        s.shared.ctrl.want_play.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn pause(&self, user: &str, session: &str) -> Result<(), String> {
        let s = self.mine(user, session)?;
        s.shared.ctrl.want_play.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn seek(&self, user: &str, session: &str, ms: u64) -> Result<(), String> {
        let s = self.mine(user, session)?;
        if matches!(s.shared.state(), MediaState::AwaitingApproval | MediaState::Failed) {
            return Err("this cannot be moved now".into());
        }
        if !s.shared.seekable.load(Ordering::SeqCst) {
            return Err("a live stream cannot be moved".into());
        }
        *lock(&s.shared.ctrl.seek) = Some(ms);
        Ok(())
    }

    fn set_rate(&self, user: &str, session: &str, permille: u32) -> Result<(), String> {
        check_rate(permille)?;
        self.mine(user, session)?.shared.ctrl.rate.store(permille, Ordering::SeqCst);
        Ok(())
    }

    fn set_volume(&self, user: &str, session: &str, percent: u32) -> Result<(), String> {
        check_volume(percent)?;
        self.mine(user, session)?.shared.ctrl.volume.store(percent, Ordering::SeqCst);
        Ok(())
    }

    fn set_mute(&self, user: &str, session: &str, mute: bool) -> Result<(), String> {
        self.mine(user, session)?.shared.ctrl.muted.store(mute, Ordering::SeqCst);
        Ok(())
    }

    fn select_track(&self, user: &str, session: &str, track: u32) -> Result<(), String> {
        let s = self.mine(user, session)?;
        let mut g = lock(&s.shared.info);
        let Some(info) = g.as_mut() else { return Err("nothing is open yet".into()) };
        let Some(kind) = info.tracks.iter().find(|t| t.id == track).map(|t| t.kind) else { return Err("that track does not exist".into()) };
        for t in info.tracks.iter_mut().filter(|t| t.kind == kind) {
            t.selected = t.id == track;
        }
        Ok(())
    }

    fn poll(&self, user: &str, session: &str) -> Vec<MediaEvent> {
        match self.mine(user, session) {
            Ok(s) => lock(&s.shared.events).drain(..).collect(),
            Err(_) => Vec::new(),
        }
    }

    fn frame(&self, user: &str, session: &str, after: u64) -> Result<Option<MediaFrame>, String> {
        let s = self.mine(user, session)?;
        let mut g = lock(&s.shared.video);
        let Some(v) = g.as_mut() else { return Ok(None) };
        let pos = s.shared.pos_ms.load(Ordering::SeqCst);
        let due = v.pacer.due_at(pos);
        let seq = due as u64 + 1;
        if seq <= after {
            return Ok(None);
        }
        if let Some((i, f)) = &v.cache {
            if *i == due {
                return Ok(Some(f.clone()));
            }
        }
        // the pacer counts the pictures that were late; the one that is due now is the only one decoded
        v.pacer.next(pos);
        let fr = v.avi.frames[due];
        let jpeg = v.data.get(fr.off..fr.off + fr.len).ok_or("a picture of the video is cut off")?;
        let (w, h, rgba) = crate::avimjpeg::to_rgba(jpeg)?;
        let f = MediaFrame { seq, pts_ms: fr.pts_ms, width: w, height: h, rgba: Arc::new(rgba) };
        v.cache = Some((due, f.clone()));
        Ok(Some(f))
    }

    fn close(&self, user: &str, session: &str) {
        if let Ok(s) = self.mine(user, session) {
            s.shared.ctrl.stop.store(true, Ordering::SeqCst);
            lock(&self.sessions).remove(session);
            self.audit.record(user, "media.close", &s.app);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------- the player thread

fn peak_of(pcm: &[i16]) -> u32 {
    (pcm.iter().map(|s| (*s as i32).unsigned_abs()).max().unwrap_or(0) * 1000 / 32768).min(1000)
}

fn run(host: Arc<dyn MediaHost>, sh: Arc<Shared>, user: String, app: String, start: Start) {
    if let Err(why) = play(&*host, &sh, &user, &app, start) {
        sh.fail(&why);
    }
}

fn play(host: &dyn MediaHost, sh: &Arc<Shared>, user: &str, app: &str, start: Start) -> Result<(), String> {
    let mut local_path = None;
    let mut local_data: Option<Arc<Vec<u8>>> = None;
    let mut feed = match start.source {
        Source::Local { path, data } => {
            local_path = Some(path);
            local_data = Some(data.clone());
            Feed::Local { data, pos: 0 }
        }
        Source::Remote { url } => {
            let mut r = host.open_remote(user, &url)?;
            let (tx, rx) = sync_channel::<Result<Vec<u8>, String>>(16);
            let stop = Arc::clone(sh);
            std::thread::spawn(move || {
                let mut b = vec![0u8; CHUNK];
                loop {
                    if stop.ctrl.stop.load(Ordering::SeqCst) {
                        return;
                    }
                    match r.read(&mut b) {
                        Ok(0) => return,
                        Ok(n) => {
                            if tx.send(Ok(b[..n].to_vec())).is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(format!("the stream stopped: {e}")));
                            return;
                        }
                    }
                }
            });
            Feed::Remote { rx, eof: false }
        }
    };
    // the first bytes say what it is
    let first = {
        let t0 = Instant::now();
        loop {
            if sh.ctrl.stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            match feed.next(Duration::from_millis(100)) {
                Fed::Bytes(b) => break b,
                Fed::Pending if t0.elapsed() < Duration::from_secs(15) => {}
                Fed::Pending => return Err("no sound arrived for 15 seconds".into()),
                Fed::Eof => return Err("the file or stream is empty".into()),
                Fed::Failed(e) => return Err(e),
            }
        }
    };
    let (fmt, codec) = if crate::avimjpeg::is_avi(&first) {
        let Some(data) = local_data.clone() else { return Err("video from the internet cannot be played yet: only video files of your home folder can".into()) };
        let avi = Arc::new(crate::avimjpeg::parse(&data)?);
        *lock(&sh.video) = Some(Video { avi: avi.clone(), data, pacer: crate::avimjpeg::Pacer::new(avi.frames.iter().map(|f| f.pts_ms).collect()), cache: None });
        (Fmt::Avi(avi), "Motion-JPEG (AVI)")
    } else {
        sniff(&first)?
    };
    let mut dec = make_dec(host, &fmt)?;
    dec.push(&first)?;
    // decode until the format is known
    let mut ready: Vec<i16> = Vec::new();
    let t0 = Instant::now();
    let mut eof = false;
    loop {
        if sh.ctrl.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        match dec.pull(&mut ready)? {
            Pull::Data => {}
            Pull::End => return Err("that file has no sound in it".into()),
            Pull::Pending => match feed.next(Duration::from_millis(20)) {
                Fed::Bytes(b) => dec.push(&b)?,
                Fed::Pending => {}
                Fed::Eof => {
                    if !eof {
                        eof = true;
                        dec.finish();
                    }
                }
                Fed::Failed(e) => return Err(e),
            },
        }
        if dec.format().is_some() {
            break;
        }
        if t0.elapsed() > Duration::from_secs(20) {
            return Err("the sound did not start in 20 seconds".into());
        }
    }
    let (rate, channels) = dec.format().unwrap_or((44_100, 2));
    let local = local_path.is_some();
    let tag = local_path.as_ref().and_then(|p| crate::media::describe_path(&p.to_string_lossy()));
    let avi_duration = lock(&sh.video).as_ref().map(|v| v.avi.duration_ms);
    let duration_ms = avi_duration.or_else(|| tag.as_ref().and_then(|t| t.seconds).map(|s| u64::from(s) * 1000)).filter(|_| local);
    let title = tag.as_ref().map(|t| t.title.clone()).filter(|t| !t.is_empty()).or_else(|| local_path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned())).or_else(|| Some(start.title.clone()).filter(|t| !t.is_empty())).unwrap_or_else(|| "Sound".into());
    let mut tracks = vec![TrackInfo { id: 0, kind: TrackKind::Audio, label: "Audio".into(), lang: String::new(), selected: true }];
    if avi_duration.is_some() {
        tracks.push(TrackInfo { id: 1, kind: TrackKind::Video, label: "Video".into(), lang: String::new(), selected: true });
    }
    let info = MediaInfo { title, duration_ms, seekable: local, has_audio: true, has_video: avi_duration.is_some(), sample_rate: rate, channels: channels as u32, codec: codec.into(), tracks };
    sh.seekable.store(local, Ordering::SeqCst);
    *lock(&sh.info) = Some(info.clone());
    sh.emit(MediaEvent::Opened { info });
    sh.set_state(MediaState::Ready);

    let open_writer = |rate_permille: u32| -> Result<SourceWriter, String> {
        let m = host.mixer().ok_or("the sound mixer is off (Settings -> Sound)")?;
        let eff = (u64::from(rate) * u64::from(rate_permille) / 1000).clamp(8_000, 192_000) as u32;
        let w = m.open_source_with(SourceKind::Media, eff, channels, &format!("Media ({app})"), false).map_err(|e| e.to_string())?;
        // the person whose player this is: a client that hears only its own person's sound gets it (F-CLIENT-03)
        w.set_owner(user);
        // the session's volume is set before the first sample, so a muted player never leaks a moment of sound while the level ramps
        w.set_gain(if sh.ctrl.muted.load(Ordering::SeqCst) { 0.0 } else { sh.ctrl.volume.load(Ordering::SeqCst) as f32 / 100.0 });
        Ok(w)
    };
    let mut cur_rate = sh.ctrl.rate.load(Ordering::SeqCst);
    let mut writer = open_writer(cur_rate)?;
    writer.set_paused(true);
    let mut base_ms: u64 = 0;
    let mut skip_frames: u64 = 0;
    let mut chunk: Vec<i16> = ready;
    let mut off = 0usize;
    let mut last_pos = Instant::now() - POSITION_EVERY;
    let mut ended = false;
    let mut draining = false;
    let mut starved_since: Option<Instant> = None;
    loop {
        if sh.ctrl.stop.load(Ordering::SeqCst) {
            writer.flush();
            return Ok(());
        }
        let g = if sh.ctrl.muted.load(Ordering::SeqCst) { 0.0f32 } else { sh.ctrl.volume.load(Ordering::SeqCst) as f32 / 100.0 };
        writer.set_gain(g);
        // a seek or a change of speed starts the decode again from the right place (the speed is the rate of the mixer source: the pitch follows)
        let seek = lock(&sh.ctrl.seek).take();
        let rate_now = sh.ctrl.rate.load(Ordering::SeqCst);
        if seek.is_some() || rate_now != cur_rate {
            let pos = pos_ms(base_ms, &writer, cur_rate, duration_ms);
            let target = seek.unwrap_or(pos).min(duration_ms.unwrap_or(u64::MAX));
            let moved = if local {
                if !feed.restart() {
                    return Err("this stream cannot be moved".into());
                }
                dec = make_dec(host, &fmt)?;
                chunk.clear();
                off = 0;
                skip_frames = target * u64::from(rate) / 1000;
                ended = false;
                draining = false;
                true
            } else {
                false
            };
            writer.flush();
            drop(writer);
            cur_rate = rate_now;
            writer = open_writer(cur_rate)?;
            writer.set_paused(true);
            base_ms = if moved { target } else { pos };
            eof = false;
            if let Some(v) = lock(&sh.video).as_mut() {
                v.pacer.reset();
                v.cache = None;
            }
            sh.emit(MediaEvent::Position { ms: base_ms });
        }
        sh.turns.fetch_add(1, Ordering::SeqCst);
        sh.pos_ms.store(if ended { duration_ms.unwrap_or(base_ms) } else { pos_ms(base_ms, &writer, cur_rate, duration_ms) }, Ordering::SeqCst);
        let want = sh.ctrl.want_play.load(Ordering::SeqCst);
        if ended {
            writer.set_paused(true);
            sh.set_state(MediaState::Ended);
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        if !want {
            writer.set_paused(true);
            if sh.state() != MediaState::Ready {
                sh.set_state(MediaState::Paused);
            }
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        writer.set_paused(false);
        if sh.state() != MediaState::Buffering || starved_since.is_none() {
            sh.set_state(MediaState::Playing);
        }
        if last_pos.elapsed() >= POSITION_EVERY {
            last_pos = Instant::now();
            sh.emit(MediaEvent::Position { ms: pos_ms(base_ms, &writer, cur_rate, duration_ms) });
        }
        if off < chunk.len() {
            let n = writer.write_i16(&chunk[off..]);
            if n == 0 && writer.is_done() {
                return Err("another sound took the speaker".into());
            }
            if n > 0 {
                sh.emit(MediaEvent::Level { peak: peak_of(&chunk[off..off + n]) });
            }
            off += n;
            if off >= chunk.len() {
                chunk.clear();
                off = 0;
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
            continue;
        }
        if draining {
            if writer.is_done() {
                draining = false;
                if !local {
                    return Err("the station stopped sending".into());
                }
                if let Some(d) = duration_ms {
                    sh.emit(MediaEvent::Position { ms: d });
                }
                ended = true;
                // the finished source is gone: a later play opens a new one
                writer = open_writer(cur_rate)?;
                base_ms = duration_ms.unwrap_or(base_ms);
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
            continue;
        }
        // more sound: decode, feeding bytes when the decoder wants them
        let mut got = Vec::new();
        match dec.pull(&mut got)? {
            Pull::Data => {
                starved_since = None;
                if skip_frames > 0 {
                    let drop_n = ((skip_frames as usize) * channels).min(got.len());
                    got.drain(..drop_n);
                    skip_frames -= (drop_n / channels) as u64;
                }
                chunk = got;
            }
            Pull::End => {
                // what is queued plays out (pause and seek still work meanwhile), then the player is at its end
                writer.finish();
                draining = true;
            }
            Pull::Pending => match feed.next(Duration::from_millis(20)) {
                Fed::Bytes(b) => {
                    starved_since = None;
                    dec.push(&b)?
                }
                Fed::Pending => {
                    let s = *starved_since.get_or_insert_with(Instant::now);
                    if s.elapsed() > Duration::from_millis(500) {
                        sh.set_state(MediaState::Buffering);
                    }
                    if s.elapsed() > Duration::from_secs(15) {
                        return Err("no sound arrived for 15 seconds".into());
                    }
                }
                Fed::Eof => {
                    if !eof {
                        eof = true;
                        dec.finish();
                    } else {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                Fed::Failed(e) => return Err(e),
            },
        }
    }
}

/// Where the sound is: the place the source started from plus what has been heard, at the speed it plays at.
fn pos_ms(base_ms: u64, w: &SourceWriter, rate_permille: u32, duration_ms: Option<u64>) -> u64 {
    let p = base_ms + w.played_ms() * u64::from(rate_permille) / 1000;
    duration_ms.map_or(p, |d| p.min(d))
}

/// Opens an http(s) address for a person through the core's network guard: the account's host policy first (`allows_host`, the same list
/// the radio and the agent use), then the streaming transport with its connect and first-answer limits, and only a 200 answer counts. The
/// error is a sentence for the person. The body is read by the player's own thread, so a slow server never blocks the core.
pub fn open_guarded(streamer: &dyn iyakki_proxy::Streamer, allow: &dyn Fn(&str) -> bool, url: &str) -> Result<Box<dyn Read + Send>, String> {
    iyakki_core_api::media::check_url(url)?;
    let host = crate::radio::host_of(url);
    if !allow(&host) {
        return Err(format!("your account may not reach {host}"));
    }
    let head = streamer.open(url, &[("Accept".to_string(), "*/*".to_string())]).map_err(|e| format!("could not connect: {e}"))?;
    if head.status != 200 {
        return Err(format!("the server answered {} instead of sound", head.status));
    }
    Ok(head.body)
}

/// The device's host: the person's home folder, the global mixer, the person's radio stations and the network through the account's
/// host policy (`policy_of`, the same one the embedded assistant uses).
pub struct ServicesMediaHost {
    pub services: Arc<crate::services::Services>,
    pub policy_of: Arc<dyn Fn(&str) -> Option<iyakki_sec::policy::Policy> + Send + Sync>,
}

impl ServicesMediaHost {
    /// The person's switch is a marker file in the core's data folder: present means off.
    fn off_marker(&self, user: &str) -> PathBuf {
        let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
        self.services.dir().join(format!("embedded-media-off-{who}"))
    }
}

impl MediaHost for ServicesMediaHost {
    fn home_root(&self, user: &str) -> PathBuf {
        crate::scope::home_root_for_writes(user, self.services.dir())
    }
    fn mixer(&self) -> Option<&Mixer> {
        crate::audiosink::mixer_if_on()
    }
    fn station_url(&self, user: &str, name: &str) -> Option<String> {
        crate::radio::StationStore::new(self.services.dir(), user).list().into_iter().find(|s| s.name == name).map(|s| s.url)
    }
    fn enabled(&self, user: &str) -> bool {
        !self.off_marker(user).exists()
    }
    fn set_enabled(&self, user: &str, on: bool) -> Result<(), String> {
        let m = self.off_marker(user);
        if on { std::fs::remove_file(&m).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }) } else { std::fs::write(&m, b"off") }.map_err(|_| "the switch could not be saved".to_string())
    }
    fn open_remote(&self, user: &str, url: &str) -> Result<Box<dyn Read + Send>, String> {
        let Some(policy) = (self.policy_of)(user) else { return Err("there is no account to play this for".into()) };
        open_guarded(&iyakki_proxy::https::UreqStreamer::default(), &|h| policy.allows_host(h), url)
    }
}
