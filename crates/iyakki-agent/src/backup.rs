//! Backup and restore of one person's own data, in one file (`docs/BACKUP.md`).
//!
//! What goes in is a fixed table of places ([`sources`]): the person's settings files, browser bookmarks and header rules (never the
//! header values: those live in the vault), dashboards, flows, skills, notes, chats, recordings and the person's vault file. The vault
//! is copied as the already-encrypted file it is; nothing is decrypted into the archive. Nothing else is read, and links are never
//! followed.
//!
//! The file: `IYAKKIBK`, a format number, a flag byte (1 = encrypted), then the body. The body is a manifest (account, release,
//! one line per file with its length and SHA-256), the file bytes in manifest order and a SHA-256 of everything before it. With a
//! passphrase the body is cut into 64 KiB pieces, each sealed with XChaCha20-Poly1305 under an Argon2id key (the same primitives
//! the vault uses); without one the file is NOT encrypted and the page says so.
//!
//! Restore reads the whole file first ([`inspect`]: format number, every hash, the total hash, the end of the file), and only then
//! stages the chosen areas next to where they belong. A place that already exists is renamed to `<name>.before-restore-<time>`, never
//! overwritten. Where a file lands is decided by the table and the signed-in person, never by a name inside the file, so a hostile
//! file cannot reach outside the person's own places.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use argon2::Argon2;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAGIC: &[u8; 8] = b"IYAKKIBK";
/// The format this release writes. A file with a higher number is refused with a clear message.
pub const FORMAT: u32 = 1;
/// The oldest format this release can read.
pub const MIN_FORMAT: u32 = 1;
pub const EXTENSION: &str = "iybak";
pub const MIN_PASSPHRASE: usize = 8;
const CHUNK: usize = 64 * 1024;
const MAX_MANIFEST: u64 = 64 << 20;
pub const MAX_FILES: usize = 100_000;
/// Files that look like private keys are left out of everything except the vault; they are only looked at below this size.
const SCAN_LIMIT: u64 = 1 << 20;

/// What the person can switch on or off: id and the words on the page.
pub const AREAS: &[(&str, &str)] = &[
    ("settings", "Settings (layout, size, screen saver, displays, radio, browser choices)"),
    ("browser", "Browser bookmarks, history and header rules (header values stay in the vault)"),
    ("dashboards", "Dashboards and their data sources (secrets are only names of vault entries)"),
    ("flows", "Flows"),
    ("skills", "Skills"),
    ("notes", "Notes, context and calendar"),
    ("chats", "Chats"),
    ("recordings", "Recordings"),
    ("vault", "Your vault, as its already encrypted file"),
];

pub fn area_label(id: &str) -> &'static str {
    AREAS.iter().find(|(i, _)| *i == id).map_or("", |(_, l)| l)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Root {
    Data,
    Home,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
}

/// The two folders a person's data lives in, and who they are.
#[derive(Clone, Debug)]
pub struct Layout {
    pub data: PathBuf,
    pub home: PathBuf,
    pub user: String,
}

#[derive(Clone, Debug)]
pub struct Source {
    pub id: &'static str,
    pub area: &'static str,
    pub kind: Kind,
    pub root: Root,
    pub rel: String,
}

impl Source {
    pub fn path(&self, l: &Layout) -> PathBuf {
        match self.root {
            Root::Data => l.data.join(&self.rel),
            Root::Home => l.home.join(&self.rel),
        }
    }
}

fn who(user: &str) -> String {
    user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect()
}

/// Every place a backup reads and a restore may write, for this person. The ids are part of the file format.
pub fn sources(l: &Layout) -> Vec<Source> {
    let w = who(&l.user);
    let u = &l.user;
    let f = |id, area, root, rel: String| Source { id, area, kind: Kind::File, root, rel };
    let d = |id, area, root, rel: String| Source { id, area, kind: Kind::Dir, root, rel };
    use Root::{Data, Home};
    vec![
        f("layout", "settings", Data, format!("layout-{w}.json")),
        f("uisize", "settings", Data, format!("ui-{w}.json")),
        f("screensaver", "settings", Data, format!("screensaver-{w}.json")),
        f("displays", "settings", Data, format!("displays-{u}.json")),
        f("radio", "settings", Data, format!("radio-{w}.json")),
        f("browsercfg", "settings", Data, format!("browser-{w}.json")),
        f("browserjs", "settings", Data, format!("browser-js-{w}.json")),
        f("browserdata", "browser", Data, format!("browser-data-{w}.json")),
        f("browserheaders", "browser", Data, format!("browser-headers-{w}.json")),
        d("dashboards", "dashboards", Home, "Dashboards".into()),
        f("dashsources", "dashboards", Data, format!("dash/sources-{u}.json")),
        d("flows", "flows", Home, "Flows".into()),
        d("skills", "skills", Data, format!("skills/user/{u}")),
        d("notes", "notes", Home, ".iyakki".into()),
        f("calendar", "notes", Data, format!("calendar/{u}.json")),
        d("chats", "chats", Data, format!("sessions/{u}")),
        d("recordings", "recordings", Home, "Recordings".into()),
        f("vault", "vault", Data, format!("vault/users/{u}.afv")),
    ]
}

// ------------------------------------------------------------------------------------------------------------------ errors

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupError {
    /// Made by a newer release.
    Newer { found: u32 },
    /// Not a file this program made (or too old to read).
    NotBackup(String),
    NeedsPassphrase,
    WrongPassphrase,
    /// Cut short, changed or damaged: nothing was touched.
    Damaged(String),
    /// Made for another account; restoring needs the person's confirmation.
    OtherAccount { from: String },
    /// A name inside the file would leave the person's places, or is not one of ours.
    Unsafe(String),
    NoRoom { need: u64, have: u64 },
    Invalid(String),
    Io(String),
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackupError::Newer { found } => write!(f, "This backup was made by a newer IYAKKI (file format {found}; this one reads up to {FORMAT}). Update IYAKKI first; nothing was changed."),
            BackupError::NotBackup(w) => write!(f, "This is not an IYAKKI backup file ({w}). Nothing was changed."),
            BackupError::NeedsPassphrase => write!(f, "This backup is encrypted. Type its passphrase."),
            BackupError::WrongPassphrase => write!(f, "Wrong passphrase, or the file is damaged. Nothing was changed."),
            BackupError::Damaged(w) => write!(f, "The backup file is damaged or incomplete ({w}). Nothing was changed."),
            BackupError::OtherAccount { from } => write!(f, "This backup belongs to the account \u{201C}{from}\u{201D}, not to you. Confirm to restore it into your own folder anyway."),
            BackupError::Unsafe(w) => write!(f, "The backup file names a place it must not touch ({w}). It was refused and nothing was changed."),
            BackupError::NoRoom { need, have } => write!(f, "Not enough room: the backup needs about {} MB and {} MB are free.", need.div_ceil(1 << 20), have / (1 << 20)),
            BackupError::Invalid(w) | BackupError::Io(w) => write!(f, "{w}"),
        }
    }
}

impl std::error::Error for BackupError {}

fn io_err(what: &str, e: io::Error) -> BackupError {
    BackupError::Io(format!("{what}: {e}"))
}

// ------------------------------------------------------------------------------------------------------------------ manifest

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub src: String,
    /// Path below the source (empty for a single-file source). Only plain names joined by `/`.
    #[serde(default)]
    pub sub: String,
    pub len: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    /// The oldest reader that understands this file (older writers left it out).
    #[serde(default)]
    pub min_reader: u32,
    pub account: String,
    #[serde(default)]
    pub release: String,
    pub created: i64,
    #[serde(default)]
    pub entries: Vec<Entry>,
}

/// What a backup file holds, after it was read through and verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contents {
    pub format: u32,
    pub encrypted: bool,
    pub account: String,
    pub release: String,
    pub created: i64,
    pub same_account: bool,
    pub areas: Vec<AreaInfo>,
    pub total_bytes: u64,
    pub total_files: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AreaInfo {
    pub id: String,
    pub label: String,
    pub files: usize,
    pub bytes: u64,
    /// Things left out and why (backup plans only).
    pub skipped: Vec<String>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha_file(p: &Path) -> io::Result<(u64, String)> {
    let mut f = File::open(p)?;
    let mut h = Sha256::new();
    let mut n = 0u64;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let k = f.read(&mut buf)?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((n, hex(&h.finalize())))
}

fn plain_sub_ok(s: &str) -> bool {
    s.is_empty() || (!s.starts_with('/') && s.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.contains(['\\', '\0']) && !c.chars().any(char::is_control)))
}

fn looks_secret(p: &Path) -> bool {
    let Ok(mut f) = File::open(p) else { return false };
    let mut b = Vec::new();
    if (&mut f).take(SCAN_LIMIT).read_to_end(&mut b).is_err() {
        return false;
    }
    let t = String::from_utf8_lossy(&b);
    ["PRIVATE KEY-----", "PrivateKey", "PresharedKey", "OPENVPN STATIC KEY", "BEGIN OPENSSH"].iter().any(|m| t.contains(m))
}

// ------------------------------------------------------------------------------------------------------------------ planning

struct Found {
    entry: Entry,
    path: PathBuf,
}

struct Plan {
    found: Vec<Found>,
    skipped: BTreeMap<&'static str, Vec<String>>,
}

fn walk(dir: &Path, sub: &str, out: &mut Vec<(String, PathBuf, u64)>, skipped: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut items: Vec<_> = rd.flatten().collect();
    items.sort_by_key(|e| e.file_name());
    for e in items {
        let name = e.file_name();
        let Some(n) = name.to_str() else {
            skipped.push(format!("{}: a name that is not text", dir.display()));
            continue;
        };
        let rel = if sub.is_empty() { n.to_string() } else { format!("{sub}/{n}") };
        if n.starts_with(".iyakki-restore-") || n.contains(".before-restore-") || n.ends_with(".part") || n.ends_with(".tmp") {
            continue;
        }
        if !plain_sub_ok(&rel) {
            skipped.push(format!("{rel}: odd name"));
            continue;
        }
        let Ok(md) = fs::symlink_metadata(e.path()) else { continue };
        if md.file_type().is_symlink() {
            skipped.push(format!("{rel}: a link (links are never followed)"));
        } else if md.is_dir() {
            walk(&e.path(), &rel, out, skipped);
        } else if md.is_file() {
            out.push((rel, e.path(), md.len()));
        }
    }
}

fn plan(l: &Layout, areas: &BTreeSet<String>) -> Result<Plan, BackupError> {
    if !iyakki_sec::policy::valid_username(&l.user) {
        return Err(BackupError::Invalid("The signed-in account name is not valid.".into()));
    }
    let mut found = Vec::new();
    let mut skipped: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for s in sources(l) {
        if !areas.contains(s.area) {
            continue;
        }
        let p = s.path(l);
        let Ok(md) = fs::symlink_metadata(&p) else { continue };
        let sk = skipped.entry(s.area).or_default();
        if md.file_type().is_symlink() {
            sk.push(format!("{}: a link (links are never followed)", s.rel));
            continue;
        }
        let mut files = Vec::new();
        match s.kind {
            Kind::File if md.is_file() => files.push((String::new(), p.clone(), md.len())),
            Kind::Dir if md.is_dir() => walk(&p, "", &mut files, sk),
            _ => continue,
        }
        for (sub, path, len) in files {
            if s.area != "vault" && len <= SCAN_LIMIT && looks_secret(&path) {
                sk.push(format!("{}{}{}: looks like a private key, left out", s.rel, if sub.is_empty() { "" } else { "/" }, sub));
                continue;
            }
            found.push(Found { entry: Entry { src: s.id.into(), sub, len, sha256: String::new() }, path });
        }
    }
    if found.len() > MAX_FILES {
        return Err(BackupError::Invalid(format!("Too many files ({}; at most {MAX_FILES}).", found.len())));
    }
    Ok(Plan { found, skipped })
}

/// What a backup of these areas would contain right now: files and bytes per area, and what is left out.
pub fn preview(l: &Layout, areas: &BTreeSet<String>) -> Result<Vec<AreaInfo>, BackupError> {
    let p = plan(l, areas)?;
    Ok(summarize(l, &p, areas))
}

fn summarize(l: &Layout, p: &Plan, areas: &BTreeSet<String>) -> Vec<AreaInfo> {
    let src = sources(l);
    let mut out: Vec<AreaInfo> = AREAS.iter().filter(|(i, _)| areas.contains(*i)).map(|(i, lab)| AreaInfo { id: (*i).into(), label: (*lab).into(), ..Default::default() }).collect();
    for f in &p.found {
        if let Some(s) = src.iter().find(|s| s.id == f.entry.src) {
            if let Some(a) = out.iter_mut().find(|a| a.id == s.area) {
                a.files += 1;
                a.bytes += f.entry.len;
            }
        }
    }
    for a in &mut out {
        a.skipped = p.skipped.get(a.id.as_str()).cloned().unwrap_or_default();
    }
    out
}

pub fn all_areas() -> BTreeSet<String> {
    AREAS.iter().map(|(i, _)| (*i).to_string()).collect()
}

// ------------------------------------------------------------------------------------------------------------------ sealing

const ARGON_M: u32 = 19 * 1024;
const ARGON_T: u32 = 2;

fn derive(pass: &str, salt: &[u8]) -> Result<[u8; 32], BackupError> {
    let params = argon2::Params::new(ARGON_M, ARGON_T, 1, Some(32)).map_err(|e| BackupError::Invalid(e.to_string()))?;
    let mut k = [0u8; 32];
    Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params).hash_password_into(pass.as_bytes(), salt, &mut k).map_err(|e| BackupError::Invalid(e.to_string()))?;
    Ok(k)
}

type Cipher = (XChaCha20Poly1305, [u8; 16], Vec<u8>);

fn nonce(prefix: &[u8; 16], c: u64) -> XNonce {
    let mut n = [0u8; 24];
    n[..16].copy_from_slice(prefix);
    n[16..].copy_from_slice(&c.to_le_bytes());
    *XNonce::from_slice(&n)
}

struct Sealer {
    out: File,
    buf: Vec<u8>,
    aead: Option<Cipher>,
    counter: u64,
}

impl Sealer {
    fn flush_chunk(&mut self, last: bool) -> io::Result<()> {
        match &self.aead {
            None => self.out.write_all(&self.buf)?,
            Some((c, prefix, header)) => {
                let mut aad = header.clone();
                aad.push(u8::from(last));
                let ct = c.encrypt(&nonce(prefix, self.counter), Payload { msg: &self.buf, aad: &aad }).map_err(|_| io::Error::other("seal failed"))?;
                self.out.write_all(&(ct.len() as u32).to_le_bytes())?;
                self.out.write_all(&[u8::from(last)])?;
                self.out.write_all(&ct)?;
                self.counter += 1;
            }
        }
        self.buf.clear();
        Ok(())
    }
    fn finish(mut self) -> io::Result<()> {
        self.flush_chunk(true)?;
        self.out.flush()?;
        self.out.sync_all()
    }
}

impl Write for Sealer {
    fn write(&mut self, mut d: &[u8]) -> io::Result<usize> {
        let n = d.len();
        while !d.is_empty() {
            let k = (CHUNK - self.buf.len()).min(d.len());
            self.buf.extend_from_slice(&d[..k]);
            d = &d[k..];
            if self.buf.len() == CHUNK {
                self.flush_chunk(false)?;
            }
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Reads the body back, unsealing as it goes.
struct Opener {
    inp: io::BufReader<File>,
    aead: Option<Cipher>,
    counter: u64,
    cur: Vec<u8>,
    pos: usize,
    done: bool,
    bad_key: bool,
}

impl Read for Opener {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.aead.is_none() {
            return self.inp.read(out);
        }
        while self.pos >= self.cur.len() {
            if self.done {
                return Ok(0);
            }
            let mut len = [0u8; 4];
            self.inp.read_exact(&mut len)?;
            let mut last = [0u8; 1];
            self.inp.read_exact(&mut last)?;
            let n = u32::from_le_bytes(len) as usize;
            if n > CHUNK + 64 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "piece too long"));
            }
            let mut ct = vec![0u8; n];
            self.inp.read_exact(&mut ct)?;
            let (c, prefix, header) = self.aead.as_ref().unwrap();
            let mut aad = header.clone();
            aad.push(last[0]);
            match c.decrypt(&nonce(prefix, self.counter), Payload { msg: &ct, aad: &aad }) {
                Ok(p) => self.cur = p,
                Err(_) => {
                    self.bad_key = self.counter == 0;
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "a piece does not open"));
                }
            }
            self.counter += 1;
            self.pos = 0;
            self.done = last[0] == 1;
        }
        let k = out.len().min(self.cur.len() - self.pos);
        out[..k].copy_from_slice(&self.cur[self.pos..self.pos + k]);
        self.pos += k;
        Ok(k)
    }
}

fn open_file(path: &Path, pass: Option<&str>) -> Result<(Opener, u32, bool), BackupError> {
    let mut f = File::open(path).map_err(|e| io_err(&format!("cannot open {}", path.display()), e))?;
    let mut head = [0u8; 13];
    f.read_exact(&mut head).map_err(|_| BackupError::NotBackup("too short".into()))?;
    if &head[..8] != MAGIC {
        return Err(BackupError::NotBackup("wrong first bytes".into()));
    }
    let fmt = u32::from_le_bytes(head[8..12].try_into().unwrap());
    if fmt > FORMAT {
        return Err(BackupError::Newer { found: fmt });
    }
    if fmt < MIN_FORMAT {
        return Err(BackupError::NotBackup(format!("format {fmt} is not one this release reads")));
    }
    let enc = match head[12] {
        0 => false,
        1 => true,
        _ => return Err(BackupError::NotBackup("unknown flags".into())),
    };
    let mut aead = None;
    if enc {
        let mut rest = [0u8; 32];
        f.read_exact(&mut rest).map_err(|_| BackupError::Damaged("header cut short".into()))?;
        let pass = pass.filter(|p| !p.is_empty()).ok_or(BackupError::NeedsPassphrase)?;
        let key = derive(pass, &rest[..16])?;
        let mut prefix = [0u8; 16];
        prefix.copy_from_slice(&rest[16..]);
        let mut header = head.to_vec();
        header.extend_from_slice(&rest);
        aead = Some((XChaCha20Poly1305::new(Key::from_slice(&key)), prefix, header));
    }
    Ok((Opener { inp: io::BufReader::with_capacity(CHUNK, f), aead, counter: 0, cur: Vec::new(), pos: 0, done: false, bad_key: false }, fmt, enc))
}

fn read_err(o: &Opener, e: io::Error) -> BackupError {
    if o.bad_key {
        BackupError::WrongPassphrase
    } else if e.kind() == io::ErrorKind::UnexpectedEof {
        BackupError::Damaged("it ends too early".into())
    } else if e.kind() == io::ErrorKind::InvalidData {
        BackupError::Damaged("a piece was changed".into())
    } else {
        BackupError::Io(e.to_string())
    }
}

// ------------------------------------------------------------------------------------------------------------------ writing

#[derive(Clone, Debug, Default)]
pub struct CreateOptions {
    pub areas: BTreeSet<String>,
    pub passphrase: Option<String>,
    pub now: i64,
    pub release: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub path: PathBuf,
    pub bytes: u64,
    pub files: usize,
    pub encrypted: bool,
    pub areas: Vec<AreaInfo>,
}

fn stamp(now: i64) -> String {
    let d = crate::release::civil_date(now.max(0) as u64).replace('-', "");
    let s = now.max(0) % 86_400;
    format!("{d}-{:02}{:02}{:02}", s / 3600, s % 3600 / 60, s % 60)
}

/// Make the backup file in `dest_dir` (a folder the person picked; it must exist). The file is written under a `.part` name and
/// renamed when complete, so an interrupted run leaves no half file under the real name.
pub fn create(l: &Layout, dest_dir: &Path, o: &CreateOptions) -> Result<Created, BackupError> {
    if let Some(p) = &o.passphrase {
        if p.chars().count() < MIN_PASSPHRASE {
            return Err(BackupError::Invalid(format!("The passphrase needs at least {MIN_PASSPHRASE} characters.")));
        }
    }
    if o.areas.is_empty() || o.areas.iter().any(|a| !AREAS.iter().any(|(i, _)| i == a)) {
        return Err(BackupError::Invalid("Choose at least one thing to back up.".into()));
    }
    let md = fs::metadata(dest_dir).map_err(|e| io_err(&format!("cannot use {}", dest_dir.display()), e))?;
    if !md.is_dir() {
        return Err(BackupError::Invalid(format!("{} is not a folder.", dest_dir.display())));
    }
    let mut p = plan(l, &o.areas)?;
    let info = summarize(l, &p, &o.areas);
    let total: u64 = p.found.iter().map(|f| f.entry.len).sum();
    if let Some((free, _)) = iyakki_storage::space(dest_dir) {
        let need = total + (total / 50) + (1 << 20);
        if free < need {
            return Err(BackupError::NoRoom { need, have: free });
        }
    }
    for f in &mut p.found {
        let (n, h) = sha_file(&f.path).map_err(|e| io_err(&format!("cannot read {}", f.path.display()), e))?;
        f.entry.len = n;
        f.entry.sha256 = h;
    }
    let manifest = Manifest { format: FORMAT, min_reader: FORMAT, account: l.user.clone(), release: o.release.clone(), created: o.now, entries: p.found.iter().map(|f| f.entry.clone()).collect() };
    let mjson = serde_json::to_vec(&manifest).map_err(|e| BackupError::Invalid(e.to_string()))?;
    let name = format!("iyakki-backup-{}-{}.{EXTENSION}", who(&l.user), stamp(o.now));
    let final_path = dest_dir.join(&name);
    if final_path.exists() {
        return Err(BackupError::Invalid(format!("{name} already exists in that folder; nothing was overwritten.")));
    }
    let part = dest_dir.join(format!("{name}.part"));
    match write_body(&part, &p, &manifest, &mjson, o.passphrase.as_deref()) {
        Ok(bytes) => {
            fs::rename(&part, &final_path).map_err(|e| {
                let _ = fs::remove_file(&part);
                io_err("cannot finish the file", e)
            })?;
            Ok(Created { path: final_path, bytes, files: manifest.entries.len(), encrypted: o.passphrase.is_some(), areas: info })
        }
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(e)
        }
    }
}

fn put(s: &mut Sealer, total: &mut Sha256, d: &[u8]) -> Result<(), BackupError> {
    total.update(d);
    s.write_all(d).map_err(|e| io_err("write failed", e))
}

fn write_body(part: &Path, p: &Plan, m: &Manifest, mjson: &[u8], pass: Option<&str>) -> Result<u64, BackupError> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut out = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(part).map_err(|e| io_err(&format!("cannot write {}", part.display()), e))?;
    let mut header = MAGIC.to_vec();
    header.extend_from_slice(&FORMAT.to_le_bytes());
    header.push(u8::from(pass.is_some()));
    let mut aead = None;
    if let Some(pw) = pass {
        let mut salt = [0u8; 16];
        let mut prefix = [0u8; 16];
        iyakki_sec::random_fill(&mut salt).map_err(|_| BackupError::Invalid("no random numbers".into()))?;
        iyakki_sec::random_fill(&mut prefix).map_err(|_| BackupError::Invalid("no random numbers".into()))?;
        header.extend_from_slice(&salt);
        header.extend_from_slice(&prefix);
        let key = derive(pw, &salt)?;
        aead = Some((XChaCha20Poly1305::new(Key::from_slice(&key)), prefix, header.clone()));
    }
    out.write_all(&header).map_err(|e| io_err("write failed", e))?;
    let mut s = Sealer { out, buf: Vec::with_capacity(CHUNK), aead, counter: 0 };
    let mut total = Sha256::new();
    put(&mut s, &mut total, &(mjson.len() as u64).to_le_bytes())?;
    put(&mut s, &mut total, mjson)?;
    let mut buf = vec![0u8; CHUNK];
    for (f, e) in p.found.iter().zip(&m.entries) {
        let mut inp = File::open(&f.path).map_err(|er| io_err(&format!("cannot read {}", f.path.display()), er))?;
        let mut h = Sha256::new();
        let mut n = 0u64;
        loop {
            let k = inp.read(&mut buf).map_err(|er| io_err("read failed", er))?;
            if k == 0 {
                break;
            }
            h.update(&buf[..k]);
            n += k as u64;
            put(&mut s, &mut total, &buf[..k])?;
        }
        if n != e.len || hex(&h.finalize()) != e.sha256 {
            return Err(BackupError::Invalid(format!("{} changed while it was being backed up; try again.", f.path.display())));
        }
    }
    let digest = total.finalize();
    s.write_all(&digest).map_err(|e| io_err("write failed", e))?;
    s.finish().map_err(|e| io_err("write failed", e))?;
    fs::metadata(part).map(|m| m.len()).map_err(|e| io_err("write failed", e))
}

// ------------------------------------------------------------------------------------------------------------------ reading

/// Read `len` bytes from `r`, feeding the whole-file hash and an own hash (returned), and writing them to `sink` when given.
fn pump(r: &mut Opener, len: u64, total: &mut Sha256, mut sink: Option<&mut dyn Write>) -> Result<String, BackupError> {
    let mut h = Sha256::new();
    let mut left = len;
    let mut buf = vec![0u8; CHUNK];
    while left > 0 {
        let want = buf.len().min(left as usize);
        let k = r.read(&mut buf[..want]).map_err(|e| read_err(r, e))?;
        if k == 0 {
            return Err(BackupError::Damaged("it ends too early".into()));
        }
        h.update(&buf[..k]);
        total.update(&buf[..k]);
        if let Some(s) = sink.as_deref_mut() {
            s.write_all(&buf[..k]).map_err(|e| io_err("cannot write while restoring", e))?;
        }
        left -= k as u64;
    }
    Ok(hex(&h.finalize()))
}

fn read_manifest(r: &mut Opener, total: &mut Sha256, fmt: u32) -> Result<Manifest, BackupError> {
    let mut lenb = [0u8; 8];
    r.read_exact(&mut lenb).map_err(|e| read_err(r, e))?;
    total.update(lenb);
    let n = u64::from_le_bytes(lenb);
    if n > MAX_MANIFEST {
        return Err(BackupError::Damaged("the list of files is too long".into()));
    }
    let mut mb = vec![0u8; n as usize];
    r.read_exact(&mut mb).map_err(|e| read_err(r, e))?;
    total.update(&mb);
    let m: Manifest = serde_json::from_slice(&mb).map_err(|_| BackupError::Damaged("the list of files cannot be read".into()))?;
    if m.format != fmt {
        return Err(BackupError::Damaged("format numbers disagree".into()));
    }
    if m.min_reader > FORMAT {
        return Err(BackupError::Newer { found: m.min_reader });
    }
    if m.entries.len() > MAX_FILES {
        return Err(BackupError::Damaged("too many files".into()));
    }
    Ok(m)
}

fn finish_check(r: &mut Opener, total: Sha256) -> Result<(), BackupError> {
    let mut tail = [0u8; 32];
    r.read_exact(&mut tail).map_err(|e| read_err(r, e))?;
    if total.finalize().as_slice() != tail {
        return Err(BackupError::Damaged("the checksum of the whole file does not match".into()));
    }
    let mut extra = [0u8; 1];
    match r.read(&mut extra) {
        Ok(0) => Ok(()),
        Ok(_) => Err(BackupError::Damaged("extra bytes at the end".into())),
        Err(e) => Err(read_err(r, e)),
    }
}

/// Read the whole file and check everything (format, each file's length and SHA-256, the total checksum, the end), without writing
/// anything. `l.user` is the signed-in account, to say whether the file is theirs.
pub fn inspect(path: &Path, pass: Option<&str>, l: &Layout) -> Result<Contents, BackupError> {
    verify(path, pass, l).map(|(c, _)| c)
}

fn verify(path: &Path, pass: Option<&str>, l: &Layout) -> Result<(Contents, Manifest), BackupError> {
    let (mut r, fmt, enc) = open_file(path, pass)?;
    let mut total = Sha256::new();
    let m = read_manifest(&mut r, &mut total, fmt)?;
    let known = sources(l);
    let mut seen = BTreeSet::new();
    for e in &m.entries {
        let short = |s: &str| s.chars().take(60).collect::<String>();
        let Some(s) = known.iter().find(|s| s.id == e.src) else { return Err(BackupError::Unsafe(format!("unknown place \u{201C}{}\u{201D}", short(&e.src)))) };
        if !plain_sub_ok(&e.sub) || (s.kind == Kind::File) != e.sub.is_empty() {
            return Err(BackupError::Unsafe(format!("path \u{201C}{}\u{201D} in {}", short(&e.sub), short(&e.src))));
        }
        if !seen.insert((e.src.clone(), e.sub.clone())) {
            return Err(BackupError::Damaged("a file is listed twice".into()));
        }
    }
    for e in &m.entries {
        if pump(&mut r, e.len, &mut total, None)? != e.sha256 {
            return Err(BackupError::Damaged(format!("{} does not match its checksum", e.src)));
        }
    }
    finish_check(&mut r, total)?;
    let mut areas: Vec<AreaInfo> = Vec::new();
    for e in &m.entries {
        let a = known.iter().find(|s| s.id == e.src).map_or("", |s| s.area);
        match areas.iter_mut().find(|x| x.id == a) {
            Some(x) => {
                x.files += 1;
                x.bytes += e.len;
            }
            None => areas.push(AreaInfo { id: a.into(), label: area_label(a).into(), files: 1, bytes: e.len, skipped: vec![] }),
        }
    }
    areas.sort_by_key(|a| AREAS.iter().position(|(i, _)| *i == a.id));
    let c = Contents { format: m.format, encrypted: enc, account: m.account.clone(), release: m.release.clone(), created: m.created, same_account: m.account == l.user, total_bytes: m.entries.iter().map(|e| e.len).sum(), total_files: m.entries.len(), areas };
    Ok((c, m))
}

#[derive(Clone, Debug, Default)]
pub struct RestoreRequest {
    /// The areas the person said yes to, one by one.
    pub areas: BTreeSet<String>,
    /// The person confirmed restoring a file made for another account.
    pub allow_other_account: bool,
    pub now: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RestoreReport {
    pub restored: Vec<AreaInfo>,
    /// Where the data that was there before was kept (renamed, never deleted).
    pub kept: Vec<PathBuf>,
}

/// Restore the chosen areas into this person's own places. Order: verify everything; stage the files next to their final place;
/// only when all of it is staged and checked again, move what is there aside and move the staged data in.
pub fn restore(l: &Layout, path: &Path, pass: Option<&str>, req: &RestoreRequest) -> Result<RestoreReport, BackupError> {
    let (c, m) = verify(path, pass, l)?;
    if !c.same_account && !req.allow_other_account {
        return Err(BackupError::OtherAccount { from: c.account });
    }
    if req.areas.is_empty() {
        return Err(BackupError::Invalid("Choose what to restore.".into()));
    }
    for a in &req.areas {
        if !c.areas.iter().any(|x| &x.id == a) {
            return Err(BackupError::Invalid(format!("The file has nothing for \u{201C}{}\u{201D}.", area_label(a))));
        }
    }
    // a vault belongs to its account: it cannot be used under another name
    if !c.same_account && req.areas.contains("vault") {
        return Err(BackupError::Invalid("A vault from another account cannot be restored into yours.".into()));
    }
    let known = sources(l);
    let tag = stamp(req.now);
    let chosen: Vec<&Source> = known.iter().filter(|s| req.areas.contains(s.area) && m.entries.iter().any(|e| e.src == s.id)).collect();
    let stage_of = |s: &Source| -> PathBuf { s.path(l).with_file_name(format!(".iyakki-restore-{tag}-{}", s.id)) };
    let cleanup = || {
        for s in &chosen {
            let p = stage_of(s);
            let _ = fs::remove_dir_all(&p);
            let _ = fs::remove_file(&p);
        }
    };
    let staged = (|| -> Result<(), BackupError> {
        let (mut r, fmt, _) = open_file(path, pass)?;
        let mut total = Sha256::new();
        if read_manifest(&mut r, &mut total, fmt)? != m {
            return Err(BackupError::Damaged("the file changed while it was being restored".into()));
        }
        for s in &chosen {
            let st = stage_of(s);
            if let Some(par) = st.parent() {
                fs::create_dir_all(par).map_err(|e| io_err("cannot make the folder", e))?;
            }
            if fs::symlink_metadata(&st).is_ok() {
                return Err(BackupError::Invalid(format!("{} is in the way; remove it and try again.", st.display())));
            }
            if s.kind == Kind::Dir {
                fs::create_dir_all(&st).map_err(|e| io_err("cannot make the folder", e))?;
            }
        }
        for e in &m.entries {
            let Some(s) = chosen.iter().find(|s| s.id == e.src) else {
                pump(&mut r, e.len, &mut total, None)?;
                continue;
            };
            let st = stage_of(s);
            let target = if e.sub.is_empty() { st } else { st.join(&e.sub) };
            if let Some(par) = target.parent() {
                fs::create_dir_all(par).map_err(|er| io_err("cannot make the folder", er))?;
            }
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&target).map_err(|er| io_err("cannot write while restoring", er))?;
            let h = pump(&mut r, e.len, &mut total, Some(&mut f))?;
            f.sync_all().map_err(|er| io_err("cannot write while restoring", er))?;
            if h != e.sha256 {
                return Err(BackupError::Damaged("a file changed while it was being restored".into()));
            }
        }
        finish_check(&mut r, total)
    })();
    if let Err(e) = staged {
        cleanup();
        return Err(e);
    }
    let mut kept: Vec<PathBuf> = Vec::new();
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    for s in &chosen {
        let dest = s.path(l);
        let st = stage_of(s);
        let mut aside = None;
        if fs::symlink_metadata(&dest).is_ok() {
            let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mut a = dest.with_file_name(format!("{name}.before-restore-{tag}"));
            let mut n = 2;
            while fs::symlink_metadata(&a).is_ok() {
                a = dest.with_file_name(format!("{name}.before-restore-{tag}-{n}"));
                n += 1;
            }
            if let Err(e) = fs::rename(&dest, &a) {
                rollback(&done);
                cleanup();
                return Err(io_err("cannot keep the current data", e));
            }
            aside = Some(a);
        }
        if let Err(e) = fs::rename(&st, &dest) {
            if let Some(a) = &aside {
                let _ = fs::rename(a, &dest);
            }
            rollback(&done);
            cleanup();
            return Err(io_err("cannot put the restored data in place", e));
        }
        done.push((dest, aside.clone()));
        kept.extend(aside);
    }
    let restored = c.areas.into_iter().filter(|a| req.areas.contains(&a.id)).collect();
    Ok(RestoreReport { restored, kept })
}

fn rollback(done: &[(PathBuf, Option<PathBuf>)]) {
    for (dest, aside) in done.iter().rev() {
        let _ = fs::remove_dir_all(dest).or_else(|_| fs::remove_file(dest));
        if let Some(a) = aside {
            let _ = fs::rename(a, dest);
        }
    }
}

/// `1.5 MB` style size for the page.
pub fn human(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1 << 20 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else if n < 1 << 30 {
        format!("{:.1} MB", n as f64 / 1_048_576.0)
    } else {
        format!("{:.2} GB", n as f64 / 1_073_741_824.0)
    }
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod tests;
