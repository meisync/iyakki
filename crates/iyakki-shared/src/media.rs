//! Types moved out of `iyakki_agent::media` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Audio,
    Video,
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

impl Track {
    pub fn duration_text(&self) -> String {
        match self.seconds {
            Some(s) if s >= 3600 => format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60),
            Some(s) => format!("{}:{:02}", s / 60, s % 60),
            None => String::new(),
        }
    }

    pub fn matches(&self, q: &str) -> bool {
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

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Check {
    Idle,
    Playing { user: String, label: String },
    /// The player ended by itself since the last check.
    Ended { user: String },
}

pub const UNKNOWN_ARTIST: &str = "Unknown artist";

pub const UNKNOWN_ALBUM: &str = "Unknown album";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub items: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Saved {
    /// `None` until the person changes it: then the default folders apply.
    pub folders: Option<Vec<String>>,
    pub playlists: Vec<Playlist>,
    pub shuffle: bool,
    pub repeat: Repeat,
}

/// A person's folders, playlists and play flags, in a small file of their own.
pub struct MediaStore {
    pub path: PathBuf,
    pub dir: PathBuf,
    pub user: String,
}

pub fn default_folders(user: &str) -> Vec<String> {
    vec![format!("/home/{user}/Music"), format!("/home/{user}/Videos")]
}

pub fn valid_playlist_name(name: &str) -> Result<String, String> {
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

    pub fn load(&self) -> Saved {
        std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, s: &Saved) -> Result<(), String> {
        iyakki_schema::write_atomic(&self.path, serde_json::to_string_pretty(s).map_err(|e| e.to_string())?.as_bytes(), 0o600)
    }

    pub fn edit<T>(&self, f: impl FnOnce(&mut Saved) -> Result<T, String>) -> Result<T, String> {
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

    pub fn m3u_path(&self, name: &str) -> PathBuf {
        let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' }).collect();
        self.dir.join(format!("{safe}.m3u"))
    }

    /// Write `items` as an extended M3U file under the person's own media folder and return its path.
    pub fn write_m3u(&self, name: &str, items: &[String], lib: &Library) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let p = self.m3u_path(name);
        iyakki_schema::write_atomic(&p, m3u_text(items, lib).as_bytes(), 0o644)?;
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

/// A file name that the in-house player can play: MP3.
pub fn is_mp3(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, e)| e.eq_ignore_ascii_case("mp3"))
}

/// The name says video (by extension).
pub fn is_video_path(path: &str) -> bool {
    let e = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    EXTENSIONS.iter().any(|(x, k)| *x == e && *k == Kind::Video)
}

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

/// The play queue, kept with the player so the MEDIA app and the chat's tools see the same one. `rev` counts changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueState {
    pub user: String,
    pub items: Vec<String>,
    pub pos: usize,
    pub active: bool,
    pub rev: u64,
}

pub const MAX_PLAYLISTS: usize = 100;

pub const MAX_ITEMS: usize = 5_000;

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

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use std::io::Read;
use std::io::{Seek, SeekFrom};

pub const MAX_FILES: usize = 20_000;

pub const MAX_DEPTH: usize = 10;

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

pub fn ext_kind(path: &Path) -> Option<Kind> {
    let e = path.extension()?.to_str()?.to_ascii_lowercase();
    EXTENSIONS.iter().find(|(x, _)| *x == e).map(|(_, k)| *k)
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

pub fn walk(dir: &Path, depth: usize, allow: &dyn Fn(&str) -> bool, lib: &mut Library, seen: &mut std::collections::BTreeSet<String>) {
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
pub fn describe_file(p: &Path, ps: &str) -> Option<Track> {
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
            "ogg" | "opus" => {
                tags = ogg_tags(&mut f);
                seconds = ogg_seconds(&mut f, size);
            }
            "webm" | "mkv" => seconds = matroska_seconds(&mut f),
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

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u16>,
}

/// The first four-digit year in a date text such as `2019-05-01`.
pub fn year_of(s: &str) -> Option<u16> {
    let b = s.as_bytes();
    (0..b.len().saturating_sub(3)).find_map(|i| {
        let w = &b[i..i + 4];
        (w.iter().all(u8::is_ascii_digit) && (i == 0 || !b[i - 1].is_ascii_digit())).then(|| s[i..i + 4].parse::<u16>().ok()).flatten().filter(|y| (1900..=2100).contains(y))
    })
}

pub fn read_at(f: &mut std::fs::File, at: u64, len: usize) -> Vec<u8> {
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

pub fn clean(s: String) -> Option<String> {
    let s = s.trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
    (!s.is_empty()).then_some(s)
}

pub fn id3_text(b: &[u8]) -> Option<String> {
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

pub fn syncsafe(b: &[u8]) -> usize {
    b.iter().fold(0usize, |a, &x| (a << 7) | (x & 0x7F) as usize)
}

pub fn id3v1(f: &mut std::fs::File, size: u64) -> Tags {
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
pub fn mp3_seconds(f: &mut std::fs::File, size: u64) -> Option<u32> {
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

pub fn le32(b: &[u8], at: usize) -> Option<usize> {
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

pub fn ogg_tags(f: &mut std::fs::File) -> Tags {
    let b = read_at(f, 0, 64 * 1024);
    for (marker, skip) in [(&b"\x03vorbis"[..], 7usize), (&b"OpusTags"[..], 8)] {
        if let Some(i) = b.windows(marker.len()).position(|w| w == marker) {
            return vorbis_comments(&b[i + skip..]);
        }
    }
    Tags::default()
}

/// The length of an Ogg Vorbis or Opus file: the granule position of the last page over the sample rate
/// (Opus counts at 48 kHz and starts after its pre-skip). None when the file shows neither.
pub fn ogg_seconds(f: &mut std::fs::File, size: u64) -> Option<u32> {
    let head = read_at(f, 0, 4096);
    let (rate, skip) = if let Some(i) = head.windows(7).position(|w| w == b"\x01vorbis") {
        (le32(&head, i + 12)? as u64, 0u64)
    } else if let Some(i) = head.windows(8).position(|w| w == b"OpusHead") {
        let pre = u16::from_le_bytes([*head.get(i + 10)?, *head.get(i + 11)?]) as u64;
        (48_000, pre)
    } else {
        return None;
    };
    if rate == 0 {
        return None;
    }
    let tail_len = size.min(64 * 1024);
    let tail = read_at(f, size - tail_len, tail_len as usize);
    let at = tail.windows(4).rposition(|w| w == b"OggS")?;
    let g = tail.get(at + 6..at + 14)?;
    let granule = u64::from_le_bytes(g.try_into().ok()?);
    // -1 (all ones) means no packet ends on that page
    if granule == u64::MAX || granule <= skip {
        return None;
    }
    Some(((granule - skip) / rate) as u32)
}

/// One EBML variable-length number: its length in bytes, and its value (the length marker removed when `id` is false).
pub fn ebml_vint(b: &[u8], at: usize, id: bool) -> Option<(usize, u64)> {
    let first = *b.get(at)?;
    let n = first.leading_zeros() as usize + 1;
    if n > 8 {
        return None;
    }
    let mut v = if id { first as u64 } else { (first as u64) & ((1u64 << (8 - n)) - 1) };
    for k in 1..n {
        v = (v << 8) | *b.get(at + k)? as u64;
    }
    Some((n, v))
}

/// The length of a WebM or Matroska file from the Segment Info (Duration times TimecodeScale; the scale is 1 ms when absent).
pub fn matroska_seconds(f: &mut std::fs::File) -> Option<u32> {
    let b = read_at(f, 0, 256 * 1024);
    let info = b.windows(4).position(|w| w == [0x15, 0x49, 0xA9, 0x66])?;
    let (n, size) = ebml_vint(&b, info + 4, false)?;
    let start = info + 4 + n;
    let end = start.checked_add(size as usize)?.min(b.len());
    let (mut at, mut scale, mut dur) = (start, 1_000_000f64, None);
    while at < end {
        let (il, id) = ebml_vint(&b, at, true)?;
        let (sl, len) = ebml_vint(&b, at + il, false)?;
        let body = b.get(at + il + sl..at + il + sl + len as usize)?;
        match id {
            0x2AD7B1 => scale = body.iter().fold(0u64, |a, x| (a << 8) | *x as u64) as f64,
            0x4489 => {
                dur = match body.len() {
                    4 => Some(f32::from_be_bytes(body.try_into().ok()?) as f64),
                    8 => Some(f64::from_be_bytes(body.try_into().ok()?)),
                    _ => None,
                }
            }
            _ => {}
        }
        at += il + sl + len as usize;
    }
    let secs = dur? * scale / 1e9;
    (secs.is_finite() && secs > 0.0).then_some(secs as u32)
}

pub fn flac_info(f: &mut std::fs::File) -> (Tags, Option<u32>) {
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

pub fn wav_seconds(f: &mut std::fs::File) -> Option<u32> {
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

pub fn be32(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]) as usize)
}

/// The boxes of an MP4 region as `(type, content)`.
pub fn mp4_boxes(b: &[u8]) -> Vec<([u8; 4], &[u8])> {
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

pub fn mp4_child<'a>(b: &'a [u8], ty: &[u8; 4]) -> Option<&'a [u8]> {
    mp4_boxes(b).into_iter().find(|(t, _)| t == ty).map(|(_, c)| c)
}

/// Title, artist and album (`©nam`, `©ART`, `©alb`) and length from the `moov` box of an MP4 or M4A.
pub fn mp4_info(f: &mut std::fs::File, size: u64) -> (Tags, Option<u32>) {
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
