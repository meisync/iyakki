//! `files` scope: copy files to and from the device, list, make folders and delete, inside a SANDBOX: the account's home, its
//! workspaces and `/tmp/iyakki-dev` (read and write); with `files:system` also `/etc`, `/lib/modules` and IYAKKI's data folder
//! (read only). Secrets (the vault, keys, password files, the paired-computer list) are never readable, whatever the scope.
//!
//! Every path is resolved with symlinks followed before it is compared with the roots, links are never written through, sizes
//! are bounded, and transfers go in chunks that can be resumed and are checked with SHA-256 at the end.

use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use iyakki_sec::policy::Policy;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Where a computer may always put things.
pub const DEV_TMP: &str = "/tmp/iyakki-dev";
/// The biggest file one push may make.
pub const MAX_FILE: u64 = 2 << 30;
/// The biggest chunk of one request (the listener's body limit is 12 MiB).
pub const MAX_CHUNK: usize = 8 << 20;
pub const MAX_LIST: usize = 2000;
const PART: &str = ".iyakki-part";

pub struct Sandbox {
    pub home: PathBuf,
    /// Read and write.
    pub rw: Vec<PathBuf>,
    /// Read only (`files:system`).
    pub ro: Vec<PathBuf>,
    /// Never readable (the data folder's secret places), real paths.
    pub deny: Vec<PathBuf>,
}

const SECRET_NAMES: [&str; 18] = ["shadow", "gshadow", "shadow-", "users.json", "keys.json", "pairing.json", "shell.json", "wifi.json", "master.key", "vault.key", "id_rsa", "id_ed25519", "id_ecdsa", "id_dsa", ".netrc", ".git-credentials", "token", "tokens.json"];

/// Is this name a secret wherever it is?
pub fn secret_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    SECRET_NAMES.contains(&n.as_str())
        || n.ends_with(".key")
        || n.ends_with("_key")
        || n.ends_with(".pem")
        || n.ends_with(".p12")
        || n.ends_with(".pfx")
        || n.ends_with(".psk")
        || n.ends_with(".afv")
        || n.starts_with("vault")
        || n.starts_with("wpa_supplicant")
        || n.ends_with(".kdbx")
}

impl Sandbox {
    /// The sandbox of an account: home, workspaces and the scratch folder; with `system` also the read-only places.
    pub fn for_account(policy: &Policy, account: &str, data_dir: &Path, system: bool, home: Option<&Path>) -> Sandbox {
        let roots = match home {
            Some(h) => crate::scope::FileRoots::with_home(policy, account, data_dir, std::fs::canonicalize(h).unwrap_or_else(|_| h.to_path_buf())),
            None => crate::scope::FileRoots::new(policy, account, data_dir),
        };
        let tmp = PathBuf::from(DEV_TMP);
        let _ = std::fs::create_dir_all(&tmp);
        let mut rw = roots.roots.clone();
        if let Ok(t) = std::fs::canonicalize(&tmp) {
            rw.push(t);
        }
        let mut ro = Vec::new();
        if system {
            for p in [PathBuf::from("/etc"), PathBuf::from("/lib/modules"), PathBuf::from("/usr/lib/modules"), PathBuf::from("/var/log"), data_dir.to_path_buf()] {
                if let Ok(r) = std::fs::canonicalize(&p) {
                    ro.push(r);
                }
            }
        }
        let mut deny = Vec::new();
        for d in ["vault", "devmode", "tls", "ssh", ".ssh", "keys", "secrets", "browser-profile"] {
            deny.push(data_dir.join(d));
            if let Ok(r) = std::fs::canonicalize(data_dir.join(d)) {
                deny.push(r);
            }
        }
        Sandbox { home: roots.home, rw, ro, deny }
    }

    /// For tests and other callers that know the roots.
    pub fn with_roots(home: &Path, rw: &[PathBuf], ro: &[PathBuf], deny: &[PathBuf]) -> Sandbox {
        Sandbox { home: home.into(), rw: rw.to_vec(), ro: ro.to_vec(), deny: deny.to_vec() }
    }

    fn denied(&self, real: &Path) -> bool {
        if self.deny.iter().any(|d| real.starts_with(d)) {
            return true;
        }
        // names below the root that holds the path (the folders above the root are the device's business)
        let root = self.rw.iter().chain(self.ro.iter()).filter(|r| real.starts_with(r)).max_by_key(|r| r.components().count());
        let below = root.and_then(|r| real.strip_prefix(r).ok()).unwrap_or(real);
        below.components().any(|c| matches!(c, std::path::Component::Normal(n) if secret_name(&n.to_string_lossy())))
    }

    /// The real path for `input`, if the sandbox allows it. `write` also needs a read-write root, and nothing may be written
    /// through a link.
    pub fn resolve(&self, input: &str, write: bool) -> Result<PathBuf, String> {
        let t = input.trim();
        if t.is_empty() || t.len() > 1024 || t.contains('\0') {
            return Err("give a path (at most 1024 characters)".into());
        }
        let expanded = if t == "~" {
            self.home.to_string_lossy().into_owned()
        } else if let Some(r) = t.strip_prefix("~/") {
            self.home.join(r).to_string_lossy().into_owned()
        } else {
            t.to_string()
        };
        let real = if write { crate::scope::resolve_for_write(&self.home, &expanded)? } else { crate::scope::resolve(&self.home, &expanded)? };
        if self.denied(&real) {
            return Err(format!("{} holds secrets and is never available to a developer computer", real.display()));
        }
        let in_rw = self.rw.iter().any(|r| real.starts_with(r));
        let in_ro = self.ro.iter().any(|r| real.starts_with(r));
        if write {
            if !in_rw {
                return Err(if in_ro { format!("{} is a read-only place (files:system reads, it never writes)", real.display()) } else { format!("{} is outside the folders a developer computer may write (home, workspaces, {DEV_TMP})", real.display()) });
            }
            if std::fs::symlink_metadata(&real).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(format!("{} is a link: nothing is written through links", real.display()));
            }
        } else if !in_rw && !in_ro {
            return Err(format!("{} is outside the folders a developer computer may read (home, workspaces, {DEV_TMP}; the system scope adds /etc and more)", real.display()));
        }
        Ok(real)
    }
}

fn kind(m: &std::fs::Metadata) -> &'static str {
    if m.is_dir() {
        "dir"
    } else if m.is_file() {
        "file"
    } else if m.file_type().is_symlink() {
        "link"
    } else {
        "other"
    }
}

pub fn ls(sb: &Sandbox, path: &str) -> Result<Value, String> {
    let real = sb.resolve(path, false)?;
    let m = std::fs::metadata(&real).map_err(|e| format!("{}: {e}", real.display()))?;
    if !m.is_dir() {
        return Ok(json!({ "path": real.display().to_string(), "entries": [entry(&real, &m)] }));
    }
    let mut names: Vec<(String, std::fs::Metadata)> = Vec::new();
    for e in std::fs::read_dir(&real).map_err(|e| format!("{}: {e}", real.display()))?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if secret_name(&name) || name.ends_with(PART) || sb.deny.iter().any(|d| real.join(&name).starts_with(d)) {
            continue;
        }
        if let Ok(md) = std::fs::symlink_metadata(e.path()) {
            names.push((name, md));
        }
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    let truncated = names.len() > MAX_LIST;
    names.truncate(MAX_LIST);
    Ok(json!({ "path": real.display().to_string(), "entries": names.iter().map(|(n, m)| entry(Path::new(n), m)).collect::<Vec<_>>(), "truncated": truncated }))
}

fn entry(p: &Path, m: &std::fs::Metadata) -> Value {
    json!({ "name": p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), "kind": kind(m), "size": if m.is_file() { m.len() } else { 0 }, "mtime": m.mtime(), "mode": m.mode() & 0o7777 })
}

pub fn sha256_file(p: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(p).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn part_of(real: &Path) -> PathBuf {
    let mut n = real.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    n.push(PART);
    real.with_file_name(n)
}

/// What is there: size and (with `hash`) SHA-256 of the file, and how much of an interrupted push is kept.
pub fn stat(sb: &Sandbox, path: &str, hash: bool) -> Result<Value, String> {
    let real = match sb.resolve(path, false) {
        Ok(r) => r,
        // a place whose folder does not exist yet: nothing is there (a push is about to make it)
        Err(e) if e.starts_with("no such directory") => return Ok(json!({ "path": path.trim(), "exists": false, "kind": "none", "size": 0, "part_size": 0 })),
        Err(e) => return Err(e),
    };
    let m = std::fs::metadata(&real).ok();
    let part = std::fs::metadata(part_of(&real)).ok().map(|m| m.len()).unwrap_or(0);
    let mut v = json!({ "path": real.display().to_string(), "exists": m.is_some(), "kind": m.as_ref().map_or("none", |m| kind(m)), "size": m.as_ref().map_or(0, |m| m.len()), "part_size": part });
    if hash {
        if let Some(m) = m.filter(|m| m.is_file()) {
            if m.len() > MAX_FILE {
                return Err("that file is too big to hash here".into());
            }
            v["sha256"] = json!(sha256_file(&real)?);
        }
    }
    Ok(v)
}

pub fn mkdir(sb: &Sandbox, path: &str) -> Result<Value, String> {
    let real = sb.resolve(path, true)?;
    std::fs::create_dir_all(&real).map_err(|e| format!("{}: {e}", real.display()))?;
    Ok(json!({ "path": real.display().to_string() }))
}

pub fn rm(sb: &Sandbox, path: &str, recursive: bool) -> Result<Value, String> {
    // a link is removed as a link: only the folder that holds it is resolved
    let t = path.trim();
    let real = match t.rsplit_once('/') {
        Some((dir, name)) if !name.is_empty() && name != "." && name != ".." && !t.ends_with('/') => {
            let parent = sb.resolve(if dir.is_empty() { "/" } else { dir }, true)?;
            let joined = parent.join(name);
            if std::fs::symlink_metadata(&joined).is_ok_and(|m| m.file_type().is_symlink()) { joined } else { sb.resolve(path, true)? }
        }
        None if !t.is_empty() && t != "." && t != ".." && t != "~" => {
            let joined = sb.home.join(t);
            if std::fs::symlink_metadata(&joined).is_ok_and(|m| m.file_type().is_symlink()) && sb.resolve(".", true).is_ok() { joined } else { sb.resolve(path, true)? }
        }
        _ => sb.resolve(path, true)?,
    };
    if real.parent().is_some_and(|p| !sb.rw.iter().any(|r| p.starts_with(r))) {
        return Err(format!("{} is outside the folders a developer computer may write", real.display()));
    }
    if sb.rw.iter().any(|r| r == &real) || real == sb.home || real.parent().is_none() {
        return Err(format!("{} is a root of the sandbox: it is not removed", real.display()));
    }
    let m = std::fs::symlink_metadata(&real).map_err(|e| format!("{}: {e}", real.display()))?;
    if m.is_dir() {
        if recursive {
            // a link inside is removed as a link, never followed; secrets inside are not special for removal
            std::fs::remove_dir_all(&real).map_err(|e| format!("{}: {e}", real.display()))?;
        } else {
            std::fs::remove_dir(&real).map_err(|e| format!("{}: {e} (use -r for a folder that is not empty)", real.display()))?;
        }
    } else {
        std::fs::remove_file(&real).map_err(|e| format!("{}: {e}", real.display()))?;
    }
    let _ = std::fs::remove_file(part_of(&real));
    Ok(json!({ "removed": real.display().to_string() }))
}

/// One chunk of a push. `offset` 0 starts (or restarts) the transfer; a later offset must equal what is already kept (the
/// answer of a refused chunk says how much that is, to resume from). The last chunk (`offset + len == total`) is checked
/// against `sha256` and moved into place atomically; a mismatch deletes the partial file.
pub fn put(sb: &Sandbox, path: &str, offset: u64, total: u64, sha256: &str, overwrite: bool, mode: Option<u32>, chunk: &[u8]) -> Result<Value, (u16, Value)> {
    let bad = |m: String| (422u16, json!({ "error": m }));
    if total > MAX_FILE {
        return Err(bad(format!("{total} bytes is more than the {} MiB a push may carry", MAX_FILE >> 20)));
    }
    if chunk.len() > MAX_CHUNK {
        return Err(bad(format!("a chunk is at most {} MiB", MAX_CHUNK >> 20)));
    }
    if offset.saturating_add(chunk.len() as u64) > total {
        return Err(bad("the chunk goes past the announced size".into()));
    }
    if sha256.len() != 64 || !sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(bad("sha256 (64 hex digits) of the whole file is required".into()));
    }
    let real = sb.resolve(path, true).map_err(|e| (403, json!({ "error": e })))?;
    if real.is_dir() {
        return Err((409, json!({ "error": format!("{} is a folder", real.display()) })));
    }
    if offset == 0 && real.exists() && !overwrite {
        return Err((409, json!({ "error": format!("{} exists already (use --force to replace it)", real.display()), "exists": true })));
    }
    if let Some(parent) = real.parent() {
        std::fs::create_dir_all(parent).map_err(|e| bad(format!("{}: {e}", parent.display())))?;
        // the folder is resolved again after it was made: it must still be a real place inside the roots
        let again = std::fs::canonicalize(parent).map_err(|e| bad(e.to_string()))?;
        if !sb.rw.iter().any(|r| again.starts_with(r)) {
            return Err((403, json!({ "error": "the folder is outside the sandbox" })));
        }
    }
    let part = part_of(&real);
    let have = std::fs::symlink_metadata(&part).ok().filter(|m| m.is_file()).map_or(0, |m| m.len());
    if offset > 0 && have != offset {
        return Err((409, json!({ "error": format!("the device holds {have} bytes of this file, not {offset}: resume from there"), "have": have })));
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).custom_flags(libc::O_NOFOLLOW).mode(0o600);
    if offset == 0 {
        opts.create(true).truncate(true);
    }
    let mut f = opts.open(&part).map_err(|e| bad(format!("{}: {e}", part.display())))?;
    f.seek(SeekFrom::Start(offset)).map_err(|e| bad(e.to_string()))?;
    f.write_all(chunk).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        bad(format!("writing failed: {e}"))
    })?;
    let now_have = offset + chunk.len() as u64;
    if now_have < total {
        return Ok(json!({ "have": now_have, "done": false }));
    }
    f.sync_all().map_err(|e| bad(e.to_string()))?;
    drop(f);
    let got = sha256_file(&part).map_err(bad)?;
    if !got.eq_ignore_ascii_case(sha256) {
        let _ = std::fs::remove_file(&part);
        return Err((422, json!({ "error": "the checksum of what arrived does not match: the transfer was dropped, send it again", "have": 0 })));
    }
    if let Some(m) = mode {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&part, std::fs::Permissions::from_mode(m & 0o777));
    }
    std::fs::rename(&part, &real).map_err(|e| bad(format!("{}: {e}", real.display())))?;
    Ok(json!({ "have": now_have, "done": true, "path": real.display().to_string(), "sha256": got }))
}

/// A chunk of a pull: at most `len` (capped at [`MAX_CHUNK`]) bytes from `offset`.
pub fn get(sb: &Sandbox, path: &str, offset: u64, len: usize) -> Result<Vec<u8>, String> {
    let real = sb.resolve(path, false)?;
    let m = std::fs::metadata(&real).map_err(|e| format!("{}: {e}", real.display()))?;
    if !m.is_file() {
        return Err(format!("{} is not a file", real.display()));
    }
    let mut f = std::fs::File::open(&real).map_err(|e| format!("{}: {e}", real.display()))?;
    f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    f.take(len.min(MAX_CHUNK) as u64).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}
