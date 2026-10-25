//! HLS (HTTP Live Streaming) for the radio: the playlist parser, a follower that keeps fetching new segments of a live
//! station, and a small MPEG transport stream demuxer. Everything is in-house; the audio it hands on is the plain
//! elementary stream (MP3 frames or ADTS AAC), which the radio decodes like any other stream. Every playlist and segment
//! address goes through the account's host policy (redirects too), sizes are bounded, and encrypted streams are refused.

use std::collections::VecDeque;
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_proxy::Streamer;

use crate::audiofmt::{sniff, skip_id3, Codec};

pub const MAX_PLAYLIST: usize = 256 * 1024;
pub const MAX_SEGMENT: usize = 8 * 1024 * 1024;
const MAX_SEGMENTS_PER_PLAYLIST: usize = 5000;
/// Segments played from before the live edge when a station is tuned in.
const STARTUP_SEGMENTS: usize = 3;
const MAX_FAILS: u32 = 6;

pub type Allow = Arc<dyn Fn(&str) -> bool + Send + Sync>;
pub type Stop = Arc<dyn Fn() -> bool + Send + Sync>;

// ---------------------------------------------------------------- addresses

fn normalize_path(pq: &str) -> String {
    let (path, query) = match pq.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (pq, None),
    };
    let mut out: Vec<&str> = Vec::new();
    let trailing = path.ends_with('/') || path.ends_with("/.") || path.ends_with("/..");
    for seg in path.split('/').skip(1) {
        match seg {
            "." | "" => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    let mut s = String::from("/");
    s.push_str(&out.join("/"));
    if trailing && !out.is_empty() {
        s.push('/');
    }
    if let Some(q) = query {
        s.push('?');
        s.push_str(q);
    }
    s
}

/// An address found in a playlist, made absolute against the address of the playlist. Only http(s) with a plain host
/// and no sign-in details are accepted, and a step from https down to http is refused.
pub fn resolve_ref(base: &str, r: &str) -> Result<String, String> {
    let r = r.trim();
    if r.is_empty() || r.len() > 4000 || r.chars().any(|c| c.is_control() || c == ' ') {
        return Err("the playlist has an unusable address".into());
    }
    let (scheme, rest) = base.split_once("://").ok_or("the playlist address is not usable")?;
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(auth_end);
    let tail = tail.split('#').next().unwrap_or("");
    let r = r.split('#').next().unwrap_or("");
    let out = if let Some((s, _)) = r.split_once("://") {
        if !(s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https")) {
            return Err("the playlist points to an address that is not http or https".into());
        }
        r.to_string()
    } else if let Some(p) = r.strip_prefix("//") {
        format!("{scheme}://{p}")
    } else if r.split('/').next().is_some_and(|f| f.contains(':')) {
        return Err("the playlist has an unusable address".into());
    } else if r.starts_with('/') {
        format!("{scheme}://{authority}{}", normalize_path(r))
    } else if r.starts_with('?') {
        format!("{scheme}://{authority}{}{r}", tail.split('?').next().unwrap_or("/"))
    } else {
        let path = tail.split('?').next().unwrap_or("");
        let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
        let dir = if dir.is_empty() { "/" } else { dir };
        format!("{scheme}://{authority}{}", normalize_path(&format!("{dir}{r}")))
    };
    let (s2, rest2) = out.split_once("://").ok_or("unusable address")?;
    let auth2 = rest2.split(['/', '?']).next().unwrap_or("");
    let host = crate::radio::host_of(&out);
    if auth2.contains('@') || host.is_empty() || host.len() > 253 || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':')) {
        return Err("the playlist has an address with sign-in details or an odd host".into());
    }
    if scheme.eq_ignore_ascii_case("https") && s2.eq_ignore_ascii_case("http") {
        return Err("the playlist steps down from https to http, which is refused".into());
    }
    Ok(out)
}

// ---------------------------------------------------------------- playlists

#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub uri: String,
    pub bandwidth: u64,
    pub codecs: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub uri: String,
    pub duration: f64,
    pub seq: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Media {
    pub target_duration: f64,
    pub media_sequence: u64,
    pub end_list: bool,
    pub segments: Vec<Segment>,
    /// Set when the stream is encrypted (METHOD of the key tag).
    pub encrypted: Option<String>,
    /// Set for fragmented MP4 streams.
    pub map_uri: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Playlist {
    Master { variants: Vec<Variant>, audio_renditions: Vec<String> },
    Media(Media),
}

/// `KEY=value,KEY="va,lue"` attribute lists.
pub fn attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let ks = i;
        while i < b.len() && b[i] != '=' && b[i] != ',' {
            i += 1;
        }
        let key: String = b[ks..i].iter().collect::<String>().trim().to_ascii_uppercase();
        if i >= b.len() || b[i] == ',' {
            i += 1;
            continue;
        }
        i += 1;
        let mut val = String::new();
        if i < b.len() && b[i] == '"' {
            i += 1;
            while i < b.len() && b[i] != '"' {
                val.push(b[i]);
                i += 1;
            }
            i += 1;
        } else {
            while i < b.len() && b[i] != ',' {
                val.push(b[i]);
                i += 1;
            }
        }
        out.push((key, val.trim().to_string()));
        if i < b.len() && b[i] == ',' {
            i += 1;
        }
    }
    out
}

fn attr<'a>(a: &'a [(String, String)], k: &str) -> Option<&'a str> {
    a.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

pub fn parse_playlist(text: &str) -> Result<Playlist, String> {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if lines.next() != Some("#EXTM3U") {
        return Err("this is not an HLS playlist".into());
    }
    let (mut target, mut seq0, mut end, mut enc, mut map) = (0.0f64, 0u64, false, None, None);
    let mut segs: Vec<Segment> = Vec::new();
    let mut variants = Vec::new();
    let mut renditions = Vec::new();
    let (mut dur, mut pending_variant): (Option<f64>, Option<(u64, String)>) = (None, None);
    for l in lines {
        if let Some(rest) = l.strip_prefix("#EXT-X-TARGETDURATION:") {
            target = rest.trim().parse().unwrap_or(0.0);
        } else if let Some(rest) = l.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            seq0 = rest.trim().parse().unwrap_or(0);
        } else if l == "#EXT-X-ENDLIST" {
            end = true;
        } else if let Some(rest) = l.strip_prefix("#EXT-X-KEY:") {
            let a = attrs(rest);
            let m = attr(&a, "METHOD").unwrap_or("NONE");
            if m != "NONE" {
                enc = Some(m.to_string());
            }
        } else if let Some(rest) = l.strip_prefix("#EXT-X-MAP:") {
            map = attr(&attrs(rest), "URI").map(str::to_string).or(Some(String::new()));
        } else if let Some(rest) = l.strip_prefix("#EXTINF:") {
            dur = Some(rest.split(',').next().unwrap_or("0").trim().parse().unwrap_or(0.0));
        } else if let Some(rest) = l.strip_prefix("#EXT-X-STREAM-INF:") {
            let a = attrs(rest);
            pending_variant = Some((attr(&a, "BANDWIDTH").and_then(|v| v.parse().ok()).unwrap_or(0), attr(&a, "CODECS").unwrap_or("").to_string()));
        } else if let Some(rest) = l.strip_prefix("#EXT-X-MEDIA:") {
            let a = attrs(rest);
            if attr(&a, "TYPE") == Some("AUDIO") {
                if let Some(u) = attr(&a, "URI") {
                    renditions.push(u.to_string());
                }
            }
        } else if !l.starts_with('#') {
            if let Some((bandwidth, codecs)) = pending_variant.take() {
                variants.push(Variant { uri: l.to_string(), bandwidth, codecs });
            } else if let Some(d) = dur.take() {
                if segs.len() >= MAX_SEGMENTS_PER_PLAYLIST {
                    return Err("the playlist lists too many segments".into());
                }
                let seq = seq0 + segs.len() as u64;
                segs.push(Segment { uri: l.to_string(), duration: d, seq });
            }
        }
    }
    if !variants.is_empty() || (!renditions.is_empty() && segs.is_empty()) {
        return Ok(Playlist::Master { variants, audio_renditions: renditions });
    }
    Ok(Playlist::Media(Media { target_duration: target, media_sequence: seq0, end_list: end, segments: segs, encrypted: enc, map_uri: map }))
}

/// The variant to play: audio only first (no video codec), then one the radio decodes without an add-on (MP3), then the
/// lowest bitrate (least to download; it is a radio). Falls back to an audio rendition.
pub fn choose_variant(variants: &[Variant], renditions: &[String]) -> Option<String> {
    let video = |v: &Variant| ["avc1", "hvc1", "hev1", "vp09", "av01"].iter().any(|c| v.codecs.contains(c));
    let mp3 = |v: &Variant| v.codecs.contains("mp4a.40.34") || v.codecs.contains("mp3");
    let best = variants.iter().min_by_key(|v| (video(v), !mp3(v), v.bandwidth));
    match best {
        Some(v) if !video(v) => Some(v.uri.clone()),
        _ => renditions.first().cloned().or_else(|| best.map(|v| v.uri.clone())),
    }
}

// ---------------------------------------------------------------- transport stream

fn ts_section(payload: &[u8], pusi: bool) -> Option<&[u8]> {
    if !pusi {
        return None;
    }
    let ptr = *payload.first()? as usize;
    let sec = payload.get(1 + ptr..)?;
    let len = ((*sec.get(1)? as usize & 0x0F) << 8) | *sec.get(2)? as usize;
    sec.get(..(3 + len).min(sec.len()))
}

/// The audio of an MPEG transport stream segment: which format, and its elementary stream bytes.
pub fn ts_audio(data: &[u8]) -> Result<(Codec, Vec<u8>), String> {
    let (mut pmt_pid, mut audio): (Option<u16>, Option<(u16, Codec)>) = (None, None);
    let mut unsupported = false;
    let mut out = Vec::with_capacity(data.len() / 2);
    for pkt in data.chunks_exact(188) {
        if pkt[0] != 0x47 {
            return Err("the segment is not a clean transport stream".into());
        }
        let pusi = pkt[1] & 0x40 != 0;
        let pid = ((pkt[1] as u16 & 0x1F) << 8) | pkt[2] as u16;
        if pkt[3] >> 6 != 0 {
            return Err("the segment is scrambled (encrypted), which is not supported".into());
        }
        let afc = (pkt[3] >> 4) & 3;
        let mut off = 4usize;
        if afc & 2 != 0 {
            off += 1 + pkt[4] as usize;
        }
        if afc & 1 == 0 {
            continue;
        }
        let Some(payload) = pkt.get(off..) else { continue };
        if pid == 0 && pmt_pid.is_none() {
            if let Some(sec) = ts_section(payload, pusi).filter(|s| s.first() == Some(&0)) {
                let mut i = 8;
                while i + 4 <= sec.len().saturating_sub(4) {
                    let prog = ((sec[i] as u16) << 8) | sec[i + 1] as u16;
                    if prog != 0 {
                        pmt_pid = Some(((sec[i + 2] as u16 & 0x1F) << 8) | sec[i + 3] as u16);
                        break;
                    }
                    i += 4;
                }
            }
        } else if Some(pid) == pmt_pid && audio.is_none() {
            if let Some(sec) = ts_section(payload, pusi).filter(|s| s.first() == Some(&2) && s.len() >= 16) {
                let pil = ((sec[10] as usize & 0x0F) << 8) | sec[11] as usize;
                let mut i = 12 + pil;
                while i + 5 <= sec.len().saturating_sub(4) {
                    let (ty, epid) = (sec[i], ((sec[i + 1] as u16 & 0x1F) << 8) | sec[i + 2] as u16);
                    match ty {
                        0x0F if audio.is_none() => audio = Some((epid, Codec::Aac)),
                        0x03 | 0x04 if audio.is_none() => audio = Some((epid, Codec::Mp3)),
                        0x11 | 0x81 | 0x87 => unsupported = true,
                        _ => {}
                    }
                    i += 5 + (((sec[i + 3] as usize & 0x0F) << 8) | sec[i + 4] as usize);
                }
            }
        } else if audio.is_some_and(|(p, _)| p == pid) {
            let mut p = payload;
            if pusi && p.len() >= 9 && p[..3] == [0, 0, 1] {
                p = p.get(9 + p[8] as usize..).unwrap_or(&[]);
            }
            out.extend_from_slice(p);
        }
    }
    match audio {
        Some((_, c)) => Ok((c, out)),
        None if unsupported => Err("the segment carries audio in a format that is not supported".into()),
        None => Err("the segment has no audio".into()),
    }
}

/// A segment's audio, whether it is a transport stream or a bare MP3/ADTS file (ID3 tags skipped).
pub fn segment_audio(bytes: &[u8]) -> Result<(Codec, Vec<u8>), SegError> {
    if bytes.len() >= 189 && bytes[0] == 0x47 && bytes[188] == 0x47 {
        return ts_audio(bytes).map_err(|m| SegError { fatal: m_is_fatal(&m), msg: m });
    }
    if bytes.len() >= 12 && ["ftyp", "styp", "moof"].iter().any(|t| &bytes[4..8] == t.as_bytes()) {
        return Err(SegError { msg: "this station uses fragmented MP4 segments, which are not supported".into(), fatal: true });
    }
    let body = skip_id3(bytes).ok_or(SegError { msg: "a segment ended inside its tag".into(), fatal: false })?;
    match sniff(body) {
        Some(c) => Ok((c, body.to_vec())),
        None => Err(SegError { msg: "a segment holds no audio the radio knows".into(), fatal: false }),
    }
}

fn m_is_fatal(m: &str) -> bool {
    m.contains("scrambled") || m.contains("not supported")
}

#[derive(Debug)]
pub struct SegError {
    pub msg: String,
    pub fatal: bool,
}

// ---------------------------------------------------------------- fetching and following

enum FetchErr {
    Stopped,
    /// Never retried: the host policy, a missing file, a size limit.
    Final(String),
    Retry(String),
}

impl FetchErr {
    fn text(&self) -> String {
        match self {
            FetchErr::Stopped => "stopped".into(),
            FetchErr::Final(s) | FetchErr::Retry(s) => s.clone(),
        }
    }
}

/// Timing, scaled down by tests.
#[derive(Clone, Copy)]
pub struct Timing {
    /// Divides every wait (1 in real use).
    pub scale: u32,
}

#[derive(Clone)]
pub struct Net {
    pub streamer: Arc<dyn Streamer>,
    pub allow: Allow,
    pub stop: Stop,
    pub timing: Timing,
}

impl Net {
    fn wait(&self, d: Duration) {
        let d = d / self.timing.scale.max(1);
        let t0 = Instant::now();
        while t0.elapsed() < d && !(self.stop)() {
            std::thread::sleep(Duration::from_millis(5).min(d));
        }
    }

    /// One address, redirects followed (each hop checked against the policy), at most `max` bytes. Returns the bytes and
    /// the final address.
    fn get(&self, url: &str, max: usize) -> Result<(Vec<u8>, String), FetchErr> {
        let mut url = url.to_string();
        for _ in 0..6 {
            if (self.stop)() {
                return Err(FetchErr::Stopped);
            }
            let host = crate::radio::host_of(&url);
            if !(self.allow)(&host) {
                return Err(FetchErr::Final(format!("your account may not reach {host}")));
            }
            let head = self.streamer.open(&url, &[("Accept".into(), "*/*".into())]).map_err(|e| FetchErr::Retry(format!("could not connect: {e}")))?;
            if (300..400).contains(&head.status) {
                let loc = head.header("location").ok_or_else(|| FetchErr::Final("a redirect without a place to go".into()))?;
                url = resolve_ref_redirect(&url, loc).map_err(FetchErr::Final)?;
                continue;
            }
            if head.status == 404 || head.status == 410 || head.status == 403 {
                return Err(FetchErr::Final(format!("the station answered {}", head.status)));
            }
            if head.status >= 400 {
                return Err(FetchErr::Retry(format!("the station answered {}", head.status)));
            }
            let mut body = Vec::new();
            head.body.take(max as u64 + 1).read_to_end(&mut body).map_err(|e| FetchErr::Retry(format!("the download broke off: {e}")))?;
            if body.len() > max {
                return Err(FetchErr::Final(format!("a download was larger than {} KB, which is refused", max / 1024)));
            }
            return Ok((body, url));
        }
        Err(FetchErr::Final("too many redirects".into()))
    }
}

/// A redirect may go from https to http (the radio's plain streams do); only the playlist's own references may not.
fn resolve_ref_redirect(base: &str, loc: &str) -> Result<String, String> {
    let l = loc.trim();
    if l.starts_with("http://") {
        return resolve_ref("http://x.invalid/", l);
    }
    resolve_ref(base, l)
}

struct Queued {
    url: String,
    tries: u32,
}

/// Follows a playlist and yields the audio of its segments as one stream. Ends when the playlist ends or when the
/// station stops answering (an error the radio turns into a reconnect).
pub struct HlsReader {
    net: Net,
    media_url: String,
    queue: VecDeque<Queued>,
    last_seq: Option<u64>,
    buf: Vec<u8>,
    pos: usize,
    target: f64,
    next_reload: Instant,
    last_new: Instant,
    ended: bool,
    fails: u32,
    codec: Codec,
}

fn io_err(m: impl Into<String>) -> std::io::Error {
    std::io::Error::other(m.into())
}

/// Whether text is an HLS playlist (and not a plain `.m3u` list of stream addresses).
pub fn is_hls(text: &str) -> bool {
    text.trim_start_matches('\u{feff}').trim_start().starts_with("#EXTM3U") && text.contains("#EXT-X-")
}

impl HlsReader {
    /// Reads the playlist (picking the variant of a master playlist), queues the last few segments and fetches the first,
    /// which tells the audio format.
    pub fn start(net: Net, url: &str, first_text: Option<&[u8]>) -> Result<HlsReader, String> {
        let mut cur = url.to_string();
        let mut pre = first_text.map(|b| b.to_vec());
        for _ in 0..3 {
            let (body, final_url) = match pre.take() {
                Some(b) => (b, cur.clone()),
                None => net.get(&cur, MAX_PLAYLIST).map_err(|e| e.text())?,
            };
            let text = String::from_utf8_lossy(&body).to_string();
            match parse_playlist(&text)? {
                Playlist::Master { variants, audio_renditions } => {
                    let pick = choose_variant(&variants, &audio_renditions).ok_or("the playlist offers no stream")?;
                    cur = resolve_ref(&final_url, &pick)?;
                }
                Playlist::Media(m) => {
                    let mut r = HlsReader { net, media_url: final_url.clone(), queue: VecDeque::new(), last_seq: None, buf: Vec::new(), pos: 0, target: m.target_duration.clamp(1.0, 60.0), next_reload: Instant::now(), last_new: Instant::now(), ended: false, fails: 0, codec: Codec::Mp3 };
                    r.take_playlist(&m, true)?;
                    // the first segment decides the format; a stream that cannot be played is reported now
                    loop {
                        if r.buf.len() > r.pos {
                            return Ok(r);
                        }
                        if !r.fill_once().map_err(|e| e.to_string())? {
                            return Err("the playlist has no audio segments".into());
                        }
                    }
                }
            }
        }
        Err("the playlist points to playlists too many times".into())
    }

    pub fn codec(&self) -> Codec {
        self.codec
    }

    /// Queues the new segments of a media playlist.
    fn take_playlist(&mut self, m: &Media, first: bool) -> Result<(), String> {
        if let Some(e) = &m.encrypted {
            return Err(format!("this station's audio is encrypted ({e}), which is not supported"));
        }
        if m.map_uri.is_some() {
            return Err("this station uses fragmented MP4 segments, which are not supported".into());
        }
        let top = m.segments.last().map(|s| s.seq);
        // a playlist that starts over (the station restarted): begin again with its newest segments
        if let (Some(last), Some(top)) = (self.last_seq, top) {
            if top + 1 < last {
                self.last_seq = None;
            }
        }
        let mut fresh: Vec<&Segment> = m.segments.iter().filter(|s| self.last_seq.is_none_or(|l| s.seq > l)).collect();
        if self.last_seq.is_none() && !m.end_list && fresh.len() > STARTUP_SEGMENTS {
            fresh.drain(..fresh.len() - STARTUP_SEGMENTS);
        }
        let _ = first;
        for s in &fresh {
            let url = resolve_ref(&self.media_url, &s.uri)?;
            let host = crate::radio::host_of(&url);
            if !(self.net.allow)(&host) {
                return Err(format!("your account may not reach {host}"));
            }
            self.queue.push_back(Queued { url, tries: 0 });
        }
        if let Some(l) = fresh.last() {
            self.last_seq = Some(l.seq);
            self.last_new = Instant::now();
        }
        self.ended = m.end_list;
        self.target = m.target_duration.clamp(1.0, 60.0);
        let wait = if fresh.is_empty() { self.target / 2.0 } else { self.target };
        self.next_reload = Instant::now() + Duration::from_secs_f64(wait) / self.net.timing.scale.max(1);
        Ok(())
    }

    /// Fetches one more segment into the buffer. `Ok(false)` at the end of a finished playlist.
    fn fill_once(&mut self) -> Result<bool, std::io::Error> {
        loop {
            if (self.net.stop)() {
                return Err(io_err("stopped"));
            }
            if let Some(mut q) = self.queue.pop_front() {
                match self.net.get(&q.url, MAX_SEGMENT) {
                    Ok((bytes, _)) => match segment_audio(&bytes) {
                        Ok((codec, es)) => {
                            self.fails = 0;
                            self.codec = codec;
                            if es.is_empty() {
                                continue;
                            }
                            self.buf = es;
                            self.pos = 0;
                            return Ok(true);
                        }
                        Err(e) if e.fatal => return Err(io_err(e.msg)),
                        Err(_) => self.note_failure("a segment could not be read")?,
                    },
                    Err(FetchErr::Stopped) => return Err(io_err("stopped")),
                    Err(FetchErr::Final(m)) if m.contains("may not reach") => return Err(io_err(m)),
                    Err(FetchErr::Final(_)) => self.note_failure("a segment is gone")?,
                    Err(FetchErr::Retry(m)) => {
                        q.tries += 1;
                        if q.tries < 3 {
                            self.queue.push_front(q);
                        }
                        self.note_failure(&m)?;
                    }
                }
                continue;
            }
            if self.ended {
                return Ok(false);
            }
            // nothing queued: wait for the playlist to be due, then read it again
            while Instant::now() < self.next_reload {
                if (self.net.stop)() {
                    return Err(io_err("stopped"));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            if self.last_new.elapsed() > Duration::from_secs_f64((self.target * 6.0).max(45.0)) / self.net.timing.scale.max(1) {
                return Err(io_err("the playlist stopped moving"));
            }
            match self.net.get(&self.media_url.clone(), MAX_PLAYLIST) {
                Ok((body, _)) => {
                    let text = String::from_utf8_lossy(&body).to_string();
                    match parse_playlist(&text) {
                        Ok(Playlist::Media(m)) => {
                            self.fails = 0;
                            self.take_playlist(&m, false).map_err(io_err)?;
                        }
                        _ => self.note_failure("the playlist could not be read")?,
                    }
                }
                Err(FetchErr::Stopped) => return Err(io_err("stopped")),
                Err(FetchErr::Final(m)) if m.contains("may not reach") => return Err(io_err(m)),
                Err(e) => self.note_failure(&e.text())?,
            }
        }
    }

    fn note_failure(&mut self, why: &str) -> Result<(), std::io::Error> {
        self.fails += 1;
        if self.fails >= MAX_FAILS {
            return Err(io_err(format!("the station keeps failing ({why})")));
        }
        self.net.wait(Duration::from_secs(2) * self.fails.min(4));
        self.next_reload = Instant::now();
        Ok(())
    }
}

impl Read for HlsReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pos >= self.buf.len() {
            if !self.fill_once()? {
                return Ok(0);
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

#[cfg(test)]
pub(crate) mod testkit {
    use iyakki_proxy::{StreamHead, Streamer};
    use std::sync::Mutex;

    pub type Route = Box<dyn Fn(usize) -> Result<(u16, Vec<(String, String)>, Vec<u8>), String> + Send + Sync>;

    /// A network that answers from a table; `n` is how many times the address has been asked for (1 the first time).
    pub struct FakeNet {
        pub asked: Mutex<Vec<String>>,
        pub routes: Mutex<std::collections::HashMap<String, Route>>,
    }

    impl Streamer for FakeNet {
        fn open(&self, url: &str, _headers: &[(String, String)]) -> Result<StreamHead, String> {
            let n = {
                let mut a = self.asked.lock().unwrap();
                a.push(url.to_string());
                a.iter().filter(|u| *u == url).count()
            };
            let routes = self.routes.lock().unwrap();
            let (status, headers, body) = routes.get(url).ok_or("connection refused")?(n)?;
            Ok(StreamHead { status, headers, body: Box::new(std::io::Cursor::new(body)) })
        }
    }

    pub fn fake_net(routes: Vec<(&str, Route)>) -> std::sync::Arc<FakeNet> {
        std::sync::Arc::new(FakeNet { asked: Mutex::new(vec![]), routes: Mutex::new(routes.into_iter().map(|(u, r)| (u.to_string(), r)).collect()) })
    }

    pub fn ok(body: Vec<u8>) -> Route {
        Box::new(move |_| Ok((200, vec![], body.clone())))
    }

    /// A live media playlist listing segments `first..first+count` named `{base}seg{N}.ts`.
    pub fn live_playlist(first: u64, count: u64, extra: &str) -> String {
        let mut t = format!("#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:{first}\n{extra}");
        for i in first..first + count {
            t.push_str(&format!("#EXTINF:6.0,\nseg{i}.ts\n"));
        }
        t
    }

    /// A transport stream segment holding `es` as one audio stream of `stream_type` (0x0F AAC, 0x03 MP3), PES-wrapped.
    pub fn build_ts(stream_type: u8, es: &[u8]) -> Vec<u8> {
        fn pkt(pid: u16, pusi: bool, payload: &[u8]) -> Vec<u8> {
            let mut p = vec![0x47, (if pusi { 0x40 } else { 0 }) | (pid >> 8) as u8, pid as u8];
            if payload.len() >= 184 {
                p.push(0x10);
                p.extend_from_slice(&payload[..184]);
            } else {
                p.push(0x30);
                let af = 183 - payload.len();
                p.push(af as u8);
                if af > 0 {
                    p.push(0);
                    p.extend(std::iter::repeat_n(0xFF, af - 1));
                }
                p.extend_from_slice(payload);
            }
            assert_eq!(p.len(), 188);
            p
        }
        let pat = [0u8, 0, 0xB0, 13, 0, 1, 0xC1, 0, 0, 0, 1, 0xE1, 0x00, 0, 0, 0, 0];
        let pmt = [0u8, 2, 0xB0, 18, 0, 1, 0xC1, 0, 0, 0xE1, 0x01, 0xF0, 0, stream_type, 0xE1, 0x01, 0xF0, 0, 0, 0, 0, 0];
        let mut out = pkt(0, true, &pat);
        out.extend(pkt(0x100, true, &pmt));
        let mut pes = vec![0, 0, 1, 0xC0, 0, 0, 0x80, 0x00, 0];
        pes.extend_from_slice(es);
        for (i, c) in pes.chunks(184).enumerate() {
            out.extend(pkt(0x101, i == 0, c));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::*;
    use super::*;
    use crate::audiofmt::tests::adts_frame;

    fn net(f: Arc<FakeNet>, allow: impl Fn(&str) -> bool + Send + Sync + 'static) -> Net {
        Net { streamer: f, allow: Arc::new(allow), stop: Arc::new(|| false), timing: Timing { scale: 2000 } }
    }

    fn mp3(n: usize) -> Vec<u8> {
        let mut v = Vec::new();
        for _ in 0..n {
            let mut f = vec![0u8; 417];
            f[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
            v.extend(f);
        }
        v
    }

    fn adts(n: usize) -> Vec<u8> {
        (0..n).flat_map(|_| adts_frame(120)).collect()
    }

    #[test]
    fn references_are_resolved_safely() {
        let b = "https://cdn.example.org/live/a/index.m3u8?token=1";
        assert_eq!(resolve_ref(b, "seg1.ts").unwrap(), "https://cdn.example.org/live/a/seg1.ts");
        assert_eq!(resolve_ref(b, "../b/seg1.ts?x=2").unwrap(), "https://cdn.example.org/live/b/seg1.ts?x=2");
        assert_eq!(resolve_ref(b, "/root/seg.ts").unwrap(), "https://cdn.example.org/root/seg.ts");
        assert_eq!(resolve_ref(b, "../../../../../x.ts").unwrap(), "https://cdn.example.org/x.ts", "cannot climb above the root");
        assert_eq!(resolve_ref(b, "//other.example.org/s.ts").unwrap(), "https://other.example.org/s.ts");
        assert_eq!(resolve_ref(b, "https://o.example.org:8443/s.ts#frag").unwrap(), "https://o.example.org:8443/s.ts");
        assert_eq!(resolve_ref(b, "?part=2").unwrap(), "https://cdn.example.org/live/a/index.m3u8?part=2");
        assert!(resolve_ref(b, "http://o.example.org/s.ts").unwrap_err().contains("https to http"));
        assert!(resolve_ref("http://a.org/p.m3u8", "https://o.example.org/s.ts").is_ok(), "stepping up is fine");
        for bad in ["", "javascript:alert(1)", "file:///etc/passwd", "ftp://x.org/a", "https://u:p@o.example.org/s.ts", "a b.ts", "https://bad_host.org/x"] {
            assert!(resolve_ref(b, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn playlists_are_parsed_and_the_variant_is_chosen() {
        let t = live_playlist(100, 3, "#EXT-X-KEY:METHOD=NONE\n");
        let Playlist::Media(m) = parse_playlist(&t).unwrap() else { panic!() };
        assert_eq!((m.media_sequence, m.target_duration, m.end_list, m.encrypted.is_none()), (100, 6.0, false, true));
        assert_eq!(m.segments.iter().map(|s| (s.seq, s.uri.as_str())).collect::<Vec<_>>(), vec![(100, "seg100.ts"), (101, "seg101.ts"), (102, "seg102.ts")]);
        let Playlist::Media(m) = parse_playlist(&live_playlist(0, 1, "#EXT-X-KEY:METHOD=AES-128,URI=\"k\"\n#EXT-X-ENDLIST\n")).unwrap() else { panic!() };
        assert!(m.encrypted.as_deref() == Some("AES-128") && m.end_list);
        let master = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=900000,CODECS=\"avc1.4d401f,mp4a.40.2\"\nvideo.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=128000,CODECS=\"mp4a.40.2\"\naac128.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=64000,CODECS=\"mp4a.40.2\"\naac64.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=192000,CODECS=\"mp4a.40.34\"\nmp3.m3u8\n";
        let Playlist::Master { variants, audio_renditions } = parse_playlist(master).unwrap() else { panic!() };
        assert_eq!(variants.len(), 4);
        assert_eq!(choose_variant(&variants, &audio_renditions).as_deref(), Some("mp3.m3u8"), "an MP3 variant needs no add-on");
        assert_eq!(choose_variant(&variants[..3], &[]).as_deref(), Some("aac64.m3u8"), "audio only, lowest bitrate");
        assert_eq!(choose_variant(&variants[..1], &["audio.m3u8".into()]).as_deref(), Some("audio.m3u8"), "a video-only list falls back to an audio rendition");
        assert!(parse_playlist("<html>nope").is_err());
        assert_eq!(attrs("A=1,B=\"x,y\",C=z").iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>(), vec!["A=1", "B=x,y", "C=z"]);
        assert!(is_hls("#EXTM3U\n#EXT-X-TARGETDURATION:6\n") && !is_hls("#EXTM3U\n#EXTINF:-1,x\nhttp://a/b\n"));
        // too many segments are refused
        let mut big = String::from("#EXTM3U\n#EXT-X-TARGETDURATION:6\n");
        for i in 0..5100 {
            big.push_str(&format!("#EXTINF:1,\ns{i}.ts\n"));
        }
        assert!(parse_playlist(&big).is_err());
    }

    #[test]
    fn transport_streams_give_up_their_audio_and_corrupt_ones_do_not_panic() {
        let es = mp3(8);
        let (c, got) = ts_audio(&testkit::build_ts(0x03, &es)).unwrap();
        assert_eq!((c, got), (Codec::Mp3, es.clone()));
        let a = adts(40);
        let (c, got) = ts_audio(&testkit::build_ts(0x0F, &a)).unwrap();
        assert_eq!((c, got), (Codec::Aac, a.clone()));
        assert_eq!(segment_audio(&testkit::build_ts(0x0F, &a)).unwrap().0, Codec::Aac);
        assert!(ts_audio(&testkit::build_ts(0x1B, &es)).unwrap_err().contains("no audio"));
        assert!(ts_audio(&testkit::build_ts(0x11, &es)).unwrap_err().contains("not supported"));
        let mut scrambled = testkit::build_ts(0x0F, &a);
        scrambled[188 * 2 + 3] |= 0x80;
        assert!(ts_audio(&scrambled).unwrap_err().contains("scrambled"));
        // raw files, with an ID3 tag in front
        let mut tagged = b"ID3\x04\x00\x00\x00\x00\x00\x04abcd".to_vec();
        tagged.extend(mp3(3));
        assert_eq!(segment_audio(&tagged).unwrap().0, Codec::Mp3);
        assert!(segment_audio(b"<html>not audio</html>").is_err());
        let mut fmp4 = vec![0, 0, 0, 24];
        fmp4.extend(b"ftypiso5\0\0\0\0iso5iso6");
        assert!(segment_audio(&fmp4).unwrap_err().fatal);
        // damage: random flips and every truncation
        let good = testkit::build_ts(0x0F, &a);
        let mut x = 99u32;
        for _ in 0..300 {
            let mut b = good.clone();
            for _ in 0..8 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let i = x as usize % b.len();
                b[i] = (x >> 9) as u8;
            }
            let _ = segment_audio(&b);
        }
        for n in (0..good.len()).step_by(37) {
            let _ = segment_audio(&good[..n]);
        }
    }

    fn segs(range: std::ops::Range<u64>, base: &str) -> Vec<(String, Route)> {
        range.map(|i| (format!("{base}seg{i}.ts"), ok(testkit::build_ts(0x03, &mp3(2 + (i % 2) as usize))))).collect()
    }

    fn read_all(r: &mut HlsReader) -> (Vec<u8>, Option<String>) {
        let mut out = Vec::new();
        let mut b = vec![0u8; 4096];
        loop {
            match r.read(&mut b) {
                Ok(0) => return (out, None),
                Ok(n) => out.extend_from_slice(&b[..n]),
                Err(e) => return (out, Some(e.to_string())),
            }
        }
    }

    #[test]
    fn a_finished_playlist_is_played_through_in_order_and_starts_on_the_audio_variant() {
        let base = "https://cdn.example.org/radio/";
        let mut routes: Vec<(String, Route)> = segs(0..4, &format!("{base}aud/"));
        routes.push((format!("{base}master.m3u8"), ok(format!("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=64000,CODECS=\"mp4a.40.34\"\naud/p.m3u8\n").into_bytes())));
        routes.push((format!("{base}aud/p.m3u8"), ok(live_playlist(0, 4, "#EXT-X-ENDLIST\n").into_bytes())));
        let f = fake_net(routes.iter().map(|(u, _)| (u.as_str(), Box::new(|_| Err("x".to_string())) as Route)).collect());
        // rebuild with the real routes (Route is not Clone)
        let mut table = f.routes.lock().unwrap();
        table.clear();
        for (u, r) in routes {
            table.insert(u, r);
        }
        drop(table);
        let mut r = HlsReader::start(net(f.clone(), |_| true), &format!("{base}master.m3u8"), None).unwrap();
        assert_eq!(r.codec(), Codec::Mp3);
        let (all, err) = read_all(&mut r);
        assert!(err.is_none());
        // a VOD list plays from its first segment: sizes 2,3,2,3 frames
        assert_eq!(all.len(), 417 * (2 + 3 + 2 + 3));
        let asked = f.asked.lock().unwrap().clone();
        assert_eq!(asked[..2], [format!("{base}master.m3u8"), format!("{base}aud/p.m3u8")]);
        assert_eq!(asked[2..].iter().filter(|u| u.ends_with(".ts")).count(), 4);
    }

    #[test]
    fn a_live_playlist_starts_three_back_follows_new_segments_once_and_survives_hiccups() {
        let base = "https://cdn.example.org/live/";
        let f = fake_net(vec![]);
        {
            let mut t = f.routes.lock().unwrap();
            // the playlist grows: the first look has 0..5, the second 0..6, then 1..8 with a failed answer between
            t.insert(format!("{base}p.m3u8"), Box::new(|n| match n {
                1 => Ok((200, vec![], live_playlist(0, 5, "").into_bytes())),
                2 => Ok((503, vec![], vec![])),
                3 => Ok((200, vec![], live_playlist(0, 6, "").into_bytes())),
                _ => Ok((200, vec![], live_playlist(2, 6, "#EXT-X-ENDLIST\n").into_bytes())),
            }));
            for i in 0..8u64 {
                t.insert(format!("{base}seg{i}.ts"), Box::new(move |n| if i == 3 && n == 1 { Ok((500, vec![], vec![])) } else { Ok((200, vec![], testkit::build_ts(0x03, &mp3(1 + i as usize % 3)))) }));
            }
        }
        let mut r = HlsReader::start(net(f.clone(), |_| true), &format!("{base}p.m3u8"), None).unwrap();
        let (all, err) = read_all(&mut r);
        assert!(err.is_none(), "{err:?}");
        let asked = f.asked.lock().unwrap().clone();
        let ts: Vec<&str> = asked.iter().filter(|u| u.ends_with(".ts")).map(|u| u.rsplit('/').next().unwrap()).collect();
        // started at 2 (the last three of 0..5 are 2,3,4), 3 was retried once, 5 arrived later, 6 and 7 after the restart-less growth
        assert_eq!(ts, vec!["seg2.ts", "seg3.ts", "seg3.ts", "seg4.ts", "seg5.ts", "seg6.ts", "seg7.ts"], "{ts:?}");
        let frames = |i: usize| 1 + i % 3;
        assert_eq!(all.len(), 417 * [2, 3, 4, 5, 6, 7].iter().map(|i| frames(*i)).sum::<usize>(), "each segment once, in order");
    }

    #[test]
    fn hosts_must_be_allowed_encrypted_and_oversized_streams_are_refused_and_a_stalled_one_ends() {
        let base = "https://cdn.example.org/s/";
        // a segment on a host the account may not reach
        let f = fake_net(vec![(&format!("{base}p.m3u8"), ok(b"#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6,\nhttps://evil.example.org/seg.ts\n".to_vec()))]);
        let e = HlsReader::start(net(f.clone(), |h| h == "cdn.example.org"), &format!("{base}p.m3u8"), None).err().unwrap();
        assert!(e.contains("may not reach evil.example.org"), "{e}");
        assert!(!f.asked.lock().unwrap().iter().any(|u| u.contains("evil")));
        // the playlist itself, and a redirect, are checked too
        let f = fake_net(vec![(&format!("{base}go.m3u8"), Box::new(|_| Ok((302, vec![("location".into(), "https://evil.example.org/p.m3u8".into())], vec![]))))]);
        let e = HlsReader::start(net(f.clone(), |h| h == "cdn.example.org"), &format!("{base}go.m3u8"), None).err().unwrap();
        assert!(e.contains("may not reach evil.example.org"), "{e}");
        // encryption
        for method in ["AES-128", "SAMPLE-AES"] {
            let f = fake_net(vec![(&format!("{base}e.m3u8"), ok(live_playlist(0, 2, &format!("#EXT-X-KEY:METHOD={method},URI=\"k.bin\"\n")).into_bytes()))]);
            let e = HlsReader::start(net(f, |_| true), &format!("{base}e.m3u8"), None).err().unwrap();
            assert!(e.contains("encrypted") && e.contains(method), "{e}");
        }
        // a segment larger than the limit
        let f = fake_net(vec![(&format!("{base}b.m3u8"), ok(live_playlist(0, 1, "").into_bytes())), (&format!("{base}seg0.ts"), ok(vec![0x47; MAX_SEGMENT + 10]))]);
        let e = HlsReader::start(net(f, |_| true), &format!("{base}b.m3u8"), None).err().unwrap_or_default();
        assert!(e.contains("stopped moving") || e.contains("keeps failing") || e.contains("larger"), "{e}");
        // a playlist that never moves ends the stream with an error (the radio then reconnects)
        let f = fake_net(vec![(&format!("{base}m.m3u8"), ok(live_playlist(0, 1, "").into_bytes())), (&format!("{base}seg0.ts"), ok(testkit::build_ts(0x03, &mp3(1))))]);
        let mut r = HlsReader::start(net(f, |_| true), &format!("{base}m.m3u8"), None).unwrap();
        let (all, err) = read_all(&mut r);
        assert_eq!(all.len(), 417);
        assert!(err.unwrap().contains("stopped moving"));
        // not a playlist
        let f = fake_net(vec![(&format!("{base}x.m3u8"), ok(b"<html>".to_vec()))]);
        assert!(HlsReader::start(net(f, |_| true), &format!("{base}x.m3u8"), None).is_err());
    }
}
