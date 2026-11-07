//! Internet radio: stations a person adds (a name and a stream address), and a player that keeps playing in the
//! background while they use other apps. Streams are MP3 over HTTP or HTTPS (Icecast and Shoutcast), decoded in-house, and
//! the same MP3 inside HLS playlists (`hls.rs`: playlist follower and transport stream reader, also in-house). AAC (ADTS
//! streams, AAC+ and HLS with AAC) is decoded by the audio add-on the person installs on request (`aacpipe.rs`, a separate
//! program); without it the station fails with a plain message the RADIO tile answers with an install button. The
//! song title the station announces is shown. The sound goes to the system's play command (the same one the voice
//! features use), so a person needs no sound server of their own. Every address, also in playlists and when a station
//! redirects, is checked against the account's host policy.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_proxy::certinfo;
use iyakki_proxy::{StreamHead, Streamer, TlsMode, TlsPlan};

use crate::audiofmt::{self, AacDecoder, AacFactory, Codec, Pcm};
use serde::{Deserialize, Serialize};

pub const MAX_STATIONS: usize = 200;
/// The largest issuer certificate fetched from a certificate's own "CA Issuers" address.
const MAX_ISSUER_CERT: usize = 16 * 1024;
/// How many issuer certificates are followed upwards.
const MAX_ISSUER_HOPS: usize = 3;

/// Stations to start from, by language: `(name, stream address)`: MP3, or AAC / HLS ones that need the audio add-on
/// (`radio_stations::needs_addon`). Each answered when this list was made (24 October 2026); stations move and disappear, so a failing one says why and the person can add their own.
pub struct Language {
    pub name: &'static str,
    pub stations: &'static [(&'static str, &'static str)],
}

pub use crate::radio_stations::LANGUAGES;

/// How many bars the level display has.
pub const BANDS: usize = 28;

/// Loudness in `BANDS` bands from low to high pitch (0.0 – 1.0, on a decibel scale), for the animated display. One
/// Goertzel filter per band over a mono mix of the frame: cheap, and enough to move a spectrum.
pub fn band_levels(pcm: &[i16], channels: usize, rate: u32) -> [f32; BANDS] {
    let ch = channels.max(1);
    let n = pcm.len() / ch;
    let mut out = [0.0f32; BANDS];
    if n < 64 || rate == 0 {
        return out;
    }
    let mono: Vec<f32> = (0..n).map(|i| pcm[i * ch..i * ch + ch].iter().map(|s| *s as f32).sum::<f32>() / ch as f32 / 32768.0).collect();
    let nyquist = rate as f32 / 2.0;
    for (b, o) in out.iter_mut().enumerate() {
        // centre frequencies spread evenly on a log scale from 50 Hz to 14 kHz (never past the Nyquist frequency)
        let f = (50.0 * (14000.0f32 / 50.0).powf(b as f32 / (BANDS - 1) as f32)).min(nyquist * 0.95);
        let w = 2.0 * std::f32::consts::PI * f / rate as f32;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for (i, x) in mono.iter().enumerate() {
            // a Hann window keeps a loud low note from leaking into every band
            let win = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos();
            let s0 = x * win + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
        let amp = (power.max(0.0).sqrt() * 2.0) / (n as f32 * 0.5);
        // music has far more energy in the bass; 3 dB per octave of tilt keeps the high bands alive on screen
        let db = 20.0 * (amp + 1e-6).log10() + 3.0 * (f / 200.0).log2();
        *o = ((db + 66.0) / 50.0).clamp(0.0, 1.0);
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Station {
    pub name: String,
    pub url: String,
    /// The certificate the person approved for this station's server (see [`CertPin`]). Old files have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_pin: Option<CertPin>,
}

impl Station {
    pub fn new(name: &str, url: &str) -> Station {
        Station { name: name.to_string(), url: url.to_string(), cert_pin: None }
    }
}

/// A server certificate a person chose to trust for one station: the exact leaf certificate (SHA-256) of one host and port.
/// It is never a general "do not verify": the connection to that host must show exactly this certificate, for that name, within
/// its dates; a renewed certificate asks again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertPin {
    pub host: String,
    pub port: u16,
    /// Upper-case hex pairs with colons.
    pub sha256: String,
}

/// `(host, port)` of an https address (port 443 when none); `None` for anything else.
pub fn https_endpoint(url: &str) -> Option<(String, u16)> {
    url.starts_with("https://").then(|| iyakki_proxy::pintls::split_https(url).ok()).flatten().map(|(h, p, _)| (h, p))
}

/// Why a station's secure connection was stopped, with what the person needs to decide about trusting the certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertProblem {
    pub station: String,
    pub url: String,
    pub host: String,
    pub port: u16,
    /// True when the station was trusted before and now shows a different certificate.
    pub changed: bool,
    pub sha256: String,
    pub subject: String,
    pub issuer: String,
    /// Unix time.
    pub not_after: i64,
    /// Trusting only helps when the certificate is for this host name and still valid.
    pub trustable: bool,
    /// The plain-language sentence for the person (also the radio's failure text).
    pub message: String,
}

impl CertProblem {
    pub fn pin(&self) -> CertPin {
        CertPin { host: self.host.clone(), port: self.port, sha256: self.sha256.clone() }
    }

    pub fn expiry(&self) -> String {
        certinfo::date_text(self.not_after)
    }
}

pub const UNKNOWN_ISSUER_TEXT: &str = "This station's server did not send its full certificate chain (the issuer is missing). Browsers hide this; IYAKKI checks strictly.";

/// `http://` or `https://`, a host, no sign-in details; returns the address tidied.
pub fn valid_url(u: &str) -> Result<String, String> {
    let u = u.trim();
    let (scheme, rest) = u.split_once("://").ok_or("the address starts with http:// or https://")?;
    if scheme != "http" && scheme != "https" {
        return Err("the address starts with http:// or https://".into());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty()).map_or(authority, |x| x.0);
    if host.is_empty() || host.len() > 253 || authority.contains('@') || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '[' | ']' | ':')) {
        return Err("that does not look like a stream address".into());
    }
    if u.len() > 1000 || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the address is too long or has spaces in it".into());
    }
    Ok(u.to_string())
}

pub fn host_of(u: &str) -> String {
    let rest = u.split_once("://").map_or(u, |x| x.1);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(authority, |x| x.0).trim_matches(['[', ']']).to_ascii_lowercase()
}

pub fn valid_name(n: &str) -> Result<String, String> {
    let n: String = n.split_whitespace().collect::<Vec<_>>().join(" ");
    if n.is_empty() || n.chars().count() > 40 || n.chars().any(|c| c.is_control()) {
        return Err("give the station a short name (up to 40 characters)".into());
    }
    Ok(n)
}

/// A person's stations, in a small file of their own.
pub struct StationStore {
    path: PathBuf,
}

impl StationStore {
    pub fn new(dir: &Path, user: &str) -> StationStore {
        let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
        StationStore { path: dir.join(format!("radio-{who}.json")) }
    }

    pub fn list(&self) -> Vec<Station> {
        // a damaged file is moved aside (`.corrupt-<time>`, audit line) before the empty list is used
        iyakki_schema::read_plain("radio stations", &self.path, iyakki_schema::now_secs()).unwrap_or_default()
    }

    fn save(&self, v: &[Station]) -> Result<(), String> {
        iyakki_schema::plain_writable("radio stations", &self.path)?;
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(v).map_err(|e| e.to_string())?).and_then(|_| std::fs::rename(&tmp, &self.path)).map_err(|e| e.to_string())
    }

    pub fn add(&self, name: &str, url: &str) -> Result<Station, String> {
        let mut st = Station { name: valid_name(name)?, url: valid_url(url)?, cert_pin: None };
        let mut v = self.list();
        if let Some(i) = v.iter().position(|s| s.name.eq_ignore_ascii_case(&st.name)) {
            // the approval belongs to the server it was given for: kept when the address still points there, gone otherwise
            st.cert_pin = v[i].cert_pin.clone().filter(|p| https_endpoint(&st.url) == Some((p.host.clone(), p.port)));
            v[i] = st.clone();
        } else {
            if v.len() >= MAX_STATIONS {
                return Err(format!("that is as many stations as fit ({MAX_STATIONS})"));
            }
            v.push(st.clone());
        }
        self.save(&v)?;
        Ok(st)
    }

    /// Keep (`Some`) or forget (`None`) the approved certificate of a station. A station that is not in the list yet (one of the
    /// built-in presets) is added with the pin. The pin must be for the host and port of the station's own address.
    pub fn set_pin(&self, station: &Station, pin: Option<CertPin>) -> Result<Station, String> {
        if let Some(p) = &pin {
            if https_endpoint(&station.url) != Some((p.host.to_ascii_lowercase(), p.port)) {
                return Err("a certificate can only be trusted for the server the station's address points to".into());
            }
            if certinfo::norm_fingerprint(&p.sha256).len() != 64 {
                return Err("that is not a SHA-256 fingerprint".into());
            }
        }
        let mut v = self.list();
        let at = match v.iter().position(|s| s.name.eq_ignore_ascii_case(&station.name)) {
            Some(i) => i,
            None => {
                if v.len() >= MAX_STATIONS {
                    return Err(format!("that is as many stations as fit ({MAX_STATIONS})"));
                }
                v.push(Station { name: valid_name(&station.name)?, url: valid_url(&station.url)?, cert_pin: None });
                v.len() - 1
            }
        };
        if pin.is_some() && https_endpoint(&v[at].url) != https_endpoint(&station.url) {
            return Err("the saved station points elsewhere now".into());
        }
        v[at].cert_pin = pin;
        let out = v[at].clone();
        self.save(&v)?;
        Ok(out)
    }

    /// The person's approval of the certificate in `problem`, for `station`: kept only when the certificate is for the host
    /// name, still valid, and from the very server the station's own address points to.
    pub fn trust(&self, station: &Station, problem: &CertProblem) -> Result<Station, String> {
        if !problem.trustable {
            return Err("this certificate cannot be trusted: it is not for this host name or it is not within its dates".into());
        }
        self.set_pin(station, Some(problem.pin()))
    }

    /// Forget the approved certificate of a station.
    pub fn forget_trust(&self, name: &str) -> Result<Station, String> {
        let st = self.list().into_iter().find(|s| s.name.eq_ignore_ascii_case(name)).ok_or("no such station")?;
        if st.cert_pin.is_none() {
            return Err("no certificate is trusted for this station".into());
        }
        self.set_pin(&st, None)
    }

    pub fn remove(&self, name: &str) -> Result<(), String> {
        let mut v = self.list();
        let before = v.len();
        v.retain(|s| s.name != name);
        if v.len() == before {
            return Err("no such station".into());
        }
        self.save(&v)
    }
}

/// The stream address inside a `.pls` or `.m3u` playlist.
pub fn playlist_target(text: &str) -> Option<String> {
    for line in text.lines().map(str::trim) {
        let candidate = if line.len() > 5 && line[..4].eq_ignore_ascii_case("file") && line.contains('=') { line.split_once('=').map(|x| x.1.trim()) } else if line.starts_with("http") { Some(line) } else { None };
        if let Some(c) = candidate.filter(|c| valid_url(c).is_ok()) {
            return Some(c.to_string());
        }
    }
    None
}

/// Removes the song-title blocks Icecast/Shoutcast stations interleave with the audio (`icy-metaint`), and keeps the latest title.
pub struct IcyReader<R> {
    inner: R,
    interval: usize,
    left: usize,
    title: Arc<Mutex<String>>,
}

impl<R: Read> IcyReader<R> {
    pub fn new(inner: R, interval: usize, title: Arc<Mutex<String>>) -> IcyReader<R> {
        IcyReader { inner, interval, left: interval, title }
    }
}

impl<R: Read> Read for IcyReader<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.interval == 0 {
            return self.inner.read(out);
        }
        if self.left == 0 {
            let mut len = [0u8; 1];
            self.inner.read_exact(&mut len)?;
            let n = len[0] as usize * 16;
            if n > 0 {
                let mut meta = vec![0u8; n];
                self.inner.read_exact(&mut meta)?;
                let text = String::from_utf8_lossy(&meta);
                if let Some(t) = text.split("StreamTitle='").nth(1).and_then(|r| r.split("';").next()) {
                    *self.title.lock().unwrap() = t.trim().chars().take(120).collect();
                }
            }
            self.left = self.interval;
        }
        let want = out.len().min(self.left);
        let n = self.inner.read(&mut out[..want])?;
        self.left -= n;
        Ok(n)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RadioState {
    Idle,
    Connecting(String),
    Playing { station: String, title: String, kbps: u32 },
    Failed { station: String, why: String },
}

struct Inner {
    state: Mutex<RadioState>,
    generation: AtomicU64,
    paused: std::sync::atomic::AtomicBool,
    volume: AtomicU8,
    /// When set, the radio's volume is the master volume of Settings -> Sound (a view of it, not a second attenuator).
    link: Mutex<Option<MasterLink>>,
    streamer: Mutex<Option<Arc<dyn Streamer>>>,
    retry_wait: Mutex<Duration>,
    levels: Mutex<[f32; BANDS]>,
    /// Where AAC decoders come from (`None`: the installed add-on); tests put a fake here.
    aac: Mutex<Option<AacFactory>>,
    /// Divides the waits of the HLS follower (1 in real use).
    hls_scale: std::sync::atomic::AtomicU32,
    /// Set when the last start failed on a server certificate the person could decide about.
    problem: Mutex<Option<CertProblem>>,
}

/// Reads (`None`) or sets (`Some(percent)`) the master volume; returns the volume in percent.
pub type MasterLink = Arc<dyn Fn(Option<u8>) -> u8 + Send + Sync>;

/// One player for the whole system: what is playing goes on while apps come and go.
#[derive(Clone)]
pub struct RadioPlayer {
    inner: Arc<Inner>,
}

type Allow = Arc<dyn Fn(&str) -> bool + Send + Sync>;

impl Default for RadioPlayer {
    fn default() -> Self {
        RadioPlayer { inner: Arc::new(Inner { state: Mutex::new(RadioState::Idle), generation: AtomicU64::new(0), paused: std::sync::atomic::AtomicBool::new(false), volume: AtomicU8::new(80), link: Mutex::new(None), streamer: Mutex::new(None), retry_wait: Mutex::new(Duration::from_secs(2)), levels: Mutex::new([0.0; BANDS]), aac: Mutex::new(None), hls_scale: std::sync::atomic::AtomicU32::new(1), problem: Mutex::new(None) }) }
    }
}

fn resolve(base: &str, loc: &str) -> Option<String> {
    let loc = loc.trim();
    if loc.starts_with("http://") || loc.starts_with("https://") {
        return Some(loc.to_string());
    }
    let (scheme, rest) = base.split_once("://")?;
    let authority = rest.split('/').next()?;
    if let Some(p) = loc.strip_prefix("//") {
        return Some(format!("{scheme}://{p}"));
    }
    loc.starts_with('/').then(|| format!("{scheme}://{authority}{loc}"))
}

impl RadioPlayer {
    pub fn set_streamer(&self, s: Arc<dyn Streamer>) {
        *self.inner.streamer.lock().unwrap() = Some(s);
    }

    /// Where AAC decoders come from instead of the installed add-on (tests).
    pub fn set_aac_factory(&self, f: AacFactory) {
        *self.inner.aac.lock().unwrap() = Some(f);
    }

    /// Shortens the HLS follower's waits by this factor (tests).
    pub fn set_hls_time_scale(&self, n: u32) {
        self.inner.hls_scale.store(n.max(1), Ordering::Relaxed);
    }

    fn aac_decoder(&self) -> Result<Box<dyn AacDecoder>, String> {
        let f = self.inner.aac.lock().unwrap().clone();
        match f {
            Some(f) => f(),
            None => crate::aacpipe::aac_decoder(),
        }
    }

    /// How long to wait before reconnecting a stream that dropped.
    pub fn set_retry_wait(&self, d: Duration) {
        *self.inner.retry_wait.lock().unwrap() = d;
    }

    pub fn state(&self) -> RadioState {
        self.inner.state.lock().unwrap().clone()
    }

    /// The levels of what is playing now, `BANDS` of them (all zero when nothing plays).
    pub fn levels(&self) -> [f32; BANDS] {
        *self.inner.levels.lock().unwrap()
    }

    /// Make the radio's volume the master volume: `f(None)` reads it (percent), `f(Some(v))` sets it (and unmutes).
    pub fn set_master_link(&self, f: MasterLink) {
        *self.inner.link.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    fn link(&self) -> Option<MasterLink> {
        self.inner.link.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn volume(&self) -> u8 {
        if let Some(l) = self.link() {
            return l(None);
        }
        self.inner.volume.load(Ordering::Relaxed)
    }

    pub fn set_volume(&self, v: u8) {
        if let Some(l) = self.link() {
            l(Some(v.min(100)));
            return;
        }
        self.inner.volume.store(v.min(100), Ordering::Relaxed);
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state(), RadioState::Connecting(_) | RadioState::Playing { .. })
    }

    /// Tell [`crate::soundhold`] that this radio holds the speaker while it plays (so the browser's "Stop it" dialog can name it
    /// and stop it cleanly). The latest radio announced is the one the dialog stops.
    pub fn announce_holder(&self) {
        let (a, b) = (self.clone(), self.clone());
        crate::soundhold::announce(
            "radio",
            Arc::new(move || match a.state() {
                RadioState::Playing { station, .. } if !a.is_paused() => Some(format!("Radio is playing {station}")),
                _ => None,
            }),
            Arc::new(move || b.stop()),
        );
    }

    /// Holds the sound where it is (a file continues from the same spot; a live station carries on live afterwards).
    /// Returns false when nothing is playing.
    pub fn pause(&self) -> bool {
        let playing = matches!(self.state(), RadioState::Playing { .. });
        if playing {
            self.inner.paused.store(true, Ordering::SeqCst);
            *self.inner.levels.lock().unwrap() = [0.0; BANDS];
        }
        playing
    }

    /// Carries on after a pause. Returns false when it was not paused.
    pub fn resume(&self) -> bool {
        self.inner.paused.swap(false, Ordering::SeqCst)
    }

    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::SeqCst) && self.is_active()
    }

    /// The certificate decision the last failed start is waiting for, if that is why it failed.
    pub fn cert_problem(&self) -> Option<CertProblem> {
        self.inner.problem.lock().unwrap().clone()
    }

    pub fn stop(&self) {
        *self.inner.problem.lock().unwrap() = None;
        self.inner.paused.store(false, Ordering::SeqCst);
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        *self.inner.state.lock().unwrap() = RadioState::Idle;
        *self.inner.levels.lock().unwrap() = [0.0; BANDS];
    }

    /// Start (or switch to) a station. `allow` is the account's host policy; `play_cmd` takes WAV on its stdin.
    pub fn play(&self, station: &Station, allow: Allow, play_cmd: &str) {
        self.inner.paused.store(false, Ordering::SeqCst);
        let gen = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.inner.state.lock().unwrap() = RadioState::Connecting(station.name.clone());
        *self.inner.problem.lock().unwrap() = None;
        let me = self.clone();
        let (station, play_cmd) = (station.clone(), play_cmd.to_string());
        std::thread::Builder::new().name("iyakki-radio".into()).spawn(move || me.run(gen, &station, allow, &play_cmd)).ok();
    }

    fn current(&self, gen: u64) -> bool {
        self.inner.generation.load(Ordering::SeqCst) == gen
    }

    fn set_state(&self, gen: u64, s: RadioState) {
        if self.current(gen) {
            *self.inner.state.lock().unwrap() = s;
        }
    }

    fn run(&self, gen: u64, station: &Station, allow: Allow, play_cmd: &str) {
        let mut played = false;
        let plan = TlsPlan::new(station.cert_pin.as_ref().map(|p| (p.host.as_str(), p.port, p.sha256.as_str())));
        for attempt in 0..4 {
            if !self.current(gen) {
                return;
            }
            match self.stream_once(gen, station, &allow, &plan, play_cmd, &mut played) {
                Ok(()) => {
                    // a file that has played to its end
                    self.set_state(gen, RadioState::Idle);
                    *self.inner.levels.lock().unwrap() = [0.0; BANDS];
                    return;
                }
                Err(StreamEnd::Stopped) => return,
                Err(StreamEnd::Failed(why)) => {
                    // a stream that was playing is worth reconnecting; one that never started is not
                    if !played || attempt == 3 {
                        self.set_state(gen, RadioState::Failed { station: station.name.clone(), why });
                        return;
                    }
                    self.set_state(gen, RadioState::Connecting(station.name.clone()));
                    let wait = *self.inner.retry_wait.lock().unwrap();
                    let t0 = std::time::Instant::now();
                    while t0.elapsed() < wait {
                        if !self.current(gen) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(20).min(wait));
                    }
                }
            }
        }
    }

    /// A local MP3 file as if it were a stream (its ID3 tag skipped). Only the tools that checked the person may read
    /// the file hand such an address to the player; the stations a person adds are http(s) only.
    fn open_file(path: &str) -> Result<StreamHead, StreamEnd> {
        let mut f = std::fs::File::open(path).map_err(|e| StreamEnd::Failed(format!("cannot open the file: {e}")))?;
        let mut head = [0u8; 10];
        let n = f.read(&mut head).map_err(|e| StreamEnd::Failed(e.to_string()))?;
        let mut skip = 0u64;
        if n == 10 && &head[..3] == b"ID3" {
            // a tag with its size in four 7-bit bytes: the audio starts after it
            skip = 10 + (((head[6] & 0x7f) as u64) << 21 | ((head[7] & 0x7f) as u64) << 14 | ((head[8] & 0x7f) as u64) << 7 | (head[9] & 0x7f) as u64);
        }
        use std::io::Seek;
        f.seek(std::io::SeekFrom::Start(skip)).map_err(|e| StreamEnd::Failed(e.to_string()))?;
        Ok(StreamHead { status: 200, headers: vec![("content-type".into(), "audio/mpeg".into())], body: Box::new(f) })
    }

    fn open(&self, gen: u64, station: &Station, url: &str, allow: &Allow, plan: &TlsPlan) -> Result<(StreamHead, Codec), StreamEnd> {
        if let Some(path) = url.strip_prefix("file://") {
            return Self::open_file(path).map(|h| (h, Codec::Mp3));
        }
        let streamer = self.inner.streamer.lock().unwrap().clone().ok_or_else(|| StreamEnd::Failed("no network connection is set up for the radio".into()))?;
        let mut url = url.to_string();
        for _ in 0..6 {
            if !self.current(gen) {
                return Err(StreamEnd::Stopped);
            }
            let host = host_of(&url);
            if !allow(&host) {
                return Err(StreamEnd::Failed(format!("your account may not reach {host}")));
            }
            let head = self.hop(gen, &streamer, plan, station, &url, allow)?;
            if (300..400).contains(&head.status) {
                let loc = head.header("location").ok_or_else(|| StreamEnd::Failed("the station sent you elsewhere without saying where".into()))?;
                url = resolve(&url, loc).ok_or_else(|| StreamEnd::Failed("the station's redirect is not usable".into()))?;
                continue;
            }
            if head.status >= 400 {
                return Err(StreamEnd::Failed(format!("the station answered {}", head.status)));
            }
            let ct = head.header("content-type").unwrap_or("").to_ascii_lowercase();
            let path = url.split(['?', '#']).next().unwrap_or("");
            if ct.contains("mpegurl") || ct.contains("scpls") || path.ends_with(".pls") || path.ends_with(".m3u") || path.ends_with(".m3u8") {
                let mut raw = Vec::new();
                let _ = head.body.take(crate::hls::MAX_PLAYLIST as u64 + 1).read_to_end(&mut raw);
                if raw.len() > crate::hls::MAX_PLAYLIST {
                    return Err(StreamEnd::Failed("the playlist is too large".into()));
                }
                let text = String::from_utf8_lossy(&raw).to_string();
                if crate::hls::is_hls(&text) {
                    let me = self.clone();
                    let net = crate::hls::Net { streamer: streamer.clone(), allow: allow.clone(), stop: Arc::new(move || !me.current(gen)), timing: crate::hls::Timing { scale: self.inner.hls_scale.load(Ordering::Relaxed) }, tls: plan.clone() };
                    let reader = crate::hls::HlsReader::start(net, &url, Some(&raw)).map_err(|e| if self.current(gen) { StreamEnd::Failed(e) } else { StreamEnd::Stopped })?;
                    let codec = reader.codec();
                    return Ok((StreamHead { status: 200, headers: vec![], body: Box::new(reader) }, codec));
                }
                url = playlist_target(&text).ok_or_else(|| StreamEnd::Failed("the playlist has no stream address".into()))?;
                continue;
            }
            let aac_ct = audiofmt::content_type_is_aac(&ct);
            if !aac_ct && (ct.contains("ogg") || ct.contains("opus") || ct.contains("mp4") || ct.contains("flac")) {
                return Err(StreamEnd::Failed(format!("this station streams {} and only MP3 and AAC are supported; many stations offer an MP3 link", ct.split(';').next().unwrap_or(&ct))));
            }
            // the first bytes tell MP3 from AAC when the station's content type does not (or lies)
            let mut body = head.body;
            let mut prefix = Vec::new();
            let mut chunk = [0u8; 16];
            while prefix.len() < 16 {
                match body.read(&mut chunk[..16 - prefix.len()]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => prefix.extend_from_slice(&chunk[..n]),
                }
                if prefix.len() >= 4 && audiofmt::sniff(&prefix).is_some() {
                    break;
                }
            }
            let codec = audiofmt::sniff(&prefix).unwrap_or(if aac_ct { Codec::Aac } else { Codec::Mp3 });
            let body: Box<dyn Read + Send> = Box::new(Cursor::new(prefix).chain(body));
            return Ok((StreamHead { status: head.status, headers: head.headers, body }, codec));
        }
        Err(StreamEnd::Failed("too many redirects".into()))
    }

    /// One request. A secure connection that fails for lack of the issuer's certificate is first completed from the issuer
    /// address the certificate itself names (`http`, size-capped, DER only, and the result must still lead to a trusted root);
    /// if that does not help, the person is told what the server showed and may approve exactly that certificate.
    fn hop(&self, gen: u64, streamer: &Arc<dyn Streamer>, plan: &TlsPlan, station: &Station, url: &str, allow: &Allow) -> Result<StreamHead, StreamEnd> {
        let headers = [("Icy-MetaData".to_string(), "1".to_string()), ("Accept".to_string(), "*/*".to_string())];
        let e = match plan.open(&**streamer, url, &headers) {
            Ok(h) => return Ok(h),
            Err(e) => e,
        };
        if !self.current(gen) {
            return Err(StreamEnd::Stopped);
        }
        let Some((host, port)) = https_endpoint(url) else { return Err(StreamEnd::Failed(format!("could not connect: {e}"))) };
        let pinned = matches!(plan.mode_for(url), Some(TlsMode::Pin(_)));
        let changed = pinned && e.contains(iyakki_proxy::pintls::CHANGED);
        let unknown_issuer = !pinned && iyakki_proxy::pintls::is_unknown_issuer(&e);
        if !changed && !unknown_issuer {
            return Err(StreamEnd::Failed(format!("could not connect: {e}")));
        }
        let chain = match streamer.peer_chain(&host, port) {
            Ok(c) => c,
            Err(_) => return Err(StreamEnd::Failed(format!("could not connect: {e}"))),
        };
        let info = match certinfo::parse(&chain[0]) {
            Ok(i) => i,
            Err(_) => return Err(StreamEnd::Failed(format!("could not connect: {e}"))),
        };
        if unknown_issuer && plan.mode_for(url).is_none() {
            if let Some(h) = self.complete_chain(streamer, plan, url, &chain[0], &info, allow, &headers) {
                return Ok(h);
            }
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let name_ok = iyakki_proxy::pintls::names_match(&chain[0], &host);
        let expired = info.not_after < now;
        let trustable = name_ok && !expired && info.not_before <= now;
        let trustable = trustable && https_endpoint(&station.url) == Some((host.clone(), port));
        let who = format!("Its certificate is for {}, issued by {}, valid until {}; SHA-256 {}.", info.subject, info.issuer, certinfo::date_text(info.not_after), certinfo::fingerprint_groups(&info.sha256));
        let message = if changed {
            let was = station.cert_pin.as_ref().map(|p| certinfo::short_fingerprint(&p.sha256)).unwrap_or_default();
            format!("The certificate of {host} changed since you trusted it (it was {was}…). A renewed certificate looks like this (Let's Encrypt renews every 60 to 90 days), and so does someone pretending to be the station; IYAKKI does not trust the new one without asking again. {who}")
        } else if !name_ok {
            format!("{UNKNOWN_ISSUER_TEXT} Its certificate is not even for {host}, so it cannot be trusted. {who}")
        } else if expired {
            format!("{UNKNOWN_ISSUER_TEXT} Its certificate has also expired. {who}")
        } else {
            format!("{UNKNOWN_ISSUER_TEXT} {who}")
        };
        if self.current(gen) {
            *self.inner.problem.lock().unwrap() = Some(CertProblem { station: station.name.clone(), url: url.to_string(), host, port, changed, sha256: info.sha256.clone(), subject: info.subject.clone(), issuer: info.issuer.clone(), not_after: info.not_after, trustable, message: message.clone() });
        }
        Err(StreamEnd::Failed(message))
    }

    /// Fetch the issuer certificate the leaf names (and, when that one is itself issued by an authority the built-in roots do
    /// not know, the next one up, at most three) and retry with them added. Everything is still verified against the roots.
    #[allow(clippy::too_many_arguments)]
    fn complete_chain(&self, streamer: &Arc<dyn Streamer>, plan: &TlsPlan, url: &str, _leaf: &[u8], info: &certinfo::CertInfo, allow: &Allow, headers: &[(String, String)]) -> Option<StreamHead> {
        let mut found: Vec<Vec<u8>> = Vec::new();
        let mut next = info.clone();
        for _ in 0..MAX_ISSUER_HOPS {
            let mut got = None;
            for u in next.ca_issuers.iter().take(2) {
                if !allow(&host_of(u)) {
                    continue;
                }
                let Ok(h) = streamer.open(u, &[("Accept".into(), "application/pkix-cert, */*".into())]) else { continue };
                if h.status != 200 {
                    continue;
                }
                let mut der = Vec::new();
                if h.body.take(MAX_ISSUER_CERT as u64 + 1).read_to_end(&mut der).is_err() || der.len() > MAX_ISSUER_CERT || der.first() != Some(&0x30) {
                    continue;
                }
                if let Ok(i) = certinfo::parse(&der) {
                    got = Some((der, i));
                    break;
                }
            }
            let (der, i) = got?;
            found.push(der);
            if let Ok(head) = streamer.open_tls(url, headers, &TlsMode::ExtraChain(found.clone())) {
                plan.add_chain(url, found);
                return Some(head);
            }
            if i.self_issued {
                return None;
            }
            next = i;
        }
        None
    }

    fn stream_once(&self, gen: u64, station: &Station, allow: &Allow, plan: &TlsPlan, play_cmd: &str, played: &mut bool) -> Result<(), StreamEnd> {
        let (head, codec) = self.open(gen, station, &station.url, allow, plan)?;
        // a missing add-on is reported before anything is read
        let aac = if codec == Codec::Aac { Some(self.aac_decoder().map_err(StreamEnd::Failed)?) } else { None };
        let interval = head.header("icy-metaint").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
        let kbps_hint = head.header("icy-br").and_then(|v| v.split(',').next()).and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0);
        let title = Arc::new(Mutex::new(String::new()));
        let said = Arc::new(Mutex::new(String::new()));
        let mut reader = IcyReader::new(head.body, interval, title.clone());
        // a thread reads the network; this one decodes and plays, and notices when the network goes quiet
        let (tx, rx) = sync_channel::<Vec<u8>>(64);
        let said2 = said.clone();
        std::thread::spawn(move || {
            let mut chunk = vec![0u8; 8192];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Err(e) => {
                        *said2.lock().unwrap() = e.to_string();
                        break;
                    }
                    Ok(n) => {
                        if tx.send(chunk[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let c = Ctx { gen, station, play_cmd, rx, title, kbps_hint, said };
        let mut sink = Sink { out: None, out_gen: crate::audioout::global().generation() };
        let result = match aac {
            Some(d) => self.aac_loop(&c, d, &mut sink, played),
            None => self.mp3_loop(&c, &mut sink, played),
        };
        if let Some((mut out, _, _)) = sink.out.take() {
            // a file that played to its end: let the sound command play what it has buffered before it is stopped
            if result.is_ok() {
                out.finish(Duration::from_secs(4), &|| self.current(gen));
            } else {
                out.stop();
            }
        }
        result
    }

    /// Hands decoded sound (interleaved, `ch` channels) to the play command, starting it again when the format or the sound
    /// output changed; updates the levels and the state.
    fn emit(&self, c: &Ctx, sink: &mut Sink, pcm: &[i16], ch: u16, rate: u32, bitrate: u32, played: &mut bool) -> Result<(), StreamEnd> {
        // the sound output changed (a monitor plugged in or moved, another output chosen): the player starts
        // again on the new device, after a short gap
        let sound_gen = crate::audioout::global().generation();
        if sink.out.as_ref().is_none_or(|o| o.1 != rate || o.2 != ch) || sink.out_gen != sound_gen {
            if let Some((mut old, _, _)) = sink.out.take() {
                old.stop();
            }
            let out = crate::audiosink::open_output(crate::mixer::SourceKind::Radio, rate, ch, "radio", c.play_cmd).map_err(StreamEnd::Failed)?;
            sink.out = Some((out, rate, ch));
            sink.out_gen = sound_gen;
        }
        // the radio's own volume (linked to the master volume, the radio adds none of its own); IYAKKI's volume and mute are
        // applied by the output, once
        let own = if self.link().is_some() { 100 } else { self.volume() };
        {
            let fresh = band_levels(pcm, ch as usize, rate);
            let mut l = self.inner.levels.lock().unwrap();
            for (o, f) in l.iter_mut().zip(fresh) {
                // rises at once, falls away smoothly
                *o = f.max(*o * 0.86);
            }
        }
        let (out, _, _) = sink.out.as_mut().expect("player started");
        out.set_volume(own);
        if let Err(said) = out.write(pcm) {
            return Err(StreamEnd::Failed(player_stopped_message(&said)));
        }
        let t = c.title.lock().unwrap().clone();
        let kbps = if bitrate > 0 { bitrate } else { c.kbps_hint };
        *played = true;
        self.set_state(c.gen, RadioState::Playing { station: c.station.name.clone(), title: t, kbps });
        Ok(())
    }

    fn stopped_sending(c: &Ctx) -> String {
        let said = c.said.lock().unwrap().clone();
        if said.is_empty() { "the station stopped sending".into() } else { format!("the station stopped sending ({said})") }
    }

    fn mp3_loop(&self, c: &Ctx, sink: &mut Sink, played: &mut bool) -> Result<(), StreamEnd> {
        let gen = c.gen;
        let mut dec = nanomp3::Decoder::new();
        let mut pcm = vec![0i16; nanomp3::MAX_SAMPLES_PER_FRAME];
        let mut buf: Vec<u8> = Vec::new();
        let mut eof = false;
        let mut garbage = 0usize;
        loop {
            if !self.current(gen) {
                return Err(StreamEnd::Stopped);
            }
            // paused: nothing is decoded or sent to the speaker until it is resumed or stopped
            while self.inner.paused.load(Ordering::SeqCst) && self.current(gen) {
                std::thread::sleep(Duration::from_millis(40));
            }
            if !self.current(gen) {
                return Err(StreamEnd::Stopped);
            }
            // wait only when there is too little to decode (a frame is trusted once the next one is seen), and
            // otherwise just take whatever has already arrived
            while !eof && buf.len() < 4096 {
                match c.rx.recv_timeout(Duration::from_secs(15)) {
                    Ok(b) => buf.extend_from_slice(&b),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => eof = true,
                }
                if !self.current(gen) {
                    break;
                }
            }
            while !eof && buf.len() < 16 * 1024 {
                match c.rx.try_recv() {
                    Ok(b) => buf.extend_from_slice(&b),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => eof = true,
                }
            }
            if buf.is_empty() {
                if eof && *played && c.station.url.starts_with("file://") {
                    return Ok(());
                }
                return Err(StreamEnd::Failed(if eof { Self::stopped_sending(c) } else { "no sound arrived for 15 seconds".into() }));
            }
            let (used, frame) = dec.decode(&buf, &mut pcm);
            buf.drain(..used.max(1).min(buf.len()));
            match frame {
                Ok(info) if info.samples_produced > 0 => {
                    garbage = 0;
                    let ch = info.channels.num() as u16;
                    let n = info.samples_produced * ch as usize;
                    self.emit(c, sink, &pcm[..n], ch, info.sample_rate, info.bitrate, played)?;
                }
                Ok(_) => {}
                Err(_) => {
                    // skipped bytes that were not audio; a stream that is all junk is not worth waiting for
                    garbage += used;
                    if garbage > 256 * 1024 {
                        return Err(StreamEnd::Failed("this does not look like an MP3 stream".into()));
                    }
                    if eof && buf.len() < 4 {
                        return Err(StreamEnd::Failed(Self::stopped_sending(c)));
                    }
                }
            }
        }
    }

    /// AAC: the stream goes to the decoder (the audio add-on) and its sound comes back to be played.
    fn aac_loop(&self, c: &Ctx, mut dec: Box<dyn AacDecoder>, sink: &mut Sink, played: &mut bool) -> Result<(), StreamEnd> {
        let mut pcm = vec![0i16; 8192];
        let mut pending: Option<Vec<u8>> = None;
        let mut eof = false;
        let mut last_sound = std::time::Instant::now();
        loop {
            if !self.current(c.gen) {
                return Err(StreamEnd::Stopped);
            }
            if self.inner.paused.load(Ordering::SeqCst) {
                while self.inner.paused.load(Ordering::SeqCst) && self.current(c.gen) {
                    std::thread::sleep(Duration::from_millis(40));
                }
                last_sound = std::time::Instant::now();
                continue;
            }
            if !eof {
                if pending.is_none() {
                    match c.rx.try_recv() {
                        Ok(b) => pending = Some(b),
                        Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            eof = true;
                            dec.finish();
                        }
                    }
                }
                if let Some(b) = pending.take() {
                    if !dec.feed(&b).map_err(StreamEnd::Failed)? {
                        pending = Some(b);
                    }
                }
            }
            match dec.read(&mut pcm, Duration::from_millis(20)).map_err(StreamEnd::Failed)? {
                Pcm::Samples(n) => {
                    last_sound = std::time::Instant::now();
                    let (rate, ch) = dec.format();
                    let n = n - n % ch.max(1);
                    if n > 0 {
                        self.emit(c, sink, &pcm[..n], ch as u16, rate, 0, played)?;
                    }
                }
                Pcm::Pending => {
                    if last_sound.elapsed() > Duration::from_secs(15) {
                        return Err(StreamEnd::Failed("no sound arrived for 15 seconds".into()));
                    }
                }
                Pcm::End => return Err(StreamEnd::Failed(Self::stopped_sending(c))),
            }
        }
    }
}

struct Ctx<'a> {
    gen: u64,
    station: &'a Station,
    play_cmd: &'a str,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    title: Arc<Mutex<String>>,
    kbps_hint: u32,
    said: Arc<Mutex<String>>,
}

/// The sound output and what it was started for: (output, rate, channels).
struct Sink {
    out: Option<(crate::audiosink::Output, u32, u16)>,
    out_gen: u64,
}

/// What the radio tells when the sound player ends while it plays: the player's own words when it had any.
pub(crate) fn player_stopped_message(said: &str) -> String {
    if said.is_empty() {
        "the sound player stopped (is the speaker set up in Settings → Voice?)".into()
    } else {
        format!("the sound player stopped: {said} (the sound output refused the stream; Settings → Sound can pick another output)")
    }
}

enum StreamEnd {
    Stopped,
    Failed(String),
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_player_that_says_why_it_stopped_is_quoted_and_a_silent_one_gets_the_old_hint() {
        assert!(player_stopped_message("").contains("Settings → Voice"));
        let m = player_stopped_message("aplay: set_params:1462: Unable to install hw params:");
        assert!(m.contains("Unable to install hw params") && m.contains("Settings → Sound") && !m.contains("Settings → Voice"), "{m}");
        let mut c = crate::voice::spawn_player("sh -c").unwrap_or_else(|_| panic!());
        let _ = c.kill();
        let mut c = std::process::Command::new("sh").args(["-c", "echo 'aplay: boom' >&2; exit 1"]).stderr(std::process::Stdio::piped()).spawn().unwrap();
        assert_eq!(crate::voice::player_failure(&mut c), "aplay: boom");
    }

    use super::*;
    use std::io::Cursor;

    /// `n` frames of MPEG-1 layer III, 128 kbit/s, 44.1 kHz, joint stereo, all silence.
    fn silence(n: usize) -> Vec<u8> {
        let mut v = Vec::new();
        for _ in 0..n {
            let mut f = vec![0u8; 417];
            f[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
            v.extend_from_slice(&f);
        }
        v
    }

    type Route = Box<dyn Fn(usize) -> Result<(u16, Vec<(String, String)>, Vec<u8>), String> + Send + Sync>;

    struct Fake {
        asked: Mutex<Vec<String>>,
        routes: Mutex<std::collections::HashMap<String, Route>>,
    }

    impl Streamer for Fake {
        fn open(&self, url: &str, headers: &[(String, String)]) -> Result<StreamHead, String> {
            assert!(headers.iter().any(|(k, v)| k == "Icy-MetaData" && v == "1"));
            let n = {
                let mut a = self.asked.lock().unwrap();
                a.push(url.to_string());
                a.iter().filter(|u| *u == url).count()
            };
            let routes = self.routes.lock().unwrap();
            let r = routes.get(url).ok_or("connection refused")?;
            let (status, headers, body) = r(n)?;
            Ok(StreamHead { status, headers, body: Box::new(Cursor::new(body)) })
        }
    }

    fn fake(routes: Vec<(&str, Route)>) -> Arc<Fake> {
        Arc::new(Fake { asked: Mutex::new(vec![]), routes: Mutex::new(routes.into_iter().map(|(u, r)| (u.to_string(), r)).collect()) })
    }

    fn ok_mp3(frames: usize) -> Route {
        Box::new(move |_| Ok((200, vec![("content-type".into(), "audio/mpeg".into()), ("icy-br".into(), "128".into())], silence(frames))))
    }

    fn player(f: Arc<Fake>) -> RadioPlayer {
        let p = RadioPlayer::default();
        p.set_streamer(f);
        p.set_retry_wait(Duration::from_millis(30));
        p
    }

    fn wait_for(p: &RadioPlayer, what: impl Fn(&RadioState) -> bool) -> RadioState {
        // polls until the state matches; a generous limit (30 s) so a loaded machine running the whole suite does not starve a worker thread into a false failure
        for _ in 0..3000 {
            let s = p.state();
            if what(&s) {
                return s;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("never got there: {:?}", p.state());
    }

    fn allow_all() -> Allow {
        Arc::new(|_| true)
    }

    fn st(url: &str) -> Station {
        Station::new("Test FM", url)
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-radio-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_language_presets_are_well_formed_and_the_level_bands_follow_the_pitch() {
        assert!(LANGUAGES.len() >= 60, "many languages: {}", LANGUAGES.len());
        assert_eq!(LANGUAGES.iter().take(5).map(|l| l.name).collect::<Vec<_>>(), vec!["Tamil", "Telugu", "Malayalam", "English", "Hindi"], "the first ones keep their place (the South Indian languages come right after each other)");
        let mut names = std::collections::HashSet::new();
        assert!(LANGUAGES.iter().all(|l| names.insert(l.name)), "a language is listed twice");
        assert!(LANGUAGES.iter().any(|l| l.name == "Tamil") && LANGUAGES.iter().any(|l| l.name == "English") && LANGUAGES.iter().any(|l| l.name == "Spanish"));
        let mut urls = std::collections::HashSet::new();
        for l in LANGUAGES {
            assert!(!l.stations.is_empty(), "{}", l.name);
            for (name, url) in l.stations {
                assert!(valid_name(name).is_ok() && valid_url(url).is_ok(), "{name} {url}");
                assert!(urls.insert(*url), "duplicate {url}");
            }
        }
        // a 1 kHz tone lights the band nearest 1 kHz more than the lowest and the highest
        let pcm: Vec<i16> = (0..2304).map(|i| (((i / 2) as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 44100.0).sin() * 12000.0) as i16).collect();
        let l = band_levels(&pcm, 2, 44100);
        let top = l.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
        let centre = |b: usize| 50.0 * (14000.0f32 / 50.0).powf(b as f32 / (BANDS - 1) as f32);
        assert!((centre(top) - 1000.0).abs() < 300.0, "the loudest band is at {} Hz", centre(top));
        assert!(l[0] < l[top] - 0.2 && l[BANDS - 1] < l[top] - 0.2, "{l:?}");
        // silence and too little data give flat zeros
        assert!(band_levels(&vec![0i16; 2304], 2, 44100).iter().all(|v| *v == 0.0));
        assert_eq!(band_levels(&[1, 2, 3], 2, 44100), [0.0; BANDS]);
    }

    #[test]
    fn addresses_names_and_playlists_are_checked() {
        assert_eq!(valid_url(" https://stream.example.org:8000/live.mp3 ").unwrap(), "https://stream.example.org:8000/live.mp3");
        for bad in ["stream.example.org", "ftp://x.org/a", "http://", "http://u:p@h.org/a", "http://h.org/a b", "javascript:alert(1)", "http://h_x.org/"] {
            assert!(valid_url(bad).is_err(), "{bad}");
        }
        assert_eq!(host_of("https://Stream.Example.org:8000/a?b=c"), "stream.example.org");
        assert_eq!(valid_name("  My   station ").unwrap(), "My station");
        assert!(valid_name("").is_err() && valid_name(&"x".repeat(41)).is_err() && valid_name("a\u{7}b").is_err() && valid_name("a\nb").unwrap() == "a b");
        assert_eq!(playlist_target("[playlist]\nNumberOfEntries=1\nFile1=http://a.org:8000/s1\nTitle1=x\n").as_deref(), Some("http://a.org:8000/s1"));
        assert_eq!(playlist_target("#EXTM3U\n#EXTINF:-1,x\nhttps://b.org/live.mp3\n").as_deref(), Some("https://b.org/live.mp3"));
        assert_eq!(playlist_target("nothing useful here"), None);
    }

    /// An endless, slowly arriving MP3 stream.
    struct Slow;
    impl Read for Slow {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(Duration::from_millis(5));
            let frame = silence(1);
            let n = buf.len().min(frame.len());
            buf[..n].copy_from_slice(&frame[..n]);
            Ok(n)
        }
    }
    struct SlowStreamer;
    impl Streamer for SlowStreamer {
        fn open(&self, _url: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
            Ok(StreamHead { status: 200, headers: vec![("content-type".into(), "audio/mpeg".into())], body: Box::new(Slow) })
        }
    }

    #[test]
    fn pausing_holds_the_sound_resuming_carries_on_and_stopping_while_paused_works() {
        let d = tmp("pause");
        let out = d.join("out.wav");
        let p = RadioPlayer::default();
        p.set_streamer(Arc::new(SlowStreamer));
        assert!(!p.pause(), "nothing to pause yet");
        assert!(!p.resume());
        p.play(&st("https://r.example.org/live.mp3"), allow_all(), &format!("tee {}", out.display()));
        wait_for(&p, |s| matches!(s, RadioState::Playing { .. }));
        let size = || std::fs::metadata(&out).map_or(0, |m| m.len());
        std::thread::sleep(Duration::from_millis(150));
        assert!(p.pause());
        assert!(p.is_paused());
        std::thread::sleep(Duration::from_millis(100)); // what was already on its way finishes
        let held = size();
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(size(), held, "no sound is sent while paused");
        assert_eq!(p.levels(), [0.0; BANDS]);
        assert!(matches!(p.state(), RadioState::Playing { .. }), "still the same station");
        assert!(p.resume());
        assert!(!p.is_paused());
        std::thread::sleep(Duration::from_millis(400));
        assert!(size() > held, "sound flows again after resuming");
        // stopping while paused ends it and clears the pause for the next song
        assert!(p.pause());
        p.stop();
        assert!(!p.is_paused() && !p.is_active());
        p.play(&st("https://r.example.org/live.mp3"), allow_all(), "tee /dev/null");
        wait_for(&p, |s| matches!(s, RadioState::Playing { .. }));
        assert!(!p.is_paused(), "a new song starts unpaused");
        p.stop();
    }

    #[test]
    fn stations_are_kept_per_person_and_replaced_by_name() {
        let d = tmp("store");
        let a = StationStore::new(&d, "alice");
        assert!(a.list().is_empty());
        a.add("Paradise", "https://stream.radioparadise.com/mp3-128").unwrap();
        a.add("KEXP", "https://kexp-mp3-128.streamguys1.com/kexp128.mp3").unwrap();
        a.add("paradise", "https://other.example.org/p.mp3").unwrap();
        assert_eq!(a.list().iter().map(|s| (s.name.as_str(), s.url.as_str())).collect::<Vec<_>>(), vec![("paradise", "https://other.example.org/p.mp3"), ("KEXP", "https://kexp-mp3-128.streamguys1.com/kexp128.mp3")]);
        assert!(StationStore::new(&d, "bob").list().is_empty());
        assert!(a.add("Bad", "not a url").is_err());
        a.remove("KEXP").unwrap();
        assert_eq!(a.list().len(), 1);
        assert!(a.remove("KEXP").is_err());
    }

    #[test]
    fn song_titles_are_taken_out_of_the_audio() {
        let title = Arc::new(Mutex::new(String::new()));
        let mut raw = Vec::new();
        raw.extend_from_slice(b"AAAAAAAA");
        let meta = b"StreamTitle='Artist - Song';";
        let len = meta.len().div_ceil(16);
        raw.push(len as u8);
        raw.extend_from_slice(meta);
        raw.extend(std::iter::repeat_n(0u8, len * 16 - meta.len()));
        raw.extend_from_slice(b"BBBBBBBB");
        raw.push(0); // an empty block keeps the title
        raw.extend_from_slice(b"CC");
        let mut r = IcyReader::new(Cursor::new(raw), 8, title.clone());
        let mut out = Vec::new();
        r.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"AAAAAAAABBBBBBBBCC");
        assert_eq!(*title.lock().unwrap(), "Artist - Song");
    }

    #[test]
    fn a_stream_plays_through_the_play_command_and_stops() {
        let d = tmp("play");
        let out = d.join("out.wav");
        let f = fake(vec![("https://r.example.org/live.mp3", ok_mp3(60))]);
        let p = player(f.clone());
        p.play(&st("https://r.example.org/live.mp3"), allow_all(), &format!("tee {}", out.display()));
        let s = wait_for(&p, |s| matches!(s, RadioState::Playing { .. }));
        assert!(matches!(&s, RadioState::Playing { station, kbps: 128, .. } if station == "Test FM"), "{s:?}");
        // it ends (the fake stream is finite) and reconnects; stop while it does
        std::thread::sleep(Duration::from_millis(150));
        p.stop();
        assert_eq!(p.state(), RadioState::Idle);
        let wav = std::fs::read(&out).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 44_100, "the sample rate of the stream is in the header");
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2);
        assert!(wav.len() > 44 + 4608, "audio followed the header: {} bytes", wav.len());
        assert!(wav[44..].iter().all(|b| *b == 0), "the fixture is silence");
    }

    #[test]
    fn a_dropped_stream_is_reconnected_and_a_station_that_never_starts_is_not() {
        let d = tmp("retry");
        let out = d.join("o.wav");
        let f = fake(vec![("https://r.example.org/live.mp3", ok_mp3(30)), ("https://dead.example.org/x", Box::new(|_| Ok((404, vec![], vec![]))))]);
        let p = player(f.clone());
        p.play(&st("https://r.example.org/live.mp3"), allow_all(), &format!("tee {}", out.display()));
        wait_for(&p, |_| f.asked.lock().unwrap().len() >= 3);
        p.stop();
        let q = player(f.clone());
        q.play(&st("https://dead.example.org/x"), allow_all(), "tee /dev/null");
        let s = wait_for(&q, |s| matches!(s, RadioState::Failed { .. }));
        assert!(matches!(&s, RadioState::Failed { why, .. } if why.contains("404")), "{s:?}");
        assert_eq!(f.asked.lock().unwrap().iter().filter(|u| u.contains("dead")).count(), 1, "no retries for a station that never played");
    }

    #[test]
    fn redirects_playlists_and_unsupported_formats_are_handled_and_the_host_policy_applies() {
        let d = tmp("hops");
        let f = fake(vec![
            ("https://a.example.org/go", Box::new(|_| Ok((302, vec![("location".into(), "https://b.example.org/real.pls".into())], vec![])))),
            ("https://b.example.org/real.pls", Box::new(|_| Ok((200, vec![("content-type".into(), "audio/x-scpls".into())], b"[playlist]\nFile1=http://c.example.org:8000/s\n".to_vec())))),
            ("http://c.example.org:8000/s", ok_mp3(40)),
            ("https://aac.example.org/s", Box::new(|_| Ok((200, vec![("content-type".into(), "audio/aacp".into())], vec![0; 100])))),
            ("https://ogg.example.org/s", Box::new(|_| Ok((200, vec![("content-type".into(), "application/ogg".into())], vec![0; 100])))),
            ("https://junk.example.org/s", Box::new(|_| Ok((200, vec![("content-type".into(), "audio/mpeg".into())], vec![7u8; 400 * 1024])))),
            ("https://evil.example.org/s", ok_mp3(40)),
            ("https://sneaky.example.org/go", Box::new(|_| Ok((302, vec![("location".into(), "https://evil.example.org/s".into())], vec![])))),
        ]);
        let p = player(f.clone());
        p.play(&st("https://a.example.org/go"), allow_all(), "tee /dev/null");
        wait_for(&p, |s| matches!(s, RadioState::Playing { .. }));
        p.stop();
        assert_eq!(f.asked.lock().unwrap()[..3].to_vec(), vec!["https://a.example.org/go", "https://b.example.org/real.pls", "http://c.example.org:8000/s"]);
        p.set_aac_factory(Arc::new(|| Err(audiofmt::NEEDS_AUDIO_ADDON.to_string())));
        p.play(&st("https://aac.example.org/s"), allow_all(), "tee /dev/null");
        let s = wait_for(&p, |s| matches!(s, RadioState::Failed { .. }));
        assert!(matches!(&s, RadioState::Failed { why, .. } if audiofmt::needs_addon(why)), "AAC without the add-on says so: {s:?}");
        p.play(&st("https://ogg.example.org/s"), allow_all(), "tee /dev/null");
        let s = wait_for(&p, |s| matches!(s, RadioState::Failed { why, .. } if why.contains("ogg")));
        assert!(matches!(&s, RadioState::Failed { why, .. } if why.contains("only MP3 and AAC")), "{s:?}");
        p.play(&st("https://junk.example.org/s"), allow_all(), "tee /dev/null");
        let s = wait_for(&p, |s| matches!(s, RadioState::Failed { .. }));
        assert!(matches!(&s, RadioState::Failed { why, .. } if why.contains("does not look like an MP3")), "{s:?}");
        // the policy covers where a redirect leads
        let only: Allow = Arc::new(|h| h == "sneaky.example.org");
        p.play(&st("https://sneaky.example.org/go"), only, "tee /dev/null");
        let s = wait_for(&p, |s| matches!(s, RadioState::Failed { .. }));
        assert!(matches!(&s, RadioState::Failed { why, .. } if why.contains("may not reach evil.example.org")), "{s:?}");
        assert!(!f.asked.lock().unwrap().iter().any(|u| u.contains("evil")), "never contacted");
        let _ = d;
    }

    /// A stand-in for the audio add-on: remembers what it was fed and answers every feed with 1024 samples of a constant.
    struct FakeDec {
        fed: Arc<Mutex<Vec<u8>>>,
        queue: usize,
        done: bool,
    }
    impl AacDecoder for FakeDec {
        fn feed(&mut self, bytes: &[u8]) -> Result<bool, String> {
            self.fed.lock().unwrap().extend_from_slice(bytes);
            self.queue += 1024;
            Ok(true)
        }
        fn finish(&mut self) {
            self.done = true;
        }
        fn read(&mut self, out: &mut [i16], wait: Duration) -> Result<Pcm, String> {
            if self.queue > 0 {
                let n = self.queue.min(out.len());
                out[..n].fill(1000);
                self.queue -= n;
                Ok(Pcm::Samples(n))
            } else if self.done {
                Ok(Pcm::End)
            } else {
                std::thread::sleep(wait.min(Duration::from_millis(5)));
                Ok(Pcm::Pending)
            }
        }
        fn format(&self) -> (u32, usize) {
            (22050, 1)
        }
    }

    fn with_fake_decoder(p: &RadioPlayer) -> Arc<Mutex<Vec<u8>>> {
        let fed = Arc::new(Mutex::new(Vec::new()));
        let f2 = fed.clone();
        p.set_aac_factory(Arc::new(move || Ok(Box::new(FakeDec { fed: f2.clone(), queue: 0, done: false }) as Box<dyn AacDecoder>)));
        fed
    }

    fn adts_stream(frames: usize) -> Vec<u8> {
        (0..frames).flat_map(|_| crate::audiofmt::tests::adts_frame(200)).collect()
    }

    #[test]
    fn an_aac_stream_goes_to_the_decoder_whatever_its_content_type_says_and_its_sound_is_played() {
        let d = tmp("aac");
        let out = d.join("out.wav");
        let body = adts_stream(30);
        // the same 30 frames over and over, slowly, with a content type that claims MP3
        struct Loop(Vec<u8>, usize);
        impl Read for Loop {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                std::thread::sleep(Duration::from_millis(5));
                let n = buf.len().min(200).min(self.0.len() - self.1);
                buf[..n].copy_from_slice(&self.0[self.1..self.1 + n]);
                self.1 = (self.1 + n) % self.0.len();
                Ok(n)
            }
        }
        struct LoopStreamer(Vec<u8>);
        impl Streamer for LoopStreamer {
            fn open(&self, _u: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
                Ok(StreamHead { status: 200, headers: vec![("content-type".into(), "audio/mpeg".into()), ("icy-br".into(), "48".into())], body: Box::new(Loop(self.0.clone(), 0)) })
            }
        }
        let p = RadioPlayer::default();
        p.set_streamer(Arc::new(LoopStreamer(body.clone())));
        let fed = with_fake_decoder(&p);
        p.play(&st("http://r.example.org/hello"), allow_all(), &format!("tee {}", out.display()));
        let s = wait_for(&p, |s| matches!(s, RadioState::Playing { .. }));
        assert!(matches!(&s, RadioState::Playing { kbps: 48, .. }), "{s:?}");
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while fed.lock().unwrap().len() < body.len() && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(150));
        p.stop();
        assert_eq!(&fed.lock().unwrap()[..body.len()], &body[..], "the decoder got the stream untouched, from its first byte");
        let wav = std::fs::read(&out).unwrap();
        assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 22_050);
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1);
        assert!(wav.len() > 44 + 2048 && wav[44..].chunks_exact(2).any(|c| i16::from_le_bytes([c[0], c[1]]) != 0), "sound followed the header");
    }

    fn hls_world(codec_type: u8, es: impl Fn(u64) -> Vec<u8> + Send + Sync + 'static) -> Arc<crate::hls::testkit::FakeNet> {
        use crate::hls::testkit::*;
        let base = "https://cdn.example.org/air/";
        let net = fake_net(vec![]);
        {
            let mut t = net.routes.lock().unwrap();
            t.insert(format!("{base}master.m3u8"), Box::new(|_| Ok((200, vec![("content-type".into(), "application/vnd.apple.mpegurl".into())], b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=64000,CODECS=\"mp4a.40.2\"\nchunks.m3u8\n".to_vec()))));
            t.insert(format!("{base}chunks.m3u8"), Box::new(|n| Ok((200, vec![], live_playlist(0, if n < 2 { 4 } else { 6 }, "").into_bytes()))));
            let es = Arc::new(es);
            for i in 0..8u64 {
                let es = es.clone();
                t.insert(format!("{base}seg{i}.ts"), Box::new(move |_| Ok((200, vec![], build_ts(codec_type, &es(i))))));
            }
        }
        net
    }

    #[test]
    fn an_hls_station_with_mp3_segments_plays_without_the_add_on_and_follows_the_live_edge() {
        let d = tmp("hls-mp3");
        let out = d.join("out.wav");
        let net = hls_world(0x03, |_| silence(4));
        let p = RadioPlayer::default();
        p.set_streamer(net.clone());
        p.set_hls_time_scale(2000);
        p.set_aac_factory(Arc::new(|| Err(audiofmt::NEEDS_AUDIO_ADDON.to_string())));
        p.play(&st("https://cdn.example.org/air/master.m3u8"), allow_all(), &format!("tee {}", out.display()));
        wait_for(&p, |s| matches!(s, RadioState::Playing { .. }));
        wait_for(&p, |_| net.asked.lock().unwrap().iter().any(|u| u.ends_with("seg4.ts")));
        std::thread::sleep(Duration::from_millis(100));
        p.stop();
        let asked = net.asked.lock().unwrap().clone();
        let ts: Vec<&str> = asked.iter().filter(|u| u.ends_with(".ts")).map(|u| u.rsplit('/').next().unwrap()).collect();
        assert_eq!(&ts[..3], ["seg1.ts", "seg2.ts", "seg3.ts"], "three back from the live edge, once each: {ts:?}");
        assert!(ts.contains(&"seg4.ts"));
        let wav = std::fs::read(&out).unwrap();
        assert_eq!(u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]), 44_100);
        assert!(wav.len() > 44 + 4608);
    }

    #[test]
    fn an_hls_station_with_aac_segments_needs_the_add_on_and_then_feeds_it_plain_adts() {
        let net = hls_world(0x0F, |i| (0..10 + i as usize).flat_map(|_| crate::audiofmt::tests::adts_frame(150)).collect());
        let p = RadioPlayer::default();
        p.set_streamer(net.clone());
        p.set_hls_time_scale(2000);
        p.set_aac_factory(Arc::new(|| Err(audiofmt::NEEDS_AUDIO_ADDON.to_string())));
        p.play(&st("https://cdn.example.org/air/master.m3u8"), allow_all(), "tee /dev/null");
        let s = wait_for(&p, |s| matches!(s, RadioState::Failed { .. }));
        assert!(matches!(&s, RadioState::Failed { why, .. } if audiofmt::needs_addon(why)), "{s:?}");
        let fed = with_fake_decoder(&p);
        p.play(&st("https://cdn.example.org/air/master.m3u8"), allow_all(), "tee /dev/null");
        wait_for(&p, |s| matches!(s, RadioState::Playing { .. }));
        wait_for(&p, |_| fed.lock().unwrap().len() >= 150 * 10 * 3);
        p.stop();
        let got = fed.lock().unwrap().clone();
        assert!(got.chunks(150).take(30).all(|c| c.len() < 150 || c[0] == 0xFF && c[1] == 0xF1), "ADTS frames, without the transport stream around them");
    }

    #[test]
    fn hls_stations_obey_the_host_policy_and_refuse_encryption() {
        use crate::hls::testkit::*;
        let net = hls_world(0x03, |_| silence(2));
        let p = RadioPlayer::default();
        p.set_streamer(net.clone());
        p.set_hls_time_scale(2000);
        // the playlist host is allowed, the segments' host is the same: only the master host is let through, then none
        let only: Allow = Arc::new(|h| h == "cdn.example.org");
        net.routes.lock().unwrap().insert("https://cdn.example.org/air/chunks.m3u8".into(), ok(b"#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6,\nhttps://tracker.example.net/seg.ts\n".to_vec()));
        p.play(&st("https://cdn.example.org/air/master.m3u8"), only, "tee /dev/null");
        let s = wait_for(&p, |s| matches!(s, RadioState::Failed { .. }));
        assert!(matches!(&s, RadioState::Failed { why, .. } if why.contains("may not reach tracker.example.net")), "{s:?}");
        assert!(!net.asked.lock().unwrap().iter().any(|u| u.contains("tracker")));
        net.routes.lock().unwrap().insert("https://cdn.example.org/air/chunks.m3u8".into(), ok(live_playlist(0, 2, "#EXT-X-KEY:METHOD=AES-128,URI=\"k\"\n").into_bytes()));
        p.play(&st("https://cdn.example.org/air/master.m3u8"), allow_all(), "tee /dev/null");
        let s = wait_for(&p, |s| matches!(s, RadioState::Failed { .. }));
        assert!(matches!(&s, RadioState::Failed { why, .. } if why.contains("encrypted")), "{s:?}");
    }

    #[test]
    fn switching_stations_and_a_missing_player_are_reported() {
        let f = fake(vec![("https://r.example.org/a", ok_mp3(40)), ("https://r.example.org/b", ok_mp3(40))]);
        let p = player(f);
        p.play(&st("https://r.example.org/a"), allow_all(), "tee /dev/null");
        wait_for(&p, |s| matches!(s, RadioState::Playing { .. }));
        p.play(&Station::new("Other", "https://r.example.org/b"), allow_all(), "tee /dev/null");
        let s = wait_for(&p, |s| matches!(s, RadioState::Playing { station, .. } if station == "Other"));
        assert!(p.is_active(), "{s:?}");
        p.play(&st("https://r.example.org/a"), allow_all(), "/nonexistent/player -q");
        let s = wait_for(&p, |s| matches!(s, RadioState::Failed { .. }));
        assert!(matches!(&s, RadioState::Failed { why, .. } if why.contains("cannot start")), "{s:?}");
        p.set_volume(250);
        assert_eq!(p.volume(), 100);
    }

    /// `IYAKKI_RADIO_URL=https://… cargo test -p iyakki-agent --lib live_stream -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_stream() {
        let Ok(url) = std::env::var("IYAKKI_RADIO_URL") else { return };
        let d = tmp("live");
        let out = d.join("live.wav");
        let p = RadioPlayer::default();
        p.set_streamer(Arc::new(iyakki_proxy::https::UreqStreamer::default()));
        p.play(&st(&url), allow_all(), &format!("tee {}", out.display()));
        let s = wait_for(&p, |s| matches!(s, RadioState::Playing { .. } | RadioState::Failed { .. }));
        eprintln!("{s:?}");
        std::thread::sleep(Duration::from_secs(6));
        eprintln!("{:?}", p.state());
        p.stop();
        let wav = std::fs::read(&out).unwrap();
        let loud = wav[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs()).max().unwrap_or(0);
        eprintln!("{} bytes written, loudest sample {loud}", wav.len());
        if let Ok(keep) = std::env::var("IYAKKI_RADIO_KEEP_WAV") {
            let _ = std::fs::write(keep, &wav);
        }
        assert!(loud > 500, "there is real sound");
    }

    // ---------------------------------------------------------------------- server certificates

    fn long_wait(p: &RadioPlayer) -> RadioState {
        for _ in 0..4000 {
            let s = p.state();
            if matches!(s, RadioState::Playing { .. } | RadioState::Failed { .. }) {
                return s;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        p.state()
    }

    /// Needs the internet. `cargo test -p iyakki-agent --lib live_incomplete_chain -- --ignored --nocapture`
    /// (`IYAKKI_RADIO_URL` to try another station). Shows what happens with a server that sends only its leaf certificate: whether
    /// the issuer could be fetched, or the decision that is offered; then plays once with the approved certificate.
    #[test]
    #[ignore]
    fn live_incomplete_chain() {
        let url = std::env::var("IYAKKI_RADIO_URL").unwrap_or_else(|_| "https://radios.crabdance.com:8002/3".into());
        let p = RadioPlayer::default();
        p.set_streamer(Arc::new(iyakki_proxy::https::UreqStreamer::default()));
        p.play(&st(&url), allow_all(), "tee /dev/null");
        let s = long_wait(&p);
        eprintln!("first try: {s:?}");
        if let Some(c) = p.cert_problem() {
            eprintln!("offered: {} | {} <- {} | until {} | {}", c.host, c.subject, c.issuer, c.expiry(), certinfo::fingerprint_groups(&c.sha256));
            p.stop();
            let mut station = st(&url);
            station.cert_pin = Some(c.pin());
            p.play(&station, allow_all(), "tee /dev/null");
            let s = long_wait(&p);
            eprintln!("with the approved certificate: {s:?}");
        }
        p.stop();
    }

    const HOST: &str = "radios.example.org";
    const URL: &str = "https://radios.example.org:8002/3";

    /// A network where `radios.example.org:8002` sends its leaf only (so the usual check says "unknown issuer").
    struct TlsFake {
        leaf: Vec<u8>,
        /// The certificate the server shows now (a renewal changes it).
        shown: Mutex<Vec<u8>>,
        aia: Option<(String, Vec<u8>)>,
        /// What happened, in order: "open <url>", "pinned <url>", "chain <url>", "fetch <url>".
        log: Mutex<Vec<String>>,
        redirect_pinned: bool,
    }

    impl TlsFake {
        fn new(aia: Option<&str>) -> Arc<TlsFake> {
            let leaf = iyakki_proxy::testcerts::leaf_der(&[HOST], None, aia);
            let inter = aia.map(|u| (u.to_string(), iyakki_proxy::testcerts::leaf_der(&["Test Intermediate"], None, None)));
            Arc::new(TlsFake { shown: Mutex::new(leaf.clone()), leaf, aia: inter, log: Mutex::new(vec![]), redirect_pinned: false })
        }
        fn fp(&self) -> String {
            certinfo::fingerprint(&self.leaf)
        }
        fn mp3(&self) -> Result<StreamHead, String> {
            Ok(StreamHead { status: 200, headers: vec![("content-type".into(), "audio/mpeg".into())], body: Box::new(Cursor::new(silence(40))) })
        }
        fn saw(&self, what: &str) -> bool {
            self.log.lock().unwrap().iter().any(|l| l.starts_with(what))
        }
    }

    impl Streamer for TlsFake {
        fn open(&self, url: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
            self.log.lock().unwrap().push(format!("open {url}"));
            if let Some((u, der)) = &self.aia {
                if url == u {
                    self.log.lock().unwrap().push(format!("fetch {url}"));
                    return Ok(StreamHead { status: 200, headers: vec![], body: Box::new(Cursor::new(der.clone())) });
                }
            }
            if url == URL {
                return Err("io: invalid peer certificate: UnknownIssuer".into());
            }
            if url.starts_with("https://elsewhere.example.org/") {
                return self.mp3();
            }
            Err("connection refused".into())
        }
        fn open_tls(&self, url: &str, _h: &[(String, String)], mode: &TlsMode) -> Result<StreamHead, String> {
            match mode {
                TlsMode::Pin(fp) => {
                    self.log.lock().unwrap().push(format!("pinned {url}"));
                    let shown = self.shown.lock().unwrap().clone();
                    if certinfo::norm_fingerprint(fp) != certinfo::norm_fingerprint(&certinfo::fingerprint(&shown)) {
                        return Err(format!("io: {} (it now shows {})", iyakki_proxy::pintls::CHANGED, certinfo::fingerprint(&shown)));
                    }
                    if self.redirect_pinned {
                        return Ok(StreamHead { status: 302, headers: vec![("location".into(), "https://elsewhere.example.org/live".into())], body: Box::new(Cursor::new(vec![])) });
                    }
                    self.mp3()
                }
                TlsMode::ExtraChain(c) => {
                    self.log.lock().unwrap().push(format!("chain {url}"));
                    match &self.aia {
                        Some((_, der)) if c == &vec![der.clone()] => self.mp3(),
                        _ => Err("io: invalid peer certificate: UnknownIssuer".into()),
                    }
                }
            }
        }
        fn peer_chain(&self, host: &str, port: u16) -> Result<Vec<Vec<u8>>, String> {
            assert_eq!((host, port), (HOST, 8002));
            Ok(vec![self.shown.lock().unwrap().clone()])
        }
    }

    fn tls_player(f: Arc<TlsFake>) -> RadioPlayer {
        let p = RadioPlayer::default();
        p.set_streamer(f);
        p.set_retry_wait(Duration::from_millis(30));
        p
    }

    #[test]
    fn a_server_without_its_intermediate_fails_with_a_plain_message_and_the_certificate_to_decide_about() {
        let f = TlsFake::new(None);
        let p = tls_player(f.clone());
        p.play(&st(URL), allow_all(), "tee /dev/null");
        let RadioState::Failed { why, .. } = wait_for(&p, |s| matches!(s, RadioState::Failed { .. })) else { unreachable!() };
        assert!(why.starts_with(UNKNOWN_ISSUER_TEXT) && why.contains("Browsers hide this; IYAKKI checks strictly."), "{why}");
        let c = p.cert_problem().expect("the decision is offered");
        assert_eq!((c.host.as_str(), c.port, c.changed, c.trustable), (HOST, 8002, false, true));
        assert_eq!(c.sha256, f.fp());
        assert_eq!(c.subject, HOST);
        assert!(why.contains(&certinfo::fingerprint_groups(&c.sha256)) && why.contains(&c.expiry()) && why.contains(&c.issuer));
        assert!(!f.saw("pinned"), "nothing is trusted without the person");
        // a new start clears the offer
        p.play(&st("https://nowhere.example.org/x"), allow_all(), "tee /dev/null");
        wait_for(&p, |s| matches!(s, RadioState::Failed { .. }));
        assert!(p.cert_problem().is_none());
    }

    #[test]
    fn an_approved_certificate_plays_for_that_host_only_and_a_redirect_elsewhere_is_verified_normally() {
        let mut f = TlsFake::new(None);
        Arc::get_mut(&mut f).unwrap().redirect_pinned = true;
        let p = tls_player(f.clone());
        let mut station = st(URL);
        station.cert_pin = Some(CertPin { host: HOST.into(), port: 8002, sha256: f.fp() });
        p.play(&station, allow_all(), "tee /dev/null");
        wait_for(&p, |s| matches!(s, RadioState::Playing { .. }));
        let log = f.log.lock().unwrap().clone();
        assert_eq!(log[0], format!("pinned {URL}"));
        assert!(log.contains(&"open https://elsewhere.example.org/live".to_string()), "the redirect target uses the ordinary check: {log:?}");
        assert!(!log.iter().any(|l| l.starts_with("pinned https://elsewhere")));
        p.stop();
    }

    #[test]
    fn a_changed_certificate_is_refused_says_so_and_is_never_pinned_silently() {
        let f = TlsFake::new(None);
        let p = tls_player(f.clone());
        let mut station = st(URL);
        station.cert_pin = Some(CertPin { host: HOST.into(), port: 8002, sha256: f.fp() });
        // the renewal
        let renewed = iyakki_proxy::testcerts::leaf_der(&[HOST], None, None);
        *f.shown.lock().unwrap() = renewed.clone();
        p.play(&station, allow_all(), "tee /dev/null");
        let RadioState::Failed { why, .. } = wait_for(&p, |s| matches!(s, RadioState::Failed { .. })) else { unreachable!() };
        assert!(why.contains("changed since you trusted it") && why.contains("renews every 60 to 90 days") && why.contains(&certinfo::fingerprint_groups(&certinfo::fingerprint(&renewed))), "{why}");
        let c = p.cert_problem().unwrap();
        assert!(c.changed && c.trustable && c.sha256 == certinfo::fingerprint(&renewed));
        // the old pin is untouched by the failure: trusting the new one is the person's separate step
        assert_eq!(station.cert_pin.as_ref().unwrap().sha256, f.fp());
    }

    #[test]
    fn a_missing_intermediate_is_fetched_from_the_certificates_own_address_when_the_policy_allows() {
        let f = TlsFake::new(Some("http://ca.example.org/int.der"));
        let p = tls_player(f.clone());
        p.play(&st(URL), allow_all(), "tee /dev/null");
        wait_for(&p, |s| matches!(s, RadioState::Playing { .. }));
        assert!(f.saw("fetch http://ca.example.org/int.der") && f.saw("chain"), "{:?}", f.log.lock().unwrap());
        assert!(p.cert_problem().is_none(), "no prompt is needed");
        p.stop();
        // the account's host policy applies to the issuer's address too: refused, so the person is asked instead
        let f = TlsFake::new(Some("http://ca.example.org/int.der"));
        let p = tls_player(f.clone());
        p.play(&st(URL), Arc::new(|h: &str| h != "ca.example.org"), "tee /dev/null");
        wait_for(&p, |s| matches!(s, RadioState::Failed { .. }));
        assert!(!f.saw("fetch") && p.cert_problem().is_some());
    }

    #[test]
    fn a_certificate_for_another_name_or_past_its_dates_cannot_be_trusted() {
        for (names, years, why) in [(vec!["other.example.org"], None, "not even for"), (vec![HOST], Some((2019, 2020)), "expired")] {
            let leaf = iyakki_proxy::testcerts::leaf_der(&names, years, None);
            let f = Arc::new(TlsFake { shown: Mutex::new(leaf.clone()), leaf, aia: None, log: Mutex::new(vec![]), redirect_pinned: false });
            let p = tls_player(f);
            p.play(&st(URL), allow_all(), "tee /dev/null");
            let RadioState::Failed { why: said, .. } = wait_for(&p, |s| matches!(s, RadioState::Failed { .. })) else { unreachable!() };
            assert!(said.contains(why), "{said}");
            assert!(!p.cert_problem().unwrap().trustable);
        }
    }

    #[test]
    fn the_approval_is_kept_per_person_with_the_station_and_old_files_still_parse() {
        let d = tmp("pins");
        // a file from before pins existed
        std::fs::write(d.join("radio-asha.json"), r#"[{"name":"Old FM","url":"https://old.example.org/live.mp3"}]"#).unwrap();
        let a = StationStore::new(&d, "asha");
        let b = StationStore::new(&d, "ravi");
        assert_eq!(a.list()[0].cert_pin, None);
        let f = TlsFake::new(None);
        let p = tls_player(f.clone());
        let station = st(URL);
        p.play(&station, allow_all(), "tee /dev/null");
        wait_for(&p, |s| matches!(s, RadioState::Failed { .. }));
        let problem = p.cert_problem().unwrap();
        // a station that is not in the list yet (a preset) is added with its pin
        let kept = a.trust(&station, &problem).unwrap();
        assert_eq!(kept.cert_pin, Some(CertPin { host: HOST.into(), port: 8002, sha256: f.fp() }));
        assert_eq!(a.list().len(), 2);
        assert!(b.list().is_empty(), "another person has not trusted anything");
        let text = std::fs::read_to_string(d.join("radio-asha.json")).unwrap();
        assert!(text.contains(&f.fp()) && text.matches("cert_pin").count() == 1, "only the station with a pin has the field: {text}");
        // saving the same station again under the same address keeps the approval; a new address drops it
        a.add("Test FM", URL).unwrap();
        assert!(a.list().iter().find(|s| s.name == "Test FM").unwrap().cert_pin.is_some());
        a.add("Test FM", "https://elsewhere.example.org/live").unwrap();
        assert!(a.list().iter().find(|s| s.name == "Test FM").unwrap().cert_pin.is_none());
        // a pin for some other server is refused, and so is trusting what cannot be trusted
        assert!(a.set_pin(&Station::new("Old FM", "https://old.example.org/live.mp3"), Some(CertPin { host: HOST.into(), port: 8002, sha256: f.fp() })).is_err());
        assert!(a.trust(&station, &CertProblem { trustable: false, ..problem.clone() }).is_err());
        // the saved station now points elsewhere, so the old offer cannot be applied to it
        assert!(a.trust(&station, &problem).is_err());
        // forgetting
        a.add("Test FM", URL).unwrap();
        a.trust(&station, &problem).unwrap();
        assert!(a.forget_trust("Test FM").unwrap().cert_pin.is_none());
        assert!(a.forget_trust("Test FM").is_err());
        assert!(a.list().iter().all(|s| s.cert_pin.is_none()));
    }
}
