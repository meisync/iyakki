//! Snapshots and backups of a tenant's volume (docs/development/CLOUD-MULTITENANT-PLAN.md, 3b): `iyakki cloud snapshot create | list |
//! verify | restore | prune | cronjob`.
//!
//! * A snapshot is a folder `<snapshots>/<id>/` holding `data.tar.zst` (a ustar archive, zstd compressed with `ruzstd`), `manifest.json`
//!   (every file and folder with size, mode and SHA-256; the archive's SHA-256; links that were skipped) and `manifest.sha256`. It is built
//!   under a temporary name and renamed, so a half-made snapshot never shows up in the list.
//! * The default snapshot folder is `<data>/cloud/snapshots`, a sibling of the live `<data>/cloud/files`, never inside it (refused if the
//!   two are nested). A second claim mounted elsewhere can be used with `--dir`.
//! * Links are never followed and never archived (they are listed as skipped), so a snapshot cannot carry another person's data out of a
//!   folder, and a restore cannot plant a link.
//! * Restore checks the manifest and the archive first, unpacks into a staging folder inside the snapshot, checks every file against the
//!   manifest and only then copies. It writes into a NEW folder by default (`<data>/cloud/restored/<id>`, refused if it exists); into any
//!   other folder a file that is already there stops the restore unless `--overwrite` is given. Every destination path goes through the
//!   path jail (no `..`, no link on the way).
//! * **Encryption at rest** (optional, on when a key is given): the archive is sealed in 64 KiB chunks with XChaCha20-Poly1305 (`data.tar.zst.enc`),
//!   each chunk bound to its position and to the end of the stream (no reordering, no truncation), and the snapshot id and key id are
//!   bound as associated data (a snapshot cannot be passed off as another, nor under another key id). The file list and sizes are sealed too,
//!   so the plain `manifest.json` of an encrypted snapshot holds only ids, sizes of the ciphertext, a random salt and the sealed list. The key
//!   comes from a [`KeySource`]: a file of 64 hex digits, or the tenant's vault ([`VaultKeyring`]: `snapshot-key.<id>` secrets in the
//!   tenant scope, `snapshot-key.current` names the one to use; a key's id is the start of its SHA-256). A restore authenticates every chunk
//!   before anything is unpacked, so a changed or truncated archive, or the wrong key, is refused with nothing written. Key rotation:
//!   [`VaultKeyring::rotate`] makes a new current key (old ones stay so old snapshots still open); [`rekey`] re-seals a snapshot under the
//!   new one (atomic swap). Honest limits: no plain tar or compressed archive is ever a file (they pass through a pipe from the walk to the
//!   sealer, and from the opener to the unpacker); a restore does unpack the files into the staging folder inside the snapshot, which is
//!   deleted, not wiped; file names are visible on the live volume anyway; whoever holds
//!   the tenant's vault key reads the snapshots.
//! * Not done: roles lists and audit are not part of a snapshot (restoring them over live ones would be a privilege decision); no
//!   incremental snapshots; a file that changes while it is read stops the snapshot (run again) rather than being stored half-written.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::cloudstorage;

pub const FORMAT: u32 = 1;
const ARCHIVE: &str = "data.tar.zst";
const MANIFEST: &str = "manifest.json";
const MANIFEST_SUM: &str = "manifest.sha256";
const ARCHIVE_ENC: &str = "data.tar.zst.enc";
pub const ENC_FORMAT: u32 = 2;
const MAGIC: &[u8; 8] = b"IYSNAP1\n";
const CHUNK: usize = 64 * 1024;
const TAG: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub path: String,
    /// `file` or `dir`.
    pub kind: String,
    pub size: u64,
    pub mode: u32,
    /// Hex SHA-256 of a file's contents (empty for a folder).
    #[serde(default)]
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub id: String,
    /// Seconds since the epoch.
    pub created: u64,
    pub archive: String,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub items: Vec<Item>,
    #[serde(default)]
    pub skipped: Vec<String>,
    /// Present when the archive is encrypted; `items` and `skipped` are then empty on disk (they are in [`Enc::sealed`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enc: Option<Enc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enc {
    /// The key's id (the start of its SHA-256), to find the right key.
    pub key_id: String,
    /// Random, 16 bytes, hex: part of every nonce and of the key derivation.
    pub salt: String,
    /// The sealed file list (hex).
    pub sealed: String,
}

impl Manifest {
    pub fn files(&self) -> usize {
        self.items.iter().filter(|i| i.kind == "file").count()
    }
    pub fn bytes(&self) -> u64 {
        self.items.iter().map(|i| i.size).sum()
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn sha_file(p: &Path) -> Result<String, String> {
    let mut f = BufReader::new(File::open(p).map_err(|e| format!("{}: {e}", p.display()))?);
    let (mut h, mut buf) = (Sha256::new(), vec![0u8; 64 * 1024]);
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(hex(&h.finalize()));
        }
        h.update(&buf[..n]);
    }
}

/// `20261103T041500Z` from seconds since the epoch (UTC), so ids sort by time.
pub fn id_for(now: u64) -> String {
    let (days, secs) = ((now / 86_400) as i64, now % 86_400);
    // civil from days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// An id this module made: `YYYYMMDDTHHMMSSZ`, optionally `-N`. Anything else is never a snapshot folder.
pub fn id_ok(s: &str) -> bool {
    let (main, n) = s.split_once('-').map_or((s, None), |(a, b)| (a, Some(b)));
    let b = main.as_bytes();
    b.len() == 16 && b[8] == b'T' && b[15] == b'Z' && b[..8].iter().chain(&b[9..15]).all(u8::is_ascii_digit) && n.is_none_or(|n| !n.is_empty() && n.len() <= 4 && n.bytes().all(|c| c.is_ascii_digit()))
}

// ---------------------------------------------------------------------------------------------------------------- keys and sealing

/// A snapshot key: 32 random bytes. Its id is the first 16 hex digits of its SHA-256, so an id names exactly one key.
pub struct SnapKey {
    pub id: String,
    key: [u8; 32],
}

impl std::fmt::Debug for SnapKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SnapKey({})", self.id)
    }
}

impl Drop for SnapKey {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

impl SnapKey {
    pub fn from_bytes(b: &[u8]) -> Result<SnapKey, String> {
        let key = <[u8; 32]>::try_from(b).map_err(|_| "a snapshot key is 32 bytes (64 hex digits)".to_string())?;
        Ok(SnapKey { id: hex(&Sha256::digest(key))[..16].to_string(), key })
    }

    pub fn from_hex(h: &str) -> Result<SnapKey, String> {
        SnapKey::from_bytes(&crate::cloudkeys::unhex(h.trim()).ok_or("a snapshot key is 64 hex digits")?)
    }

    pub fn generate() -> Result<SnapKey, String> {
        SnapKey::from_bytes(&crate::cloudkeys::random::<32>()?)
    }

    pub fn to_hex(&self) -> String {
        hex(&self.key)
    }

    fn cipher(&self, snapshot: &str, salt: &[u8; 16]) -> XChaCha20Poly1305 {
        let mut h = Sha256::new();
        h.update(b"iyakki-snapshot-v1\0");
        h.update(self.key);
        h.update(snapshot.as_bytes());
        h.update(salt);
        XChaCha20Poly1305::new(Key::from_slice(&h.finalize()))
    }
}

fn key_id_ok(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// Where the keys come from. `get` finds an old key by id (old snapshots), `current` is the one new snapshots use.
pub trait KeySource {
    fn current(&self) -> Result<SnapKey, String>;
    fn get(&self, id: &str) -> Result<SnapKey, String>;
}

/// Keys held in memory (from files); the first is the current one.
pub struct KeyRing(pub Vec<SnapKey>);

impl KeySource for KeyRing {
    fn current(&self) -> Result<SnapKey, String> {
        self.0.first().map(|k| SnapKey { id: k.id.clone(), key: k.key }).ok_or_else(|| "no snapshot key given".to_string())
    }
    fn get(&self, id: &str) -> Result<SnapKey, String> {
        self.0.iter().find(|k| k.id == id).map(|k| SnapKey { id: k.id.clone(), key: k.key }).ok_or_else(|| format!("no key {id} was given for this snapshot"))
    }
}

/// Keys kept in the tenant's vault scope (`tenant`: written by tenant admins, read by members that hold the scope key): the secret
/// `snapshot-key.<id>` holds the 32 raw bytes, `snapshot-key.current` the id of the key new snapshots use.
pub struct VaultKeyring<'a> {
    pub client: &'a crate::cloudvaultd::Client,
}

const CURRENT: &str = "snapshot-key.current";

impl VaultKeyring<'_> {
    fn scope() -> crate::cloudvaultd::Scope {
        crate::cloudvaultd::Scope::Tenant
    }

    /// Make a new key, store it, and make it the current one. The old keys stay (their snapshots still open, or are [`rekey`]ed).
    pub fn rotate(&self) -> Result<SnapKey, String> {
        let k = SnapKey::generate()?;
        self.client.put(&Self::scope(), &format!("snapshot-key.{}", k.id), &k.key).map_err(|e| format!("the vault refused the key: {e}"))?;
        self.client.put(&Self::scope(), CURRENT, k.id.as_bytes()).map_err(|e| format!("the vault refused the key: {e}"))?;
        Ok(k)
    }
}

impl KeySource for VaultKeyring<'_> {
    fn current(&self) -> Result<SnapKey, String> {
        let id = self.client.get(&Self::scope(), CURRENT).map_err(|e| format!("no current snapshot key in the vault ({e}); make one with: iyakki cloud snapshot key-rotate"))?;
        self.get(std::str::from_utf8(&id).map_err(|_| "the current key's id is damaged")?)
    }
    fn get(&self, id: &str) -> Result<SnapKey, String> {
        if !key_id_ok(id) {
            return Err("that is not a key id".into());
        }
        let b = self.client.get(&Self::scope(), &format!("snapshot-key.{id}")).map_err(|e| format!("key {id} is not in the vault ({e})"))?;
        let k = SnapKey::from_bytes(&b)?;
        if k.id != id {
            return Err(format!("the vault's key {id} does not match its id"));
        }
        Ok(k)
    }
}

fn nonce(salt: &[u8; 16], idx: u64, last: bool) -> XNonce {
    let mut n = [0u8; 24];
    n[..16].copy_from_slice(salt);
    n[16..].copy_from_slice(&(idx | (u64::from(last) << 63)).to_be_bytes());
    *XNonce::from_slice(&n)
}

/// What every sealed piece of snapshot `id` under key `key_id` is bound to.
fn aad(id: &str, key_id: &str, what: &str) -> Vec<u8> {
    format!("iyakki-snapshot-v1\n{id}\n{key_id}\n{what}").into_bytes()
}

const WRONG: &str = "does not open with this key (wrong key, or the snapshot was changed or cut short)";

/// Seals what is written to it in chunks; call [`SealWriter::finish`] to write the last one.
struct SealWriter<W: Write> {
    w: W,
    c: XChaCha20Poly1305,
    salt: [u8; 16],
    aad: Vec<u8>,
    buf: Vec<u8>,
    idx: u64,
}

impl<W: Write> SealWriter<W> {
    fn new(mut w: W, key: &SnapKey, id: &str, salt: [u8; 16]) -> std::io::Result<SealWriter<W>> {
        w.write_all(MAGIC)?;
        Ok(SealWriter { w, c: key.cipher(id, &salt), salt, aad: aad(id, &key.id, "archive"), buf: Vec::new(), idx: 0 })
    }

    fn seal(&mut self, data: &[u8], last: bool) -> std::io::Result<()> {
        let ct = self.c.encrypt(&nonce(&self.salt, self.idx, last), Payload { msg: data, aad: &self.aad }).map_err(|_| std::io::Error::other("sealing failed"))?;
        self.idx += 1;
        self.w.write_all(&ct)
    }

    fn finish(mut self) -> std::io::Result<W> {
        let rest = std::mem::take(&mut self.buf);
        self.seal(&rest, true)?;
        self.w.flush()?;
        Ok(self.w)
    }
}

impl<W: Write> Write for SealWriter<W> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(b);
        // at least one byte stays buffered, so the last chunk is never mistaken for a middle one
        while self.buf.len() > CHUNK {
            let rest = self.buf.split_off(CHUNK);
            let chunk = std::mem::replace(&mut self.buf, rest);
            self.seal(&chunk, false)?;
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn read_block<R: Read>(r: &mut R, n: usize) -> std::io::Result<Vec<u8>> {
    let mut v = vec![0u8; n];
    let mut at = 0;
    while at < n {
        let k = r.read(&mut v[at..])?;
        if k == 0 {
            break;
        }
        at += k;
    }
    v.truncate(at);
    Ok(v)
}

/// Opens a sealed archive into `out`; every chunk is authenticated, and the stream must end with the chunk marked as the last.
/// Returns the plain length.
fn open_stream<R: Read, W: Write>(mut r: R, mut out: W, key: &SnapKey, id: &str, salt: &[u8; 16]) -> Result<u64, String> {
    let io = |e: std::io::Error| e.to_string();
    if read_block(&mut r, MAGIC.len()).map_err(io)? != MAGIC {
        return Err("not an encrypted IYAKKI snapshot archive".into());
    }
    let (c, aad) = (key.cipher(id, salt), aad(id, &key.id, "archive"));
    let (mut idx, mut total) = (0u64, 0u64);
    let mut cur = read_block(&mut r, CHUNK + TAG).map_err(io)?;
    loop {
        let next = if cur.len() == CHUNK + TAG { read_block(&mut r, CHUNK + TAG).map_err(io)? } else { Vec::new() };
        let last = next.is_empty();
        if cur.len() < TAG {
            return Err(format!("the archive {WRONG}"));
        }
        let plain = c.decrypt(&nonce(salt, idx, last), Payload { msg: &cur, aad: &aad }).map_err(|_| format!("the archive {WRONG}"))?;
        out.write_all(&plain).map_err(io)?;
        total += plain.len() as u64;
        if last {
            return Ok(total);
        }
        idx += 1;
        cur = next;
    }
}

const MANIFEST_IDX: u64 = u64::MAX >> 1;

fn seal_list(key: &SnapKey, id: &str, salt: &[u8; 16], items: &[Item], skipped: &[String]) -> Result<String, String> {
    let text = serde_json::to_vec(&serde_json::json!({"items": items, "skipped": skipped})).map_err(|e| e.to_string())?;
    let ct = key.cipher(id, salt).encrypt(&nonce(salt, MANIFEST_IDX, true), Payload { msg: &text, aad: &aad(id, &key.id, "list") }).map_err(|_| "sealing failed".to_string())?;
    Ok(hex(&ct))
}

fn open_list(key: &SnapKey, id: &str, e: &Enc) -> Result<(Vec<Item>, Vec<String>), String> {
    let salt = salt_of(e)?;
    let ct = crate::cloudkeys::unhex(&e.sealed).ok_or("the sealed file list is damaged")?;
    let plain = key.cipher(id, &salt).decrypt(&nonce(&salt, MANIFEST_IDX, true), Payload { msg: &ct, aad: &aad(id, &key.id, "list") }).map_err(|_| format!("the file list {WRONG}"))?;
    #[derive(Deserialize)]
    struct L {
        items: Vec<Item>,
        skipped: Vec<String>,
    }
    let l: L = serde_json::from_slice(&plain).map_err(|e| e.to_string())?;
    Ok((l.items, l.skipped))
}

fn salt_of(e: &Enc) -> Result<[u8; 16], String> {
    crate::cloudkeys::unhex(&e.salt).and_then(|b| <[u8; 16]>::try_from(b).ok()).ok_or_else(|| "the salt is damaged".to_string())
}

// ---------------------------------------------------------------------------------------------------------------- tar

fn put(field: &mut [u8], v: u64) {
    let s = format!("{:0width$o}\0", v, width = field.len() - 1);
    field.copy_from_slice(s.as_bytes());
}

/// One ustar header; names up to 255 bytes are split into prefix and name at a `/`.
fn header(name: &str, kind: u8, mode: u32, size: u64) -> Result<[u8; 512], String> {
    let (prefix, base) = if name.len() <= 100 {
        ("", name)
    } else {
        let i = name.char_indices().filter(|(i, c)| *c == '/' && *i <= 155 && name.len() - i - 1 <= 100).map(|(i, _)| i).last().ok_or_else(|| format!("{name}: the path is too long for the archive"))?;
        (&name[..i], &name[i + 1..])
    };
    let mut h = [0u8; 512];
    h[..base.len()].copy_from_slice(base.as_bytes());
    put(&mut h[100..108], u64::from(mode & 0o7777));
    put(&mut h[108..116], 0);
    put(&mut h[116..124], 0);
    put(&mut h[124..136], size);
    put(&mut h[136..148], 0);
    h[148..156].copy_from_slice(b"        ");
    h[156] = kind;
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    let sum: u32 = h.iter().map(|b| u32::from(*b)).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    Ok(h)
}

struct Walk {
    tar: BufWriter<Box<dyn Write + Send>>,
    items: Vec<Item>,
    skipped: Vec<String>,
}

impl Walk {
    fn dir(&mut self, at: &Path, rel: &str) -> Result<(), String> {
        let mut names: Vec<_> = std::fs::read_dir(at).map_err(|e| format!("{}: {e}", at.display()))?.flatten().map(|e| e.file_name()).collect();
        names.sort();
        for n in names {
            let Some(name) = n.to_str() else {
                self.skipped.push(format!("{rel}/{} (name is not UTF-8)", n.to_string_lossy()));
                continue;
            };
            let (p, r) = (at.join(&n), if rel.is_empty() { name.to_string() } else { format!("{rel}/{name}") });
            let m = std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let mode = mode_of(&m);
            if m.is_dir() {
                self.tar.write_all(&header(&format!("{r}/"), b'5', mode, 0)?).map_err(|e| e.to_string())?;
                self.items.push(Item { path: r.clone(), kind: "dir".into(), size: 0, mode, sha256: String::new() });
                self.dir(&p, &r)?;
            } else if m.is_file() {
                self.file(&p, &r, mode)?;
            } else {
                self.skipped.push(format!("{r} ({})", if m.file_type().is_symlink() { "link" } else { "not a regular file" }));
            }
        }
        Ok(())
    }

    fn file(&mut self, p: &Path, rel: &str, mode: u32) -> Result<(), String> {
        let mut f = File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let size = f.metadata().map_err(|e| e.to_string())?.len();
        self.tar.write_all(&header(rel, b'0', mode, size)?).map_err(|e| e.to_string())?;
        let (mut h, mut left, mut buf) = (Sha256::new(), size, vec![0u8; 64 * 1024]);
        while left > 0 {
            let want = buf.len().min(left as usize);
            let n = f.read(&mut buf[..want]).map_err(|e| e.to_string())?;
            if n == 0 {
                return Err(format!("{rel} became shorter while it was read; run the snapshot again"));
            }
            h.update(&buf[..n]);
            self.tar.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            left -= n as u64;
        }
        self.tar.write_all(&vec![0u8; ((512 - size % 512) % 512) as usize]).map_err(|e| e.to_string())?;
        self.items.push(Item { path: rel.to_string(), kind: "file".into(), size, mode, sha256: hex(&h.finalize()) });
        Ok(())
    }
}

#[cfg(unix)]
fn mode_of(m: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o777
}
#[cfg(not(unix))]
fn mode_of(_: &std::fs::Metadata) -> u32 {
    0o600
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode & 0o777));
}
#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) {}

// ---------------------------------------------------------------------------------------------------------------- create

fn nested(a: &Path, b: &Path) -> bool {
    let (a, b) = (std::fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf()), std::fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf()));
    a.starts_with(&b) || b.starts_with(&a)
}

/// Snapshot `source` (the tenant's files folder; a missing one is an empty tenant) into a new folder under `snaps`.
pub fn create(source: &Path, snaps: &Path, now: u64) -> Result<Manifest, String> {
    create_with(source, snaps, now, None)
}

/// [`create`], encrypting the archive and the file list with `key` when one is given.
pub fn create_with(source: &Path, snaps: &Path, now: u64, key: Option<&SnapKey>) -> Result<Manifest, String> {
    if snaps.exists() && source.exists() && nested(source, snaps) {
        return Err("the snapshot folder and the files folder must not be inside each other".into());
    }
    std::fs::create_dir_all(snaps).map_err(|e| format!("{}: {e}", snaps.display()))?;
    set_mode(snaps, 0o700);
    if source.exists() && nested(source, snaps) {
        return Err("the snapshot folder and the files folder must not be inside each other".into());
    }
    let mut id = id_for(now);
    let mut n = 1;
    while snaps.join(&id).exists() {
        id = format!("{}-{n}", id_for(now));
        n += 1;
    }
    let tmp = snaps.join(format!(".making-{id}"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let r = build(source, &tmp, &id, now, key);
    match r {
        Ok(m) => {
            std::fs::rename(&tmp, snaps.join(&id)).map_err(|e| {
                let _ = std::fs::remove_dir_all(&tmp);
                e.to_string()
            })?;
            Ok(m)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Err(e)
        }
    }
}

fn build(source: &Path, tmp: &Path, id: &str, now: u64, key: Option<&SnapKey>) -> Result<Manifest, String> {
    let arch = tmp.join(if key.is_some() { ARCHIVE_ENC } else { ARCHIVE });
    let salt = crate::cloudkeys::random::<16>()?;
    // The tar is never a file: the walk writes into a pipe, the compressor (and, with a key, the sealer) reads it on another thread and
    // writes the archive, so nothing readable beyond the live files exists on disk while a snapshot is made.
    let (rd, wr) = std::io::pipe().map_err(|e| e.to_string())?;
    let (walked, packed) = std::thread::scope(|sc| {
        let h = sc.spawn(|| -> Result<(), String> {
            let src = BufReader::new(rd);
            let mut out = BufWriter::new(File::create(&arch).map_err(|e| e.to_string())?);
            match key {
                None => ruzstd::encoding::compress(src, &mut out, ruzstd::encoding::CompressionLevel::Fastest),
                Some(k) => {
                    let mut sw = SealWriter::new(&mut out, k, id, salt).map_err(|e| e.to_string())?;
                    ruzstd::encoding::compress(src, &mut sw, ruzstd::encoding::CompressionLevel::Fastest);
                    sw.finish().map_err(|e| e.to_string())?;
                }
            }
            out.flush().map_err(|e| e.to_string())
        });
        let mut w = Walk { tar: BufWriter::new(Box::new(wr)), items: vec![], skipped: vec![] };
        let walked = (|| -> Result<(), String> {
            if source.is_dir() {
                w.dir(source, "")?;
            }
            w.tar.write_all(&[0u8; 1024]).map_err(|e| e.to_string())?;
            w.tar.flush().map_err(|e| e.to_string())
        })();
        let Walk { items, skipped, tar } = w;
        drop(tar); // the end of the pipe: the compressor finishes
        let packed = h.join().unwrap_or_else(|_| Err("the archive writer stopped unexpectedly".into()));
        (walked.map(|_| (items, skipped)), packed)
    });
    // a failed archive writer breaks the pipe under the walk: its own reason is the useful one
    packed?;
    let (items, skipped) = walked?;
    let full = Manifest {
        format: if key.is_some() { ENC_FORMAT } else { FORMAT },
        id: id.into(),
        created: now,
        archive: if key.is_some() { ARCHIVE_ENC } else { ARCHIVE }.into(),
        archive_sha256: sha_file(&arch)?,
        archive_bytes: std::fs::metadata(&arch).map_err(|e| e.to_string())?.len(),
        items,
        skipped,
        enc: None,
    };
    write_manifest(tmp, &full, key.map(|k| (k, salt)))?;
    set_mode(tmp, 0o700);
    Ok(Manifest { enc: key.map(|k| Enc { key_id: k.id.clone(), salt: hex(&salt), sealed: String::new() }), ..full })
}

/// Writes `manifest.json` and its checksum. With a key, the file list is sealed and left out of the plain manifest.
fn write_manifest(tmp: &Path, full: &Manifest, key: Option<(&SnapKey, [u8; 16])>) -> Result<(), String> {
    let m = match key {
        None => full.clone(),
        Some((k, salt)) => Manifest { items: vec![], skipped: vec![], enc: Some(Enc { key_id: k.id.clone(), salt: hex(&salt), sealed: seal_list(k, &full.id, &salt, &full.items, &full.skipped)? }), ..full.clone() },
    };
    let text = serde_json::to_string_pretty(&m).map_err(|e| e.to_string())?;
    std::fs::write(tmp.join(MANIFEST), &text).map_err(|e| e.to_string())?;
    std::fs::write(tmp.join(MANIFEST_SUM), format!("{}  {MANIFEST}\n", hex(&Sha256::digest(text.as_bytes())))).map_err(|e| e.to_string())?;
    for f in [m.archive.as_str(), MANIFEST, MANIFEST_SUM] {
        set_mode(&tmp.join(f), 0o600);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------- list, verify

/// The manifest of snapshot `id`, after checking the manifest's own checksum and its names.
pub fn read_manifest(snaps: &Path, id: &str) -> Result<Manifest, String> {
    if !id_ok(id) {
        return Err(format!("\"{id}\" is not a snapshot id"));
    }
    let dir = snaps.join(id);
    let text = std::fs::read_to_string(dir.join(MANIFEST)).map_err(|e| format!("{id}: {e}"))?;
    let sum = std::fs::read_to_string(dir.join(MANIFEST_SUM)).map_err(|e| format!("{id}: {e}"))?;
    if sum.split_whitespace().next() != Some(hex(&Sha256::digest(text.as_bytes())).as_str()) {
        return Err(format!("{id}: the manifest does not match its checksum"));
    }
    let m: Manifest = serde_json::from_str(&text).map_err(|e| format!("{id}: {e}"))?;
    let shape_ok = match &m.enc {
        None => m.format == FORMAT && m.archive == ARCHIVE,
        Some(e) => m.format == ENC_FORMAT && m.archive == ARCHIVE_ENC && key_id_ok(&e.key_id) && salt_of(e).is_ok() && m.items.is_empty() && m.skipped.is_empty(),
    };
    if !shape_ok || m.id != id {
        return Err(format!("{id}: a manifest this version does not understand"));
    }
    check_items(id, &m.items)?;
    Ok(m)
}

fn check_items(id: &str, items: &[Item]) -> Result<(), String> {
    for i in items {
        let sane = !i.path.is_empty() && Path::new(&i.path).components().all(|c| matches!(c, std::path::Component::Normal(_)));
        let kind_ok = matches!(i.kind.as_str(), "file" | "dir");
        let sum_ok = i.kind == "dir" || (i.sha256.len() == 64 && i.sha256.bytes().all(|c| c.is_ascii_hexdigit()));
        if !sane || !kind_ok || !sum_ok {
            return Err(format!("{id}: the manifest names a path or checksum that is not valid ({})", i.path.chars().take(60).collect::<String>()));
        }
    }
    Ok(())
}

/// The manifest with the file list opened (an encrypted snapshot needs its key; a plain one needs none).
pub fn open_manifest(snaps: &Path, id: &str, keys: Option<&dyn KeySource>) -> Result<Manifest, String> {
    let m = read_manifest(snaps, id)?;
    let Some(e) = &m.enc else { return Ok(m) };
    let key = keys.ok_or_else(|| format!("{id}: the snapshot is encrypted (key {}): give its key", e.key_id))?.get(&e.key_id)?;
    let (items, skipped) = open_list(&key, id, e).map_err(|e| format!("{id}: {e}"))?;
    check_items(id, &items)?;
    Ok(Manifest { items, skipped, ..m })
}

/// The manifest, and the archive checked against its checksum and size.
pub fn verify(snaps: &Path, id: &str) -> Result<Manifest, String> {
    let m = read_manifest(snaps, id)?;
    let arch = snaps.join(id).join(&m.archive);
    let len = std::fs::metadata(&arch).map_err(|e| format!("{id}: {e}"))?.len();
    if len != m.archive_bytes || sha_file(&arch)? != m.archive_sha256 {
        return Err(format!("{id}: the archive does not match the manifest (damaged or changed)"));
    }
    Ok(m)
}

/// [`verify`] and, for an encrypted snapshot, every chunk of the archive authenticated and the file list opened with the key.
pub fn verify_with(snaps: &Path, id: &str, keys: Option<&dyn KeySource>) -> Result<Manifest, String> {
    let m = verify(snaps, id)?;
    let Some(e) = &m.enc else { return Ok(m) };
    let m = open_manifest(snaps, id, keys)?;
    let key = keys.ok_or("give the key")?.get(&e.key_id)?;
    let f = BufReader::new(File::open(snaps.join(id).join(&m.archive)).map_err(|e| e.to_string())?);
    open_stream(f, std::io::sink(), &key, id, &salt_of(e)?).map_err(|e| format!("{id}: {e}"))?;
    Ok(m)
}

/// Every snapshot folder, oldest first; one that cannot be read is listed with the reason.
pub fn list(snaps: &Path) -> Vec<(String, Result<Manifest, String>)> {
    let mut ids: Vec<String> = std::fs::read_dir(snaps).into_iter().flatten().flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).filter(|n| id_ok(n)).collect();
    ids.sort();
    ids.into_iter().map(|id| (id.clone(), read_manifest(snaps, &id))).collect()
}

// ---------------------------------------------------------------------------------------------------------------- restore

#[derive(Debug, PartialEq, Eq)]
pub struct Restored {
    pub files: usize,
    pub dirs: usize,
    pub bytes: u64,
}

/// Restore snapshot `id` into `target`. Without `overwrite`, any file that already exists there stops the restore before anything is
/// written. Staging is inside the snapshot's folder and is always removed.
pub fn restore(snaps: &Path, id: &str, target: &Path, overwrite: bool) -> Result<Restored, String> {
    restore_with(snaps, id, target, overwrite, None)
}

/// [`restore`] for a snapshot that may be encrypted: the whole archive is authenticated with the key before anything is unpacked or
/// written, so a changed archive or the wrong key writes nothing.
pub fn restore_with(snaps: &Path, id: &str, target: &Path, overwrite: bool, keys: Option<&dyn KeySource>) -> Result<Restored, String> {
    let m = verify_with(snaps, id, keys)?;
    std::fs::create_dir_all(target).map_err(|e| format!("{}: {e}", target.display()))?;
    // 1. conflicts, before anything is written (and a link on the way is a refusal)
    let mut conflicts = Vec::new();
    for i in &m.items {
        let p = cloudstorage::jail(target, &i.path)?;
        match std::fs::symlink_metadata(&p) {
            Ok(e) if i.kind == "file" || !e.is_dir() => conflicts.push(i.path.clone()),
            _ => {}
        }
    }
    if !conflicts.is_empty() && !overwrite {
        let shown: Vec<_> = conflicts.iter().take(5).cloned().collect();
        return Err(format!("{} file(s) already exist in {} (for example {}); nothing was written. Restore into a new folder, or give --overwrite", conflicts.len(), target.display(), shown.join(", ")));
    }
    // 2. unpack into staging and check every file against the manifest
    let stage = snaps.join(id).join(format!(".restore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    let r = (|| -> Result<Restored, String> {
        match &m.enc {
            None => {
                crate::archive::untar_zst(&snaps.join(id).join(&m.archive), &stage, m.bytes() + (1 << 20))?;
            }
            Some(e) => {
                // opened straight into the unpacker through a pipe: the plain archive is never a file
                let key = keys.ok_or("give the key")?.get(&e.key_id)?;
                let salt = salt_of(e)?;
                let (rd, wr) = std::io::pipe().map_err(|e| e.to_string())?;
                let max = m.bytes() + (1 << 20);
                let archive = snaps.join(id).join(&m.archive);
                let (unpacked, opened) = std::thread::scope(|sc| {
                    let h = sc.spawn(|| -> Result<(), String> {
                        let src = BufReader::new(File::open(&archive).map_err(|e| e.to_string())?);
                        let mut out = BufWriter::new(wr);
                        open_stream(src, &mut out, &key, id, &salt)?;
                        out.flush().map_err(|e| e.to_string())
                    });
                    let mut br = BufReader::new(rd);
                    let unpacked = ruzstd::decoding::StreamingDecoder::new(&mut br).map_err(|e| format!("not a zstd file ({e})")).and_then(|dec| crate::archive::untar_stream(dec, &stage, max));
                    if unpacked.is_ok() {
                        // the rest of the stream must still be read to its authenticated end
                        let _ = std::io::copy(&mut br, &mut std::io::sink());
                    }
                    drop(br);
                    (unpacked, h.join().unwrap_or_else(|_| Err("the archive reader stopped unexpectedly".into())))
                });
                unpacked?;
                opened?;
            }
        }
        for i in m.items.iter().filter(|i| i.kind == "file") {
            let p = stage.join(&i.path);
            let md = std::fs::symlink_metadata(&p).map_err(|_| format!("{}: missing from the archive", i.path))?;
            if !md.is_file() || md.len() != i.size || sha_file(&p)? != i.sha256 {
                return Err(format!("{}: does not match the manifest; nothing was restored", i.path));
            }
        }
        // 3. copy, folder by folder, file by file (a copy beside the target, then a rename)
        let mut out = Restored { files: 0, dirs: 0, bytes: 0 };
        for i in &m.items {
            let dest = cloudstorage::jail(target, &i.path)?;
            if i.kind == "dir" {
                std::fs::create_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
                set_mode(&dest, i.mode | 0o700);
                out.dirs += 1;
            } else {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
                }
                let tmp = dest.with_file_name(format!(".iyakki-restore-{}", std::process::id()));
                std::fs::copy(stage.join(&i.path), &tmp).map_err(|e| format!("{}: {e}", i.path))?;
                set_mode(&tmp, i.mode | 0o600);
                std::fs::rename(&tmp, &dest).map_err(|e| {
                    let _ = std::fs::remove_file(&tmp);
                    format!("{}: {e}", i.path)
                })?;
                out.files += 1;
                out.bytes += i.size;
            }
        }
        Ok(out)
    })();
    let _ = std::fs::remove_dir_all(&stage);
    r
}

// ---------------------------------------------------------------------------------------------------------------- prune

/// Keep the newest `keep` snapshots (at least 1), remove the rest. Returns the ids removed (or that would be, with `dry_run`). Only
/// folders whose names are snapshot ids are touched, and only after they were read as snapshots (an unreadable one is kept and left
/// for a person to look at).
pub fn prune(snaps: &Path, keep: usize, dry_run: bool) -> Result<Vec<String>, String> {
    if keep == 0 {
        return Err("--keep must be at least 1".into());
    }
    let all: Vec<String> = list(snaps).into_iter().filter(|(_, m)| m.is_ok()).map(|(id, _)| id).collect();
    let drop = all.len().saturating_sub(keep);
    let gone: Vec<String> = all.into_iter().take(drop).collect();
    if !dry_run {
        for id in &gone {
            std::fs::remove_dir_all(snaps.join(id)).map_err(|e| format!("{id}: {e}"))?;
        }
    }
    Ok(gone)
}

// ---------------------------------------------------------------------------------------------------------------- key rotation

/// Seal snapshot `id` again under `new` (it may be plain, or sealed under any key `old` can give). The old archive is authenticated
/// first; the new one is made beside it and swapped in with renames, so a failure leaves the old snapshot as it was.
pub fn rekey(snaps: &Path, id: &str, old: Option<&dyn KeySource>, new: &SnapKey) -> Result<Manifest, String> {
    let m = verify_with(snaps, id, old)?;
    if m.enc.as_ref().is_some_and(|e| e.key_id == new.id) {
        return Ok(m);
    }
    let (dir, tmp) = (snaps.join(id), snaps.join(format!(".making-{id}-rekey")));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let r = (|| -> Result<Manifest, String> {
        let salt = crate::cloudkeys::random::<16>()?;
        let arch = tmp.join(ARCHIVE_ENC);
        {
            let src = BufReader::new(File::open(dir.join(&m.archive)).map_err(|e| e.to_string())?);
            let mut out = BufWriter::new(File::create(&arch).map_err(|e| e.to_string())?);
            let mut sw = SealWriter::new(&mut out, new, id, salt).map_err(|e| e.to_string())?;
            match &m.enc {
                None => {
                    std::io::copy(&mut BufReader::new(src), &mut sw).map_err(|e| e.to_string())?;
                }
                Some(e) => {
                    let oldk = old.ok_or("give the old key")?.get(&e.key_id)?;
                    open_stream(src, &mut sw, &oldk, id, &salt_of(e)?)?;
                }
            }
            sw.finish().map_err(|e| e.to_string())?;
            out.flush().map_err(|e| e.to_string())?;
        }
        let full = Manifest { format: ENC_FORMAT, archive: ARCHIVE_ENC.into(), archive_sha256: sha_file(&arch)?, archive_bytes: std::fs::metadata(&arch).map_err(|e| e.to_string())?.len(), enc: None, ..m.clone() };
        write_manifest(&tmp, &full, Some((new, salt)))?;
        set_mode(&tmp, 0o700);
        Ok(Manifest { enc: Some(Enc { key_id: new.id.clone(), salt: hex(&salt), sealed: String::new() }), ..full })
    })();
    match r {
        Ok(done) => {
            let aside = snaps.join(format!(".old-{id}"));
            let _ = std::fs::remove_dir_all(&aside);
            std::fs::rename(&dir, &aside).map_err(|e| e.to_string())?;
            if let Err(e) = std::fs::rename(&tmp, &dir) {
                let _ = std::fs::rename(&aside, &dir);
                let _ = std::fs::remove_dir_all(&tmp);
                return Err(e.to_string());
            }
            let _ = std::fs::remove_dir_all(&aside);
            Ok(done)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------- Kubernetes

/// A CronJob that runs the snapshot (and the prune) in the tenant's namespace. The pod mounts the tenant's data claim like the service
/// does (the claim is ReadWriteOnce, so the pod is placed on the node that runs the service), and, when `backup_claim` is given, a second
/// claim for the snapshots, so they leave the volume they protect.
pub struct CronSpec<'a> {
    pub tenant: &'a str,
    /// Five fields of digits, `*`, `/`, `,` and `-`.
    pub schedule: &'a str,
    pub image: &'a str,
    pub keep: u32,
    pub backup_claim: Option<&'a str>,
}

pub fn cronjob(s: &CronSpec) -> Result<String, String> {
    if !crate::cloudtenant::dns_label(s.tenant) || s.tenant.len() > 40 {
        return Err(format!("tenant \"{}\": use lower-case letters, digits and -", s.tenant));
    }
    let fields: Vec<&str> = s.schedule.split(' ').collect();
    if fields.len() != 5 || fields.iter().any(|f| f.is_empty() || !f.bytes().all(|c| c.is_ascii_digit() || matches!(c, b'*' | b'/' | b',' | b'-'))) {
        return Err("the schedule is five fields such as \"17 3 * * *\"".into());
    }
    if s.image.is_empty() || s.image.len() > 200 || s.image.starts_with(['-', '/', ':']) || !s.image.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-' | b'/' | b':' | b'@')) {
        return Err("that is not an image name".into());
    }
    if s.keep == 0 || s.keep > 1000 {
        return Err("keep is between 1 and 1000".into());
    }
    if let Some(c) = s.backup_claim {
        if !crate::cloudtenant::dns_subdomain(c) {
            return Err(format!("backup claim \"{c}\" is not a claim name"));
        }
    }
    let (t, ns) = (s.tenant, format!("iyakki-{}", s.tenant));
    let dir_arg = if s.backup_claim.is_some() { ", \"--dir\", \"/backup\"" } else { "" };
    let (backup_mount, backup_vol) = match s.backup_claim {
        Some(c) => ("            - { name: backup, mountPath: /backup }\n".to_string(), format!("            - name: backup\n              persistentVolumeClaim: {{ claimName: {c} }}\n")),
        None => (String::new(), String::new()),
    };
    Ok(format!(
        "apiVersion: batch/v1\nkind: CronJob\nmetadata:\n  name: iyakki-snapshot\n  namespace: {ns}\n  labels:\n    app.kubernetes.io/name: iyakki-snapshot\n    iyakki.dev/tenant: {t}\nspec:\n  schedule: \"{sched}\"\n  concurrencyPolicy: Forbid\n  successfulJobsHistoryLimit: 1\n  failedJobsHistoryLimit: 3\n  jobTemplate:\n    spec:\n      backoffLimit: 1\n      template:\n        metadata:\n          labels:\n            app.kubernetes.io/name: iyakki-snapshot\n            iyakki.dev/tenant: {t}\n        spec:\n          restartPolicy: Never\n          automountServiceAccountToken: false\n          securityContext:\n            runAsNonRoot: true\n            runAsUser: 65532\n            runAsGroup: 65532\n            fsGroup: 65532\n            seccompProfile:\n              type: RuntimeDefault\n          affinity:\n            podAffinity:\n              requiredDuringSchedulingIgnoredDuringExecution:\n                - topologyKey: kubernetes.io/hostname\n                  labelSelector:\n                    matchLabels:\n                      app.kubernetes.io/name: iyakki\n                      app.kubernetes.io/instance: {t}\n          containers:\n            - name: snapshot\n              image: {img}\n              imagePullPolicy: IfNotPresent\n              args: [\"cloud\", \"snapshot\", \"create\", \"--data\", \"/var/lib/iyakki\"{dir_arg}, \"--keep\", \"{keep}\"]\n              securityContext:\n                allowPrivilegeEscalation: false\n                readOnlyRootFilesystem: true\n                capabilities:\n                  drop: [\"ALL\"]\n              resources:\n                requests: {{ cpu: 50m, memory: 64Mi }}\n                limits: {{ cpu: \"1\", memory: 256Mi }}\n              volumeMounts:\n                - {{ name: data, mountPath: /var/lib/iyakki }}\n{backup_mount}                - {{ name: tmp, mountPath: /tmp }}\n          volumes:\n            - name: data\n              persistentVolumeClaim: {{ claimName: iyakki-data }}\n{backup_vol}            - name: tmp\n              emptyDir: {{}}\n",
        sched = s.schedule,
        img = s.image,
        keep = s.keep,
    ))
}

// ---------------------------------------------------------------------------------------------------------------- command

fn opt(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn mb(b: u64) -> String {
    format!("{:.1} MB", b as f64 / 1e6)
}

/// The keys the command line names: `--key-file F` (the first is the current key; more may follow), `--old-key-file F`, or the tenant's vault
/// with `--vault-key FILE --as USER` (and `--vault URL --vault-fingerprint FP --tenant T`, defaulting to the `IYAKKI_VAULT_*` variables).
struct CliKeys {
    ring: Option<KeyRing>,
    vault: Option<crate::cloudvaultd::Client>,
}

impl KeySource for CliKeys {
    fn current(&self) -> Result<SnapKey, String> {
        match (&self.ring, &self.vault) {
            (Some(r), _) if !r.0.is_empty() => r.current(),
            (_, Some(c)) => VaultKeyring { client: c }.current(),
            _ => Err("no snapshot key given (--key-file FILE, or --vault-key FILE --as USER)".into()),
        }
    }
    fn get(&self, id: &str) -> Result<SnapKey, String> {
        if let Some(Ok(k)) = self.ring.as_ref().map(|r| r.get(id)) {
            return Ok(k);
        }
        match &self.vault {
            Some(c) => VaultKeyring { client: c }.get(id),
            None => Err(format!("no key {id} was given for this snapshot")),
        }
    }
}

fn cli_keys(a: &[String]) -> Result<Option<CliKeys>, String> {
    let all = |name: &str| -> Vec<String> { a.iter().enumerate().filter(|(_, x)| *x == name).filter_map(|(i, _)| a.get(i + 1).cloned()).collect() };
    let read = |f: &String| -> Result<SnapKey, String> { SnapKey::from_hex(&std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?) };
    let mut ring = Vec::new();
    for f in all("--key-file").iter().chain(all("--old-key-file").iter()) {
        ring.push(read(f)?);
    }
    let vault = match opt(a, "--vault-key") {
        None => None,
        Some(kf) => {
            let user = opt(a, "--as").ok_or("--vault-key needs --as USER")?;
            let env = |k: &str| std::env::var(k).ok();
            let link = crate::cloudvaultlink::Link::from_values(
                opt(a, "--vault").or_else(|| env(crate::cloudvaultlink::ENV_URL)).as_deref(),
                opt(a, "--vault-fingerprint").or_else(|| env(crate::cloudvaultlink::ENV_FINGERPRINT)).as_deref(),
                opt(a, "--tenant").or_else(|| env(crate::cloudvaultlink::ENV_TENANT)).as_deref(),
            );
            let keys = crate::cloudkeys::MemberKeys::from_hex(&std::fs::read_to_string(&kf).map_err(|e| format!("{kf}: {e}"))?)?;
            match link {
                crate::cloudvaultlink::Link::Target(t) => Some(t.client(&user, keys)?),
                crate::cloudvaultlink::Link::NotConfigured => return Err("no vault address (--vault URL or IYAKKI_VAULT_URL)".into()),
                crate::cloudvaultlink::Link::Invalid(e) => return Err(format!("the vault address is not usable: {e}")),
            }
        }
    };
    Ok((!ring.is_empty() || vault.is_some()).then_some(CliKeys { ring: (!ring.is_empty()).then_some(KeyRing(ring)), vault }))
}

/// `iyakki cloud snapshot create | list | verify | restore | rekey | key-new | key-rotate | prune | cronjob` (the arguments after `snapshot`).
pub fn cli(a: &[String], out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let data = PathBuf::from(opt(a, "--data").unwrap_or_else(|| "/var/lib/iyakki".into()));
    let files = data.join("cloud").join("files");
    let snaps = opt(a, "--dir").map(PathBuf::from).unwrap_or_else(|| data.join("cloud").join("snapshots"));
    let keys = cli_keys(a)?;
    let ks = || keys.as_ref().map(|k| k as &dyn KeySource);
    let id = || a.get(1).filter(|x| !x.starts_with("--")).cloned().ok_or_else(|| "give the snapshot id (see: iyakki cloud snapshot list)".to_string());
    match a.first().map(String::as_str) {
        Some("create") => {
            let cur = keys.as_ref().map(|k| k.current()).transpose()?;
            let m = create_with(&files, &snaps, now(), cur.as_ref())?;
            if let Some(k) = &cur {
                out(&format!("  encrypted with key {}", k.id));
            }
            out(&format!("snapshot {}: {} files, {} of data, {} archived, in {}", m.id, m.files(), mb(m.bytes()), mb(m.archive_bytes), snaps.join(&m.id).display()));
            for s in &m.skipped {
                out(&format!("  not archived: {s}"));
            }
            if let Some(k) = opt(a, "--keep") {
                let keep: usize = k.parse().map_err(|_| "--keep takes a number".to_string())?;
                for g in prune(&snaps, keep, false)? {
                    out(&format!("  pruned {g}"));
                }
            }
            Ok(())
        }
        Some("list") => {
            let l = list(&snaps);
            if l.is_empty() {
                out(&format!("no snapshots in {}", snaps.display()));
            }
            for (id, m) in l {
                match m {
                    Ok(m) if m.enc.is_some() => out(&format!("{id}  ENCRYPTED (key {})  archive {}", m.enc.as_ref().map(|e| e.key_id.as_str()).unwrap_or(""), mb(m.archive_bytes))),
                    Ok(m) => out(&format!("{id}  {} files  {}  archive {}", m.files(), mb(m.bytes()), mb(m.archive_bytes))),
                    Err(e) => out(&format!("{id}  UNREADABLE: {e}")),
                }
            }
            Ok(())
        }
        Some("verify") => {
            let m = verify_with(&snaps, &id()?, ks())?;
            out(&format!("snapshot {} is intact ({} files, archive checksum {})", m.id, m.files(), &m.archive_sha256[..12]));
            Ok(())
        }
        Some("restore") => {
            let id = id()?;
            let target = match (opt(a, "--to"), a.iter().any(|x| x == "--live")) {
                (Some(_), true) => return Err("give --to or --live, not both".into()),
                (Some(t), false) => PathBuf::from(t),
                (None, true) => files.clone(),
                (None, false) => {
                    let t = data.join("cloud").join("restored").join(&id);
                    if t.exists() {
                        return Err(format!("{} exists already; choose another folder with --to", t.display()));
                    }
                    t
                }
            };
            let r = restore_with(&snaps, &id, &target, a.iter().any(|x| x == "--overwrite"), ks())?;
            out(&format!("restored {} files and {} folders ({}) into {}", r.files, r.dirs, mb(r.bytes), target.display()));
            Ok(())
        }
        Some("rekey") => {
            let new = keys.as_ref().ok_or("usage: iyakki cloud snapshot rekey ID --key-file NEW [--old-key-file OLD] (or --vault-key FILE --as USER)")?.current()?;
            let m = rekey(&snaps, &id()?, ks(), &new)?;
            out(&format!("snapshot {} is now sealed with key {}", m.id, new.id));
            Ok(())
        }
        Some("key-new") => {
            use std::os::unix::fs::OpenOptionsExt;
            let f = opt(a, "--out").ok_or("usage: iyakki cloud snapshot key-new --out FILE")?;
            let k = SnapKey::generate()?;
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&f).map_err(|e| format!("{f}: {e}"))?;
            file.write_all(format!("{}\n", k.to_hex()).as_bytes()).map_err(|e| e.to_string())?;
            out(&format!("key {} written to {f} (keep a copy elsewhere: without it the snapshots cannot be opened)", k.id));
            Ok(())
        }
        Some("key-rotate") => {
            let c = keys.as_ref().and_then(|k| k.vault.as_ref()).ok_or("usage: iyakki cloud snapshot key-rotate --vault-key FILE --as USER")?;
            let k = VaultKeyring { client: c }.rotate()?;
            out(&format!("the current snapshot key is now {} (older keys stay in the vault; run rekey on old snapshots, then prune)", k.id));
            Ok(())
        }
        Some("prune") => {
            let keep: usize = opt(a, "--keep").ok_or("usage: iyakki cloud snapshot prune --keep N [--dry-run]")?.parse().map_err(|_| "--keep takes a number".to_string())?;
            let dry = a.iter().any(|x| x == "--dry-run");
            let gone = prune(&snaps, keep, dry)?;
            out(&format!("{} {} snapshot(s)", if dry { "would remove" } else { "removed" }, gone.len()));
            for g in gone {
                out(&format!("  {g}"));
            }
            Ok(())
        }
        Some("cronjob") => {
            let tenant = opt(a, "--tenant").ok_or("usage: iyakki cloud snapshot cronjob --tenant T [--schedule \"17 3 * * *\"] [--keep 7] [--image REF] [--backup-claim NAME]")?;
            let keep = opt(a, "--keep").map(|k| k.parse::<u32>().map_err(|_| "--keep takes a number".to_string())).transpose()?.unwrap_or(7);
            let backup = opt(a, "--backup-claim");
            let image = opt(a, "--image").unwrap_or_else(|| "iyakki:latest".into());
            let schedule = opt(a, "--schedule").unwrap_or_else(|| "17 3 * * *".into());
            out(&cronjob(&CronSpec { tenant: &tenant, schedule: &schedule, image: &image, keep, backup_claim: backup.as_deref() })?);
            Ok(())
        }
        _ => Err("usage: iyakki cloud snapshot <create | list | verify ID | restore ID | prune | cronjob>  [--data DIR (default /var/lib/iyakki)] [--dir SNAPSHOT_FOLDER]\n  \
                  create [--keep N]   restore ID [--to DIR | --live] [--overwrite]   prune --keep N [--dry-run]   cronjob --tenant T [--schedule S] [--keep N] [--image REF] [--backup-claim NAME]"
            .into()),
    }
}

#[cfg(test)]
#[path = "cloudsnapshot_tests.rs"]
mod tests;
