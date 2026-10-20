//! The Indic voice models on the device: what is installed, downloading one (checksummed, resumable), deleting one, and taking one
//! from a folder or USB stick.
//!
//! Folder layout (the same the server reads): `<data>/tts/indic/<lang>/{fastpitch.onnx, hifigan.onnx, vocab.json}`.
//!
//! **No download happens by itself.** [`plan`] says what a download would fetch (the host, each file, the total size); the page shows
//! that on an approval card and only an administrator's confirmation starts [`download`]. Every file is pinned by its SHA-256 and
//! size (the built-in table, or the administrator's own `catalog.json` in the same folder for bundles they made and host
//! themselves); a file that does not match is deleted and never used. Files are written as `*.part`, continued with a `Range`
//! request after an interruption, and renamed into place only after the checksum matched.
//!
//! The bundles are the ONNX exports of AI4Bharat's checkpoints made with `dist/tts-indic/tools/export_onnx.py`. AI4Bharat publishes
//! the checkpoints (PyTorch, 1.5 GB each), not ONNX files, so the address the bundles are fetched from is the administrator's: a
//! private store, a web server, a k3s service or any HTTPS address (`source.txt` in the models folder, or typed on the page).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use iyakki_proxy::Streamer;
use sha2::{Digest, Sha256};

/// The files of one language, in the order they are fetched.
pub const FILES: [&str; 3] = ["vocab.json", "hifigan.onnx", "fastpitch.onnx"];

/// A file of a bundle, pinned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    pub name: String,
    pub sha256: String,
    pub bytes: u64,
}

include!("pins.rs");

/// The folder of the models inside the data folder.
pub fn root(data: &Path) -> PathBuf {
    data.join("tts").join("indic")
}

fn valid_lang(l: &str) -> bool {
    super::lang_info(l).is_some()
}

/// The pins of a language: the administrator's `catalog.json` first (`{"ta": {"vocab.json": {"sha256": "..", "bytes": 1}, ...}}`),
/// else the built-in table.
pub fn pins(root: &Path, lang: &str) -> Option<Vec<Pin>> {
    if !valid_lang(lang) {
        return None;
    }
    if let Ok(t) = std::fs::read_to_string(root.join("catalog.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
            if let Some(o) = v.get(lang).and_then(|x| x.as_object()) {
                let mut out = Vec::new();
                for f in FILES {
                    let e = o.get(f)?;
                    let sha = e.get("sha256")?.as_str()?.to_ascii_lowercase();
                    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
                        return None;
                    }
                    out.push(Pin { name: f.to_string(), sha256: sha, bytes: e.get("bytes")?.as_u64()? });
                }
                return Some(out);
            }
        }
    }
    BUILTIN.iter().find(|(l, _)| *l == lang).map(|(_, files)| files.iter().map(|(n, s, b)| Pin { name: n.to_string(), sha256: s.to_string(), bytes: *b }).collect())
}

/// The address bundles come from (empty: not set).
pub fn source(root: &Path) -> String {
    std::fs::read_to_string(root.join("source.txt")).map(|s| s.trim().trim_end_matches('/').to_string()).unwrap_or_default()
}

pub fn set_source(root: &Path, url: &str) -> Result<(), String> {
    let url = url.trim().trim_end_matches('/');
    if !url.is_empty() {
        check_source(url)?;
    }
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    std::fs::write(root.join("source.txt"), url).map_err(|e| e.to_string())
}

/// A bundle address is HTTPS, or plain HTTP to a computer on the local network or this one (a k3s NodePort, a PC).
pub fn check_source(url: &str) -> Result<(), String> {
    super::validate_url(url)?;
    if url.starts_with("https://") {
        return Ok(());
    }
    let host = iyakki_sec::url_host(url).unwrap_or_default();
    let private = host == "localhost" || host.ends_with(".local") || host.ends_with(".lan") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
        std::net::IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local(),
        std::net::IpAddr::V6(v) => v.is_loopback(),
    });
    if private {
        Ok(())
    } else {
        Err("a public address must be https:// (plain http is only for this computer and the local network)".into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    NotInstalled,
    /// Some files are there (an interrupted download): how many bytes of how many.
    Partial { have: u64, total: u64 },
    Installed { bytes: u64 },
    /// Files are there but a check failed.
    Damaged(String),
    /// There is no pin for it: this build has no bundle for the language.
    NoBundle,
}

/// The state of one language.
pub fn status(root: &Path, lang: &str) -> Status {
    let Some(pins) = pins(root, lang) else { return Status::NoBundle };
    let dir = root.join(lang);
    let total: u64 = pins.iter().map(|p| p.bytes).sum();
    let mut have = 0u64;
    let mut all = true;
    for p in &pins {
        match std::fs::metadata(dir.join(&p.name)) {
            Ok(m) if m.len() == p.bytes => have += m.len(),
            Ok(m) => return Status::Damaged(format!("{} has {} bytes, expected {}", p.name, m.len(), p.bytes)),
            Err(_) => {
                all = false;
                have += std::fs::metadata(dir.join(format!("{}.part", p.name))).map_or(0, |m| m.len());
            }
        }
    }
    if all {
        Status::Installed { bytes: total }
    } else if have > 0 {
        Status::Partial { have, total }
    } else {
        Status::NotInstalled
    }
}

/// Check the content of an installed language against its pins (reads every file).
pub fn verify(root: &Path, lang: &str) -> Result<(), String> {
    let pins = pins(root, lang).ok_or("no bundle is known for this language")?;
    for p in &pins {
        let got = sha256_file(&root.join(lang).join(&p.name))?;
        if got != p.sha256 {
            return Err(format!("{} does not match its checksum", p.name));
        }
    }
    Ok(())
}

pub fn sha256_file(p: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// SHA-256 of bytes, as lower-case hex.
pub fn sha256_hex(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// What a download would do, for the approval card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub lang: String,
    /// The host the files are fetched from, as shown to the person.
    pub host: String,
    pub files: Vec<(String, Pin)>,
    pub total_bytes: u64,
    /// Bytes already on disk from an interrupted attempt.
    pub resume_bytes: u64,
}

impl Plan {
    pub fn card(&self) -> String {
        let name = super::lang_info(&self.lang).map_or(self.lang.as_str(), |l| l.name);
        format!("Download the {name} voice ({}) from {}{}. The files are checked against built-in checksums; nothing else is sent.", human(self.total_bytes), self.host, if self.resume_bytes > 0 { format!(", continuing after {}", human(self.resume_bytes)) } else { String::new() })
    }
}

pub fn human(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
    } else if b >= 1 << 20 {
        format!("{:.0} MB", b as f64 / (1u64 << 20) as f64)
    } else {
        format!("{:.0} KB", b as f64 / 1024.0)
    }
}

/// What downloading `lang` from `base` would fetch. `base` is the bundle address (see [`source`]).
pub fn plan(root: &Path, lang: &str, base: &str) -> Result<Plan, String> {
    let pins = pins(root, lang).ok_or_else(|| format!("this build knows no bundle for {lang}"))?;
    if base.trim().is_empty() {
        return Err("no bundle address is set: type the address the bundles are published at, or use a folder or USB stick".into());
    }
    check_source(base.trim())?;
    let base = base.trim().trim_end_matches('/');
    let host = iyakki_sec::url_host(base).ok_or("the address has no host")?;
    let dir = root.join(lang);
    let mut resume = 0u64;
    let files: Vec<(String, Pin)> = pins
        .iter()
        .map(|p| {
            resume += if dir.join(&p.name).is_file() { p.bytes } else { std::fs::metadata(dir.join(format!("{}.part", p.name))).map_or(0, |m| m.len()) };
            (format!("{base}/{lang}/{}", p.name), p.clone())
        })
        .collect();
    let total_bytes = pins.iter().map(|p| p.bytes).sum();
    Ok(Plan { lang: lang.to_string(), host, files, total_bytes, resume_bytes: resume })
}

#[derive(Default)]
pub struct Progress {
    pub bytes: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn percent(&self) -> u32 {
        let t = self.total.load(Ordering::Relaxed);
        if t == 0 {
            0
        } else {
            (self.bytes.load(Ordering::Relaxed) * 100 / t).min(100) as u32
        }
    }
}

const MAX_REDIRECTS: usize = 5;

/// Fetch every file of the plan into `<root>/<lang>/`. Admin-approved callers only (the page checks). Resumes `*.part` files.
pub fn download(root: &Path, plan: &Plan, streamer: &dyn Streamer, progress: &Progress) -> Result<(), String> {
    let dir = root.join(&plan.lang);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    progress.total.store(plan.total_bytes, Ordering::Relaxed);
    progress.bytes.store(0, Ordering::Relaxed);
    let mut done = 0u64;
    for (url, pin) in &plan.files {
        let target = dir.join(&pin.name);
        if target.is_file() && sha256_file(&target).is_ok_and(|h| h == pin.sha256) {
            done += pin.bytes;
            progress.bytes.store(done, Ordering::Relaxed);
            continue;
        }
        let part = dir.join(format!("{}.part", pin.name));
        fetch_file(streamer, url, &part, pin, progress, done)?;
        let got = sha256_file(&part)?;
        if got != pin.sha256 {
            let _ = std::fs::remove_file(&part);
            return Err(format!("{} does not match its checksum, so it was deleted (expected {}…, got {}…); the address may serve another file", pin.name, &pin.sha256[..12], &got[..12]));
        }
        std::fs::rename(&part, &target).map_err(|e| format!("{}: {e}", target.display()))?;
        done += pin.bytes;
        progress.bytes.store(done, Ordering::Relaxed);
    }
    Ok(())
}

fn fetch_file(streamer: &dyn Streamer, url: &str, part: &Path, pin: &Pin, progress: &Progress, base_done: u64) -> Result<(), String> {
    let mut have = std::fs::metadata(part).map_or(0, |m| m.len());
    if have > pin.bytes {
        let _ = std::fs::remove_file(part);
        have = 0;
    }
    let mut cur = url.to_string();
    let mut head = None;
    for _ in 0..=MAX_REDIRECTS {
        check_source(&cur)?;
        let mut h = vec![("Accept".to_string(), "application/octet-stream".to_string())];
        if have > 0 {
            h.push(("Range".into(), format!("bytes={have}-")));
        }
        let r = streamer.open(&cur, &h)?;
        if (300..400).contains(&r.status) {
            let loc = r.header("location").ok_or("a redirect without an address")?.to_string();
            cur = if loc.starts_with("http") { loc } else { format!("{}{}", cur.split('/').take(3).collect::<Vec<_>>().join("/"), loc) };
            continue;
        }
        head = Some(r);
        break;
    }
    let mut r = head.ok_or("too many redirects")?;
    match r.status {
        206 => {}
        200 => {
            // the server ignored the range: start again
            have = 0;
        }
        416 if have == pin.bytes => return Ok(()),
        s => return Err(format!("HTTP {s} for {}", pin.name)),
    }
    let mut f = if have > 0 { std::fs::OpenOptions::new().append(true).open(part) } else { std::fs::File::create(part) }.map_err(|e| format!("{}: {e}", part.display()))?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut written = have;
    loop {
        if progress.cancel.load(Ordering::Relaxed) {
            return Err("cancelled; what was fetched is kept and the download can be continued".into());
        }
        let n = r.body.read(&mut buf).map_err(|e| format!("the download was interrupted after {} (press Download again to continue): {e}", human(written)))?;
        if n == 0 {
            break;
        }
        written += n as u64;
        if written > pin.bytes {
            drop(f);
            let _ = std::fs::remove_file(part);
            return Err(format!("{} is larger than its pinned size, so it was deleted", pin.name));
        }
        f.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", part.display()))?;
        progress.bytes.store(base_done + written, Ordering::Relaxed);
    }
    f.flush().map_err(|e| e.to_string())?;
    if written != pin.bytes {
        return Err(format!("the download of {} stopped at {} of {} (press Download again to continue)", pin.name, human(written), human(pin.bytes)));
    }
    Ok(())
}

/// Remove an installed (or half-installed) language.
pub fn delete(root: &Path, lang: &str) -> Result<(), String> {
    if !valid_lang(lang) {
        return Err("unknown language".into());
    }
    let dir = root.join(lang);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", dir.display())),
    }
}

/// Take a language from a folder (a USB stick, a network share): `<folder>/<lang>/` or `<folder>/` holding the three files. Every
/// file is checked against its pin before it is copied into place, so a stick with other files installs nothing.
pub fn import_folder(root: &Path, lang: &str, folder: &Path) -> Result<(), String> {
    let pins = pins(root, lang).ok_or_else(|| format!("this build knows no bundle for {lang}"))?;
    let src = if folder.join(lang).join(FILES[0]).is_file() { folder.join(lang) } else { folder.to_path_buf() };
    for p in &pins {
        let f = src.join(&p.name);
        let m = std::fs::metadata(&f).map_err(|_| format!("{} is not in {}", p.name, src.display()))?;
        if m.len() != p.bytes {
            return Err(format!("{} has the wrong size ({} bytes, expected {})", p.name, m.len(), p.bytes));
        }
        if sha256_file(&f)? != p.sha256 {
            return Err(format!("{} does not match its checksum; nothing was installed", p.name));
        }
    }
    let dir = root.join(lang);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for p in &pins {
        let tmp = dir.join(format!("{}.part", p.name));
        std::fs::copy(src.join(&p.name), &tmp).map_err(|e| format!("copying {}: {e}", p.name))?;
        std::fs::rename(&tmp, dir.join(&p.name)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The languages installed (all three files present with the pinned size).
pub fn installed(root: &Path) -> Vec<&'static str> {
    super::LANGS.iter().filter(|l| matches!(status(root, l.code), Status::Installed { .. })).map(|l| l.code).collect()
}

/// Disk used by the models folder.
pub fn disk_used(root: &Path) -> u64 {
    fn walk(p: &Path) -> u64 {
        std::fs::read_dir(p).map_or(0, |rd| rd.flatten().map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => walk(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        }).sum())
    }
    walk(root)
}

/// The voice server's address kept with the models (`server.txt`; the default address when none).
pub fn server(root: &Path) -> String {
    std::fs::read_to_string(root.join("server.txt")).map(|s| s.trim().to_string()).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| super::DEFAULT_URL.to_string())
}

pub fn set_server(root: &Path, url: &str) -> Result<(), String> {
    let url = url.trim().trim_end_matches('/');
    if !url.is_empty() {
        super::validate_url(url)?;
    }
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    std::fs::write(root.join("server.txt"), url).map_err(|e| e.to_string())
}
