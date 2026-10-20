//! Internet radio: stations a person adds (a name and a stream address), and a player that keeps playing in the
//! background while they use other apps. Streams are MP3 over HTTP or HTTPS (Icecast and Shoutcast); the song title
//! the station announces is shown. The sound goes to the system's play command (the same one the voice features use),
//! so a person needs no sound server of their own. Every address is checked against the account's host policy, also
//! when a station redirects.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_proxy::{StreamHead, Streamer};
use serde::{Deserialize, Serialize};

pub const MAX_STATIONS: usize = 200;

/// Stations to start from, by language: `(name, MP3 stream address)`. Each answered with MP3 audio when this list was
/// made (11 October 2026); stations move and disappear, so a failing one says why and the person can add their own.
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
}

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
        std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save(&self, v: &[Station]) -> Result<(), String> {
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(v).map_err(|e| e.to_string())?).and_then(|_| std::fs::rename(&tmp, &self.path)).map_err(|e| e.to_string())
    }

    pub fn add(&self, name: &str, url: &str) -> Result<Station, String> {
        let st = Station { name: valid_name(name)?, url: valid_url(url)? };
        let mut v = self.list();
        if let Some(i) = v.iter().position(|s| s.name.eq_ignore_ascii_case(&st.name)) {
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
    streamer: Mutex<Option<Arc<dyn Streamer>>>,
    retry_wait: Mutex<Duration>,
    levels: Mutex<[f32; BANDS]>,
}

/// One player for the whole system: what is playing goes on while apps come and go.
#[derive(Clone)]
pub struct RadioPlayer {
    inner: Arc<Inner>,
}

type Allow = Arc<dyn Fn(&str) -> bool + Send + Sync>;

impl Default for RadioPlayer {
    fn default() -> Self {
        RadioPlayer { inner: Arc::new(Inner { state: Mutex::new(RadioState::Idle), generation: AtomicU64::new(0), paused: std::sync::atomic::AtomicBool::new(false), volume: AtomicU8::new(80), streamer: Mutex::new(None), retry_wait: Mutex::new(Duration::from_secs(2)), levels: Mutex::new([0.0; BANDS]) }) }
    }
}

fn wav_header(rate: u32, channels: u16) -> Vec<u8> {
    let mut h = Vec::with_capacity(44);
    let data_len: u32 = 0x7FFF_F000;
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(data_len + 36).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&channels.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    h.extend_from_slice(&(channels * 2).to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data_len.to_le_bytes());
    h
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

    pub fn volume(&self) -> u8 {
        self.inner.volume.load(Ordering::Relaxed)
    }

    pub fn set_volume(&self, v: u8) {
        self.inner.volume.store(v.min(100), Ordering::Relaxed);
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state(), RadioState::Connecting(_) | RadioState::Playing { .. })
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

    pub fn stop(&self) {
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
        for attempt in 0..4 {
            if !self.current(gen) {
                return;
            }
            match self.stream_once(gen, station, &allow, play_cmd, &mut played) {
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

    fn open(&self, gen: u64, url: &str, allow: &Allow) -> Result<StreamHead, StreamEnd> {
        if let Some(path) = url.strip_prefix("file://") {
            return Self::open_file(path);
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
            let head = streamer.open(&url, &[("Icy-MetaData".into(), "1".into()), ("Accept".into(), "*/*".into())]).map_err(|e| StreamEnd::Failed(format!("could not connect: {e}")))?;
            if (300..400).contains(&head.status) {
                let loc = head.header("location").ok_or_else(|| StreamEnd::Failed("the station sent you elsewhere without saying where".into()))?;
                url = resolve(&url, loc).ok_or_else(|| StreamEnd::Failed("the station's redirect is not usable".into()))?;
                continue;
            }
            if head.status >= 400 {
                return Err(StreamEnd::Failed(format!("the station answered {}", head.status)));
            }
            let ct = head.header("content-type").unwrap_or("").to_ascii_lowercase();
            if ct.contains("mpegurl") || ct.contains("scpls") || url.ends_with(".pls") || url.ends_with(".m3u") {
                let mut text = String::new();
                let _ = head.body.take(64 * 1024).read_to_string(&mut text);
                url = playlist_target(&text).ok_or_else(|| StreamEnd::Failed("the playlist has no stream address".into()))?;
                continue;
            }
            if ct.contains("aac") || ct.contains("ogg") || ct.contains("opus") || ct.contains("mp4") || ct.contains("x-mpegurl") {
                return Err(StreamEnd::Failed(format!("this station streams {} and only MP3 is supported so far; many stations offer an MP3 link", ct.split(';').next().unwrap_or(&ct))));
            }
            return Ok(head);
        }
        Err(StreamEnd::Failed("too many redirects".into()))
    }

    fn stream_once(&self, gen: u64, station: &Station, allow: &Allow, play_cmd: &str, played: &mut bool) -> Result<(), StreamEnd> {
        let head = self.open(gen, &station.url, allow)?;
        let interval = head.header("icy-metaint").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
        let kbps_hint = head.header("icy-br").and_then(|v| v.split(',').next()).and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0);
        let title = Arc::new(Mutex::new(String::new()));
        let mut reader = IcyReader::new(head.body, interval, title.clone());
        // a thread reads the network; this one decodes and plays, and notices when the network goes quiet
        let (tx, rx) = sync_channel::<Vec<u8>>(64);
        std::thread::spawn(move || {
            let mut chunk = vec![0u8; 8192];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(chunk[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let mut dec = nanomp3::Decoder::new();
        let mut pcm = vec![0i16; nanomp3::MAX_SAMPLES_PER_FRAME];
        let mut buf: Vec<u8> = Vec::new();
        let mut eof = false;
        let mut out: Option<(std::process::Child, std::process::ChildStdin, u32, u16)> = None;
        let mut out_gen = crate::audioout::global().generation();
        let mut garbage = 0usize;
        let result = loop {
            if !self.current(gen) {
                break Err(StreamEnd::Stopped);
            }
            // paused: nothing is decoded or sent to the speaker until it is resumed or stopped
            while self.inner.paused.load(Ordering::SeqCst) && self.current(gen) {
                std::thread::sleep(Duration::from_millis(40));
            }
            if !self.current(gen) {
                break Err(StreamEnd::Stopped);
            }
            // wait only when there is too little to decode (a frame is trusted once the next one is seen), and
            // otherwise just take whatever has already arrived
            while !eof && buf.len() < 4096 {
                match rx.recv_timeout(Duration::from_secs(15)) {
                    Ok(b) => buf.extend_from_slice(&b),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => eof = true,
                }
                if !self.current(gen) {
                    break;
                }
            }
            while !eof && buf.len() < 16 * 1024 {
                match rx.try_recv() {
                    Ok(b) => buf.extend_from_slice(&b),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => eof = true,
                }
            }
            if buf.is_empty() {
                if eof && *played && station.url.starts_with("file://") {
                    break Ok(());
                }
                break if eof { Err(StreamEnd::Failed("the station stopped sending".into())) } else { Err(StreamEnd::Failed("no sound arrived for 15 seconds".into())) };
            }
            let (used, frame) = dec.decode(&buf, &mut pcm);
            buf.drain(..used.max(1).min(buf.len()));
            match frame {
                Ok(info) if info.samples_produced > 0 => {
                    garbage = 0;
                    let ch = info.channels.num() as u16;
                    // the sound output changed (a monitor plugged in or moved, another output chosen): the player starts
                    // again on the new device, after a short gap
                    let sound_gen = crate::audioout::global().generation();
                    if out.as_ref().is_none_or(|o| o.2 != info.sample_rate || o.3 != ch) || out_gen != sound_gen {
                        if let Some((mut c, _, _, _)) = out.take() {
                            crate::voice::kill_child(&mut c);
                        }
                        let mut child = crate::voice::spawn_player(play_cmd).map_err(StreamEnd::Failed)?;
                        let mut stdin = child.stdin.take().ok_or_else(|| StreamEnd::Failed("the player has no input".into()))?;
                        stdin.write_all(&wav_header(info.sample_rate, ch)).map_err(|_| StreamEnd::Failed("the player stopped".into()))?;
                        out = Some((child, stdin, info.sample_rate, ch));
                        out_gen = sound_gen;
                    }
                    // the radio's own volume, then IYAKKI's volume and mute (Settings -> Sound; HDMI has no hardware volume)
                    let vol = self.volume() as i32 * crate::audioout::global().gain() as i32 / 100;
                    let n = info.samples_produced * ch as usize;
                    {
                        let fresh = band_levels(&pcm[..n], ch as usize, info.sample_rate);
                        let mut l = self.inner.levels.lock().unwrap();
                        for (o, f) in l.iter_mut().zip(fresh) {
                            // rises at once, falls away smoothly
                            *o = f.max(*o * 0.86);
                        }
                    }
                    let mut bytes = Vec::with_capacity(n * 2);
                    for s in &pcm[..n] {
                        bytes.extend_from_slice(&((*s as i32 * vol / 100) as i16).to_le_bytes());
                    }
                    let (_, stdin, _, _) = out.as_mut().expect("player started");
                    if stdin.write_all(&bytes).is_err() {
                        break Err(StreamEnd::Failed("the sound player stopped (is the speaker set up in Settings → Voice?)".into()));
                    }
                    let t = title.lock().unwrap().clone();
                    let kbps = if info.bitrate > 0 { info.bitrate } else { kbps_hint };
                    *played = true;
                    self.set_state(gen, RadioState::Playing { station: station.name.clone(), title: t, kbps });
                }
                Ok(_) => {}
                Err(_) => {
                    // skipped bytes that were not audio; a stream that is all junk is not worth waiting for
                    garbage += used;
                    if garbage > 256 * 1024 {
                        break Err(StreamEnd::Failed("this does not look like an MP3 stream".into()));
                    }
                    if eof && buf.len() < 4 {
                        break Err(StreamEnd::Failed("the station stopped sending".into()));
                    }
                }
            }
        };
        if let Some((mut c, stdin, _, _)) = out.take() {
            drop(stdin);
            // a file that played to its end: let the sound command play what it has buffered before it is stopped
            if result.is_ok() {
                let t0 = std::time::Instant::now();
                while t0.elapsed() < Duration::from_secs(4) && self.current(gen) && matches!(c.try_wait(), Ok(None)) {
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
            crate::voice::kill_child(&mut c);
        }
        result
    }
}

enum StreamEnd {
    Stopped,
    Failed(String),
}

#[cfg(test)]
mod tests {
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
        for _ in 0..500 {
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
        Station { name: "Test FM".into(), url: url.into() }
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
            ("https://junk.example.org/s", Box::new(|_| Ok((200, vec![("content-type".into(), "audio/mpeg".into())], vec![7u8; 400 * 1024])))),
            ("https://evil.example.org/s", ok_mp3(40)),
            ("https://sneaky.example.org/go", Box::new(|_| Ok((302, vec![("location".into(), "https://evil.example.org/s".into())], vec![])))),
        ]);
        let p = player(f.clone());
        p.play(&st("https://a.example.org/go"), allow_all(), "tee /dev/null");
        wait_for(&p, |s| matches!(s, RadioState::Playing { .. }));
        p.stop();
        assert_eq!(f.asked.lock().unwrap()[..3].to_vec(), vec!["https://a.example.org/go", "https://b.example.org/real.pls", "http://c.example.org:8000/s"]);
        p.play(&st("https://aac.example.org/s"), allow_all(), "tee /dev/null");
        let s = wait_for(&p, |s| matches!(s, RadioState::Failed { .. }));
        assert!(matches!(&s, RadioState::Failed { why, .. } if why.contains("only MP3")), "{s:?}");
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

    #[test]
    fn switching_stations_and_a_missing_player_are_reported() {
        let f = fake(vec![("https://r.example.org/a", ok_mp3(40)), ("https://r.example.org/b", ok_mp3(40))]);
        let p = player(f);
        p.play(&st("https://r.example.org/a"), allow_all(), "tee /dev/null");
        wait_for(&p, |s| matches!(s, RadioState::Playing { .. }));
        p.play(&Station { name: "Other".into(), url: "https://r.example.org/b".into() }, allow_all(), "tee /dev/null");
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
        assert!(loud > 500, "there is real sound");
    }
}
