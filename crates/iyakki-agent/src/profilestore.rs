//! Protection of the full browser's profile (cookies, local storage, IndexedDB) with a per-person key.
//! Design and decisions: `docs/development/BROWSER-PROFILE-PROTECTION.md`.
//!
//! * At rest the profile is ONE sealed file, `<data>/web-profile/<user>.sealed` (a tar-like archive of the profile's `data` and
//!   `config` folders, encrypted in 1 MiB chunks with XChaCha20-Poly1305; the chunk counter and a final flag are authenticated, so a
//!   reordered, dropped or cut chunk is detected). The previous sealed copy stays as `<user>.sealed.bak`.
//! * While the person is signed in the profile lives decrypted in a 0700 folder on a RAM disk (`/run/iyakki/profile-live/<user>`);
//!   `<data>/web-profile/<user>` becomes a link to it, so the engine, the helper and the add-on code find the profile where they
//!   always did. Sign-out seals it and wipes the RAM copy; a start of the shell wipes what a crash left.
//! * The 256-bit profile key is a random secret kept in the person's vault (`browser-profile`), so it is wrapped by the login
//!   password the vault already uses: a password change re-wraps it, nothing is re-encrypted. It is never written unwrapped.
//!
//! No file under the data folder holds the profile in clear while it is protected, except the short window of the migration
//! (the plain copy `<user>.pre-protect` is removed as soon as the sealed file has been opened once). Removal overwrites the
//! files with zeros first; that is best effort on a journalling file system or flash (see the design document).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// The vault secret that holds the profile key.
pub const VAULT_SECRET: &str = "browser-profile";
const MAGIC: &[u8; 8] = b"IYKBP1\0\0";
const CHUNK: usize = 1 << 20;
const TAG: usize = 16;
const CHECK: &[u8] = b"iyakki-browser-profile-ok";
/// Most plain bytes one sealed profile may hold (a bigger one is refused, never cut).
pub const MAX_BYTES: u64 = 4 << 30;
const MAX_ENTRIES: u64 = 400_000;
/// The folders of a profile that are sealed. `cache` is rebuilt, kept in RAM only and never sealed.
const SEALED_DIRS: [&str; 2] = ["data", "config"];
/// Where the production image keeps the decrypted copy (a RAM disk).
pub const RUN_ROOT: &str = "/run/iyakki/profile-live";
const PRODUCTION_DATA: &str = "/var/lib/iyakki";

#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum ProfileError {
    #[error("browser data is not protected")]
    NotProtected,
    #[error("browser data is already protected")]
    AlreadyProtected,
    #[error("the protected copy is open: close the browser and sign out first")]
    Open,
    #[error("the protected copy is not open")]
    NotOpen,
    #[error("wrong key: the protected browser data cannot be opened with it")]
    WrongKey,
    #[error("the protected file is damaged or was changed: {0}")]
    Damaged(String),
    #[error("the profile is too big or has too many files to protect")]
    TooBig,
    #[error("the profile changed while it was copied: {0}")]
    Unstable(String),
    #[error("stopped at {0}")]
    Interrupted(&'static str),
    #[error("protection was interrupted earlier: press Protect again")]
    NeedsFinish,
    #[error("storage error: {0}")]
    Storage(String),
}

fn io(e: std::io::Error) -> ProfileError {
    ProfileError::Storage(e.to_string())
}

/// The profile key. Zeroized on drop; `Debug` never shows it.
#[derive(Clone)]
pub struct ProfileKey(Zeroizing<[u8; 32]>);

impl std::fmt::Debug for ProfileKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProfileKey(..)")
    }
}

impl ProfileKey {
    pub fn generate() -> Result<ProfileKey, ProfileError> {
        let mut k = Zeroizing::new([0u8; 32]);
        iyakki_sec::random_fill(&mut *k).map_err(|e| ProfileError::Storage(format!("no entropy: {e}")))?;
        Ok(ProfileKey(k))
    }

    pub fn from_bytes(b: [u8; 32]) -> ProfileKey {
        ProfileKey(Zeroizing::new(b))
    }

    /// The text form stored in the vault.
    pub fn to_hex(&self) -> Zeroizing<String> {
        Zeroizing::new(self.0.iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn from_hex(s: &str) -> Option<ProfileKey> {
        let s = s.trim();
        if s.len() != 64 || !s.is_ascii() {
            return None;
        }
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(ProfileKey::from_bytes(k))
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(Key::from_slice(&self.0[..]))
    }
}

/// The key of a vault: read, or (with `create`) made and stored. Pure vault code, so a password change (which re-wraps the
/// vault's data key) leaves this key and every sealed file as they are.
pub fn key_in_vault(v: &mut crate::vault::LocalVault, create: bool) -> Result<ProfileKey, String> {
    match v.get(VAULT_SECRET) {
        Ok(t) => ProfileKey::from_hex(&t).ok_or_else(|| "the key in the vault is not valid".to_string()),
        Err(crate::vault::VaultError::NotFound(_)) if create => {
            let k = ProfileKey::generate().map_err(|e| e.to_string())?;
            v.set(VAULT_SECRET, &k.to_hex()).map_err(|e| e.to_string())?;
            Ok(k)
        }
        Err(crate::vault::VaultError::NotFound(_)) => Err("there is no browser key in your vault".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

// ------------------------------------------------------------------ the encrypted stream

fn nonce(id: &[u8; 16], ctr: u64) -> [u8; 24] {
    let mut n = [0u8; 24];
    n[..16].copy_from_slice(id);
    n[16..].copy_from_slice(&ctr.to_be_bytes());
    n
}

fn aad(hdr: &[u8; 24], ctr: u64, fin: bool) -> [u8; 33] {
    let mut a = [0u8; 33];
    a[..24].copy_from_slice(hdr);
    a[24..32].copy_from_slice(&ctr.to_be_bytes());
    a[32] = fin as u8;
    a
}

/// A `Write` that encrypts what is written in chunks. `finish` writes the last chunk and returns the digest and size of the plain bytes.
struct SealWriter<W: Write> {
    w: W,
    key: ProfileKey,
    id: [u8; 16],
    hdr: [u8; 24],
    ctr: u64,
    buf: Vec<u8>,
    hash: Sha256,
    total: u64,
}

impl<W: Write> SealWriter<W> {
    fn new(mut w: W, key: &ProfileKey) -> Result<Self, ProfileError> {
        let mut id = [0u8; 16];
        iyakki_sec::random_fill(&mut id).map_err(|e| ProfileError::Storage(format!("no entropy: {e}")))?;
        let mut hdr = [0u8; 24];
        hdr[..8].copy_from_slice(MAGIC);
        hdr[8..].copy_from_slice(&id);
        let check = key.cipher().encrypt(XNonce::from_slice(&nonce(&id, u64::MAX)), Payload { msg: CHECK, aad: &aad(&hdr, u64::MAX, false) }).map_err(|_| ProfileError::Storage("encryption failed".into()))?;
        w.write_all(&hdr).and_then(|_| w.write_all(&check)).map_err(io)?;
        Ok(SealWriter { w, key: key.clone(), id, hdr, ctr: 0, buf: Vec::with_capacity(CHUNK), hash: Sha256::new(), total: 0 })
    }

    fn flush_chunk(&mut self, fin: bool) -> std::io::Result<()> {
        let ct = self
            .key
            .cipher()
            .encrypt(XNonce::from_slice(&nonce(&self.id, self.ctr)), Payload { msg: &self.buf, aad: &aad(&self.hdr, self.ctr, fin) })
            .map_err(|_| std::io::Error::other("encryption failed"))?;
        self.w.write_all(&(ct.len() as u32).to_be_bytes())?;
        self.w.write_all(&ct)?;
        self.ctr += 1;
        self.buf.clear();
        Ok(())
    }

    fn finish(mut self) -> Result<(W, [u8; 32], u64), ProfileError> {
        self.flush_chunk(true).map_err(io)?;
        self.w.flush().map_err(io)?;
        Ok((self.w, self.hash.finalize().into(), self.total))
    }
}

impl<W: Write> Write for SealWriter<W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        if self.buf.len() == CHUNK {
            self.flush_chunk(false)?;
        }
        let n = data.len().min(CHUNK - self.buf.len());
        self.buf.extend_from_slice(&data[..n]);
        self.hash.update(&data[..n]);
        self.total += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A `Read` that decrypts and authenticates. A bad chunk is an `InvalidData` error.
struct SealReader<R: Read> {
    r: R,
    key: ProfileKey,
    id: [u8; 16],
    hdr: [u8; 24],
    ctr: u64,
    cur: Vec<u8>,
    pos: usize,
    next_len: Option<u32>,
    done: bool,
}

fn read_len<R: Read>(r: &mut R) -> std::io::Result<Option<u32>> {
    let mut b = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        let n = r.read(&mut b[got..])?;
        if n == 0 {
            return if got == 0 { Ok(None) } else { Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "cut off")) };
        }
        got += n;
    }
    Ok(Some(u32::from_be_bytes(b)))
}

fn bad(m: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string())
}

impl<R: Read> SealReader<R> {
    fn open(mut r: R, key: &ProfileKey) -> Result<Self, ProfileError> {
        let mut hdr = [0u8; 24];
        r.read_exact(&mut hdr).map_err(|_| ProfileError::Damaged("too short".into()))?;
        if &hdr[..8] != MAGIC {
            return Err(ProfileError::Damaged("not a protected profile".into()));
        }
        let mut id = [0u8; 16];
        id.copy_from_slice(&hdr[8..]);
        let mut check = vec![0u8; CHECK.len() + TAG];
        r.read_exact(&mut check).map_err(|_| ProfileError::Damaged("too short".into()))?;
        let ok = key.cipher().decrypt(XNonce::from_slice(&nonce(&id, u64::MAX)), Payload { msg: &check, aad: &aad(&hdr, u64::MAX, false) }).map(|p| p == CHECK).unwrap_or(false);
        if !ok {
            return Err(ProfileError::WrongKey);
        }
        let next_len = read_len(&mut r).map_err(|_| ProfileError::Damaged("cut off".into()))?;
        if next_len.is_none() {
            return Err(ProfileError::Damaged("cut off".into()));
        }
        Ok(SealReader { r, key: key.clone(), id, hdr, ctr: 0, cur: Vec::new(), pos: 0, next_len, done: false })
    }

    fn fill(&mut self) -> std::io::Result<()> {
        let len = self.next_len.take().ok_or_else(|| bad("cut off"))? as usize;
        if !(TAG..=CHUNK + TAG).contains(&len) {
            return Err(bad("bad chunk size"));
        }
        let mut ct = vec![0u8; len];
        self.r.read_exact(&mut ct).map_err(|_| bad("cut off"))?;
        self.next_len = read_len(&mut self.r)?;
        let fin = self.next_len.is_none();
        let plain = self.key.cipher().decrypt(XNonce::from_slice(&nonce(&self.id, self.ctr)), Payload { msg: &ct, aad: &aad(&self.hdr, self.ctr, fin) }).map_err(|_| bad("a chunk does not authenticate"))?;
        self.ctr += 1;
        self.cur = plain;
        self.pos = 0;
        self.done = fin;
        Ok(())
    }
}

impl<R: Read> Read for SealReader<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pos >= self.cur.len() {
            if self.done {
                return Ok(0);
            }
            self.fill()?;
        }
        let n = out.len().min(self.cur.len() - self.pos);
        out[..n].copy_from_slice(&self.cur[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn damaged(e: std::io::Error) -> ProfileError {
    if e.kind() == std::io::ErrorKind::InvalidData || e.kind() == std::io::ErrorKind::UnexpectedEof {
        ProfileError::Damaged(e.to_string())
    } else {
        io(e)
    }
}

// ------------------------------------------------------------------ the archive (tar-like)

const K_END: u8 = 0;
const K_DIR: u8 = 1;
const K_FILE: u8 = 2;

fn sorted_entries(dir: &Path) -> Result<Vec<PathBuf>, ProfileError> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map_err(io)?.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    v.sort();
    Ok(v)
}

/// Write `data` and `config` of `base` into `w`. Symbolic links and special files are left out. Returns the entry count.
fn pack<W: Write>(w: &mut W, base: &Path) -> Result<u64, ProfileError> {
    struct St {
        entries: u64,
        bytes: u64,
    }
    fn rec<W: Write>(w: &mut W, rel: &str, kind: u8, size: u64) -> Result<(), ProfileError> {
        let rb = rel.as_bytes();
        if rb.len() > 1024 {
            return Err(ProfileError::Unstable("a path is too long".into()));
        }
        w.write_all(&[kind]).and_then(|_| w.write_all(&(rb.len() as u16).to_be_bytes())).and_then(|_| w.write_all(rb)).map_err(io)?;
        if kind == K_FILE {
            w.write_all(&size.to_be_bytes()).map_err(io)?;
        }
        Ok(())
    }
    fn walk<W: Write>(w: &mut W, dir: &Path, rel: &str, st: &mut St) -> Result<(), ProfileError> {
        for p in sorted_entries(dir)? {
            let name = p.file_name().and_then(|n| n.to_str()).ok_or_else(|| ProfileError::Unstable("a file name is not text".into()))?;
            let r = format!("{rel}/{name}");
            let md = match std::fs::symlink_metadata(&p) {
                Ok(m) => m,
                Err(_) => continue, // vanished while we looked
            };
            st.entries += 1;
            if st.entries > MAX_ENTRIES {
                return Err(ProfileError::TooBig);
            }
            if md.is_dir() {
                rec(w, &r, K_DIR, 0)?;
                walk(w, &p, &r, st)?;
            } else if md.is_file() {
                let size = md.len();
                st.bytes += size;
                if st.bytes > MAX_BYTES {
                    return Err(ProfileError::TooBig);
                }
                rec(w, &r, K_FILE, size)?;
                let mut f = std::fs::File::open(&p).map_err(io)?;
                let n = std::io::copy(&mut (&mut f).take(size), w).map_err(io)?;
                if n != size {
                    return Err(ProfileError::Unstable(r));
                }
            } else {
                st.entries -= 1;
            }
        }
        Ok(())
    }
    let mut st = St { entries: 0, bytes: 0 };
    for top in SEALED_DIRS {
        let d = base.join(top);
        if std::fs::symlink_metadata(&d).map(|m| m.is_dir()).unwrap_or(false) {
            st.entries += 1;
            rec(w, top, K_DIR, 0)?;
            walk(w, &d, top, &mut st)?;
        }
    }
    w.write_all(&[K_END]).map_err(io)?;
    Ok(st.entries)
}

/// `Some` only for a relative path of plain names that starts in a sealed folder.
fn clean_rel(s: &str) -> Option<PathBuf> {
    if s.is_empty() || s.contains('\0') || s.len() > 1024 {
        return None;
    }
    let p = Path::new(s);
    let mut n = 0;
    for c in p.components() {
        match c {
            std::path::Component::Normal(_) => n += 1,
            _ => return None,
        }
    }
    let first = p.components().next()?.as_os_str().to_str()?;
    (n > 0 && SEALED_DIRS.contains(&first)).then(|| p.to_path_buf())
}

enum Mode<'a> {
    Extract(&'a Path),
    Compare(&'a Path),
}

fn read_exact_or<R: Read>(r: &mut R, b: &mut [u8]) -> Result<(), ProfileError> {
    r.read_exact(b).map_err(damaged)
}

fn unpack<R: Read>(r: &mut R, mode: Mode) -> Result<u64, ProfileError> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let mut entries = 0u64;
    let mut bytes = 0u64;
    loop {
        let mut k = [0u8; 1];
        read_exact_or(r, &mut k)?;
        if k[0] == K_END {
            break;
        }
        if k[0] != K_DIR && k[0] != K_FILE {
            return Err(ProfileError::Damaged("unknown record".into()));
        }
        let mut l = [0u8; 2];
        read_exact_or(r, &mut l)?;
        let mut name = vec![0u8; u16::from_be_bytes(l) as usize];
        read_exact_or(r, &mut name)?;
        let rel = clean_rel(std::str::from_utf8(&name).map_err(|_| ProfileError::Damaged("bad name".into()))?).ok_or_else(|| ProfileError::Damaged("a path leaves the profile".into()))?;
        entries += 1;
        if entries > MAX_ENTRIES {
            return Err(ProfileError::TooBig);
        }
        let size = if k[0] == K_FILE {
            let mut s = [0u8; 8];
            read_exact_or(r, &mut s)?;
            let s = u64::from_be_bytes(s);
            bytes += s;
            if bytes > MAX_BYTES {
                return Err(ProfileError::TooBig);
            }
            s
        } else {
            0
        };
        match &mode {
            Mode::Extract(dest) => {
                let p = dest.join(&rel);
                if k[0] == K_DIR {
                    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&p).map_err(io)?;
                } else {
                    if let Some(d) = p.parent() {
                        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(d).map_err(io)?;
                    }
                    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&p).map_err(io)?;
                    let n = std::io::copy(&mut Read::take(&mut *r, size), &mut f).map_err(damaged)?;
                    if n != size {
                        return Err(ProfileError::Damaged("cut off".into()));
                    }
                }
            }
            Mode::Compare(src) => {
                let p = src.join(&rel);
                let md = std::fs::symlink_metadata(&p).map_err(|_| ProfileError::Damaged(format!("verification: {} is missing", rel.display())))?;
                if k[0] == K_DIR {
                    if !md.is_dir() {
                        return Err(ProfileError::Damaged("verification: a folder differs".into()));
                    }
                } else {
                    if !md.is_file() || md.len() != size {
                        return Err(ProfileError::Damaged("verification: a file differs".into()));
                    }
                    let mut f = std::fs::File::open(&p).map_err(io)?;
                    let (mut a, mut b) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
                    let mut left = size as usize;
                    while left > 0 {
                        let n = left.min(a.len());
                        read_exact_or(r, &mut a[..n])?;
                        f.read_exact(&mut b[..n]).map_err(io)?;
                        if a[..n] != b[..n] {
                            return Err(ProfileError::Damaged("verification: a file differs".into()));
                        }
                        left -= n;
                    }
                }
            }
        }
    }
    let mut extra = [0u8; 1];
    if r.read(&mut extra).map_err(damaged)? != 0 {
        return Err(ProfileError::Damaged("data after the end".into()));
    }
    Ok(entries)
}

fn count_entries(base: &Path) -> u64 {
    fn walk(d: &Path) -> u64 {
        let mut n = 0;
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let Ok(md) = std::fs::symlink_metadata(e.path()) else { continue };
            if md.is_dir() {
                n += 1 + walk(&e.path());
            } else if md.is_file() {
                n += 1;
            }
        }
        n
    }
    SEALED_DIRS.iter().map(|t| base.join(t)).filter(|d| std::fs::symlink_metadata(d).map(|m| m.is_dir()).unwrap_or(false)).map(|d| 1 + walk(&d)).sum()
}

// ------------------------------------------------------------------ removal

/// Overwrite the files below `p` with zeros, then remove them (best effort; see the module comment). Links are only unlinked.
pub fn secure_remove(p: &Path) {
    let Ok(md) = std::fs::symlink_metadata(p) else { return };
    if md.is_dir() {
        for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
            secure_remove(&e.path());
        }
        let _ = std::fs::remove_dir(p);
    } else {
        if md.is_file() {
            if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(p) {
                let zeros = vec![0u8; 64 * 1024];
                let mut left = md.len();
                while left > 0 {
                    let n = left.min(zeros.len() as u64) as usize;
                    if f.write_all(&zeros[..n]).is_err() {
                        break;
                    }
                    left -= n as u64;
                }
                let _ = f.sync_all();
            }
        }
        let _ = std::fs::remove_file(p);
    }
}

fn sync_dir(d: &Path) {
    if let Ok(f) = std::fs::File::open(d) {
        let _ = f.sync_all();
    }
}

// ------------------------------------------------------------------ the store

/// Where the decrypted copy lives for this data folder: the RAM disk on the production image (or `IYAKKI_PROFILE_RUN`), else a
/// folder inside the data folder (development and tests: say so in the status).
pub fn run_root(data: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os("IYAKKI_PROFILE_RUN").map(PathBuf::from).filter(|p| p.is_absolute()) {
        return p;
    }
    if data == Path::new(PRODUCTION_DATA) {
        PathBuf::from(RUN_ROOT)
    } else {
        data.join("web-profile-live")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not protected: plain files (the default).
    Off,
    /// Protected, closed (nobody signed in, or the browser data was sealed).
    Sealed,
    /// Protected, decrypted copy in RAM while the person is signed in.
    Open,
    /// A migration stopped half way: the sealed file and plain files both exist.
    NeedsFinish,
}

impl Status {
    /// The short text shown in Settings and by `iyakki dev state` (never a key, path or name).
    pub fn text(self) -> &'static str {
        match self {
            Status::Off => "off: browser data is plain files",
            Status::Sealed => "protected: sealed",
            Status::Open => "protected: open while signed in",
            Status::NeedsFinish => "protection was interrupted; press Protect again",
        }
    }
}

/// Test fault points of [`ProfileStore::seal`] and the migration (a "kill" there returns `Interrupted`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    NewWritten,
    NewVerified,
    BackupLinked,
    Rotated,
    SealedPlaced,
}

pub struct ProfileStore {
    base: PathBuf,
    safe: String,
    live_root: PathBuf,
    fault: Option<Step>,
}

static OPS: Mutex<()> = Mutex::new(());

fn lock_ops() -> std::sync::MutexGuard<'static, ()> {
    OPS.lock().unwrap_or_else(|e| e.into_inner())
}

impl ProfileStore {
    pub fn new(data: &Path, user: &str) -> ProfileStore {
        ProfileStore::with_run_root(data, user, &run_root(data))
    }

    pub fn with_run_root(data: &Path, user: &str, run: &Path) -> ProfileStore {
        ProfileStore { base: data.join("web-profile"), safe: crate::webengine::safe_name(user), live_root: run.to_path_buf(), fault: None }
    }

    #[cfg(test)]
    pub fn with_fault(mut self, s: Step) -> ProfileStore {
        self.fault = Some(s);
        self
    }

    fn point(&self, s: Step) -> Result<(), ProfileError> {
        if self.fault == Some(s) {
            return Err(ProfileError::Interrupted(match s {
                Step::NewWritten => "new-written",
                Step::NewVerified => "new-verified",
                Step::BackupLinked => "backup-linked",
                Step::Rotated => "rotated",
                Step::SealedPlaced => "sealed-placed",
            }));
        }
        Ok(())
    }

    fn f(&self, ext: &str) -> PathBuf {
        self.base.join(format!("{}.{ext}", self.safe))
    }
    fn sealed(&self) -> PathBuf {
        self.f("sealed")
    }
    fn backup(&self) -> PathBuf {
        self.f("sealed.bak")
    }
    fn newf(&self) -> PathBuf {
        self.f("sealed.new")
    }
    fn pre(&self) -> PathBuf {
        self.f("pre-protect")
    }
    /// What the engine uses: `<data>/web-profile/<user>` (a link to the RAM copy while open).
    pub fn profile_path(&self) -> PathBuf {
        self.base.join(&self.safe)
    }
    fn live(&self) -> PathBuf {
        self.live_root.join(&self.safe)
    }

    pub fn is_protected(&self) -> bool {
        self.sealed().is_file() || self.backup().is_file()
    }

    fn is_open(&self) -> bool {
        self.live().join(".open").is_file()
    }

    fn plain_present(&self) -> bool {
        let p = self.profile_path();
        std::fs::symlink_metadata(&p).map(|m| m.is_dir()).unwrap_or(false) && count_entries(&p) > 0
    }

    pub fn status(&self) -> Status {
        if !self.is_protected() {
            Status::Off
        } else if self.plain_present() {
            Status::NeedsFinish
        } else if self.is_open() {
            Status::Open
        } else {
            Status::Sealed
        }
    }

    fn mkdir(&self, d: &Path) -> Result<(), ProfileError> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(d).map_err(io)
    }

    /// Write the archive of `src` as a sealed file at `dest`, verified by reading it back. Returns the entry count.
    fn write_sealed(&self, src: &Path, dest: &Path, key: &ProfileKey) -> Result<u64, ProfileError> {
        use std::os::unix::fs::OpenOptionsExt;
        self.mkdir(&self.base)?;
        let file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(dest).map_err(io)?;
        let mut w = SealWriter::new(std::io::BufWriter::new(file), key)?;
        let entries = pack(&mut w, src)?;
        let (bw, digest, total) = w.finish()?;
        let file = bw.into_inner().map_err(|e| io(e.into_error()))?;
        file.sync_all().map_err(io)?;
        self.point(Step::NewWritten)?;
        // read it back: decrypt everything and compare the digest of the plain stream
        let mut r = SealReader::open(std::io::BufReader::new(std::fs::File::open(dest).map_err(io)?), key)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut n_total = 0u64;
        loop {
            let n = r.read(&mut buf).map_err(damaged)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            n_total += n as u64;
        }
        let d: [u8; 32] = h.finalize().into();
        if d != digest || n_total != total {
            return Err(ProfileError::Damaged("the new sealed file did not read back".into()));
        }
        self.point(Step::NewVerified)?;
        Ok(entries)
    }

    fn extract(&self, from: &Path, key: &ProfileKey, to: &Path) -> Result<u64, ProfileError> {
        let mut r = SealReader::open(std::io::BufReader::new(std::fs::File::open(from).map_err(io)?), key)?;
        let res = (|| {
            self.mkdir(to)?;
            unpack(&mut r, Mode::Extract(to))
        })();
        if res.is_err() {
            secure_remove(to);
        }
        res
    }

    /// Make the plain profile (`data`, `config`) a sealed one. The plain copy is verified against the sealed file and moved to
    /// `<user>.pre-protect`; [`ProfileStore::open`] removes it after the first successful open. Safe to run again after a stop.
    pub fn protect(&self, key: &ProfileKey) -> Result<(), ProfileError> {
        let _g = lock_ops();
        if self.is_open() {
            return Err(ProfileError::Open);
        }
        if self.is_protected() && !self.plain_present() {
            return Err(ProfileError::AlreadyProtected);
        }
        let plain = self.profile_path();
        if std::fs::symlink_metadata(&plain).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            let _ = std::fs::remove_file(&plain);
        }
        let _ = std::fs::create_dir_all(&plain);
        let entries = self.write_sealed(&plain, &self.newf(), key)?;
        // the sealed file must match the plain files, not just itself
        {
            let mut r = SealReader::open(std::io::BufReader::new(std::fs::File::open(self.newf()).map_err(io)?), key)?;
            unpack(&mut r, Mode::Compare(&plain))?;
            if count_entries(&plain) != entries {
                return Err(ProfileError::Unstable("files changed while they were copied".into()));
            }
        }
        std::fs::rename(self.newf(), self.sealed()).map_err(io)?;
        sync_dir(&self.base);
        self.point(Step::SealedPlaced)?;
        if self.pre().exists() {
            secure_remove(&self.pre());
        }
        std::fs::rename(&plain, self.pre()).map_err(io)?;
        sync_dir(&self.base);
        Ok(())
    }

    /// Decrypt into the RAM copy and link it in place. Falls back to the backup copy when the main one is damaged. Returns whether
    /// the backup was used. A wrong key never falls back.
    pub fn open(&self, key: &ProfileKey) -> Result<bool, ProfileError> {
        let _g = lock_ops();
        if !self.is_protected() {
            return Err(ProfileError::NotProtected);
        }
        if self.plain_present() {
            return Err(ProfileError::NeedsFinish);
        }
        if self.is_open() {
            return Ok(false);
        }
        let live = self.live();
        secure_remove(&live);
        self.mkdir(&self.live_root)?;
        let tmp = self.live_root.join(format!("{}.tmp", self.safe));
        secure_remove(&tmp);
        let mut used_backup = false;
        let mut r = Err(ProfileError::NotProtected);
        for (i, p) in [self.sealed(), self.backup()].iter().enumerate() {
            if !p.is_file() {
                continue;
            }
            r = self.extract(p, key, &tmp);
            match &r {
                Ok(_) => {
                    used_backup = i == 1;
                    break;
                }
                Err(ProfileError::WrongKey) => return Err(ProfileError::WrongKey),
                Err(_) => continue,
            }
        }
        r?;
        std::fs::rename(&tmp, &live).map_err(io)?;
        self.mkdir(&live.join("cache"))?;
        std::fs::write(live.join(".open"), b"").map_err(io)?;
        // the engine's path now leads to the RAM copy
        let link = self.profile_path();
        self.mkdir(&self.base)?;
        if std::fs::symlink_metadata(&link).is_ok() {
            if std::fs::symlink_metadata(&link).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                let _ = std::fs::remove_file(&link);
            } else {
                secure_remove(&live);
                return Err(ProfileError::NeedsFinish);
            }
        }
        std::os::unix::fs::symlink(&live, &link).map_err(io)?;
        // the sealed file has now been opened once: the plain migration copy is no longer needed
        if self.pre().exists() {
            secure_remove(&self.pre());
        }
        Ok(used_backup)
    }

    /// Write the RAM copy back (atomic, the previous sealed file stays as the backup) and keep it open.
    pub fn checkpoint(&self, key: &ProfileKey) -> Result<(), ProfileError> {
        let _g = lock_ops();
        self.seal_inner(key)
    }

    fn seal_inner(&self, key: &ProfileKey) -> Result<(), ProfileError> {
        if !self.is_open() {
            return Err(ProfileError::NotOpen);
        }
        let new = self.newf();
        let r = self.write_sealed(&self.live(), &new, key).and_then(|_| {
            // the old sealed file becomes the backup through a link, so a sealed file exists at every moment
            if self.sealed().is_file() {
                let tmp = self.f("sealed.bak.tmp");
                let _ = std::fs::remove_file(&tmp);
                if std::fs::hard_link(self.sealed(), &tmp).is_err() {
                    std::fs::copy(self.sealed(), &tmp).map_err(io)?;
                }
                self.point(Step::BackupLinked)?;
                std::fs::rename(&tmp, self.backup()).map_err(io)?;
            }
            self.point(Step::Rotated)?;
            std::fs::rename(&new, self.sealed()).map_err(io)?;
            sync_dir(&self.base);
            Ok(())
        });
        if r.is_err() && !matches!(r, Err(ProfileError::Interrupted(_))) {
            let _ = std::fs::remove_file(&new);
        }
        r
    }

    /// Seal (write back) and wipe the RAM copy and its link. If sealing fails the RAM copy is kept so nothing is lost.
    pub fn seal(&self, key: &ProfileKey) -> Result<(), ProfileError> {
        let _g = lock_ops();
        self.seal_inner(key)?;
        self.wipe_live();
        Ok(())
    }

    fn wipe_live(&self) {
        let link = self.profile_path();
        if std::fs::symlink_metadata(&link).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            let _ = std::fs::remove_file(&link);
        }
        secure_remove(&self.live());
    }

    /// Re-encrypt under a new key (key rotation; a password change does not need this). The backup is dropped.
    pub fn rekey(&self, old: &ProfileKey, new: &ProfileKey) -> Result<(), ProfileError> {
        let _g = lock_ops();
        if self.is_open() {
            return Err(ProfileError::Open);
        }
        if !self.is_protected() {
            return Err(ProfileError::NotProtected);
        }
        let tmp = self.live_root.join(format!("{}.rekey", self.safe));
        secure_remove(&tmp);
        self.extract(&self.sealed(), old, &tmp)?;
        let r = self.write_sealed(&tmp, &self.newf(), new).and_then(|_| {
            std::fs::rename(self.newf(), self.sealed()).map_err(io)?;
            let _ = std::fs::remove_file(self.backup());
            sync_dir(&self.base);
            Ok(())
        });
        secure_remove(&tmp);
        if r.is_err() {
            let _ = std::fs::remove_file(self.newf());
        }
        r
    }

    /// "Reset browser data": delete the protected profile for good (also when the key is lost). Protection is off afterwards.
    pub fn reset(&self) {
        let _g = lock_ops();
        self.wipe_live();
        for p in [self.sealed(), self.backup(), self.newf(), self.f("sealed.bak.tmp"), self.pre()] {
            secure_remove(&p);
        }
        let plain = self.profile_path();
        if std::fs::symlink_metadata(&plain).map(|m| m.is_dir()).unwrap_or(false) {
            secure_remove(&plain);
        }
        secure_remove(&self.live_root.join(format!("{}.tmp", self.safe)));
    }

    /// Switch protection off: decrypt into plain files again, then remove the sealed files.
    pub fn unprotect(&self, key: &ProfileKey) -> Result<(), ProfileError> {
        let _g = lock_ops();
        if self.is_open() {
            return Err(ProfileError::Open);
        }
        if !self.is_protected() {
            return Err(ProfileError::NotProtected);
        }
        let plain = self.profile_path();
        let tmp = self.f("restoring");
        secure_remove(&tmp);
        let mut last = Err(ProfileError::NotProtected);
        for p in [self.sealed(), self.backup()] {
            if p.is_file() {
                last = self.extract(&p, key, &tmp);
                match last {
                    Ok(_) => break,
                    Err(ProfileError::WrongKey) => return Err(ProfileError::WrongKey),
                    Err(_) => {}
                }
            }
        }
        last?;
        for d in ["cache"] {
            self.mkdir(&tmp.join(d))?;
        }
        if std::fs::symlink_metadata(&plain).is_ok() {
            secure_remove(&plain);
        }
        std::fs::rename(&tmp, &plain).map_err(io)?;
        for p in [self.sealed(), self.backup(), self.pre()] {
            secure_remove(&p);
        }
        sync_dir(&self.base);
        Ok(())
    }

    /// Undo a migration that was not finished: bring back the plain copy kept as `<user>.pre-protect`.
    pub fn rollback(&self) -> Result<(), ProfileError> {
        let _g = lock_ops();
        if !self.pre().is_dir() {
            return Err(ProfileError::NotProtected);
        }
        self.wipe_live();
        for p in [self.sealed(), self.backup(), self.newf()] {
            secure_remove(&p);
        }
        let plain = self.profile_path();
        if std::fs::symlink_metadata(&plain).is_ok() {
            secure_remove(&plain);
        }
        std::fs::rename(self.pre(), plain).map_err(io)
    }
}

/// At the start of the shell: wipe every decrypted copy a crash left (and their dangling links). Nothing is sealed here, because no
/// key is known yet; at most the changes since the last write-back are lost (the design document, decision D6).
pub fn wipe_stale(data: &Path) -> usize {
    let _g = lock_ops();
    let root = run_root(data);
    let mut n = 0;
    for e in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        secure_remove(&e.path());
        n += 1;
    }
    let base = data.join("web-profile");
    for e in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        let p = e.path();
        if std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(false) && !p.exists() {
            let _ = std::fs::remove_file(&p);
        }
        // half-written files of an interrupted write
        if p.extension().is_some_and(|x| x == "new" || x == "tmp") {
            let _ = std::fs::remove_file(&p);
        }
    }
    n
}

/// One line for `iyakki dev diag`: how many protected profiles there are and how many are open. No names, paths or keys.
pub fn summary(data: &Path) -> String {
    let base = data.join("web-profile");
    let root = run_root(data);
    let (mut sealed, mut open) = (0, 0);
    for e in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if let Some(safe) = n.strip_suffix(".sealed") {
            sealed += 1;
            if root.join(safe).join(".open").is_file() {
                open += 1;
            }
        }
    }
    if sealed == 0 {
        "off for everybody (browser data is plain files)".to_string()
    } else {
        format!("{sealed} protected profile(s), {open} open")
    }
}

// ------------------------------------------------------------------ in the running shell

/// Keys of the people who are signed in and whose profile is open (by RAM folder), so sign-out and checkpoints can seal.
static HELD: Mutex<Option<HashMap<PathBuf, ProfileKey>>> = Mutex::new(None);

fn hold(path: PathBuf, k: Option<ProfileKey>) {
    let mut g = HELD.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    match k {
        Some(k) => {
            m.insert(path, k);
        }
        None => {
            m.remove(&path);
        }
    }
}

fn held(path: &Path) -> Option<ProfileKey> {
    HELD.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(path).cloned())
}

/// What happened at sign-in (for one audit line and one toast).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignIn {
    /// Protection is off for this person: nothing was done.
    Off,
    Opened { from_backup: bool },
    /// Protected but not opened (the key is not available, or the file is damaged): the reason, without paths or keys.
    Locked(String),
}

/// Called after the person's vault was opened with the password they typed. `key` is the vault's profile key (None when the vault
/// is locked or has no such key).
pub fn on_sign_in(data: &Path, user: &str, key: Option<ProfileKey>) -> SignIn {
    let st = ProfileStore::new(data, user);
    if !st.is_protected() {
        return SignIn::Off;
    }
    let Some(key) = key else {
        return SignIn::Locked("the browser key is not in your vault (forgotten password or reset vault): reset the browser data in Settings -> Browser".into());
    };
    match st.open(&key) {
        Ok(b) => {
            hold(st.live(), Some(key));
            SignIn::Opened { from_backup: b }
        }
        Err(ProfileError::WrongKey) => SignIn::Locked("the key does not open the protected data: reset the browser data in Settings -> Browser".into()),
        Err(e) => SignIn::Locked(e.to_string()),
    }
}

/// Sign-out: seal and wipe. On failure the RAM copy stays (nothing is lost) and the error is returned. The key is forgotten either way.
pub fn on_sign_out(data: &Path, user: &str) -> Result<bool, ProfileError> {
    let st = ProfileStore::new(data, user);
    let key = held(&st.live());
    hold(st.live(), None);
    if !st.is_open() {
        return Ok(false);
    }
    match key {
        Some(k) => st.seal(&k).map(|_| true),
        None => Err(ProfileError::NotOpen),
    }
}

/// Write the open copy back without closing it (an idle moment, or the person leaving the browser tile).
pub fn on_checkpoint(data: &Path, user: &str) -> Result<(), ProfileError> {
    let st = ProfileStore::new(data, user);
    match held(&st.live()) {
        Some(k) if st.is_open() => st.checkpoint(&k),
        _ => Ok(()),
    }
}

/// Used by `webengine::prepare_profile`: the person's browser data is protected but not open, so the engine must not start (and
/// must not create a plain profile beside the sealed one). `None` = go ahead.
pub fn engine_blocked(data: &Path, user: &str) -> Option<String> {
    let st = ProfileStore::new(data, user);
    match st.status() {
        Status::Off | Status::Open => None,
        Status::Sealed => Some("Your browser data is protected and not open. Sign out and in again, or see Settings -> Browser.".into()),
        Status::NeedsFinish => Some("Protecting your browser data was interrupted. Press Protect in Settings -> Browser again.".into()),
    }
}

// ------------------------------------------------------------------ Settings actions (need the services for the vault)

fn svc_key(svc: &crate::services::Services, user: &str, create: bool) -> Result<ProfileKey, String> {
    match svc.user_get(user, VAULT_SECRET) {
        Ok(t) => ProfileKey::from_hex(&t).ok_or_else(|| "the browser key in your vault is not valid".to_string()),
        Err(crate::vault::VaultError::NotFound(_)) if create => {
            let k = ProfileKey::generate().map_err(|e| e.to_string())?;
            svc.user_set(user, VAULT_SECRET, &k.to_hex()).map_err(|e| e.to_string())?;
            Ok(k)
        }
        Err(crate::vault::VaultError::NotFound(_)) => Err("your vault has no browser key".into()),
        Err(crate::vault::VaultError::Locked) => Err("your vault is locked: sign out and in again".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// The key for the sign-in hook: only when protection is on, never creates one.
pub fn sign_in_for(svc: &crate::services::Services, user: &str) -> SignIn {
    let key = svc_key(svc, user, false).ok();
    on_sign_in(svc.dir(), user, key)
}

pub fn status_of(svc: &crate::services::Services, user: &str) -> Status {
    ProfileStore::new(svc.dir(), user).status()
}

/// "Protect my browser data": make or reuse the key in the vault, seal the plain profile, open it (which proves the sealed file
/// reads back) and remove the plain copy.
pub fn protect_for(svc: &crate::services::Services, user: &str) -> Result<String, String> {
    let st = ProfileStore::new(svc.dir(), user);
    let key = svc_key(svc, user, true)?;
    st.protect(&key).map_err(|e| e.to_string())?;
    match on_sign_in(svc.dir(), user, Some(key)) {
        SignIn::Opened { .. } => Ok("Your browser data is protected. Sites you were signed in to stay signed in.".into()),
        SignIn::Locked(why) => {
            let _ = st.rollback();
            Err(format!("Could not open the protected copy ({why}); nothing was changed."))
        }
        SignIn::Off => Err("protection did not start".into()),
    }
}

/// "Stop protecting": decrypt into plain files again.
pub fn unprotect_for(svc: &crate::services::Services, user: &str) -> Result<String, String> {
    let st = ProfileStore::new(svc.dir(), user);
    let key = svc_key(svc, user, false)?;
    // close the open copy first so the latest changes are in the sealed file
    on_sign_out(svc.dir(), user).map_err(|e| e.to_string())?;
    st.unprotect(&key).map_err(|e| e.to_string())?;
    let _ = svc.user_delete(user, VAULT_SECRET);
    Ok("Browser data protection is off. The browser keeps its files as plain files again.".into())
}

/// "Reset browser data": delete the profile (cookies, sign-ins, site storage) and the key; protection is off afterwards.
pub fn reset_for(svc: &crate::services::Services, user: &str) -> String {
    let st = ProfileStore::new(svc.dir(), user);
    hold(st.live(), None);
    st.reset();
    let _ = svc.user_delete(user, VAULT_SECRET);
    "Browser data was deleted: cookies, sign-ins and site storage. Protection is off.".into()
}

#[cfg(test)]
#[path = "profilestore_tests.rs"]
mod tests;
