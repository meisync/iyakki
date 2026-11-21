//! Downloading a media file from an address into the person's library: a song, video or picture at a direct link. The host
//! must be one the account may reach (checked first, again at every redirect), the file goes only where the account may
//! write, is never allowed to overwrite another file, is checked by its first bytes (a web page is refused, whatever the
//! address says) and is capped in size. A finished file is added to the media index with the types, tags and rating chosen
//! at download time. Pages such as youtube.com/watch are not media files and are not scraped.

pub use iyakki_shared::mediadl::{ext_from_type, ext_of_url, kind_of_content_type, kind_of_ext, kind_of_url, split_words};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use iyakki_proxy::Streamer;
use iyakki_sec::policy::Policy;

use crate::mediaindex::Extra;

/// The default size cap (2 GB).
pub const DEFAULT_MAX_BYTES: u64 = 2 << 30;
const MAX_REDIRECTS: usize = 5;
pub const MAX_EXPANDED: usize = 50;

/// Hosts that serve web pages with media inside, not media files.
const PAGE_HOSTS: &[&str] = &["youtube.com", "www.youtube.com", "m.youtube.com", "youtu.be", "music.youtube.com", "vimeo.com", "www.vimeo.com", "dailymotion.com", "www.dailymotion.com", "open.spotify.com", "www.instagram.com", "www.facebook.com", "twitter.com", "x.com", "www.tiktok.com"];

#[derive(Default)]
pub struct Progress {
    pub bytes: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
    /// The folder the file is going to, once it is known (an automatic choice is made after the first response bytes).
    pub folder: Mutex<String>,
}

impl Progress {
    pub fn percent(&self) -> Option<u32> {
        let t = self.total.load(Ordering::Relaxed);
        (t > 0).then(|| (self.bytes.load(Ordering::Relaxed) * 100 / t).min(100) as u32)
    }
}

/// What a download found.
#[derive(Debug)]
pub enum Fetched {
    File { path: PathBuf, bytes: u64 },
    /// The address was a playlist or a podcast feed: the media addresses in it.
    Links(Vec<String>),
}


fn is_media_ext(e: &str) -> bool {
    crate::media::EXTENSIONS.iter().any(|(x, _)| *x == e) || crate::mediaindex::IMAGE_EXTENSIONS.contains(&e)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && b.len() >= i + 3 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A safe file name from the last part of an address: no folders, no hidden files, an extension that fits the content.
pub fn file_name_for(url: &str, ext: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').next().unwrap_or("");
    let decoded = percent_decode(last);
    let stem = decoded.rsplit_once('.').map_or(decoded.as_str(), |(s, e)| if e.len() <= 5 && is_media_ext(&e.to_ascii_lowercase()) { s } else { decoded.as_str() });
    let mut clean: String = stem.chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '(' | ')' | '.') { c } else { '_' }).collect();
    clean = clean.trim_matches(|c: char| c == '.' || c == '_' || c.is_whitespace()).chars().take(100).collect();
    if clean.is_empty() {
        clean = "download".into();
    }
    format!("{clean}.{ext}")
}

/// A file name that does not exist yet in `dir`: `name.mp3`, `name (2).mp3`, …
pub fn unique_name(dir: &Path, name: &str) -> String {
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    for n in 1..10_000 {
        let cand = if n == 1 { name.to_string() } else { format!("{stem} ({n}).{ext}") };
        if !dir.join(&cand).exists() && !dir.join(format!("{cand}.part")).exists() {
            return cand;
        }
    }
    format!("{stem}-{}.{ext}", crate::clock::now_unix())
}


fn looks_like_html(head: &[u8]) -> bool {
    let t = String::from_utf8_lossy(&head[..head.len().min(512)]).to_ascii_lowercase();
    let t = t.trim_start_matches(['\u{feff}', ' ', '\n', '\r', '\t']);
    t.starts_with("<!doctype html") || t.starts_with("<html") || t.starts_with("<head") || t.starts_with("<body") || t.contains("<html")
}

fn sniff_ext(head: &[u8]) -> Option<&'static str> {
    if let Some((e, _)) = crate::media::sniff(head) {
        return Some(e);
    }
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpg");
    }
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("png");
    }
    if head.starts_with(b"GIF8") {
        return Some("gif");
    }
    if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

/// Check an address before touching the network: web address, a host the account may reach, not a known video page.
pub fn check_url(policy: &Policy, url: &str) -> Result<String, String> {
    let url = url.trim();
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("that is not a web address".into());
    }
    let host = iyakki_sec::url_host(url).ok_or("that is not a web address (it must start with http:// or https://)")?;
    if !policy.allows_host(&host) {
        return Err(format!("your account may not reach {host}"));
    }
    if PAGE_HOSTS.contains(&host.as_str()) {
        return Err(format!("{host} serves web pages, not media files, and IYAKKI does not scrape them. Open it in the browser, or give the address of the file itself."));
    }
    Ok(host)
}

fn resolve_ref(base: &str, r: &str) -> Option<String> {
    let r = r.trim();
    if r.starts_with("http://") || r.starts_with("https://") {
        return Some(r.to_string());
    }
    let (scheme, rest) = base.split_once("://")?;
    let host_end = rest.find('/').unwrap_or(rest.len());
    let (host, path) = rest.split_at(host_end);
    if let Some(abs) = r.strip_prefix('/') {
        return Some(format!("{scheme}://{host}/{abs}"));
    }
    let dir = path.split(['?', '#']).next().unwrap_or("").rsplit_once('/').map_or("", |(d, _)| d);
    (!r.contains(':')).then(|| format!("{scheme}://{host}{dir}/{r}"))
}

/// The media addresses in an `.m3u`/`.pls` playlist or a podcast RSS feed (at most [`MAX_EXPANDED`]).
pub fn playlist_links(text: &str, base: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut push = |s: &str| {
        if let Some(u) = resolve_ref(base, &s.replace("&amp;", "&")) {
            if out.len() < MAX_EXPANDED && !out.contains(&u) {
                out.push(u);
            }
        }
    };
    if text.contains("<enclosure") || text.contains("<rss") {
        let mut rest = text;
        while let Some(i) = rest.find("<enclosure") {
            let tag = &rest[i..rest[i..].find('>').map_or(rest.len(), |e| i + e)];
            if let Some(u) = tag.split("url=").nth(1).and_then(|x| x.chars().next().map(|q| (q, &x[1..]))).and_then(|(q, x)| x.split(q).next()) {
                push(u);
            }
            rest = &rest[i + 10..];
        }
    } else {
        for l in text.lines() {
            let l = l.trim();
            if let Some((k, v)) = l.split_once('=') {
                if k.to_ascii_lowercase().starts_with("file") {
                    push(v);
                }
            } else if !l.is_empty() && !l.starts_with('#') && !l.starts_with('[') {
                push(l);
            }
        }
    }
    out
}

/// Where a download goes: one folder chosen by the person, or the matching one among their writable library folders,
/// decided from the kind of file (first from the address, then from what the server and the first bytes say).
#[derive(Clone, Copy, Debug)]
pub enum Target<'a> {
    Dir(&'a Path),
    Auto(&'a [String]),
}




/// The folder an automatic download of this kind goes to, among `folders` (writable ones only), made when it does not exist
/// yet (a new account has no Music or Videos folder).
pub fn auto_folder(folders: &[String], policy: &Policy, user: &str, kind: Option<crate::mediaindex::ItemKind>) -> Result<PathBuf, String> {
    let f = planned_folder(folders, policy, user, kind).ok_or("none of your library folders may be written to (outside your write scope)")?;
    folder_ready(policy, user, &f)
}

/// Where an automatic download of this kind would go, without making anything (for showing it before the download starts).
pub fn planned_folder(folders: &[String], policy: &Policy, user: &str, kind: Option<crate::mediaindex::ItemKind>) -> Option<PathBuf> {
    default_folder(folders, policy, user, kind.unwrap_or(crate::mediaindex::ItemKind::Song)).map(PathBuf::from)
}

/// The folder as a real path the account may write in. A folder that does not exist yet is made, but only after its real
/// place has been worked out (through the nearest folder that exists, so a link cannot lead out of the write scope) and
/// checked against the write scope.
fn folder_ready(policy: &Policy, user: &str, dir: &Path) -> Result<PathBuf, String> {
    let norm = iyakki_vfs::normalize(&dir.to_string_lossy()).map_err(|_| format!("invalid path: {}", dir.display()))?;
    let mut missing: Vec<std::ffi::OsString> = vec![];
    let mut cur = PathBuf::from(&norm);
    while !cur.exists() {
        missing.push(cur.file_name().ok_or("invalid path")?.to_owned());
        cur = cur.parent().ok_or("invalid path")?.to_path_buf();
    }
    let mut real = std::fs::canonicalize(&cur).map_err(|e| format!("no such directory: {}: {e}", cur.display()))?;
    for m in missing.iter().rev() {
        real.push(m);
    }
    crate::scope::check_write(policy, user, &real.join("x"))?;
    if !missing.is_empty() {
        std::fs::create_dir_all(&real).map_err(|e| format!("cannot make the folder {}: {e}", real.display()))?;
    }
    if !real.is_dir() {
        return Err(format!("{} is not a folder", real.display()));
    }
    Ok(real)
}

/// Fetch `url` into `dir`. See the module notes for the checks.
#[allow(clippy::too_many_arguments)]
pub fn download(streamer: &dyn Streamer, policy: &Policy, user: &str, url: &str, dir: &Path, max_bytes: u64, progress: &Progress) -> Result<Fetched, String> {
    download_to(streamer, policy, user, url, Target::Dir(dir), max_bytes, progress)
}

/// Fetch `url` into the folder `target` names. With [`Target::Auto`] the folder follows the kind of file.
#[allow(clippy::too_many_arguments)]
pub fn download_to(streamer: &dyn Streamer, policy: &Policy, user: &str, url: &str, target: Target, max_bytes: u64, progress: &Progress) -> Result<Fetched, String> {
    download_impl(streamer, policy, user, url, target, max_bytes, progress, true)
}

/// What is left of an earlier download of the same address that broke off: the `.part` file and a note next to it
/// (`<name>.part.src`: the address, the server's validator and the full size) so the next try asks only for the rest.
struct Resume {
    dir: PathBuf,
    name: String,
    part: PathBuf,
    offset: u64,
    validator: String,
}

impl Resume {
    fn note_path(part: &Path) -> PathBuf {
        PathBuf::from(format!("{}.src", part.display()))
    }
    fn discard(&self) {
        let _ = std::fs::remove_file(&self.part);
        let _ = std::fs::remove_file(Resume::note_path(&self.part));
    }
}

/// Look in `dirs` for a broken download of exactly this address that can be continued.
fn find_resume(dirs: &[PathBuf], url: &str) -> Option<Resume> {
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().into_owned();
            let Some(name) = fname.strip_suffix(".part.src") else { continue };
            let Ok(note) = std::fs::read_to_string(e.path()) else { continue };
            let mut lines = note.lines();
            if lines.next() != Some("iyakki-resume 1") || lines.next() != Some(url) {
                continue;
            }
            let validator = lines.next().unwrap_or("").to_string();
            let part = dir.join(format!("{name}.part"));
            let offset = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            if validator.is_empty() || offset == 0 {
                continue;
            }
            // the validator goes into a request header as it is: a note that holds anything but plain visible text (a bare carriage return
            // could start another header line) or a name that is not a plain file name is not ours (found by the fuzz test)
            if validator.len() > 200 || !validator.bytes().all(|b| (0x20..0x7f).contains(&b)) || name.is_empty() || name == "." || name == ".." {
                continue;
            }
            return Some(Resume { dir: dir.clone(), name: name.to_string(), part, offset, validator });
        }
    }
    None
}

/// A broken download that nobody continued is forgotten after this long.
const STALE_PART_AGE: std::time::Duration = std::time::Duration::from_secs(14 * 24 * 3600);

/// Remove the `.part` file and its `.part.src` note when the note is older than `max_age` at `now` (a note with no `.part` beside it goes
/// too). Only files with those two endings are touched. Returns how many notes were removed.
fn clean_stale_parts(dirs: &[PathBuf], max_age: std::time::Duration, now: std::time::SystemTime) -> usize {
    let mut removed = 0;
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().into_owned();
            let Some(name) = fname.strip_suffix(".part.src") else { continue };
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > max_age);
            let part = dir.join(format!("{name}.part"));
            if old || !part.exists() {
                let _ = std::fs::remove_file(&part);
                if std::fs::remove_file(e.path()).is_ok() {
                    removed += 1;
                }
            }
        }
    }
    removed
}

/// The validator a server gives for `If-Range`: a strong entity tag, else the modification date.
fn validator_of(h: &iyakki_proxy::StreamHead) -> Option<String> {
    h.header("etag").filter(|t| !t.starts_with("W/")).or_else(|| h.header("last-modified")).map(str::to_string).filter(|v| !v.is_empty() && !v.contains('\n'))
}

/// `Content-Range: bytes 100-999/1000` gives (100, Some(1000)).
fn content_range(v: &str) -> Option<(u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (a, _) = range.split_once('-')?;
    Some((a.trim().parse().ok()?, total.trim().parse().ok()))
}

enum Stop {
    Cancelled,
    Net(String),
    TooBig,
    Disk(String),
}

/// Copy the body (after `pending`, bytes already read) into `f`; `start` bytes were written by an earlier try.
fn pump(body: &mut dyn Read, pending: &[u8], f: &mut std::fs::File, start: u64, max_bytes: u64, progress: &Progress) -> Result<u64, Stop> {
    let mut total = start;
    let mut chunk = vec![0u8; 64 * 1024];
    let mut pending: &[u8] = pending;
    loop {
        if progress.cancel.load(Ordering::SeqCst) {
            return Err(Stop::Cancelled);
        }
        let n = if !pending.is_empty() {
            let n = pending.len();
            chunk[..n].copy_from_slice(pending);
            pending = &[];
            n
        } else {
            match body.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => return Err(Stop::Net(e.to_string())),
            }
        };
        total += n as u64;
        if total > max_bytes {
            return Err(Stop::TooBig);
        }
        if let Err(e) = f.write_all(&chunk[..n]) {
            return Err(Stop::Disk(e.to_string()));
        }
        progress.bytes.store(total, Ordering::Relaxed);
    }
    Ok(total)
}

/// The words for a stop, and the part file's fate: only a broken connection with a validator keeps what arrived.
fn settle(stop: Stop, part: &Path, keep: Option<(&str, &str)>, max_bytes: u64) -> String {
    let (msg, resumable) = match stop {
        Stop::Cancelled => ("cancelled".to_string(), false),
        Stop::Net(e) => (format!("the download failed: {e}"), true),
        Stop::TooBig => (format!("the file is larger than the {} MB limit; stopped", max_bytes >> 20), false),
        Stop::Disk(e) => (format!("cannot write the file: {e}"), false),
    };
    match keep {
        Some((url, validator)) if resumable && std::fs::metadata(part).map(|m| m.len() > 0).unwrap_or(false) => {
            let _ = std::fs::write(Resume::note_path(part), format!("iyakki-resume 1\n{url}\n{validator}\n"));
        }
        _ => {
            let _ = std::fs::remove_file(part);
            let _ = std::fs::remove_file(Resume::note_path(part));
        }
    }
    msg
}

#[allow(clippy::too_many_arguments)]
fn download_impl(streamer: &dyn Streamer, policy: &Policy, user: &str, url: &str, target: Target, max_bytes: u64, progress: &Progress, resume_ok: bool) -> Result<Fetched, String> {
    let mut url = url.trim().to_string();
    let asked_url = url.clone();
    check_url(policy, &url)?;
    // write scope first: nothing is fetched for a folder the account may not write (an automatic choice is made when the
    // address already says what kind of file it is, otherwise once the first bytes arrive)
    let mut dir: Option<PathBuf> = match target {
        Target::Dir(d) => Some(folder_ready(policy, user, d)?),
        Target::Auto(f) => match kind_of_url(&url) {
            Some(k) => Some(auto_folder(f, policy, user, Some(k))?),
            None => None,
        },
    };
    if let Some(d) = &dir {
        *progress.folder.lock().unwrap_or_else(|e| e.into_inner()) = d.to_string_lossy().into_owned();
    }
    let mut headers = vec![("Accept".to_string(), "audio/*,video/*,image/*,application/octet-stream;q=0.8,*/*;q=0.5".to_string())];
    // a broken download of this very address is continued from where it stopped (only the rest is asked for)
    let resume = if resume_ok {
        let dirs: Vec<PathBuf> = match (&dir, target) {
            (Some(d), _) => vec![d.clone()],
            (None, Target::Auto(f)) => f.iter().filter(|p| Path::new(p).is_dir()).filter_map(|p| folder_ready(policy, user, Path::new(p)).ok()).collect(),
            (None, Target::Dir(_)) => vec![],
        };
        clean_stale_parts(&dirs, STALE_PART_AGE, std::time::SystemTime::now());
        find_resume(&dirs, &asked_url)
    } else {
        None
    };
    if let Some(r) = &resume {
        headers.push(("Range".to_string(), format!("bytes={}-", r.offset)));
        headers.push(("If-Range".to_string(), r.validator.clone()));
    }
    let mut head = None;
    for _ in 0..=MAX_REDIRECTS {
        let h = streamer.open(&url, &headers)?;
        if (300..400).contains(&h.status) {
            let loc = h.header("location").ok_or("the server redirected without saying where")?.to_string();
            url = resolve_ref(&url, &loc).ok_or("the server redirected to an address that cannot be used")?;
            check_url(policy, &url)?;
            continue;
        }
        head = Some(h);
        break;
    }
    let mut h = head.ok_or("too many redirects")?;
    if let Some(r) = resume {
        let range = if h.status == 206 { h.header("content-range").and_then(content_range) } else { None };
        match range {
            Some((start, total)) if start == r.offset => {
                if let Some(t) = total {
                    if t > max_bytes {
                        r.discard();
                        return Err(format!("the file is {} MB, more than the {} MB limit", t >> 20, max_bytes >> 20));
                    }
                    progress.total.store(t, Ordering::Relaxed);
                }
                *progress.folder.lock().unwrap_or_else(|e| e.into_inner()) = r.dir.to_string_lossy().into_owned();
                progress.bytes.store(r.offset, Ordering::Relaxed);
                let mut f = std::fs::OpenOptions::new().append(true).open(&r.part).map_err(|e| format!("cannot write in {}: {e}", r.dir.display()))?;
                let keep = validator_of(&h).unwrap_or_else(|| r.validator.clone());
                let end = match pump(&mut h.body, &[], &mut f, r.offset, max_bytes, progress) {
                    Ok(n) => n,
                    Err(stop) => {
                        drop(f);
                        return Err(settle(stop, &r.part, Some((&asked_url, &keep)), max_bytes));
                    }
                };
                drop(f);
                if total.is_some_and(|t| t != end) {
                    return Err(settle(Stop::Net("the connection ended early".into()), &r.part, Some((&asked_url, &keep)), max_bytes));
                }
                let dest = r.dir.join(&r.name);
                if let Err(e) = std::fs::hard_link(&r.part, &dest) {
                    r.discard();
                    return Err(format!("cannot keep the file: {e}"));
                }
                r.discard();
                return Ok(Fetched::File { path: dest, bytes: end });
            }
            _ => {
                // the file changed, the server ignores ranges or the range is wrong: what was kept is no use; ask for the whole file
                r.discard();
                if h.status == 206 || h.status == 416 {
                    drop(h);
                    return download_impl(streamer, policy, user, &asked_url, target, max_bytes, progress, false);
                }
            }
        }
    }
    if h.status != 200 {
        return Err(format!("the server answered {}", h.status));
    }
    let ct = h.header("content-type").unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if ct == "text/html" || ct == "application/xhtml+xml" {
        return Err("that address is a web page, not a media file".into());
    }
    let declared: u64 = h.header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if declared > max_bytes {
        return Err(format!("the file is {} MB, more than the {} MB limit", declared >> 20, max_bytes >> 20));
    }
    progress.total.store(declared, Ordering::Relaxed);
    let mut first = vec![0u8; 8192];
    let mut got = 0;
    while got < first.len() {
        match h.body.read(&mut first[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) => return Err(format!("the download failed: {e}")),
        }
        if got >= 64 {
            break;
        }
    }
    first.truncate(got);
    if first.is_empty() {
        return Err("the server sent an empty file".into());
    }
    if looks_like_html(&first) {
        return Err("that address is a web page, not a media file".into());
    }
    let url_ext = ext_of_url(&url).filter(|e| is_media_ext(e));
    let playlist_like = matches!(url_ext_any(&url).as_deref(), Some("m3u" | "m3u8" | "pls" | "rss" | "xml")) || matches!(ct.as_str(), "audio/x-mpegurl" | "audio/mpegurl" | "audio/x-scpls" | "application/rss+xml" | "application/xml" | "text/xml" | "application/vnd.apple.mpegurl");
    let sniffed = sniff_ext(&first);
    if sniffed.is_none() && playlist_like {
        let mut text = first.clone();
        h.body.by_ref().take(1 << 20).read_to_end(&mut text).map_err(|e| format!("the download failed: {e}"))?;
        let links = playlist_links(&String::from_utf8_lossy(&text), &url);
        let links: Vec<String> = links.into_iter().filter(|l| check_url(policy, l).is_ok()).collect();
        return if links.is_empty() { Err("that playlist or feed has no media addresses in it".into()) } else { Ok(Fetched::Links(links)) };
    }
    let ext = match (sniffed, url_ext.as_deref().or_else(|| ext_from_type(&ct))) {
        (Some(s), _) => s.to_string(),
        // an unknown signature is accepted only when the address or the content type names a media file
        (None, Some(e)) if is_media_ext(e) => e.to_string(),
        _ => return Err("that file is not a song, video or picture IYAKKI recognises".into()),
    };
    // the kind is known now (first bytes, then the address, then the content type): an automatic folder is chosen
    let dir = match (dir.take(), target) {
        (Some(d), Target::Auto(f)) => {
            // the address guessed a kind; the bytes may say otherwise (a .mp3 address that serves a video)
            let kind = kind_of_ext(&ext);
            match (kind, kind_of_url(&url)) {
                (Some(a), Some(b)) if a != b => auto_folder(f, policy, user, Some(a))?,
                _ => d,
            }
        }
        (Some(d), _) => d,
        (None, Target::Auto(f)) => auto_folder(f, policy, user, kind_of_ext(&ext).or_else(|| kind_of_content_type(&ct)))?,
        (None, Target::Dir(d)) => folder_ready(policy, user, d)?,
    };
    *progress.folder.lock().unwrap_or_else(|e| e.into_inner()) = dir.to_string_lossy().into_owned();
    let name = unique_name(&dir, &file_name_for(&url, &ext));
    let part = dir.join(format!("{name}.part"));
    let mut f = { use std::os::unix::fs::OpenOptionsExt; std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&part) }.map_err(|e| format!("cannot write in {}: {e}", dir.display()))?;
    let fail = |part: &Path, msg: String| -> Result<Fetched, String> {
        let _ = std::fs::remove_file(part);
        Err(msg)
    };
    let keep = validator_of(&h);
    let total = match pump(&mut h.body, &first, &mut f, 0, max_bytes, progress) {
        Ok(n) if declared > 0 && n != declared => {
            drop(f);
            return Err(settle(Stop::Net("the connection ended early".into()), &part, keep.as_deref().map(|v| (asked_url.as_str(), v)), max_bytes));
        }
        Ok(n) => n,
        Err(stop) => {
            drop(f);
            return Err(settle(stop, &part, keep.as_deref().map(|v| (asked_url.as_str(), v)), max_bytes));
        }
    };
    let _ = f.flush();
    drop(f);
    let dest = dir.join(&name);
    // a hard link fails when the name exists: a file is never replaced
    if let Err(e) = std::fs::hard_link(&part, &dest) {
        return fail(&part, format!("cannot keep the file: {e}"));
    }
    let _ = std::fs::remove_file(&part);
    Ok(Fetched::File { path: dest, bytes: total })
}

fn url_ext_any(url: &str) -> Option<String> {
    ext_of_url(url)
}

/// What the person chose for the new file.
#[derive(Clone, Debug, Default)]
pub struct Choice {
    pub types: Vec<String>,
    pub tags: Vec<String>,
    pub rating: u8,
}


impl Choice {
    /// A free-text category field (any words, comma separated) and tags: a word the vocabulary knows (kutthu, melody,
    /// bhajan, tamil…) becomes a type of the item, any other word (action, horror, my road trip…) becomes a tag, so that
    /// nothing a person types is ever refused.
    pub fn from_words(categories: &[String], tags: &[String], rating: u8) -> Choice {
        let mut c = Choice { rating: rating.min(5), ..Default::default() };
        let known = |w: &str| crate::mediaindex::canonical_type(w).is_some() || crate::mediaindex::LANGUAGES.iter().any(|l| l.1.contains(&w.to_lowercase().as_str()));
        for w in categories {
            let w: String = w.trim().chars().filter(|c| !c.is_control()).take(40).collect();
            if w.is_empty() {
                continue;
            }
            if known(&w) {
                c.types.push(w);
            } else {
                c.tags.push(w.to_lowercase());
            }
        }
        c.tags.extend(tags.iter().map(|t| t.trim().chars().filter(|c| !c.is_control()).take(40).collect::<String>()).filter(|t| !t.is_empty()));
        c
    }
}

/// Download on the calling thread into one folder (see [`run_blocking_to`]).
#[allow(clippy::too_many_arguments)]
pub fn run_blocking(svc: &Arc<crate::services::Services>, streamer: &dyn Streamer, policy: &Policy, user: &str, url: &str, dir: &Path, choice: &Choice, max_bytes: u64, progress: &Progress) -> Result<Fetched, String> {
    run_blocking_to(svc, streamer, policy, user, url, Target::Dir(dir), choice, max_bytes, progress)
}

/// Download on the calling thread (the download queue's worker runs this): the file, its index entry with what the person
/// chose, and a note in the inbox.
#[allow(clippy::too_many_arguments)]
pub fn run_blocking_to(svc: &Arc<crate::services::Services>, streamer: &dyn Streamer, policy: &Policy, user: &str, url: &str, target: Target, choice: &Choice, max_bytes: u64, progress: &Progress) -> Result<Fetched, String> {
    let r = download_to(streamer, policy, user, url, target, max_bytes, progress);
    let shown: String = url.chars().take(80).collect();
    match &r {
        Ok(Fetched::File { path, bytes }) => {
            let extra = Extra { types: choice.types.clone(), tags: choice.tags.clone(), rating: choice.rating, source_url: url.to_string() };
            let ix = svc.media_index(user);
            let added = {
                let mut g = ix.lock().unwrap_or_else(|e| e.into_inner());
                let r = g.add_file(&path.to_string_lossy(), &extra, crate::clock::now_unix());
                let _ = g.save();
                r
            };
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            crate::appevent_more::transfer_finished(&*svc.app_events, user, "download", name, true);
            let _ = svc.notify(user, crate::inbox::Kind::Download, &format!("Downloaded {name}"), &format!("{} KB from {shown}{}", bytes / 1024, if added.is_err() { " (not added to the library index)" } else { "" }), Some(crate::inbox::Link::new("media", "")));
        }
        Ok(Fetched::Links(_)) => {}
        Err(e) if e != "cancelled" => {
            crate::appevent_more::transfer_finished(&*svc.app_events, user, "download", "media", false);
            let _ = svc.notify(user, crate::inbox::Kind::Error, "Download failed", &format!("{shown}: {e}"), Some(crate::inbox::Link::new("media", "")));
        }
        Err(_) => {}
    }
    r
}

/// The folder a download goes to by default for a kind of file, among the person's writable library folders.
pub fn default_folder(folders: &[String], policy: &Policy, user: &str, kind: crate::mediaindex::ItemKind) -> Option<String> {
    let want = match kind {
        crate::mediaindex::ItemKind::Video => "videos",
        crate::mediaindex::ItemKind::Image => "pictures",
        _ => "music",
    };
    let writable: Vec<&String> = folders.iter().filter(|f| policy.allows_write(user, &format!("{f}/x"))).collect();
    writable.iter().find(|f| f.rsplit('/').next().is_some_and(|l| l.eq_ignore_ascii_case(want))).or(writable.first()).map(|f| f.to_string())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use iyakki_sec::policy::Role;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A tiny HTTP server: path → (content type, body), plus redirects. Returns its base address.
    pub fn serve(routes: Vec<(&'static str, &'static str, Vec<u8>)>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let routes = routes.clone();
                std::thread::spawn(move || {
                    let mut s = s;
                    let mut buf = [0u8; 4096];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let r = routes.iter().find(|r| r.0 == path);
                    let out: Vec<u8> = match r {
                        Some((_, ct, body)) if ct.starts_with("redirect:") => format!("HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", &ct[9..]).into_bytes().into_iter().chain(body.clone()).collect(),
                        Some((_, ct, body)) => [format!("HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes(), body.clone()].concat(),
                        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = s.write_all(&out);
                });
            }
        });
        format!("http://{addr}")
    }

    pub fn streamer() -> iyakki_proxy::https::UreqStreamer {
        iyakki_proxy::https::UreqStreamer::default()
    }

    pub fn policy_for(dir: &Path) -> Policy {
        let mut p = Role::User.template();
        p.fs_read = vec![dir.display().to_string()];
        p.fs_write = vec![dir.display().to_string()];
        p.net_hosts = vec!["127.0.0.1".into()];
        p
    }

    pub fn tiny_mp3() -> Vec<u8> {
        crate::media::tests::mp3(&[("TIT2", "Remote Song"), ("TPE1", "Net Artist")], 40)
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-dl-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::canonicalize(&d).unwrap()
    }

    #[test]
    fn names_are_safe_unique_and_never_overwrite() {
        assert_eq!(file_name_for("https://x.test/a/My%20Song.MP3?token=1", "mp3"), "My Song.mp3");
        assert_eq!(file_name_for("https://x.test/..%2F..%2Fetc%2Fpasswd", "mp3"), "etc_passwd.mp3");
        assert_eq!(file_name_for("https://x.test/", "ogg"), "download.ogg");
        assert_eq!(file_name_for("https://x.test/.hidden", "png"), "hidden.png");
        let d = tmp("names");
        std::fs::write(d.join("a.mp3"), b"x").unwrap();
        assert_eq!(unique_name(&d, "a.mp3"), "a (2).mp3");
        assert_eq!(unique_name(&d, "b.mp3"), "b.mp3");
    }

    #[test]
    fn playlists_and_feeds_expand_to_media_addresses_capped_at_fifty() {
        let m3u = "#EXTM3U\n#EXTINF:1,x\nsong1.mp3\nhttp://other.test/two.mp3\n/abs/three.mp3\n";
        assert_eq!(playlist_links(m3u, "http://h.test/lists/p.m3u"), vec!["http://h.test/lists/song1.mp3", "http://other.test/two.mp3", "http://h.test/abs/three.mp3"]);
        assert_eq!(playlist_links("[playlist]\nFile1=http://a.test/1.mp3\nTitle1=x\nFile2=http://a.test/2.mp3\n", "http://h.test/p.pls").len(), 2);
        let rss = "<rss><channel><item><enclosure url=\"http://p.test/e1.mp3?a=1&amp;b=2\" type=\"audio/mpeg\"/></item><item><enclosure length=\"5\" url='http://p.test/e2.mp3'/></item></channel></rss>";
        assert_eq!(playlist_links(rss, "http://p.test/feed"), vec!["http://p.test/e1.mp3?a=1&b=2", "http://p.test/e2.mp3"]);
        let many: String = (0..80).map(|i| format!("http://a.test/{i}.mp3\n")).collect();
        assert_eq!(playlist_links(&many, "http://a.test/").len(), MAX_EXPANDED);
    }

    #[test]
    fn a_direct_file_is_downloaded_named_safely_and_never_overwrites() {
        let base = serve(vec![("/dir/Remote%20Song.mp3", "audio/mpeg", tiny_mp3()), ("/old", "redirect:/dir/Remote%20Song.mp3", vec![]), ("/pic.png", "image/png", b"\x89PNG\r\n\x1a\n0000IHDR".to_vec())]);
        let d = tmp("ok");
        let p = policy_for(&d);
        let prog = Progress::default();
        let Fetched::File { path, bytes } = download(&streamer(), &p, "alice", &format!("{base}/dir/Remote%20Song.mp3"), &d, DEFAULT_MAX_BYTES, &prog).unwrap() else { panic!() };
        assert_eq!(path, d.join("Remote Song.mp3"));
        assert_eq!(std::fs::read(&path).unwrap().len() as u64, bytes);
        assert_eq!(prog.percent(), Some(100));
        // the same address again: a new name, the first file untouched
        let Fetched::File { path: second, .. } = download(&streamer(), &p, "alice", &format!("{base}/old"), &d, DEFAULT_MAX_BYTES, &prog).unwrap() else { panic!() };
        assert_eq!(second, d.join("Remote Song (2).mp3"));
        let Fetched::File { path: pic, .. } = download(&streamer(), &p, "alice", &format!("{base}/pic.png"), &d, DEFAULT_MAX_BYTES, &prog).unwrap() else { panic!() };
        assert!(pic.ends_with("pic.png"));
        assert!(std::fs::read_dir(&d).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".part")), "no partial files left");
    }

    #[test]
    fn web_pages_are_refused_whatever_the_address_says() {
        let page = b"<!DOCTYPE html><html><body>hello</body></html>".to_vec();
        let base = serve(vec![("/song.mp3", "audio/mpeg", page.clone()), ("/page", "text/html; charset=utf-8", page.clone()), ("/blob", "application/octet-stream", page), ("/text", "application/octet-stream", b"just some words".to_vec())]);
        let d = tmp("html");
        let p = policy_for(&d);
        for path in ["/song.mp3", "/page", "/blob"] {
            let e = download(&streamer(), &p, "alice", &format!("{base}{path}"), &d, DEFAULT_MAX_BYTES, &Progress::default()).unwrap_err();
            assert!(e.contains("web page, not a media file"), "{path}: {e}");
        }
        assert!(download(&streamer(), &p, "alice", &format!("{base}/text"), &d, DEFAULT_MAX_BYTES, &Progress::default()).unwrap_err().contains("not a song, video or picture"));
        assert!(download(&streamer(), &p, "alice", &format!("{base}/missing.mp3"), &d, DEFAULT_MAX_BYTES, &Progress::default()).unwrap_err().contains("404"));
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0, "nothing was kept");
        // video sites are pages
        let mut open = p.clone();
        open.net_hosts = vec!["*".into()];
        let e = download(&streamer(), &open, "alice", "https://www.youtube.com/watch?v=abc", &d, DEFAULT_MAX_BYTES, &Progress::default()).unwrap_err();
        assert!(e.contains("not scrape") && e.contains("browser"), "{e}");
    }

    #[test]
    fn size_cap_cancel_scope_and_host_policy_are_enforced() {
        let big = [tiny_mp3(), vec![0u8; 200_000]].concat();
        let base = serve(vec![("/big.mp3", "audio/mpeg", big)]);
        let d = tmp("limits");
        let p = policy_for(&d);
        let e = download(&streamer(), &p, "alice", &format!("{base}/big.mp3"), &d, 50_000, &Progress::default()).unwrap_err();
        assert!(e.contains("limit"), "{e}");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0, "the partial file is removed");
        // cancelled before the first chunk is written
        let prog = Progress::default();
        prog.cancel.store(true, Ordering::SeqCst);
        assert_eq!(download(&streamer(), &p, "alice", &format!("{base}/big.mp3"), &d, DEFAULT_MAX_BYTES, &prog).unwrap_err(), "cancelled");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
        // a folder outside the write scope
        let other = tmp("elsewhere");
        let e = download(&streamer(), &p, "alice", &format!("{base}/big.mp3"), &other, DEFAULT_MAX_BYTES, &Progress::default()).unwrap_err();
        assert!(e.contains("permission denied"), "{e}");
        assert_eq!(std::fs::read_dir(&other).unwrap().count(), 0);
        // a host the account may not reach is refused before any request
        let mut nohost = p.clone();
        nohost.net_hosts = vec!["example.com".into()];
        assert!(download(&streamer(), &nohost, "alice", &format!("{base}/big.mp3"), &d, DEFAULT_MAX_BYTES, &Progress::default()).unwrap_err().contains("may not reach 127.0.0.1"));
        assert!(check_url(&p, "ftp://x/y").is_err() && check_url(&p, "http://127.0.0.1/a b").is_err());
        // a redirect to a host that is not allowed is refused too
        let base2 = serve(vec![("/r", "redirect:http://evil.test/x.mp3", vec![])]);
        assert!(download(&streamer(), &p, "alice", &format!("{base2}/r"), &d, DEFAULT_MAX_BYTES, &Progress::default()).unwrap_err().contains("may not reach evil.test"));
    }

    #[test]
    fn a_playlist_address_gives_its_links_instead_of_a_file() {
        let base = serve(vec![("/list.m3u", "audio/x-mpegurl", b"#EXTM3U\none.mp3\ntwo.mp3\n".to_vec())]);
        let d = tmp("m3u");
        let p = policy_for(&d);
        let Fetched::Links(l) = download(&streamer(), &p, "alice", &format!("{base}/list.m3u"), &d, DEFAULT_MAX_BYTES, &Progress::default()).unwrap() else { panic!() };
        assert_eq!(l, vec![format!("{base}/one.mp3"), format!("{base}/two.mp3")]);
    }

    #[test]
    fn a_missing_library_folder_is_made_inside_the_write_scope_and_never_outside_it() {
        let d = tmp("mkdir");
        let p = policy_for(&d);
        let base = serve(vec![("/s.mp3", "audio/mpeg", tiny_mp3())]);
        // a new account has no Music folder yet: the first download makes it (and the folders above it, inside the scope)
        let folders = vec![format!("{}/home/ann/Music", d.display()), format!("{}/home/ann/Videos", d.display())];
        let Fetched::File { path, .. } = download_to(&streamer(), &p, "ann", &format!("{base}/s.mp3"), Target::Auto(&folders), DEFAULT_MAX_BYTES, &Progress::default()).unwrap() else { panic!() };
        assert_eq!(path, d.join("home/ann/Music/s.mp3"));
        assert!(!d.join("home/ann/Videos").exists(), "only the folder that is used is made");
        // showing the plan makes nothing
        assert_eq!(planned_folder(&folders, &p, "ann", Some(crate::mediaindex::ItemKind::Video)), Some(PathBuf::from(&folders[1])));
        assert!(!d.join("home/ann/Videos").exists());
        // a folder outside the write scope is not made, nor one reached through a link out of the scope
        let outside = tmp("mkdir-out");
        let e = download_to(&streamer(), &p, "ann", &format!("{base}/s.mp3"), Target::Dir(&outside.join("new")), DEFAULT_MAX_BYTES, &Progress::default()).unwrap_err();
        assert!(e.contains("permission denied"), "{e}");
        assert!(!outside.join("new").exists());
        std::os::unix::fs::symlink(&outside, d.join("home/ann/escape")).unwrap();
        let e = download_to(&streamer(), &p, "ann", &format!("{base}/s.mp3"), Target::Dir(&d.join("home/ann/escape/sub")), DEFAULT_MAX_BYTES, &Progress::default()).unwrap_err();
        assert!(e.contains("permission denied"), "{e}");
        assert!(!outside.join("sub").exists());
        // a file where a folder should be
        std::fs::write(d.join("home/ann/file"), b"x").unwrap();
        assert!(download_to(&streamer(), &p, "ann", &format!("{base}/s.mp3"), Target::Dir(&d.join("home/ann/file")), DEFAULT_MAX_BYTES, &Progress::default()).unwrap_err().contains("not a folder"));
    }

    #[test]
    fn the_default_folder_follows_the_kind_among_writable_folders() {
        let d = tmp("folders");
        let p = policy_for(&d);
        let f = vec![format!("{}/Videos", d.display()), format!("{}/Music", d.display()), format!("{}/Pictures", d.display()), "/elsewhere/Music".to_string()];
        use crate::mediaindex::ItemKind::*;
        assert!(default_folder(&f, &p, "alice", Song).unwrap().ends_with("/Music") && default_folder(&f, &p, "alice", Song).unwrap().starts_with(&d.display().to_string()));
        assert!(default_folder(&f, &p, "alice", Video).unwrap().ends_with("/Videos"));
        assert!(default_folder(&f, &p, "alice", Image).unwrap().ends_with("/Pictures"));
        assert_eq!(default_folder(&["/elsewhere/Music".to_string()], &p, "alice", Song), None);
    }

    #[derive(Clone, Copy, PartialEq)]
    pub enum RangeMode {
        Honour,
        Ignore,
        Reject416,
    }

    /// A server that cuts the first answer after `cut` bytes, then answers `Range` requests the way `mode` says. Returns the base
    /// address and the `Range` header of each request (empty text when there was none).
    pub fn serve_ranged(body: Vec<u8>, cut: usize, mode: RangeMode) -> (String, Arc<Mutex<Vec<String>>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut s = s;
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let range = req.lines().find(|l| l.to_ascii_lowercase().starts_with("range:")).map(|l| l[6..].trim().to_string()).unwrap_or_default();
                let first = { let mut g = seen2.lock().unwrap(); g.push(range.clone()); g.len() == 1 };
                let total = body.len();
                let start: usize = range.strip_prefix("bytes=").and_then(|r| r.trim_end_matches('-').parse().ok()).unwrap_or(0);
                let head = |code: &str, extra: &str, len: usize| format!("HTTP/1.1 {code}\r\nContent-Type: audio/mpeg\r\nETag: \"v1\"\r\n{extra}Content-Length: {len}\r\nConnection: close\r\n\r\n").into_bytes();
                if !range.is_empty() && mode == RangeMode::Reject416 {
                    let _ = s.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                } else if !range.is_empty() && mode == RangeMode::Honour {
                    let rest = &body[start..];
                    let _ = s.write_all(&head("206 Partial Content", &format!("Content-Range: bytes {start}-{}/{total}\r\n", total - 1), rest.len()));
                    let _ = s.write_all(rest);
                } else if first {
                    let _ = s.write_all(&head("200 OK", "", total));
                    let _ = s.write_all(&body[..cut]);
                    let _ = s.flush();
                    // closing early: the client has been promised more than it gets
                } else {
                    let _ = s.write_all(&head("200 OK", "", total));
                    let _ = s.write_all(&body);
                }
            }
        });
        (format!("http://{addr}"), seen)
    }

    fn leftovers(d: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn a_broken_download_is_continued_from_where_it_stopped() {
        let body = [tiny_mp3(), (0..150_000u32).map(|i| (i % 251) as u8).collect::<Vec<u8>>()].concat();
        let (base, seen) = serve_ranged(body.clone(), 70_000, RangeMode::Honour);
        let d = tmp("resume");
        let p = policy_for(&d);
        let url = format!("{base}/song.mp3");
        let e = download(&streamer(), &p, "alice", &url, &d, DEFAULT_MAX_BYTES, &Progress::default()).unwrap_err();
        assert!(e.contains("failed") || e.contains("ended early"), "{e}");
        let names = leftovers(&d);
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.iter().any(|n| n.ends_with(".part")) && names.iter().any(|n| n.ends_with(".part.src")));
        // the second try asks only for the rest
        let prog = Progress::default();
        let Fetched::File { path, bytes } = download(&streamer(), &p, "alice", &url, &d, DEFAULT_MAX_BYTES, &prog).unwrap() else { panic!() };
        assert_eq!(bytes as usize, body.len());
        assert_eq!(std::fs::read(&path).unwrap(), body, "the joined file is identical");
        assert_eq!(prog.bytes.load(Ordering::Relaxed) as usize, body.len());
        let ranges = seen.lock().unwrap().clone();
        assert_eq!(ranges.len(), 2);
        assert!(ranges[0].is_empty() && ranges[1].starts_with("bytes=") && ranges[1] != "bytes=0-", "{ranges:?}");
        assert_eq!(leftovers(&d), vec![path.file_name().unwrap().to_string_lossy().into_owned()], "no part file or note is left");
    }

    #[test]
    fn a_server_that_ignores_ranges_or_refuses_them_gets_a_fresh_download() {
        let body = [tiny_mp3(), vec![7u8; 120_000]].concat();
        for mode in [RangeMode::Ignore, RangeMode::Reject416] {
            let (base, seen) = serve_ranged(body.clone(), 50_000, mode);
            let d = tmp(if mode == RangeMode::Ignore { "ignore" } else { "reject" });
            let p = policy_for(&d);
            let url = format!("{base}/a.mp3");
            assert!(download(&streamer(), &p, "alice", &url, &d, DEFAULT_MAX_BYTES, &Progress::default()).is_err());
            let Fetched::File { path, .. } = download(&streamer(), &p, "alice", &url, &d, DEFAULT_MAX_BYTES, &Progress::default()).unwrap() else { panic!() };
            assert_eq!(std::fs::read(&path).unwrap(), body);
            assert_eq!(leftovers(&d).len(), 1, "{:?} {}", leftovers(&d), seen.lock().unwrap().len());
        }
    }

    #[test]
    fn a_kept_part_is_used_only_for_the_same_address_and_never_after_cancel_or_a_size_stop() {
        let body = [tiny_mp3(), vec![9u8; 100_000]].concat();
        let (base, _) = serve_ranged(body.clone(), 40_000, RangeMode::Honour);
        let d = tmp("other");
        let p = policy_for(&d);
        assert!(download(&streamer(), &p, "alice", &format!("{base}/one.mp3"), &d, DEFAULT_MAX_BYTES, &Progress::default()).is_err());
        // another address does not touch it
        assert!(find_resume(&[d.clone()], &format!("{base}/two.mp3")).is_none());
        assert!(find_resume(&[d.clone()], &format!("{base}/one.mp3")).is_some_and(|r| r.offset > 0));
        // a note with no validator is not used
        let note = std::fs::read_dir(&d).unwrap().flatten().find(|e| e.file_name().to_string_lossy().ends_with(".part.src")).unwrap().path();
        std::fs::write(&note, format!("iyakki-resume 1\n{base}/one.mp3\n\n")).unwrap();
        assert!(find_resume(&[d.clone()], &format!("{base}/one.mp3")).is_none());
        // a size stop and a cancel leave nothing to resume
        let d2 = tmp("nokeep");
        let (base2, _) = serve_ranged(body.clone(), 90_000, RangeMode::Honour);
        let e = download(&streamer(), &policy_for(&d2), "alice", &format!("{base2}/x.mp3"), &d2, 30_000, &Progress::default()).unwrap_err();
        assert!(e.contains("limit"), "{e}");
        assert!(leftovers(&d2).is_empty(), "{:?}", leftovers(&d2));
    }

    #[test]
    fn old_part_notes_are_cleaned_with_their_part_and_young_ones_and_other_files_stay() {
        let d = tmp("stale");
        let mk = |n: &str| std::fs::write(d.join(n), b"x").unwrap();
        mk("old.mp3.part");
        mk("old.mp3.part.src");
        mk("young.mp3.part");
        mk("young.mp3.part.src");
        mk("orphan.mp3.part.src");
        mk("song.mp3");
        mk("notes.part.txt");
        let day = std::time::Duration::from_secs(24 * 3600);
        // "now" is 20 days ahead of the files for the old check, then the files are 1 day old for the young one
        let none = clean_stale_parts(&[d.clone()], 14 * day, std::time::SystemTime::now());
        assert_eq!(none, 1, "only the orphan note goes while everything is fresh");
        let later = std::time::SystemTime::now() + 20 * day;
        let n = clean_stale_parts(&[d.clone()], 14 * day, later);
        assert_eq!(n, 2, "both remaining notes are older than 14 days at that time");
        assert_eq!(leftovers(&d), vec!["notes.part.txt".to_string(), "song.mp3".to_string()]);
    }
}

#[cfg(test)]
#[path = "mediadl_fuzz_tests.rs"]
mod fuzz;
