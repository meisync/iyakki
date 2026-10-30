//! The media library: which music and video files a person has in the folders their account may read (plus the built-in,
//! read-only sample folder), what they are called (tags where reading them is cheap, the file name otherwise), playlists kept per
//! person, and the shared player. MP3 plays inside IYAKKI itself (the radio player's in-house decoder, see [`SystemLauncher`]);
//! everything else (video, other audio formats, playlist files) is handed to an external player program (mpv, VLC, ffplay or
//! mplayer) started as a separate process by [`crate::voice`] and stopped again by killing its process group. A video on the
//! `web` image is played by the MEDIA app through the embedded web engine ([`crate::mediaserve`]).

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

pub const MAX_FILES: usize = 20_000;
pub const MAX_PLAYLISTS: usize = 100;
pub const MAX_ITEMS: usize = 5_000;
const MAX_DEPTH: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Audio,
    Video,
}

/// Extensions we list, with the kind each means.
pub const EXTENSIONS: [(&str, Kind); 10] = [
    ("mp3", Kind::Audio),
    ("flac", Kind::Audio),
    ("ogg", Kind::Audio),
    ("opus", Kind::Audio),
    ("wav", Kind::Audio),
    ("m4a", Kind::Audio),
    ("mp4", Kind::Video),
    ("mkv", Kind::Video),
    ("webm", Kind::Video),
    ("avi", Kind::Video),
];

/// The container named by the first bytes of a file, as one of the extensions above, and its kind.
pub fn sniff(head: &[u8]) -> Option<(&'static str, Kind)> {
    // an ID3 tag, or a valid MPEG audio frame header (sync, known version and layer, usable bitrate and rate)
    let frame = head.len() > 2 && head[0] == 0xFF && head[1] & 0xE0 == 0xE0 && (head[1] >> 3) & 3 != 1 && (head[1] >> 1) & 3 != 0 && head[2] >> 4 != 15 && (head[2] >> 2) & 3 != 3;
    if head.starts_with(b"ID3") || frame {
        return Some(("mp3", Kind::Audio));
    }
    if head.starts_with(b"fLaC") {
        return Some(("flac", Kind::Audio));
    }
    if head.starts_with(b"OggS") {
        let opus = head.windows(8).take(120).any(|w| w == b"OpusHead");
        return Some((if opus { "opus" } else { "ogg" }, Kind::Audio));
    }
    if head.len() >= 12 && &head[..4] == b"RIFF" {
        return match &head[8..12] {
            b"WAVE" => Some(("wav", Kind::Audio)),
            b"AVI " => Some(("avi", Kind::Video)),
            _ => None,
        };
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        return Some(if &head[8..12] == b"M4A " { ("m4a", Kind::Audio) } else { ("mp4", Kind::Video) });
    }
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        let webm = head.windows(4).take(60).any(|w| w == b"webm");
        return Some((if webm { "webm" } else { "mkv" }, Kind::Video));
    }
    None
}

fn ext_kind(path: &Path) -> Option<Kind> {
    let e = path.extension()?.to_str()?.to_ascii_lowercase();
    EXTENSIONS.iter().find(|(x, _)| *x == e).map(|(_, k)| *k)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub path: String,
    pub kind: Kind,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub seconds: Option<u32>,
    /// The genre tag as written in the file (empty when there is none).
    pub genre: String,
    /// The year from the tags, 0 when unknown.
    pub year: u16,
    /// The file's size in bytes and its modified time (seconds), for noticing changes between scans.
    pub size: u64,
    pub mtime: i64,
}

pub const UNKNOWN_ARTIST: &str = "Unknown artist";
pub const UNKNOWN_ALBUM: &str = "Unknown album";

impl Track {
    pub fn duration_text(&self) -> String {
        match self.seconds {
            Some(s) if s >= 3600 => format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60),
            Some(s) => format!("{}:{:02}", s / 60, s % 60),
            None => String::new(),
        }
    }

    fn matches(&self, q: &str) -> bool {
        let q = q.to_lowercase();
        [&self.title, &self.artist, &self.album, &self.path].iter().any(|f| f.to_lowercase().contains(&q))
    }
}

#[derive(Default, Clone, Debug)]
pub struct Library {
    pub tracks: Vec<Track>,
    /// More files than we list were found.
    pub truncated: bool,
}

impl Library {
    pub fn audio(&self, q: &str) -> Vec<&Track> {
        self.tracks.iter().filter(|t| t.kind == Kind::Audio && (q.is_empty() || t.matches(q))).collect()
    }

    pub fn videos(&self, q: &str) -> Vec<&Track> {
        self.tracks.iter().filter(|t| t.kind == Kind::Video && (q.is_empty() || t.matches(q))).collect()
    }

    /// Artists with their track counts, by name.
    pub fn artists(&self, q: &str) -> Vec<(String, usize)> {
        let mut m: BTreeMap<String, usize> = BTreeMap::new();
        for t in self.audio(q) {
            *m.entry(t.artist.clone()).or_default() += 1;
        }
        m.into_iter().collect()
    }

    /// Albums as `(artist, album, tracks)`.
    pub fn albums(&self, q: &str) -> Vec<(String, String, usize)> {
        let mut m: BTreeMap<(String, String), usize> = BTreeMap::new();
        for t in self.audio(q) {
            *m.entry((t.artist.clone(), t.album.clone())).or_default() += 1;
        }
        m.into_iter().map(|((a, b), n)| (a, b, n)).collect()
    }

    pub fn by_artist(&self, artist: &str) -> Vec<&Track> {
        self.tracks.iter().filter(|t| t.kind == Kind::Audio && t.artist == artist).collect()
    }

    pub fn by_album(&self, artist: &str, album: &str) -> Vec<&Track> {
        self.tracks.iter().filter(|t| t.kind == Kind::Audio && t.artist == artist && t.album == album).collect()
    }

    pub fn find(&self, path: &str) -> Option<&Track> {
        self.tracks.iter().find(|t| t.path == path)
    }
}

/// Walk `folders` (only those `allow` accepts, and only files it accepts) and describe every media file found. Links
/// are not followed, so a link cannot lead out of the allowed folders.
pub fn scan(folders: &[String], allow: &dyn Fn(&str) -> bool) -> Library {
    let mut lib = Library::default();
    let mut seen = std::collections::BTreeSet::new();
    for f in folders {
        let f = f.trim_end_matches('/');
        if f.is_empty() || !allow(f) {
            continue;
        }
        walk(Path::new(f), 0, allow, &mut lib, &mut seen);
        if lib.truncated {
            break;
        }
    }
    lib.tracks.sort_by(|a, b| (a.artist.to_lowercase(), a.album.to_lowercase(), a.path.to_lowercase()).cmp(&(b.artist.to_lowercase(), b.album.to_lowercase(), b.path.to_lowercase())));
    lib
}

fn walk(dir: &Path, depth: usize, allow: &dyn Fn(&str) -> bool, lib: &mut Library, seen: &mut std::collections::BTreeSet<String>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if lib.truncated {
            return;
        }
        let p = e.path();
        let Some(ps) = p.to_str().map(str::to_string) else { continue };
        let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
        if md.is_dir() {
            if allow(&ps) {
                walk(&p, depth + 1, allow, lib, seen);
            }
        } else if md.is_file() && allow(&ps) {
            if let Some(t) = describe_file(&p, &ps) {
                if lib.tracks.len() >= MAX_FILES {
                    lib.truncated = true;
                    return;
                }
                if seen.insert(ps) {
                    lib.tracks.push(t);
                }
            }
        }
    }
}

/// Read one media file's name, tags and length (None when it is not a song or video).
pub fn describe_path(path: &str) -> Option<Track> {
    describe_file(Path::new(path), path)
}

/// What kind of media a file is: the first bytes decide, the extension covers what the first bytes do not show.
fn describe_file(p: &Path, ps: &str) -> Option<Track> {
    // only files named like media, or with no extension at all, are opened
    if ext_kind(p).is_none() && p.extension().is_some() {
        return None;
    }
    let mut f = std::fs::File::open(p).ok()?;
    let mut head = [0u8; 96];
    let n = f.read(&mut head).ok()?;
    // the first bytes decide; a media name whose bytes say nothing we know is still listed under its name
    let (ext, kind) = match sniff(&head[..n]) {
        Some(found) => found,
        None => {
            let e = p.extension()?.to_str()?.to_ascii_lowercase();
            *EXTENSIONS.iter().find(|(x, _)| *x == e)?
        }
    };
    let mut tags = Tags::default();
    let mut seconds = None;
    drop(f);
    if let Ok(mut f) = std::fs::File::open(p) {
        let size = f.metadata().map_or(0, |m| m.len());
        match ext {
            "mp3" => {
                tags = id3_tags(&mut f, size);
                seconds = mp3_seconds(&mut f, size);
            }
            "flac" => {
                let (t, s) = flac_info(&mut f);
                tags = t;
                seconds = s;
            }
            "ogg" | "opus" => tags = ogg_tags(&mut f),
            "m4a" | "mp4" => {
                let (t, s) = mp4_info(&mut f, size);
                tags = t;
                seconds = s;
            }
            "wav" => seconds = wav_seconds(&mut f),
            _ => {}
        }
    }
    let stem = p.file_stem()?.to_str()?.replace('_', " ");
    let (mut title, mut artist) = (tags.title.unwrap_or_default(), tags.artist.unwrap_or_default());
    if title.is_empty() {
        match (stem.split_once(" - "), artist.is_empty()) {
            (Some((a, t)), true) if !a.trim().is_empty() && !t.trim().is_empty() => {
                artist = a.trim().to_string();
                title = t.trim().to_string();
            }
            _ => title = stem.trim().to_string(),
        }
    }
    if title.is_empty() {
        title = ps.rsplit('/').next().unwrap_or(ps).to_string();
    }
    let md = std::fs::metadata(p).ok();
    Some(Track {
        genre: tags.genre.unwrap_or_default(),
        year: tags.year.unwrap_or(0),
        size: md.as_ref().map_or(0, |m| m.len()),
        mtime: md.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64),
        path: ps.to_string(),
        kind,
        title,
        artist: if artist.is_empty() { UNKNOWN_ARTIST.into() } else { artist },
        album: tags.album.filter(|a| !a.is_empty()).unwrap_or_else(|| UNKNOWN_ALBUM.into()),
        seconds,
    })
}

// ------------------------------------------------------------------ tags

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u16>,
}

/// The first four-digit year in a date text such as `2019-05-01`.
fn year_of(s: &str) -> Option<u16> {
    let b = s.as_bytes();
    (0..b.len().saturating_sub(3)).find_map(|i| {
        let w = &b[i..i + 4];
        (w.iter().all(u8::is_ascii_digit) && (i == 0 || !b[i - 1].is_ascii_digit())).then(|| s[i..i + 4].parse::<u16>().ok()).flatten().filter(|y| (1900..=2100).contains(y))
    })
}

fn read_at(f: &mut std::fs::File, at: u64, len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    if f.seek(SeekFrom::Start(at)).is_err() {
        return Vec::new();
    }
    let mut got = 0;
    while got < len {
        match f.read(&mut buf[got..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    buf.truncate(got);
    buf
}

fn clean(s: String) -> Option<String> {
    let s = s.trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
    (!s.is_empty()).then_some(s)
}

fn id3_text(b: &[u8]) -> Option<String> {
    let (enc, body) = b.split_first()?;
    let s = match enc {
        0 => body.iter().map(|&c| c as char).collect(),
        1 | 2 => {
            let (be, body) = match body {
                [0xFF, 0xFE, rest @ ..] => (false, rest),
                [0xFE, 0xFF, rest @ ..] => (true, rest),
                _ => (*enc == 2, body),
            };
            let u: Vec<u16> = body.chunks_exact(2).map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) }).collect();
            String::from_utf16_lossy(&u)
        }
        _ => String::from_utf8_lossy(body).into_owned(),
    };
    clean(s.split('\0').next().unwrap_or("").to_string())
}

fn syncsafe(b: &[u8]) -> usize {
    b.iter().fold(0usize, |a, &x| (a << 7) | (x & 0x7F) as usize)
}

fn id3v1(f: &mut std::fs::File, size: u64) -> Tags {
    if size < 128 {
        return Tags::default();
    }
    let b = read_at(f, size - 128, 128);
    if b.len() < 128 || &b[..3] != b"TAG" {
        return Tags::default();
    }
    let field = |r: std::ops::Range<usize>| clean(b[r].iter().map(|&c| c as char).collect::<String>());
    Tags { title: field(3..33), artist: field(33..63), album: field(63..93), genre: None, year: field(93..97).and_then(|y| year_of(&y)) }
}

/// Title, artist and album from an ID3v2 tag at the start of the file, else from an ID3v1 tag at its end.
pub fn id3_tags(f: &mut std::fs::File, size: u64) -> Tags {
    let head = read_at(f, 0, 10);
    if head.len() < 10 || &head[..3] != b"ID3" {
        return id3v1(f, size);
    }
    let major = head[3];
    let total = syncsafe(&head[6..10]).min(1 << 20);
    let body = read_at(f, 10, total);
    let mut t = Tags::default();
    let mut at = 0;
    let (idlen, hdr) = if major == 2 { (3, 6) } else { (4, 10) };
    while at + hdr <= body.len() {
        let id = &body[at..at + idlen];
        if id[0] == 0 {
            break;
        }
        let len = if major == 2 {
            ((body[at + 3] as usize) << 16) | ((body[at + 4] as usize) << 8) | body[at + 5] as usize
        } else if major == 4 {
            syncsafe(&body[at + 4..at + 8])
        } else {
            u32::from_be_bytes([body[at + 4], body[at + 5], body[at + 6], body[at + 7]]) as usize
        };
        let data = body.get(at + hdr..at + hdr + len);
        let Some(data) = data else { break };
        match id {
            b"TIT2" | b"TT2" => t.title = id3_text(data),
            b"TPE1" | b"TP1" => t.artist = id3_text(data),
            b"TALB" | b"TAL" => t.album = id3_text(data),
            b"TCON" | b"TCO" => t.genre = id3_text(data).map(|g| g.trim_matches(|c| c == '(' || c == ')').to_string()).filter(|g| !g.is_empty() && !g.chars().all(|c| c.is_ascii_digit())),
            b"TDRC" | b"TYER" | b"TYE" | b"TDRL" => t.year = id3_text(data).and_then(|y| year_of(&y)),
            _ => {}
        }
        at += hdr + len;
    }
    if t == Tags::default() {
        return id3v1(f, size);
    }
    t
}

/// Playing time of an MP3: the frame count of a Xing/Info header, else the size over the first frame's bitrate.
fn mp3_seconds(f: &mut std::fs::File, size: u64) -> Option<u32> {
    let head = read_at(f, 0, 10);
    let start = if head.len() == 10 && &head[..3] == b"ID3" { 10 + syncsafe(&head[6..10]) as u64 } else { 0 };
    let b = read_at(f, start, 4096);
    let i = b.windows(2).position(|w| w[0] == 0xFF && w[1] & 0xE0 == 0xE0)?;
    let h = b.get(i..i + 4)?;
    let version = (h[1] >> 3) & 3; // 3 = MPEG1, 2 = MPEG2, 0 = MPEG2.5
    let layer = (h[1] >> 1) & 3; // 1 = layer III
    if version == 1 || layer != 1 {
        return None;
    }
    let (bi, si) = ((h[2] >> 4) as usize, ((h[2] >> 2) & 3) as usize);
    const V1: [u32; 16] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
    const V2: [u32; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];
    const RATES: [[u32; 3]; 3] = [[44100, 48000, 32000], [22050, 24000, 16000], [11025, 12000, 8000]];
    let kbps = if version == 3 { V1[bi] } else { V2[bi] };
    let rate = RATES[match version { 3 => 0, 2 => 1, _ => 2 }].get(si).copied()?;
    if kbps == 0 {
        return None;
    }
    let per_frame = if version == 3 { 1152 } else { 576 };
    let tag = b[i..].windows(4).take(60).position(|w| w == b"Xing" || w == b"Info");
    if let Some(x) = tag {
        let at = i + x + 4;
        if let Some(fl) = b.get(at..at + 8) {
            if fl[3] & 1 == 1 {
                let frames = u32::from_be_bytes([fl[4], fl[5], fl[6], fl[7]]) as u64;
                return Some((frames * per_frame / rate as u64) as u32);
            }
        }
    }
    Some((size.saturating_sub(start) * 8 / (kbps as u64 * 1000)) as u32)
}

fn le32(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]) as usize)
}

/// Fields of a Vorbis comment block (also used by FLAC and Opus): vendor, then `KEY=value` entries.
pub fn vorbis_comments(b: &[u8]) -> Tags {
    let mut t = Tags::default();
    let Some(vl) = le32(b, 0) else { return t };
    let Some(mut at) = vl.checked_add(4) else { return t };
    let Some(n) = le32(b, at) else { return t };
    at += 4;
    for _ in 0..n.min(500) {
        let Some(l) = le32(b, at) else { break };
        let Some(e) = b.get(at + 4..at + 4 + l) else { break };
        at += 4 + l;
        let s = String::from_utf8_lossy(e);
        if let Some((k, v)) = s.split_once('=') {
            match k.to_ascii_uppercase().as_str() {
                "TITLE" => t.title = clean(v.into()),
                "ARTIST" => t.artist = clean(v.into()),
                "ALBUM" => t.album = clean(v.into()),
                "GENRE" => t.genre = clean(v.into()),
                "DATE" | "YEAR" => t.year = year_of(v),
                _ => {}
            }
        }
    }
    t
}

fn ogg_tags(f: &mut std::fs::File) -> Tags {
    let b = read_at(f, 0, 64 * 1024);
    for (marker, skip) in [(&b"\x03vorbis"[..], 7usize), (&b"OpusTags"[..], 8)] {
        if let Some(i) = b.windows(marker.len()).position(|w| w == marker) {
            return vorbis_comments(&b[i + skip..]);
        }
    }
    Tags::default()
}

fn flac_info(f: &mut std::fs::File) -> (Tags, Option<u32>) {
    let b = read_at(f, 0, 512 * 1024);
    let (mut tags, mut secs) = (Tags::default(), None);
    if !b.starts_with(b"fLaC") {
        return (tags, secs);
    }
    let mut at = 4;
    while at + 4 <= b.len() {
        let (last, ty) = (b[at] & 0x80 != 0, b[at] & 0x7F);
        let len = ((b[at + 1] as usize) << 16) | ((b[at + 2] as usize) << 8) | b[at + 3] as usize;
        let Some(body) = b.get(at + 4..at + 4 + len) else { break };
        match ty {
            0 if body.len() >= 18 => {
                let rate = ((body[10] as u64) << 12) | ((body[11] as u64) << 4) | (body[12] as u64 >> 4);
                let total = (((body[13] & 0x0F) as u64) << 32) | u32::from_be_bytes([body[14], body[15], body[16], body[17]]) as u64;
                if rate > 0 && total > 0 {
                    secs = Some((total / rate) as u32);
                }
            }
            4 => tags = vorbis_comments(body),
            _ => {}
        }
        if last {
            break;
        }
        at += 4 + len;
    }
    (tags, secs)
}

fn wav_seconds(f: &mut std::fs::File) -> Option<u32> {
    let b = read_at(f, 0, 4096);
    let (mut at, mut byte_rate, mut data) = (12, 0usize, None);
    while at + 8 <= b.len() {
        let len = le32(&b, at + 4)?;
        match &b[at..at + 4] {
            b"fmt " => byte_rate = le32(&b, at + 16)?,
            b"data" => {
                data = Some(len);
                break;
            }
            _ => {}
        }
        at += 8 + len + (len & 1);
    }
    if byte_rate > 0 { data.map(|d| (d / byte_rate) as u32) } else { None }
}

fn be32(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]) as usize)
}

/// The boxes of an MP4 region as `(type, content)`.
fn mp4_boxes(b: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 8 <= b.len() && out.len() < 200 {
        let size = be32(b, at).unwrap_or(0);
        let ty = [b[at + 4], b[at + 5], b[at + 6], b[at + 7]];
        if size < 8 {
            break;
        }
        let end = (at + size).min(b.len());
        out.push((ty, &b[at + 8..end]));
        at = end;
    }
    out
}

fn mp4_child<'a>(b: &'a [u8], ty: &[u8; 4]) -> Option<&'a [u8]> {
    mp4_boxes(b).into_iter().find(|(t, _)| t == ty).map(|(_, c)| c)
}

/// Title, artist and album (`©nam`, `©ART`, `©alb`) and length from the `moov` box of an MP4 or M4A.
fn mp4_info(f: &mut std::fs::File, size: u64) -> (Tags, Option<u32>) {
    let (mut tags, mut secs) = (Tags::default(), None);
    let mut at = 0u64;
    for _ in 0..64 {
        let h = read_at(f, at, 16);
        if h.len() < 8 {
            break;
        }
        let mut len = be32(&h, 0).unwrap_or(0) as u64;
        if len == 1 && h.len() >= 16 {
            len = u64::from_be_bytes([h[8], h[9], h[10], h[11], h[12], h[13], h[14], h[15]]);
        } else if len == 0 {
            len = size - at;
        }
        if len < 8 {
            break;
        }
        if &h[4..8] == b"moov" {
            let b = read_at(f, at + 8, (len - 8).min(8 << 20) as usize);
            if let Some(mvhd) = mp4_child(&b, b"mvhd") {
                let (ts, dur) = if mvhd.first() == Some(&1) { (be32(mvhd, 20), mvhd.get(24..32).map(|x| u64::from_be_bytes([x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7]]))) } else { (be32(mvhd, 12), be32(mvhd, 16).map(|d| d as u64)) };
                if let (Some(ts), Some(d)) = (ts, dur) {
                    if ts > 0 {
                        secs = Some((d / ts as u64) as u32);
                    }
                }
            }
            let ilst = mp4_child(&b, b"udta").and_then(|u| mp4_child(u, b"meta")).and_then(|m| m.get(4..)).and_then(|m| mp4_child(m, b"ilst"));
            for (ty, item) in ilst.map(mp4_boxes).unwrap_or_default() {
                let text = mp4_child(item, b"data").and_then(|d| d.get(8..)).and_then(|d| clean(String::from_utf8_lossy(d).into_owned()));
                match &ty {
                    b"\xA9nam" => tags.title = text,
                    b"\xA9ART" => tags.artist = text,
                    b"\xA9alb" => tags.album = text,
                    b"\xA9gen" => tags.genre = text,
                    b"\xA9day" => tags.year = text.as_deref().and_then(year_of),
                    _ => {}
                }
            }
            break;
        }
        at += len;
        if at >= size {
            break;
        }
    }
    (tags, secs)
}

// ------------------------------------------------------------------ per-person settings and playlists

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn next(self) -> Repeat {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub items: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Saved {
    /// `None` until the person changes it: then the default folders apply.
    folders: Option<Vec<String>>,
    playlists: Vec<Playlist>,
    shuffle: bool,
    repeat: Repeat,
}

/// A person's folders, playlists and play flags, in a small file of their own.
pub struct MediaStore {
    path: PathBuf,
    dir: PathBuf,
    user: String,
}

pub fn default_folders(user: &str) -> Vec<String> {
    vec![format!("/home/{user}/Music"), format!("/home/{user}/Videos")]
}

fn valid_playlist_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 60 || n.chars().any(char::is_control) {
        return Err("a playlist name is 1 to 60 characters".into());
    }
    Ok(n.to_string())
}

impl MediaStore {
    pub fn new(dir: &Path, user: &str) -> MediaStore {
        let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
        MediaStore { path: dir.join(format!("media-{who}.json")), dir: dir.join(format!("media-{who}")), user: user.to_string() }
    }

    fn load(&self) -> Saved {
        std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save(&self, s: &Saved) -> Result<(), String> {
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(s).map_err(|e| e.to_string())?).and_then(|_| std::fs::rename(&tmp, &self.path)).map_err(|e| e.to_string())
    }

    fn edit<T>(&self, f: impl FnOnce(&mut Saved) -> Result<T, String>) -> Result<T, String> {
        let mut s = self.load();
        let r = f(&mut s)?;
        self.save(&s)?;
        Ok(r)
    }

    pub fn folders(&self) -> Vec<String> {
        self.load().folders.unwrap_or_else(|| default_folders(&self.user))
    }

    /// Host folders this person shared (window mode), as they are right now: only those still there. The library reads
    /// them in place and never writes to them.
    pub fn host_mounts(&self) -> Vec<crate::hostfiles::HostMount> {
        self.path.parent().map(|d| crate::hostfiles::active_mounts(d, &self.user)).unwrap_or_default()
    }

    /// Every shared host folder with whether it can be reached now: an unplugged or moved one is listed as not available,
    /// and comes back by itself when the folder does.
    pub fn host_mount_states(&self) -> Vec<(crate::hostfiles::HostMount, bool)> {
        let Some(d) = self.path.parent() else { return Vec::new() };
        if !crate::hostfiles::enabled() {
            return Vec::new();
        }
        let valid = self.host_mounts();
        crate::hostfiles::HostMounts::load(d, &self.user).items.into_iter().map(|m| {
            let ok = valid.iter().any(|v| v.name == m.name);
            (m, ok)
        }).collect()
    }

    pub fn host_mount_for(&self, path: &str) -> Option<crate::hostfiles::HostMount> {
        self.host_mounts().into_iter().find(|m| Path::new(path).starts_with(&m.path))
    }

    pub fn add_folder(&self, folder: &str) -> Result<(), String> {
        let f = folder.trim().trim_end_matches('/').to_string();
        if !f.starts_with('/') || f.split('/').any(|p| p == "..") || f.chars().any(char::is_control) {
            return Err("a folder is a full path such as /home/me/Music".into());
        }
        let mut v = self.folders();
        if v.contains(&f) {
            return Err("that folder is listed already".into());
        }
        if v.len() >= 20 {
            return Err("that is as many folders as fit".into());
        }
        v.push(f);
        self.edit(|s| {
            s.folders = Some(v);
            Ok(())
        })
    }

    pub fn remove_folder(&self, folder: &str) -> Result<(), String> {
        let mut v = self.folders();
        let n = v.len();
        v.retain(|f| f != folder);
        if v.len() == n {
            return Err("no such folder".into());
        }
        self.edit(|s| {
            s.folders = Some(v);
            Ok(())
        })
    }

    pub fn playlists(&self) -> Vec<Playlist> {
        self.load().playlists
    }

    pub fn playlist(&self, name: &str) -> Option<Playlist> {
        self.load().playlists.into_iter().find(|p| p.name == name)
    }

    pub fn create(&self, name: &str) -> Result<String, String> {
        let n = valid_playlist_name(name)?;
        self.edit(|s| {
            if s.playlists.iter().any(|p| p.name.eq_ignore_ascii_case(&n)) {
                return Err("a playlist with that name exists".into());
            }
            if s.playlists.len() >= MAX_PLAYLISTS {
                return Err("that is as many playlists as fit".into());
            }
            s.playlists.push(Playlist { name: n.clone(), items: vec![] });
            Ok(n)
        })
    }

    pub fn rename(&self, old: &str, new: &str) -> Result<String, String> {
        let n = valid_playlist_name(new)?;
        self.edit(|s| {
            if s.playlists.iter().any(|p| p.name.eq_ignore_ascii_case(&n) && p.name != old) {
                return Err("a playlist with that name exists".into());
            }
            s.playlists.iter_mut().find(|p| p.name == old).ok_or("no such playlist")?.name = n.clone();
            Ok(n)
        })
    }

    pub fn delete(&self, name: &str) -> Result<(), String> {
        let _ = std::fs::remove_file(self.m3u_path(name));
        self.edit(|s| {
            let before = s.playlists.len();
            s.playlists.retain(|p| p.name != name);
            if s.playlists.len() == before { Err("no such playlist".into()) } else { Ok(()) }
        })
    }

    pub fn add_item(&self, name: &str, path: &str) -> Result<usize, String> {
        self.edit(|s| {
            let p = s.playlists.iter_mut().find(|p| p.name == name).ok_or("no such playlist")?;
            if p.items.len() >= MAX_ITEMS {
                return Err("that playlist is full".into());
            }
            p.items.push(path.to_string());
            Ok(p.items.len())
        })
    }

    pub fn remove_item(&self, name: &str, index: usize) -> Result<(), String> {
        self.edit(|s| {
            let p = s.playlists.iter_mut().find(|p| p.name == name).ok_or("no such playlist")?;
            if index >= p.items.len() {
                return Err("no such entry".into());
            }
            p.items.remove(index);
            Ok(())
        })
    }

    /// Move entry `from` to position `to`.
    pub fn move_item(&self, name: &str, from: usize, to: usize) -> Result<(), String> {
        self.edit(|s| {
            let p = s.playlists.iter_mut().find(|p| p.name == name).ok_or("no such playlist")?;
            if from >= p.items.len() || to >= p.items.len() {
                return Err("no such entry".into());
            }
            let it = p.items.remove(from);
            p.items.insert(to, it);
            Ok(())
        })
    }

    pub fn flags(&self) -> (bool, Repeat) {
        let s = self.load();
        (s.shuffle, s.repeat)
    }

    pub fn set_flags(&self, shuffle: bool, repeat: Repeat) -> Result<(), String> {
        self.edit(|s| {
            s.shuffle = shuffle;
            s.repeat = repeat;
            Ok(())
        })
    }

    fn m3u_path(&self, name: &str) -> PathBuf {
        let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' }).collect();
        self.dir.join(format!("{safe}.m3u"))
    }

    /// Write `items` as an extended M3U file under the person's own media folder and return its path.
    pub fn write_m3u(&self, name: &str, items: &[String], lib: &Library) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let p = self.m3u_path(name);
        std::fs::write(&p, m3u_text(items, lib)).map_err(|e| e.to_string())?;
        Ok(p)
    }
}

/// An extended M3U playlist for `items` (one path per line, with a title line when the library knows the file).
pub fn m3u_text(items: &[String], lib: &Library) -> String {
    let mut out = String::from("#EXTM3U\n");
    for p in items {
        if p.contains('\n') || p.contains('\r') {
            continue;
        }
        if let Some(t) = lib.find(p) {
            let name = if t.artist == UNKNOWN_ARTIST { t.title.clone() } else { format!("{} - {}", t.artist, t.title) };
            out.push_str(&format!("#EXTINF:{},{}\n", t.seconds.map_or(-1, |s| s as i64), name.replace(['\n', '\r'], " ")));
        }
        out.push_str(p);
        out.push('\n');
    }
    out
}

/// `items` in a random order that depends on `seed` (a Fisher-Yates shuffle over a xorshift generator).
pub fn shuffled(items: &[String], seed: u64) -> Vec<String> {
    let mut v = items.to_vec();
    let mut x = seed | 1;
    for i in (1..v.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.swap(i, (x % (i as u64 + 1)) as usize);
    }
    v
}

pub fn clock_seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64)
}

// ------------------------------------------------------------------ the external player

/// A started player process.
pub trait Running: Send {
    fn alive(&mut self) -> bool;
    fn stop(&mut self);
}

/// Starts the player on one file or playlist file.
pub trait Launcher: Send + Sync {
    fn launch(&self, target: &str) -> Result<Box<dyn Running>, String>;
    /// This launcher plays MP3 files inside IYAKKI itself (no other program), so a queue can be stepped through by the
    /// caller. False for a launcher that hands files to an external program.
    fn plays_mp3_in_process(&self) -> bool {
        false
    }
}

pub(crate) struct ChildRun(pub(crate) std::process::Child);

impl Running for ChildRun {
    fn alive(&mut self) -> bool {
        iyakki_core_api::child_status(&mut self.0).is_running()
    }
    fn stop(&mut self) {
        crate::voice::kill_child(&mut self.0);
    }
}

// ------------------------------------------------------------------ MP3 inside IYAKKI

/// What the in-house MP3 path needs: the radio's player (it decodes with `nanomp3` and writes WAV to the sound command) and
/// the sound command from Settings → Voice.
#[derive(Clone)]
struct Builtin {
    radio: crate::radio::RadioPlayer,
    play_cmd: Arc<dyn Fn() -> String + Send + Sync>,
}

static BUILTIN: Mutex<Option<Builtin>> = Mutex::new(None);

/// Let the shared player play MP3 files itself, through `radio` (the services call this once they exist).
pub fn set_builtin_audio(radio: crate::radio::RadioPlayer, play_cmd: Arc<dyn Fn() -> String + Send + Sync>) {
    *BUILTIN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Builtin { radio, play_cmd });
}

fn builtin() -> Option<Builtin> {
    BUILTIN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A file name that the in-house player can play: MP3.
pub fn is_mp3(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, e)| e.eq_ignore_ascii_case("mp3"))
}

/// A track playing in the radio player. It lives while the radio player is on *this* file.
struct BuiltinRun {
    radio: crate::radio::RadioPlayer,
    name: String,
}

impl Running for BuiltinRun {
    fn alive(&mut self) -> bool {
        use crate::radio::RadioState::*;
        match self.radio.state() {
            Connecting(n) | Playing { station: n, .. } => n == self.name,
            _ => false,
        }
    }
    fn stop(&mut self) {
        if self.alive() {
            self.radio.stop();
        }
    }
}

fn launch_builtin(b: &Builtin, target: &str) -> Result<Box<dyn Running>, String> {
    let name = format!("{} (file)", target.rsplit('/').next().unwrap_or("audio"));
    let allow: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(|_| false);
    b.radio.play(&crate::radio::Station::new(&name, &format!("file://{target}")), allow, &(b.play_cmd)());
    // a file starts at once or fails at once (a missing sound command, a file that is not MP3)
    for _ in 0..300 {
        match b.radio.state() {
            crate::radio::RadioState::Playing { .. } => return Ok(Box::new(BuiltinRun { radio: b.radio.clone(), name })),
            crate::radio::RadioState::Failed { why, .. } => return Err(format!("could not play it: {why}")),
            _ => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
    b.radio.stop();
    Err("the sound player did not start (is the speaker set up in Settings → Voice?)".into())
}

/// The real thing: MP3 in IYAKKI's own player when it is wired up; everything else (video, other audio formats, playlists)
/// through mpv, VLC, ffplay or mplayer (or the configured command) as a separate process.
pub struct SystemLauncher {
    pub configured: String,
}

impl Launcher for SystemLauncher {
    fn launch(&self, target: &str) -> Result<Box<dyn Running>, String> {
        if is_mp3(target) {
            if let Some(b) = builtin() {
                return launch_builtin(&b, target);
            }
        }
        let argv = crate::voice::find_media_player(&self.configured).ok_or_else(|| {
            if is_video_path(target) {
                "Video needs the web image or an installed player (mpv, VLC, ffplay)".to_string()
            } else {
                "this format needs an installed media player (mpv, VLC, ffplay or mplayer); MP3 plays in IYAKKI itself".to_string()
            }
        })?;
        Ok(Box::new(ChildRun(crate::voice::spawn_media_player(&argv, target)?)))
    }
    fn plays_mp3_in_process(&self) -> bool {
        builtin().is_some()
    }
}

/// The name says video (by extension).
pub fn is_video_path(path: &str) -> bool {
    let e = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    EXTENSIONS.iter().any(|(x, k)| *x == e && *k == Kind::Video)
}

// ------------------------------------------------------------------ the built-in sample folder

/// Where the images put the sample media (read-only, the same on every image).
pub const SAMPLES_DIR: &str = "/usr/share/iyakki/samples";
/// A development machine can point at another folder (or an empty value for none).
pub const SAMPLES_ENV: &str = "IYAKKI_SAMPLES_DIR";
/// How the sample folder is called in the lists.
pub const SAMPLES_LABEL: &str = "IYAKKI samples";
/// The tag the sample files carry.
pub const SAMPLES_TAG: &str = "sample";

/// The sample folder, when it exists here.
pub fn samples_dir() -> Option<PathBuf> {
    let dir = match std::env::var_os(SAMPLES_ENV) {
        Some(v) if v.is_empty() => return None,
        Some(v) => PathBuf::from(v),
        None => PathBuf::from(SAMPLES_DIR),
    };
    dir.is_dir().then_some(dir)
}

/// Is `path` the sample folder `dir` or something inside it (no `..` in the way)?
pub fn is_sample_in(dir: &Path, path: &str) -> bool {
    let p = Path::new(path);
    p.starts_with(dir) && !p.components().any(|c| matches!(c, std::path::Component::ParentDir))
}

/// May this account read `path`? Its read scope decides, except that the built-in samples (public domain, read-only) can
/// always be listed and played.
pub fn may_read(policy: &iyakki_sec::policy::Policy, user: &str, path: &str) -> bool {
    policy.allows_read(user, path) || samples_dir().is_some_and(|d| is_sample_in(&d, path))
}

/// The folders to scan: the person's own, then the sample folder (read-only, never written to).
pub fn scan_folders(own: &[String], samples: Option<&Path>) -> Vec<String> {
    let mut v = own.to_vec();
    if let Some(s) = samples {
        let s = s.to_string_lossy().trim_end_matches('/').to_string();
        if !v.contains(&s) {
            v.push(s);
        }
    }
    v
}

#[derive(Debug, PartialEq, Eq)]
pub enum Check {
    Idle,
    Playing { user: String, label: String },
    /// The player ended by itself since the last check.
    Ended { user: String },
}

struct Current {
    run: Box<dyn Running>,
    user: String,
    label: String,
    target: String,
    since: std::time::Instant,
}

/// At most one thing plays at a time; starting another stops the first.
pub struct MediaPlayer {
    launcher: Arc<dyn Launcher>,
    cur: Mutex<Option<Current>>,
    queue: Mutex<QueueState>,
}

/// The play queue, kept with the player so the MEDIA app and the chat's tools see the same one. `rev` counts changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueueState {
    pub user: String,
    pub items: Vec<String>,
    pub pos: usize,
    pub active: bool,
    pub rev: u64,
}

impl MediaPlayer {
    pub fn new(launcher: Arc<dyn Launcher>) -> MediaPlayer {
        MediaPlayer { launcher, cur: Mutex::new(None), queue: Mutex::new(QueueState::default()) }
    }

    /// MP3 plays inside IYAKKI (see [`Launcher::plays_mp3_in_process`]).
    pub fn plays_mp3_in_process(&self) -> bool {
        self.launcher.plays_mp3_in_process()
    }

    pub fn play(&self, user: &str, label: &str, target: &str) -> Result<(), String> {
        self.stop();
        let run = self.launcher.launch(target)?;
        *self.cur.lock().unwrap_or_else(|e| e.into_inner()) = Some(Current { run, user: user.into(), label: label.into(), target: target.into(), since: std::time::Instant::now() });
        Ok(())
    }

    /// Replace the shared queue; returns its new revision.
    pub fn set_queue(&self, user: &str, items: Vec<String>, pos: usize, active: bool) -> u64 {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        *q = QueueState { user: user.into(), items, pos, active, rev: q.rev + 1 };
        q.rev
    }

    pub fn queue_state(&self) -> QueueState {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn stop(&self) {
        if let Some(mut c) = self.cur.lock().unwrap_or_else(|e| e.into_inner()).take() {
            c.run.stop();
        }
    }

    pub fn check(&self) -> Check {
        let mut g = self.cur.lock().unwrap_or_else(|e| e.into_inner());
        let Some(c) = g.as_mut() else { return Check::Idle };
        if c.run.alive() {
            return Check::Playing { user: c.user.clone(), label: c.label.clone() };
        }
        let user = g.take().map(|c| c.user).unwrap_or_default();
        Check::Ended { user }
    }

    /// The file (or playlist file) that plays now, if anything.
    pub fn now_target(&self) -> Option<String> {
        let mut g = self.cur.lock().unwrap_or_else(|e| e.into_inner());
        let c = g.as_mut()?;
        c.run.alive().then(|| c.target.clone())
    }

    /// Seconds since the current thing was started.
    pub fn elapsed(&self) -> Option<u64> {
        let mut g = self.cur.lock().unwrap_or_else(|e| e.into_inner());
        let c = g.as_mut()?;
        c.run.alive().then(|| c.since.elapsed().as_secs())
    }

    /// What plays now, if anything (looking does not use up the "ended" notice that [`MediaPlayer::check`] gives).
    pub fn now_playing(&self) -> Option<String> {
        let mut g = self.cur.lock().unwrap_or_else(|e| e.into_inner());
        let c = g.as_mut()?;
        c.run.alive().then(|| c.label.clone())
    }
}

/// The player the whole shell shares, so that music goes on when the MEDIA app is left.
pub fn shared() -> Arc<MediaPlayer> {
    static P: OnceLock<Arc<MediaPlayer>> = OnceLock::new();
    P.get_or_init(|| Arc::new(MediaPlayer::new(Arc::new(SystemLauncher { configured: String::new() })))).clone()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-media-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn id3_frame(id: &str, text: &str) -> Vec<u8> {
        let mut data = vec![3u8];
        data.extend(text.as_bytes());
        let mut f = id.as_bytes().to_vec();
        f.extend((data.len() as u32).to_be_bytes());
        f.extend([0, 0]);
        f.extend(data);
        f
    }

    /// An MP3 with an ID3v2.3 tag and `frames` silent 128 kbps frames.
    pub fn mp3(tags: &[(&str, &str)], frames: usize) -> Vec<u8> {
        let body: Vec<u8> = tags.iter().flat_map(|(i, t)| id3_frame(i, t)).collect();
        let mut v = b"ID3\x03\x00\x00".to_vec();
        let n = body.len();
        v.extend([((n >> 21) & 0x7F) as u8, ((n >> 14) & 0x7F) as u8, ((n >> 7) & 0x7F) as u8, (n & 0x7F) as u8]);
        v.extend(body);
        for _ in 0..frames {
            let mut fr = vec![0u8; 417];
            fr[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
            v.extend(fr);
        }
        v
    }

    fn comments(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut v = 6u32.to_le_bytes().to_vec();
        v.extend(b"iyakki");
        v.extend((pairs.len() as u32).to_le_bytes());
        for (k, val) in pairs {
            let e = format!("{k}={val}");
            v.extend((e.len() as u32).to_le_bytes());
            v.extend(e.as_bytes());
        }
        v
    }

    /// A FLAC file: STREAMINFO (44100 Hz, 441000 samples = 10 s) and a Vorbis comment block.
    pub fn flac(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut v = b"fLaC".to_vec();
        let mut si = vec![0u8; 34];
        // sample rate 44100 in 20 bits, 1 channel, 16 bits, 441000 samples in 36 bits
        si[10] = (44100 >> 12) as u8;
        si[11] = (44100 >> 4) as u8;
        si[12] = ((44100 & 0xF) << 4) as u8;
        si[13] = ((441000u64 >> 32) & 0xF) as u8;
        si[14..18].copy_from_slice(&(441000u32).to_be_bytes());
        v.extend([0u8, 0, 0, 34]);
        v.extend(si);
        let c = comments(pairs);
        v.extend([0x84u8, (c.len() >> 16) as u8, (c.len() >> 8) as u8, c.len() as u8]);
        v.extend(c);
        v
    }

    /// An Ogg Vorbis start: a page holding the comment packet (no audio follows).
    pub fn ogg(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut v = b"OggS\0\x02".to_vec();
        v.extend([0u8; 20]);
        v.extend(b"\x03vorbis");
        v.extend(comments(pairs));
        v
    }

    pub fn wav(seconds: u32) -> Vec<u8> {
        let rate = 8000u32;
        let data = rate * seconds;
        let mut v = b"RIFF".to_vec();
        v.extend((36 + data).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend([1, 0, 1, 0]);
        v.extend(rate.to_le_bytes());
        v.extend(rate.to_le_bytes());
        v.extend([1, 0, 8, 0]);
        v.extend(b"data");
        v.extend(data.to_le_bytes());
        v.extend(vec![128u8; 64]); // only the start of the samples
        v
    }

    fn mp4_box(ty: &[u8], content: &[u8]) -> Vec<u8> {
        let mut v = ((content.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend(ty);
        v.extend(content);
        v
    }

    /// An M4A with `moov` holding `mvhd` (1000 ticks a second, 90 s) and `©nam`/`©ART`/`©alb` items.
    pub fn m4a(title: &str, artist: &str, album: &str) -> Vec<u8> {
        let mut v = mp4_box(b"ftyp", b"M4A \0\0\0\0M4A isom");
        let mut mvhd = vec![0u8; 20];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&90_000u32.to_be_bytes());
        let item = |ty: &[u8], text: &str| {
            let mut d = vec![0, 0, 0, 1, 0, 0, 0, 0];
            d.extend(text.as_bytes());
            mp4_box(ty, &mp4_box(b"data", &d))
        };
        let ilst: Vec<u8> = [item(b"\xA9nam", title), item(b"\xA9ART", artist), item(b"\xA9alb", album)].concat();
        let mut meta = vec![0u8; 4];
        meta.extend(mp4_box(b"ilst", &ilst));
        let udta = mp4_box(b"udta", &mp4_box(b"meta", &meta));
        let moov = mp4_box(b"moov", &[mp4_box(b"mvhd", &mvhd), udta].concat());
        v.extend(mdat());
        v.extend(moov); // moov after the media data, as in many files
        v
    }

    fn mdat() -> Vec<u8> {
        mp4_box(b"mdat", &[0u8; 100])
    }

    pub fn mp4_video() -> Vec<u8> {
        let mut v = mp4_box(b"ftyp", b"isom\0\0\0\0isomiso2");
        v.extend(mdat());
        v
    }

    pub fn mkv() -> Vec<u8> {
        let mut v = vec![0x1A, 0x45, 0xDF, 0xA3, 0x84, 0x42, 0x82, 0x84];
        v.extend(b"webm");
        v.extend([0u8; 20]);
        v
    }

    fn put(dir: &Path, rel: &str, bytes: &[u8]) -> String {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p.to_str().unwrap().to_string()
    }

    #[test]
    fn tags_and_lengths_are_read_from_each_container() {
        let d = temp("tags");
        let m = put(&d, "a.mp3", &mp3(&[("TIT2", "Sunrise"), ("TPE1", "The Band"), ("TALB", "Dawn")], 40));
        let f = put(&d, "b.flac", &flac(&[("TITLE", "Flac Song"), ("artist", "Flac Artist"), ("ALBUM", "Lossless")]));
        let o = put(&d, "c.ogg", &ogg(&[("TITLE", "Ogg Song"), ("ARTIST", "Ogg Artist")]));
        let w = put(&d, "d.wav", &wav(3));
        let a = put(&d, "e.m4a", &m4a("Mp4 Song", "Mp4 Artist", "Mp4 Album"));
        let lib = scan(&[d.to_str().unwrap().to_string()], &|_| true);
        let t = |p: &str| lib.find(p).unwrap().clone();
        assert_eq!((t(&m).title.as_str(), t(&m).artist.as_str(), t(&m).album.as_str()), ("Sunrise", "The Band", "Dawn"));
        assert!(t(&m).seconds.is_some_and(|s| s <= 2), "40 frames of 26 ms: {:?}", t(&m).seconds);
        assert_eq!((t(&f).title.as_str(), t(&f).artist.as_str(), t(&f).album.as_str(), t(&f).seconds), ("Flac Song", "Flac Artist", "Lossless", Some(10)));
        assert_eq!((t(&o).title.as_str(), t(&o).artist.as_str(), t(&o).album.as_str()), ("Ogg Song", "Ogg Artist", UNKNOWN_ALBUM));
        assert_eq!((t(&w).title.as_str(), t(&w).artist.as_str(), t(&w).seconds), ("d", UNKNOWN_ARTIST, Some(3)));
        assert_eq!((t(&a).title.as_str(), t(&a).artist.as_str(), t(&a).album.as_str(), t(&a).seconds), ("Mp4 Song", "Mp4 Artist", "Mp4 Album", Some(90)));
        assert_eq!(t(&a).kind, Kind::Audio, "an M4A brand is audio");
        assert_eq!(t(&f).duration_text(), "0:10");
    }

    #[test]
    fn id3v1_and_file_names_are_the_fallbacks() {
        let d = temp("fallback");
        let mut v1 = vec![0xFFu8, 0xFB, 0x90, 0x64];
        v1.extend(vec![0u8; 300]);
        let mut tag = b"TAG".to_vec();
        for s in ["Old Title", "Old Artist", "Old Album"] {
            let mut f = s.as_bytes().to_vec();
            f.resize(30, 0);
            tag.extend(f);
        }
        tag.extend([0u8; 35]);
        v1.extend(tag);
        let a = put(&d, "x.mp3", &v1);
        let b = put(&d, "Some Artist - Some_Song.mp3", &mp3(&[], 2));
        let c = put(&d, "plain_name.mp3", &mp3(&[], 2));
        let lib = scan(&[d.to_str().unwrap().to_string()], &|_| true);
        let (x, y, z) = (lib.find(&a).unwrap(), lib.find(&b).unwrap(), lib.find(&c).unwrap());
        assert_eq!((x.title.as_str(), x.artist.as_str(), x.album.as_str()), ("Old Title", "Old Artist", "Old Album"));
        assert_eq!((y.title.as_str(), y.artist.as_str()), ("Some Song", "Some Artist"));
        assert_eq!((z.title.as_str(), z.artist.as_str()), ("plain name", UNKNOWN_ARTIST));
    }

    #[test]
    fn files_are_recognised_by_extension_and_by_their_first_bytes() {
        let d = temp("kinds");
        put(&d, "v.mp4", &mp4_video());
        put(&d, "w.mkv", &mkv());
        put(&d, "noext", &mp3(&[("TIT2", "Hidden")], 2)); // no extension: bytes decide
        put(&d, "wrongname.dat", &mp3(&[("TIT2", "Hidden 2")], 2)); // an unknown extension is not listed
        put(&d, "notes.txt", b"hello");
        put(&d, "empty.mp3", b""); // media name, nothing inside: still listed by its name
        put(&d, "sub/deep/song.flac", &flac(&[("TITLE", "Deep")]));
        put(&d, "disguised.mp4", &flac(&[("TITLE", "Really audio")])); // the bytes win over the name
        let lib = scan(&[d.to_str().unwrap().to_string()], &|_| true);
        let names: Vec<_> = lib.tracks.iter().map(|t| (t.path.rsplit('/').next().unwrap().to_string(), t.kind)).collect();
        for want in [("v.mp4", Kind::Video), ("w.mkv", Kind::Video), ("noext", Kind::Audio), ("empty.mp3", Kind::Audio), ("song.flac", Kind::Audio), ("disguised.mp4", Kind::Audio)] {
            assert!(names.contains(&(want.0.to_string(), want.1)), "{want:?} in {names:?}");
        }
        assert!(!names.iter().any(|(n, _)| n == "notes.txt" || n == "wrongname.dat"), "{names:?}");
        assert_eq!(lib.videos("").len(), 2);
        assert_eq!(sniff(b"RIFF\0\0\0\0AVI LIST"), Some(("avi", Kind::Video)));
        assert_eq!(sniff(b"OggS\0\x02\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0OpusHead"), Some(("opus", Kind::Audio)));
    }

    #[test]
    fn the_scan_stays_inside_what_the_account_may_read() {
        let d = temp("scope");
        let ok = put(&d, "mine/a.mp3", &mp3(&[("TIT2", "Mine")], 2));
        put(&d, "theirs/b.mp3", &mp3(&[("TIT2", "Theirs")], 2));
        put(&d, "mine/private/c.mp3", &mp3(&[("TIT2", "Hidden")], 2));
        std::os::unix::fs::symlink(d.join("theirs"), d.join("mine/link")).unwrap();
        let mine = d.join("mine").to_str().unwrap().to_string();
        let private = d.join("mine/private").to_str().unwrap().to_string();
        let allow = move |p: &str| p.starts_with(&mine) && !p.starts_with(&private);
        let both = [d.join("mine").to_str().unwrap().to_string(), d.join("theirs").to_str().unwrap().to_string()];
        let lib = scan(&both, &allow);
        assert_eq!(lib.tracks.iter().map(|t| t.path.clone()).collect::<Vec<_>>(), vec![ok], "other folders, a forbidden subfolder and links are left out");
    }

    #[test]
    fn artists_albums_and_search() {
        let d = temp("browse");
        put(&d, "1.mp3", &mp3(&[("TIT2", "One"), ("TPE1", "Ann"), ("TALB", "First")], 2));
        put(&d, "2.mp3", &mp3(&[("TIT2", "Two"), ("TPE1", "Ann"), ("TALB", "First")], 2));
        put(&d, "3.mp3", &mp3(&[("TIT2", "Three"), ("TPE1", "Bob"), ("TALB", "Second")], 2));
        let lib = scan(&[d.to_str().unwrap().to_string()], &|_| true);
        assert_eq!(lib.artists(""), vec![("Ann".to_string(), 2), ("Bob".to_string(), 1)]);
        assert_eq!(lib.albums(""), vec![("Ann".to_string(), "First".to_string(), 2), ("Bob".to_string(), "Second".to_string(), 1)]);
        assert_eq!(lib.audio("thr").len(), 1);
        assert_eq!(lib.artists("bob"), vec![("Bob".to_string(), 1)]);
        assert_eq!(lib.by_album("Ann", "First").len(), 2);
        assert_eq!(lib.by_artist("Bob")[0].title, "Three");
    }

    #[test]
    fn playlists_and_flags_are_kept_per_person() {
        let d = temp("lists");
        let s = MediaStore::new(&d, "alice");
        assert_eq!(s.folders(), vec!["/home/alice/Music", "/home/alice/Videos"]);
        s.add_folder("/mnt/stick/Music/").unwrap();
        assert!(s.add_folder("/mnt/stick/Music").is_err() && s.add_folder("relative").is_err() && s.add_folder("/a/../b").is_err());
        s.remove_folder("/home/alice/Videos").unwrap();
        assert_eq!(s.folders(), vec!["/home/alice/Music", "/mnt/stick/Music"]);
        assert_eq!(s.create("  Road trip ").unwrap(), "Road trip");
        assert!(s.create("road TRIP").is_err() && s.create("").is_err());
        s.create("Focus").unwrap();
        for p in ["/m/a.mp3", "/m/b.mp3", "/m/c.mp3"] {
            s.add_item("Road trip", p).unwrap();
        }
        s.move_item("Road trip", 2, 0).unwrap();
        s.remove_item("Road trip", 1).unwrap();
        assert_eq!(s.playlist("Road trip").unwrap().items, vec!["/m/c.mp3", "/m/b.mp3"]);
        assert!(s.move_item("Road trip", 0, 5).is_err() && s.remove_item("Road trip", 9).is_err() && s.add_item("Nope", "/x").is_err());
        assert_eq!(s.rename("Focus", "Deep work").unwrap(), "Deep work");
        assert!(s.rename("Deep work", "road trip").is_err());
        s.set_flags(true, Repeat::One).unwrap();
        // a fresh handle (a restart) sees everything; another person sees nothing
        let again = MediaStore::new(&d, "alice");
        assert_eq!(again.playlists().iter().map(|p| p.name.clone()).collect::<Vec<_>>(), vec!["Road trip", "Deep work"]);
        assert_eq!(again.flags(), (true, Repeat::One));
        assert_eq!(again.folders()[1], "/mnt/stick/Music");
        let bob = MediaStore::new(&d, "bob");
        assert!(bob.playlists().is_empty() && bob.flags() == (false, Repeat::Off) && bob.folders()[0] == "/home/bob/Music");
        again.delete("Deep work").unwrap();
        assert!(again.delete("Deep work").is_err());
        assert_eq!(again.playlists().len(), 1);
    }

    #[test]
    fn m3u_files_and_shuffles() {
        let d = temp("m3u");
        let p = put(&d, "t.mp3", &mp3(&[("TIT2", "Song"), ("TPE1", "Ann")], 2));
        let lib = scan(&[d.to_str().unwrap().to_string()], &|_| true);
        let s = MediaStore::new(&d, "alice");
        let items = vec![p.clone(), "/elsewhere/x.flac".to_string(), "/bad\nname.mp3".to_string()];
        let path = s.write_m3u("Road trip!", &items, &lib).unwrap();
        assert!(path.starts_with(d.join("media-alice")) && path.to_str().unwrap().ends_with("Road_trip_.m3u"));
        let text = std::fs::read_to_string(path).unwrap();
        assert_eq!(text, format!("#EXTM3U\n#EXTINF:0,Ann - Song\n{p}\n/elsewhere/x.flac\n"));
        let many: Vec<String> = (0..30).map(|i| i.to_string()).collect();
        let (a, b) = (shuffled(&many, 7), shuffled(&many, 8));
        assert_ne!(a, many);
        assert_ne!(a, b);
        let mut sorted = a.clone();
        sorted.sort_by_key(|x| x.parse::<u32>().unwrap());
        assert_eq!(sorted, many, "a shuffle keeps every entry");
        assert_eq!(shuffled(&many, 7), a, "and is repeatable for one seed");
    }

    pub struct FakeRun(pub Arc<Mutex<(bool, bool)>>); // (alive, stopped)
    impl Running for FakeRun {
        fn alive(&mut self) -> bool {
            self.0.lock().unwrap().0
        }
        fn stop(&mut self) {
            let mut g = self.0.lock().unwrap();
            g.0 = false;
            g.1 = true;
        }
    }

    #[derive(Default)]
    pub struct FakeLauncher {
        pub launched: Mutex<Vec<String>>,
        pub procs: Mutex<Vec<Arc<Mutex<(bool, bool)>>>>,
        pub fail: Mutex<bool>,
    }
    impl Launcher for FakeLauncher {
        fn launch(&self, target: &str) -> Result<Box<dyn Running>, String> {
            if *self.fail.lock().unwrap() {
                return Err("no media player is installed".into());
            }
            self.launched.lock().unwrap().push(target.to_string());
            let st = Arc::new(Mutex::new((true, false)));
            self.procs.lock().unwrap().push(st.clone());
            Ok(Box::new(FakeRun(st)))
        }
    }

    #[test]
    fn the_player_starts_one_thing_stops_it_and_notices_its_end() {
        let l = Arc::new(FakeLauncher::default());
        let p = MediaPlayer::new(l.clone());
        assert_eq!(p.check(), Check::Idle);
        p.play("alice", "Song", "/m/a.mp3").unwrap();
        assert_eq!(p.now_playing().as_deref(), Some("Song"));
        p.play("alice", "Other", "/m/b.mp3").unwrap();
        assert!(l.procs.lock().unwrap()[0].lock().unwrap().1, "starting another stops the first");
        // the player quits by itself
        l.procs.lock().unwrap()[1].lock().unwrap().0 = false;
        assert_eq!(p.check(), Check::Ended { user: "alice".into() });
        assert_eq!(p.check(), Check::Idle);
        p.play("alice", "Third", "/m/c.mp3").unwrap();
        p.stop();
        assert!(l.procs.lock().unwrap()[2].lock().unwrap().1);
        assert_eq!(p.now_playing(), None);
        *l.fail.lock().unwrap() = true;
        assert!(p.play("alice", "x", "/y").unwrap_err().contains("no media player"));
    }

    #[test]
    fn a_real_player_process_can_be_stopped() {
        // `sleep` stands in for a player: the launcher is given a program that ignores its argument and stays alive
        let argv = vec!["sleep".to_string(), "30".to_string()];
        let Ok(mut child) = crate::voice::spawn_media_player(&argv[..1], "30") else { return };
        assert!(iyakki_core_api::child_status(&mut child).is_running());
        crate::voice::kill_child(&mut child);
        assert!(iyakki_core_api::child_status(&mut child).is_over());
    }

    /// The sample files that ship in `dist/samples` (and in every image under /usr/share/iyakki/samples).
    fn sample(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/samples").join(rel)
    }

    #[test]
    fn the_sample_mp3s_have_their_tags_and_decode_with_the_in_house_decoder() {
        for (file, title, genre, secs) in [("Music/IYAKKI Sample Melody.mp3", "IYAKKI Sample Melody", "melody", 26), ("Music/IYAKKI Sample Beats.mp3", "IYAKKI Sample Beats", "kuthu", 20)] {
            let p = sample(file);
            let t = describe_path(p.to_str().unwrap()).unwrap_or_else(|| panic!("{file} is a media file"));
            assert_eq!((t.title.as_str(), t.artist.as_str(), t.album.as_str(), t.genre.to_lowercase().as_str(), t.kind), (title, "IYAKKI", "IYAKKI Samples", genre, Kind::Audio), "{t:?}");
            assert!(t.seconds.is_some_and(|s| s.abs_diff(secs) <= 1), "{:?}", t.seconds);
            // the same decoder the radio and the MEDIA app use: every frame decodes, the length matches the tag
            let bytes = std::fs::read(&p).unwrap();
            assert!(bytes.len() < 400_000, "a small file");
            let skip = if bytes.starts_with(b"ID3") { 10 + ((bytes[6] as usize & 0x7f) << 21 | (bytes[7] as usize & 0x7f) << 14 | (bytes[8] as usize & 0x7f) << 7 | (bytes[9] as usize & 0x7f)) } else { 0 };
            let mut dec = nanomp3::Decoder::new();
            let mut pcm = vec![0i16; nanomp3::MAX_SAMPLES_PER_FRAME];
            let (mut pos, mut samples, mut rate, mut peak) = (skip, 0usize, 0u32, 0i16);
            while pos < bytes.len() {
                let (used, frame) = dec.decode(&bytes[pos..], &mut pcm);
                pos += used.max(1);
                if let Ok(f) = frame {
                    rate = f.sample_rate;
                    samples += f.samples_produced;
                    peak = peak.max(pcm[..f.samples_produced * f.channels as usize].iter().map(|v| v.saturating_abs()).max().unwrap_or(0));
                }
            }
            assert!(rate > 0 && (samples as f64 / rate as f64 - secs as f64).abs() < 1.0, "{file}: {} s decoded", samples as f64 / rate as f64);
            assert!(peak > 3000 && peak < 32700, "{file}: audible and not clipped, peak {peak}");
        }
        // the video is a WebM that the library scan lists as a video
        let v = describe_path(sample("Videos/IYAKKI Sample Video.webm").to_str().unwrap()).unwrap();
        assert_eq!((v.kind, v.title.as_str()), (Kind::Video, "IYAKKI Sample Video"));
        let total: u64 = ["Music/IYAKKI Sample Melody.mp3", "Music/IYAKKI Sample Beats.mp3", "Videos/IYAKKI Sample Video.webm"].iter().map(|f| std::fs::metadata(sample(f)).unwrap().len()).sum();
        assert!(total <= 4 << 20, "the samples are small: {total} bytes");
    }

    #[test]
    fn mp3_plays_through_the_in_house_player_and_everything_else_needs_an_external_one() {
        let out = temp("builtin").join("sound.wav");
        let radio = crate::radio::RadioPlayer::default();
        let b = Builtin { radio: radio.clone(), play_cmd: Arc::new({ let o = out.display().to_string(); move || format!("tee {o}") }) };
        // the first 3 seconds of the beats sample (a debug build decodes slowly)
        let part = out.with_file_name("part.mp3");
        std::fs::write(&part, &std::fs::read(sample("Music/IYAKKI Sample Beats.mp3")).unwrap()[..36_000]).unwrap();
        let mut run = launch_builtin(&b, part.to_str().unwrap()).unwrap();
        for _ in 0..1500 {
            if !run.alive() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!run.alive(), "the track played to its end");
        let wav = std::fs::read(&out).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert!(wav.len() > 44 + 2 * 44100 * 2, "seconds of 16-bit audio reached the sound command: {} bytes", wav.len());
        // a file that is not MP3 data says so instead of staying silent; a missing sound command too
        let junk = out.with_file_name("junk.mp3");
        std::fs::write(&junk, vec![9u8; 100_000]).unwrap();
        assert!(launch_builtin(&b, junk.to_str().unwrap()).is_err());
        let nocmd = Builtin { radio, play_cmd: Arc::new(|| "/nonexistent/player".to_string()) };
        assert!(launch_builtin(&nocmd, sample("Music/IYAKKI Sample Melody.mp3").to_str().unwrap()).is_err_and(|e| e.contains("could not play")));
        assert!(is_mp3("/a/B.MP3") && !is_mp3("/a/b.m4a") && !is_mp3("/a/mp3"));
        assert!(is_video_path("/a/b.WEBM") && is_video_path("c.mkv") && !is_video_path("c.mp3"));
    }

    #[test]
    fn the_sample_folder_is_read_only_scanned_and_readable_by_every_account() {
        let dir = Path::new("/usr/share/iyakki/samples");
        assert!(is_sample_in(dir, "/usr/share/iyakki/samples/Music/a.mp3") && is_sample_in(dir, "/usr/share/iyakki/samples/Videos/v.webm"));
        assert!(is_sample_in(dir, "/usr/share/iyakki/samples"), "the folder itself, so a scan can start in it");
        assert!(!is_sample_in(dir, "/usr/share/iyakki/samples/../../../etc/passwd") && !is_sample_in(dir, "/usr/share/iyakki/samples2/a.mp3") && !is_sample_in(dir, "/etc/passwd"));
        let own = vec!["/home/a/Music".to_string()];
        assert_eq!(scan_folders(&own, None), own);
        assert_eq!(scan_folders(&own, Some(dir)), vec!["/home/a/Music".to_string(), "/usr/share/iyakki/samples".to_string()]);
        assert_eq!(scan_folders(&scan_folders(&own, Some(dir)), Some(dir)).len(), 2, "not listed twice");
        // a scan of the real sample folder finds the three files whatever the account may read elsewhere
        let lib = scan(&[sample("").display().to_string()], &|_| true);
        let mut names: Vec<String> = lib.tracks.iter().map(|t| t.title.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["IYAKKI Sample Beats", "IYAKKI Sample Melody", "IYAKKI Sample Video"]);
        // the account's own scope does not decide for the samples (the real folder exists only on an image)
        let p = iyakki_sec::policy::Role::User.template();
        assert!(!may_read(&p, "alice", "/etc/passwd"));
    }
}
