//! `update` scope, small edition: a **hot patch** (`iyakki dev patch`, docs/development/REMOTE-PATCH.md) adds, replaces or deletes a few
//! files of the running system without an image rebuild, keeps them across restarts, and can be taken away again.
//!
//! * A **pack** (`IYPATCH1` container, or a folder with `patch.json` and `files/`) carries a manifest (every file with mode, owner, size and
//!   SHA-256) and the files; the whole pack is checksummed. Steps before and after are a fixed vocabulary ([`Step`]); a free `shell`
//!   step needs the `shell` scope and its approval.
//! * On the device a patch is the folder `iyakki/patches/<id>/` on the DATA volume (never the boot partition): `patch.json`, `layer/` (the
//!   files as they appear in the root), `layer.txt` (what the first-stage init checks, see `iyakki_stage1::parse_layer_info`), `undo/` (what
//!   was there before), `state.json`. The file `iyakki/patches/active` lists the active ids, the winner first.
//! * **Live**: a regular file is replaced by a bind mount of the layer's file (the original stays under it), a symlink is swapped for the
//!   new file, a deletion unlinks. Everything done live is written to `state.json` so it can be reversed. The root's writable layer is RAM,
//!   so none of that survives a restart by itself: **at boot** the first stage puts the active layers under the writable layer
//!   (`lowerdir=patches:root`), and skips any that fail ([`iyakki_stage1::plan_patches`]).
//! * What cannot be live (PID 1, kernel modules, firmware) is stored and takes effect at the next start.
//!
//! The system effects (bind mounts, service restarts...) are behind [`Live`]; tests use [`FakeLive`].

use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::files;
use super::DevMode;

pub const FORMAT: u32 = 1;
pub const MANIFEST: &str = "patch.json";
pub const MAGIC: &[u8; 8] = b"IYPATCH1";
pub const MAX_OPS: usize = 200;
/// The most a patch file may carry; for a delta file this is the size of the DELTA, not of the file it makes.
pub const MAX_FILE: u64 = 64 << 20;
/// The most a delta may make: the size of the file it produces on the device.
pub const MAX_TARGET: u64 = crate::bindelta::MAX_FILE;
pub const MAX_TOTAL: u64 = 128 << 20;
/// Room kept on the data volume beyond a patch (its layer and the copies of what it replaces).
pub const DATA_MARGIN: u64 = 64 << 20;
pub const MAX_PATCHES: usize = iyakki_stage1::PATCH_MAX_LAYERS;
/// Services a step may restart: helpers that are started again by whoever uses them, never IYAKKI's own parts (those need a restart).
pub const SERVICES: [&str; 7] = ["sshd", "wpa_supplicant", "dhcpcd", "udhcpc", "dnsmasq", "chronyd", "pipewire"];

// -------------------------------------------------------------------------------------------- the manifest

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Add,
    Replace,
    Delete,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    File,
    Symlink,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Op {
    /// Absolute path in the running system.
    pub path: String,
    pub action: Action,
    #[serde(default)]
    pub kind: Kind,
    /// Octal text, `0755`. Files default to `0644`.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub uid: u32,
    #[serde(default)]
    pub gid: u32,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub sha256: String,
    /// The target of a symlink.
    #[serde(default)]
    pub link: Option<String>,
    /// Present: the content carried for this file is a binary delta (docs/development/REMOTE-PATCH-DELTA.md), `size` and `sha256` are
    /// the delta's, and the file the device ends up with is the one described here. Only for `replace`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<DeltaSpec>,
}

/// What a delta op promises about the file on the device before and after.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct DeltaSpec {
    /// SHA-256 of the file as it must be on the device now.
    #[serde(default)]
    pub old_sha256: String,
    /// SHA-256 and size of the file that results.
    #[serde(default)]
    pub new_sha256: String,
    #[serde(default)]
    pub new_size: u64,
}

fn is_sha(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|c| c.is_ascii_hexdigit())
}

/// The fixed vocabulary of steps. No free command unless the computer holds the `shell` scope and its approval.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Step {
    RestartService { name: String },
    Remount { path: String, rw: bool },
    DropCaches,
    Sync,
    Shell { cmd: String },
}

fn star() -> String {
    "*".into()
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub format: u32,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub created: String,
    /// The root (tree id) the patch is for: `*` for any, `current` for whichever is current when it is applied.
    #[serde(default = "star")]
    pub base: String,
    /// The IYAKKI version that fixes this for good: from there on the patch is `obsolete` and is no longer used.
    #[serde(default)]
    pub obsolete_from: Option<String>,
    pub ops: Vec<Op>,
    #[serde(default)]
    pub pre: Vec<Step>,
    #[serde(default)]
    pub post: Vec<Step>,
}

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn clean_component(c: &str) -> bool {
    !c.is_empty() && c.len() <= 100 && c != "." && c != ".." && c.chars().all(|ch| ch.is_ascii_alphanumeric() || "._-+~@".contains(ch))
}

/// How far a path reaches.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Reach {
    /// The places a plain `update` scope may patch.
    Normal,
    /// Needs the `shell` scope and its approval: the rest of the system.
    Wide,
    /// Needs the next start (PID 1, kernel modules, firmware); also wide.
    Reboot,
}

const NORMAL: [&str; 9] = ["/usr/bin/", "/usr/sbin/", "/usr/lib/", "/usr/libexec/", "/usr/share/", "/usr/local/", "/opt/", "/etc/iyakki/", "/etc/alsa/"];
const NORMAL_FILES: [&str; 2] = ["/etc/asound.conf", "/etc/fonts/local.conf"];
const NEVER: [&str; 13] = ["/boot", "/.stage1", "/var/lib/iyakki", "/proc", "/sys", "/dev", "/run", "/tmp", "/mnt", "/media", "/home", "/root", "/lost+found"];
const NEVER_FILES: [&str; 11] = ["/etc/passwd", "/etc/group", "/etc/shadow", "/etc/gshadow", "/etc/sudoers", "/etc/ssh/sshd_config", "/etc/hostname", "/etc/machine-id", "/etc/ssl/private", "/etc/iyakki/vault", "/etc/iyakki/keys"];
const REBOOT: [&str; 8] = ["/init", "/sbin/init", "/usr/sbin/init", "/usr/bin/iyakki", "/usr/sbin/iyakki", "/bin/iyakki", "/lib/modules", "/lib/firmware"];

fn under(p: &str, prefix: &str) -> bool {
    p == prefix || p.strip_prefix(prefix).is_some_and(|r| r.starts_with('/') || prefix.ends_with('/'))
}

/// May a patch touch this path, and how far does that reach? Errors are the rules that are never lifted.
pub fn path_reach(p: &str) -> Result<Reach, String> {
    if !p.starts_with('/') || p.len() > 400 || p.ends_with('/') {
        return Err(format!("{p:?} is not an absolute file path"));
    }
    let comps: Vec<&str> = p.split('/').skip(1).collect();
    if comps.len() > 12 || !comps.iter().all(|c| clean_component(c)) {
        return Err(format!("{p:?} has a component that is not a plain name (letters, digits, . _ - + ~ @)"));
    }
    if NEVER.iter().any(|n| under(p, n)) || NEVER_FILES.iter().any(|n| under(p, n)) {
        return Err(format!("{p} is never patched (boot partition, data volume, accounts, keys, user files)"));
    }
    if comps.iter().any(|c| files::secret_name(c) || *c == ".ssh" || c.starts_with("ssh_host_")) {
        return Err(format!("{p} looks like a secret or a key: never patched"));
    }
    if REBOOT.iter().any(|n| under(p, n)) {
        return Ok(Reach::Reboot);
    }
    if NORMAL.iter().any(|n| p.starts_with(n)) || NORMAL_FILES.contains(&p) {
        return Ok(Reach::Normal);
    }
    Ok(Reach::Wide)
}

fn parse_mode(m: &Option<String>, kind: Kind) -> Result<u32, String> {
    match (m, kind) {
        (_, Kind::Symlink) => Ok(0o777),
        (None, _) => Ok(0o644),
        (Some(t), _) => {
            let v = u32::from_str_radix(t.trim_start_matches("0o"), 8).map_err(|_| format!("mode {t:?} is not octal"))?;
            if v > 0o7777 {
                return Err(format!("mode {t} is out of range"));
            }
            Ok(v)
        }
    }
}

fn check_step(s: &Step, wide: bool) -> Result<(), String> {
    match s {
        Step::RestartService { name } if SERVICES.contains(&name.as_str()) => Ok(()),
        Step::RestartService { name } => Err(format!("service {name:?} cannot be restarted by a patch (allowed: {})", SERVICES.join(", "))),
        Step::Remount { path, .. } => match path_reach(&format!("{}/x", path.trim_end_matches('/'))) {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("remount: {e}")),
        },
        Step::DropCaches | Step::Sync => Ok(()),
        Step::Shell { cmd } if !wide => Err(format!("a shell step ({:?}) needs the shell scope and its approval on the device", cmd.chars().take(30).collect::<String>())),
        Step::Shell { cmd } if cmd.is_empty() || cmd.len() > 2000 || cmd.contains('\0') => Err("the shell step is empty or too long".into()),
        Step::Shell { .. } => Ok(()),
    }
}

/// The manifest on its own terms (the paths, the numbers, the steps); `wide`: the shell scope and approval are there.
pub fn check_manifest(m: &Manifest, wide: bool) -> Result<(), String> {
    if m.format != FORMAT {
        return Err(format!("patch format {} is not understood (this device reads format {FORMAT})", m.format));
    }
    if !iyakki_stage1::valid_id(&m.id) {
        return Err("the patch id must be 1 to 64 letters, digits, . _ -".into());
    }
    if m.title.is_empty() || m.title.len() > 120 || m.title.chars().any(char::is_control) || m.description.len() > 2000 {
        return Err("the title is missing or too long".into());
    }
    if m.base != "*" && m.base != "current" && !iyakki_stage1::valid_id(&m.base) {
        return Err("base: * (any root), current, or a root id".into());
    }
    if m.ops.is_empty() || m.ops.len() > MAX_OPS {
        return Err(format!("a patch carries 1 to {MAX_OPS} files"));
    }
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for o in &m.ops {
        let reach = path_reach(&o.path)?;
        if reach != Reach::Normal && !wide {
            return Err(format!("{} is outside the places a patch may change without the shell scope and its approval", o.path));
        }
        if !seen.insert(o.path.clone()) {
            return Err(format!("{} is listed twice", o.path));
        }
        let mode = parse_mode(&o.mode, o.kind)?;
        if mode & 0o6000 != 0 && !wide {
            return Err(format!("{}: setuid and setgid need the shell scope and its approval", o.path));
        }
        match (o.action, o.kind) {
            (Action::Delete, _) => {
                if !o.sha256.is_empty() || o.size != 0 || o.link.is_some() || o.delta.is_some() {
                    return Err(format!("{}: a deletion carries no content", o.path));
                }
            }
            (_, Kind::Symlink) => {
                if o.delta.is_some() {
                    return Err(format!("{}: a link has no delta", o.path));
                }
                let l = o.link.as_deref().unwrap_or("");
                if l.is_empty() || l.len() > 400 || l.contains(['\0', '\n']) || o.size != 0 || !o.sha256.is_empty() {
                    return Err(format!("{}: a symlink needs a link target and no content", o.path));
                }
            }
            (_, Kind::File) => {
                if o.link.is_some() {
                    return Err(format!("{}: a file has no link target", o.path));
                }
                if !is_sha(&o.sha256) {
                    return Err(format!("{} has no valid SHA-256", o.path));
                }
                if let Some(d) = &o.delta {
                    if o.action != Action::Replace {
                        return Err(format!("{}: a delta only replaces a file that is on the device", o.path));
                    }
                    if !is_sha(&d.old_sha256) || !is_sha(&d.new_sha256) {
                        return Err(format!("{}: a delta needs the SHA-256 of the old and of the new file", o.path));
                    }
                    if d.new_size > MAX_TARGET {
                        return Err(format!("{} would be {} MB: a delta makes a file of at most {} MB", o.path, d.new_size >> 20, MAX_TARGET >> 20));
                    }
                }
                if o.size > MAX_FILE {
                    return Err(format!("{} is {} MB: a patch file is at most {} MB{}", o.path, o.size >> 20, MAX_FILE >> 20, if o.delta.is_some() { " (the delta is too big: send the whole file in an update instead)" } else { "" }));
                }
                total += o.size;
            }
        }
    }
    if total > MAX_TOTAL {
        return Err(format!("the files add up to {} MB: a patch carries at most {} MB", total >> 20, MAX_TOTAL >> 20));
    }
    for s in m.pre.iter().chain(&m.post) {
        check_step(s, wide)?;
    }
    if m.pre.len() > 8 || m.post.len() > 8 {
        return Err("at most 8 steps before and 8 after".into());
    }
    Ok(())
}

impl Manifest {
    /// Does anything in the patch need the next start (and cannot be live)?
    pub fn needs_reboot(&self) -> bool {
        self.ops.iter().any(|o| path_reach(&o.path) == Ok(Reach::Reboot))
    }

    fn content_ops(&self) -> impl Iterator<Item = (usize, &Op)> {
        self.ops.iter().enumerate().filter(|(_, o)| o.action != Action::Delete && o.kind == Kind::File)
    }
}

// -------------------------------------------------------------------------------------------- the pack

/// A manifest and the content of its files (`(index into ops, bytes)`), checked.
#[derive(Debug)]
pub struct Pack {
    pub manifest: Manifest,
    pub content: Vec<(usize, Vec<u8>)>,
}

/// Make a pack from a folder: `patch.json` (the manifest without sizes and hashes) and `files/<path>`.
pub fn build(src: &Path) -> Result<Vec<u8>, String> {
    build_with(src, None)
}

/// The old version of `path` for a delta, from `from`: a folder laid out like the root (or like a build folder, with `files/`), a pack
/// (the file it carries whole), or, for a patch that changes one file, that file itself.
fn old_version(from: &Path, path: &str, single: bool) -> Result<Option<Vec<u8>>, String> {
    let rel = path.trim_start_matches('/');
    if from.is_dir() {
        for p in [from.join(rel), from.join("files").join(rel)] {
            if p.is_file() {
                return std::fs::read(&p).map(Some).map_err(|e| format!("{}: {e}", p.display()));
            }
        }
        return Ok(None);
    }
    let bytes = std::fs::read(from).map_err(|e| format!("{}: {e}", from.display()))?;
    if bytes.starts_with(MAGIC) {
        let pack = decode(&bytes)?;
        let Some((i, o)) = pack.manifest.ops.iter().enumerate().find(|(_, o)| o.path == path) else { return Ok(None) };
        if o.delta.is_some() {
            return Err(format!("{} in {} is itself a delta: give the old program file, or a pack that carries it whole (--delta-from)", path, from.display()));
        }
        return Ok(pack.content.into_iter().find(|(j, _)| *j == i).map(|(_, b)| b));
    }
    if !single {
        return Err(format!("--delta-from {} is one file but the patch changes several: give the old pack or a folder laid out like the root", from.display()));
    }
    Ok(Some(bytes))
}

/// [`build`], with `delta_from`: for each file that replaces one on the device, a binary delta against the old version found there
/// (docs/development/REMOTE-PATCH-DELTA.md). An op that carries `"delta": {}` in `patch.json` must have an old version; without that mark
/// the delta is used only where it is smaller than the file.
pub fn build_with(src: &Path, delta_from: Option<&Path>) -> Result<Vec<u8>, String> {
    let text = std::fs::read(src.join(MANIFEST)).map_err(|e| format!("{}: {e}", src.join(MANIFEST).display()))?;
    let mut m: Manifest = serde_json::from_slice(&text).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let marked = m.ops.iter().filter(|o| o.delta.is_some()).count();
    if marked > 0 && delta_from.is_none() {
        return Err("this patch has delta files: say which version they are against with --delta-from OLD_FILE_OR_PACK_OR_FOLDER".into());
    }
    let candidates = if marked > 0 { marked } else { m.ops.iter().filter(|o| o.action == Action::Replace && o.kind == Kind::File).count() };
    let mut content = Vec::new();
    for (i, o) in m.ops.iter_mut().enumerate() {
        if o.action == Action::Delete || o.kind == Kind::Symlink {
            continue;
        }
        let p = src.join("files").join(o.path.trim_start_matches('/'));
        let md = std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {e} (put the file in files{})", p.display(), o.path))?;
        if !md.is_file() {
            return Err(format!("{} is not a regular file", p.display()));
        }
        if md.len() > MAX_TARGET {
            return Err(format!("{} is {} MB: more than the {} MB a patch file may become", p.display(), md.len() >> 20, MAX_TARGET >> 20));
        }
        let explicit = o.delta.is_some();
        let mut bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        if o.mode.is_none() {
            // the file's own mode, so an executable stays one
            o.mode = Some(format!("{:04o}", md.mode() & 0o7777));
        }
        let wants = explicit || (marked == 0 && o.action == Action::Replace);
        if let (Some(from), true) = (delta_from, wants) {
            match old_version(from, &o.path, candidates == 1)? {
                Some(old) => {
                    let d = crate::bindelta::make_delta(&old, &bytes);
                    // a delta that is not smaller than the file is no help (unless it was asked for by name)
                    if explicit || d.len() < bytes.len() {
                        o.delta = Some(DeltaSpec { old_sha256: sha256_hex(&old), new_sha256: sha256_hex(&bytes), new_size: bytes.len() as u64 });
                        bytes = d;
                    }
                }
                None if explicit => return Err(format!("no old version of {} in {}", o.path, from.display())),
                None => {}
            }
        } else if md.len() > MAX_FILE {
            return Err(format!("{} is {} MB: a patch file is at most {} MB (a delta against the old version, --delta-from, makes it small)", o.path, md.len() >> 20, MAX_FILE >> 20));
        }
        o.size = bytes.len() as u64;
        o.sha256 = sha256_hex(&bytes);
        content.push((i, bytes));
    }
    check_manifest(&m, true)?;
    Ok(encode(&m, &content))
}

pub fn encode(m: &Manifest, content: &[(usize, Vec<u8>)]) -> Vec<u8> {
    let mj = serde_json::to_vec(m).unwrap_or_default();
    let mut out = MAGIC.to_vec();
    out.extend((mj.len() as u32).to_be_bytes());
    out.extend(&mj);
    for (_, b) in content {
        out.extend(b);
    }
    let sum = Sha256::digest(&out);
    out.extend(sum);
    out
}

/// Read and check a pack file: the checksum of the whole, the manifest, every file's size and SHA-256.
pub fn decode(bytes: &[u8]) -> Result<Pack, String> {
    if bytes.len() < MAGIC.len() + 4 + 32 || &bytes[..8] != MAGIC {
        return Err("this is not an IYAKKI patch pack".into());
    }
    let (body, sum) = bytes.split_at(bytes.len() - 32);
    if Sha256::digest(body).as_slice() != sum {
        return Err("the pack's checksum does not match: it is damaged or was changed".into());
    }
    let mlen = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let rest = &body[12..];
    if mlen > rest.len() || mlen > 4 << 20 {
        return Err("the pack's manifest is cut off".into());
    }
    let m: Manifest = serde_json::from_slice(&rest[..mlen]).map_err(|e| format!("{MANIFEST}: {e}"))?;
    check_manifest(&m, true)?;
    let mut at = mlen;
    let mut content = Vec::new();
    for (i, o) in m.content_ops() {
        let end = at.checked_add(o.size as usize).filter(|e| *e <= rest.len()).ok_or_else(|| format!("{} is cut off in the pack", o.path))?;
        let b = &rest[at..end];
        if sha256_hex(b) != o.sha256 {
            return Err(format!("{} does not match its SHA-256: the pack is damaged", o.path));
        }
        content.push((i, b.to_vec()));
        at = end;
    }
    if at != rest.len() {
        return Err("the pack carries more bytes than its manifest lists".into());
    }
    Ok(Pack { manifest: m, content })
}

/// A pack file, or a folder in the build layout (checked the same way).
pub fn load(path: &Path) -> Result<Pack, String> {
    if path.is_dir() {
        return decode(&build(path)?);
    }
    decode(&std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
}

// -------------------------------------------------------------------------------------------- the system effects

/// Everything a patch does to the running system that is not a plain file operation. The device uses [`SysLive`].
pub trait Live: Send + Sync {
    /// Make `target` show the content of `src` (a bind mount); the original stays underneath.
    fn bind(&self, src: &Path, target: &Path) -> Result<(), String>;
    fn unbind(&self, target: &Path) -> Result<(), String>;
    /// A whiteout (an overlay "this file is deleted") at `path` in a layer.
    fn whiteout(&self, path: &Path) -> Result<(), String>;
    fn restart_service(&self, name: &str) -> Result<(), String>;
    fn remount(&self, path: &str, rw: bool) -> Result<(), String>;
    fn drop_caches(&self) -> Result<(), String>;
    fn sync(&self) -> Result<(), String>;
    fn shell(&self, cmd: &str) -> Result<(), String>;
    /// Identifies this start of the system (a patch applied live is live only during the start it was applied in).
    fn boot_id(&self) -> String;
    /// Is the process allowed to change owners (root)?
    fn is_root(&self) -> bool;
}

/// The real thing: bind mounts, signals, `/proc`. Not exercised by the unit tests (they need root and a device).
pub struct SysLive;

fn c(p: &Path) -> Result<std::ffi::CString, String> {
    std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).map_err(|_| "a path with a NUL".to_string())
}

impl Live for SysLive {
    fn bind(&self, src: &Path, target: &Path) -> Result<(), String> {
        let (s, t) = (c(src)?, c(target)?);
        // SAFETY: valid NUL-terminated paths; a failure is reported.
        let r = unsafe { libc::mount(s.as_ptr(), t.as_ptr(), std::ptr::null(), libc::MS_BIND, std::ptr::null()) };
        if r != 0 {
            return Err(format!("bind mount {}: {}", target.display(), std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn unbind(&self, target: &Path) -> Result<(), String> {
        let t = c(target)?;
        // SAFETY: valid path; a busy file is detached lazily (running programs keep their copy).
        let r = unsafe { libc::umount2(t.as_ptr(), 0) };
        if r != 0 {
            let r2 = unsafe { libc::umount2(t.as_ptr(), libc::MNT_DETACH) };
            if r2 != 0 {
                return Err(format!("unmount {}: {}", target.display(), std::io::Error::last_os_error()));
            }
        }
        Ok(())
    }

    fn whiteout(&self, path: &Path) -> Result<(), String> {
        let t = c(path)?;
        // SAFETY: valid path; mknod of a 0/0 character device is the overlay whiteout.
        let r = unsafe { libc::mknod(t.as_ptr(), libc::S_IFCHR, 0) };
        if r != 0 {
            return Err(format!("whiteout {}: {} (a deletion needs root)", path.display(), std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn restart_service(&self, name: &str) -> Result<(), String> {
        if !SERVICES.contains(&name) {
            return Err(format!("{name} is not a service a patch may restart"));
        }
        let mut hit = 0;
        for e in std::fs::read_dir("/proc").map_err(|e| e.to_string())?.flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
            if std::fs::read_to_string(e.path().join("comm")).is_ok_and(|n| n.trim() == name) {
                // SAFETY: plain signal to a process found by exact name.
                if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
                    hit += 1;
                }
            }
        }
        if hit == 0 {
            return Err(format!("{name} is not running (nothing to restart)"));
        }
        Ok(())
    }

    fn remount(&self, path: &str, rw: bool) -> Result<(), String> {
        let t = c(Path::new(path))?;
        let flags = libc::MS_REMOUNT | if rw { 0 } else { libc::MS_RDONLY };
        // SAFETY: valid path.
        if unsafe { libc::mount(std::ptr::null(), t.as_ptr(), std::ptr::null(), flags, std::ptr::null()) } != 0 {
            return Err(format!("remount {path}: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    fn drop_caches(&self) -> Result<(), String> {
        self.sync()?;
        std::fs::write("/proc/sys/vm/drop_caches", "3").map_err(|e| format!("drop caches: {e}"))
    }

    fn sync(&self) -> Result<(), String> {
        // SAFETY: no arguments.
        unsafe { libc::sync() };
        Ok(())
    }

    fn shell(&self, cmd: &str) -> Result<(), String> {
        let mut ch = std::process::Command::new("/bin/sh").arg("-c").arg(cmd).stdin(std::process::Stdio::null()).spawn().map_err(|e| e.to_string())?;
        let start = std::time::Instant::now();
        loop {
            match iyakki_core_api::child_status(&mut ch) {
                s if s.success() => return Ok(()),
                // the status was taken by another reaper: it is over, but whether it worked is unknown
                iyakki_core_api::ChildState::Gone => return Err("the command ended, its exit status is not available (another process took it)".into()),
                s if s.is_over() => return Err(format!("the command ended with {}", s.describe())),
                _ if start.elapsed().as_secs() > 60 => {
                    let _ = ch.kill();
                    let _ = ch.wait();
                    return Err("the command took more than 60 s and was stopped".into());
                }
                _ => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        }
    }

    fn boot_id(&self) -> String {
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map(|s| s.trim().to_string()).unwrap_or_default()
    }

    fn is_root(&self) -> bool {
        // SAFETY: no arguments.
        unsafe { libc::geteuid() == 0 }
    }
}

/// A [`Live`] for tests: "binding" copies the layer's file over the target (remembering the original) and "unbinding" puts it back;
/// every call is recorded in order.
#[derive(Default)]
pub struct FakeLive {
    pub calls: Mutex<Vec<String>>,
    originals: Mutex<Vec<(PathBuf, Vec<u8>)>>,
    /// Calls that fail, by their text (`restart sshd`...).
    pub fail: Mutex<Vec<String>>,
    pub root: bool,
}

impl FakeLive {
    fn log(&self, s: String) -> Result<(), String> {
        let bad = self.fail.lock().unwrap().contains(&s);
        self.calls.lock().unwrap().push(s.clone());
        if bad { Err(format!("{s} failed (test)")) } else { Ok(()) }
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Live for FakeLive {
    fn bind(&self, src: &Path, target: &Path) -> Result<(), String> {
        self.log(format!("bind {}", target.display()))?;
        let orig = std::fs::read(target).map_err(|e| e.to_string())?;
        self.originals.lock().unwrap().push((target.to_path_buf(), orig));
        std::fs::copy(src, target).map(|_| ()).map_err(|e| e.to_string())
    }

    fn unbind(&self, target: &Path) -> Result<(), String> {
        self.log(format!("unbind {}", target.display()))?;
        let mut o = self.originals.lock().unwrap();
        let i = o.iter().rposition(|(p, _)| p == target).ok_or("not bound")?;
        let (p, b) = o.remove(i);
        std::fs::write(p, b).map_err(|e| e.to_string())
    }

    fn whiteout(&self, path: &Path) -> Result<(), String> {
        self.log(format!("whiteout {}", path.display()))?;
        std::fs::write(path, b"").map_err(|e| e.to_string())
    }

    fn restart_service(&self, name: &str) -> Result<(), String> {
        self.log(format!("restart {name}"))
    }

    fn remount(&self, path: &str, rw: bool) -> Result<(), String> {
        self.log(format!("remount {path} {}", if rw { "rw" } else { "ro" }))
    }

    fn drop_caches(&self) -> Result<(), String> {
        self.log("drop_caches".into())
    }

    fn sync(&self) -> Result<(), String> {
        self.log("sync".into())
    }

    fn shell(&self, cmd: &str) -> Result<(), String> {
        self.log(format!("shell {cmd}"))
    }

    fn boot_id(&self) -> String {
        "boot-1".into()
    }

    fn is_root(&self) -> bool {
        self.root
    }
}

// -------------------------------------------------------------------------------------------- where things are

pub fn patches_dir(data: &Path) -> PathBuf {
    data.join(iyakki_stage1::PATCHES_DIR)
}

/// Patches being sent are staged outside `iyakki/patches` (their ids could collide with a folder name there).
pub fn staging_dir(data: &Path, id: &str) -> PathBuf {
    data.join("iyakki/patch-staging").join(id)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::File::create(&tmp).and_then(|mut f| f.write_all(bytes).and_then(|_| f.sync_all())).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn read_active(data: &Path) -> Vec<String> {
    iyakki_stage1::parse_active(&std::fs::read_to_string(patches_dir(data).join("active")).unwrap_or_default())
}

fn write_active(data: &Path, ids: &[String]) -> Result<(), String> {
    write_atomic(&patches_dir(data).join("active"), ids.iter().map(|i| format!("{i}\n")).collect::<String>().as_bytes())
}

/// The root the system was started from, as the first stage names it (the `current` pointer of the data volume).
pub fn current_root_id(data: &Path) -> Option<String> {
    std::fs::read_to_string(data.join(iyakki_stage1::ROOTS_DIR).join("current")).ok().and_then(|t| iyakki_stage1::Pointer::parse(&t)).map(|p| p.id)
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
struct Applied {
    path: String,
    /// `bind`, `swap` (a link or file replaced in the writable layer), `delete`.
    how: String,
    /// The file did not exist before.
    created: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
struct State {
    /// `active`, `obsolete`, `removed` (waiting for the next start to be deleted: its layer is in use).
    state: String,
    applied_at: i64,
    by: String,
    /// The start of the system in which it was applied live (empty: not live).
    live_boot: String,
    applied: Vec<Applied>,
    reason: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Undo {
    path: String,
    /// `none` (did not exist), `file`, `link`.
    was: String,
    mode: u32,
    uid: u32,
    gid: u32,
    link: String,
    /// Name of the copy in `undo/files/`.
    copy: String,
}

fn read_state(dir: &Path) -> Option<State> {
    serde_json::from_slice(&std::fs::read(dir.join("state.json")).ok()?).ok()
}

fn write_state(dir: &Path, s: &State) -> Result<(), String> {
    write_atomic(&dir.join("state.json"), &serde_json::to_vec_pretty(s).unwrap_or_default())
}

fn read_manifest(dir: &Path) -> Option<Manifest> {
    serde_json::from_slice(&std::fs::read(dir.join(MANIFEST)).ok()?).ok()
}

// -------------------------------------------------------------------------------------------- the environment

pub struct Env<'a> {
    pub dm: &'a DevMode,
    pub live: &'a dyn Live,
    /// The running system's root (`/`); a folder in tests.
    pub root: PathBuf,
    /// The text of `/etc/iyakki-release`.
    pub release: String,
    /// The computer holds the `shell` scope and a valid approval: the wide paths, free steps and setuid bits are open.
    pub wide: bool,
    pub data_free: &'a dyn Fn() -> u64,
}

impl Env<'_> {
    fn data(&self) -> &Path {
        self.dm.data_dir()
    }

    fn target(&self, p: &str) -> PathBuf {
        self.root.join(p.trim_start_matches('/'))
    }

    fn version(&self) -> String {
        let r = crate::relinfo::ReleaseInfo::parse(&self.release);
        if r.version.is_empty() { crate::relinfo::VERSION.to_string() } else { r.version }
    }
}

// -------------------------------------------------------------------------------------------- the plan

/// What the device says about a patch before anything is sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub id: String,
    pub files: usize,
    pub bytes: u64,
    /// Paths that cannot be live (they take effect at the next start).
    pub next_boot_only: Vec<String>,
    /// The id is already there (applying replaces it).
    pub replaces: bool,
    pub notes: Vec<String>,
}

fn ver_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    super::update::compare_versions(a, b)
}

/// Check everything that can be checked before a byte moves.
pub fn plan(env: &Env, m: &Manifest) -> Result<Plan, String> {
    check_manifest(m, env.wide)?;
    if let Some(v) = &m.obsolete_from {
        if ver_cmp(&env.version(), v) != std::cmp::Ordering::Less {
            return Err(format!("this patch is obsolete from IYAKKI {v} and this device runs {}: the fix is in the system already", env.version()));
        }
    }
    let root_id = current_root_id(env.data());
    if m.base != "*" && m.base != "current" && root_id.as_deref() != Some(m.base.as_str()) {
        return Err(format!("this patch is for the root {} and the device runs {}", m.base, root_id.as_deref().unwrap_or("no lean root")));
    }
    let dir = patches_dir(env.data()).join(&m.id);
    let others = read_active(env.data()).iter().filter(|i| **i != m.id).count();
    if others >= MAX_PATCHES {
        return Err(format!("{MAX_PATCHES} patches are active already: remove one first (iyakki dev patch list)"));
    }
    let bytes: u64 = m.ops.iter().map(|o| o.size).sum();
    // a delta's layer file and its undo copy are as big as the file it makes
    let need: u64 = m.ops.iter().map(|o| o.delta.as_ref().map_or(o.size * 2, |d| d.new_size * 2 + o.size)).sum();
    if (env.data_free)() < need + DATA_MARGIN {
        return Err(format!("the data volume has {} MB free; this patch needs about {} MB (layer and undo copies) plus {} MB spare", (env.data_free)() >> 20, need >> 20, DATA_MARGIN >> 20));
    }
    let mut notes = Vec::new();
    let mut next_boot_only = Vec::new();
    for o in &m.ops {
        if path_reach(&o.path) == Ok(Reach::Reboot) {
            next_boot_only.push(o.path.clone());
        }
        let t = env.target(&o.path);
        let md = std::fs::symlink_metadata(&t).ok();
        if md.as_ref().is_some_and(|m| m.is_dir()) {
            return Err(format!("{} is a folder: a patch changes files and links, not folders", o.path));
        }
        match (o.action, &md) {
            (Action::Replace, None) | (Action::Delete, None) => return Err(format!("{} does not exist on this device ({} needs a file that is there; use add for a new one)", o.path, if o.action == Action::Replace { "replace" } else { "delete" })),
            (Action::Add, Some(_)) => notes.push(format!("{} exists already: add will replace it", o.path)),
            _ => {}
        }
        if let Some(d) = &o.delta {
            // before a byte is sent: is the file here the one the delta was made against?
            check_delta_old(&t, &o.path, d)?;
        }
    }
    if !next_boot_only.is_empty() {
        notes.push(format!("{} file(s) take effect at the next start only (PID 1, kernel modules, firmware)", next_boot_only.len()));
    }
    Ok(Plan { id: m.id.clone(), files: m.ops.len(), bytes, next_boot_only, replaces: dir.join(MANIFEST).exists(), notes })
}

/// The file on this device must be exactly the version a delta was made against. Says so in plain words; changes nothing.
fn check_delta_old(target: &Path, path: &str, d: &DeltaSpec) -> Result<(), String> {
    let md = std::fs::metadata(target).map_err(|e| format!("{path}: {e}"))?;
    if !md.is_file() || md.len() > MAX_TARGET {
        return Err(format!("{path} is not a regular file of a size a delta can start from"));
    }
    if files::sha256_file(target)? != d.old_sha256 {
        return Err(format!("{path} on this device is not the version this delta was made against (its SHA-256 differs from the one recorded in the patch): nothing was changed. Build the delta against the program that runs on the device, or send the whole file."));
    }
    Ok(())
}

/// Make the new file from the file on the device and the staged delta; written beside its final name, flushed to disk, then renamed in
/// (so `dst` is either not there or whole).
fn write_delta_result(target: &Path, staged: &Path, dst: &Path, path: &str, d: &DeltaSpec) -> Result<u64, String> {
    check_delta_old(target, path, d)?;
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dst.with_file_name(format!(".{name}.delta-new"));
    // streamed: the old file is read where the delta points and the result goes straight to the file beside its final name
    let info = crate::bindelta::apply_delta_files(target, staged, &tmp).map_err(|e| format!("{path}: {e}"))?;
    if info.new_len != d.new_size || info.new_sha256 != d.new_sha256 {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{path}: the file made from the delta is not the one the patch describes (size or SHA-256): nothing was written"));
    }
    if let Err(e) = std::fs::rename(&tmp, dst) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{path}: {e}"));
    }
    if let Some(dir) = dst.parent() {
        let _ = std::fs::File::open(dir).and_then(|f| f.sync_all());
    }
    Ok(info.new_len)
}

/// The folder a client sends one file of the patch to. Staged files are named by their position, never by the target path.
pub fn staged_name(i: usize) -> String {
    format!("f{i:04}")
}

/// Check that every file of the patch is staged whole and matches the manifest.
fn verify_staged(env: &Env, m: &Manifest) -> Result<PathBuf, String> {
    let st = staging_dir(env.data(), &m.id);
    for (i, o) in m.content_ops() {
        let p = st.join(staged_name(i));
        let md = std::fs::symlink_metadata(&p).map_err(|_| format!("{} was not sent", o.path))?;
        if !md.is_file() || md.len() != o.size || files::sha256_file(&p)? != o.sha256 {
            return Err(format!("{} on the device does not match the manifest: send it again", o.path));
        }
    }
    Ok(st)
}

// -------------------------------------------------------------------------------------------- building the layer

fn chown_best_effort(live: &dyn Live, p: &Path, uid: u32, gid: u32) -> Result<(), String> {
    if live.is_root() {
        std::os::unix::fs::lchown(p, Some(uid), Some(gid)).map_err(|e| format!("owner of {}: {e}", p.display()))
    } else {
        Ok(())
    }
}

/// Write the layer (`layer/`), `layer.txt` and the undo copies into `dir` (a new folder). Nothing is live yet.
fn write_patch_dir(env: &Env, m: &Manifest, staged: &Path, dir: &Path, base: &str) -> Result<(), String> {
    let layer = dir.join("layer");
    let undo_dir = dir.join("undo");
    let mut entries: Vec<String> = Vec::new();
    let mut undo: Vec<Undo> = Vec::new();
    std::fs::create_dir_all(&layer).and_then(|_| std::fs::create_dir_all(undo_dir.join("files"))).map_err(|e| e.to_string())?;
    for (i, o) in m.ops.iter().enumerate() {
        let dst = layer.join(o.path.trim_start_matches('/'));
        std::fs::create_dir_all(dst.parent().unwrap_or(&layer)).map_err(|e| e.to_string())?;
        let mode = parse_mode(&o.mode, o.kind)?;
        // what is there now
        let t = env.target(&o.path);
        let mut u = Undo { path: o.path.clone(), was: "none".into(), mode: 0, uid: 0, gid: 0, link: String::new(), copy: String::new() };
        if let Ok(md) = std::fs::symlink_metadata(&t) {
            u.mode = md.mode() & 0o7777;
            u.uid = md.uid();
            u.gid = md.gid();
            if md.file_type().is_symlink() {
                u.was = "link".into();
                u.link = std::fs::read_link(&t).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            } else if md.is_file() {
                u.was = "file".into();
                u.copy = staged_name(i);
                std::fs::copy(&t, undo_dir.join("files").join(&u.copy)).map_err(|e| format!("keeping {}: {e}", o.path))?;
            }
        }
        undo.push(u);
        match (o.action, o.kind) {
            (Action::Delete, _) => {
                env.live.whiteout(&dst)?;
                entries.push(format!("del 0 {}", o.path));
            }
            (_, Kind::Symlink) => {
                let _ = std::fs::remove_file(&dst);
                std::os::unix::fs::symlink(o.link.as_deref().unwrap_or(""), &dst).map_err(|e| format!("{}: {e}", o.path))?;
                chown_best_effort(env.live, &dst, o.uid, o.gid)?;
                entries.push(format!("link 0 {}", o.path));
            }
            (_, Kind::File) => {
                // a delta file: the layer holds the file the delta makes, so the size the first stage checks is that file's
                let size = match &o.delta {
                    Some(d) => write_delta_result(&t, &staged.join(staged_name(i)), &dst, &o.path, d)?,
                    None => {
                        std::fs::copy(staged.join(staged_name(i)), &dst).map_err(|e| format!("{}: {e}", o.path))?;
                        o.size
                    }
                };
                std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())?;
                chown_best_effort(env.live, &dst, o.uid, o.gid)?;
                entries.push(format!("file {size} {}", o.path));
            }
        }
    }
    let txt = format!("iyakki-patch-layer 1\nid={}\nbase={base}\n{}\n", m.id, entries.join("\n"));
    // the first stage's own reader must accept what is written (the check that matters at boot)
    if iyakki_stage1::parse_layer_info(&txt).is_none() {
        return Err("internal: the layer description would not be accepted at boot".into());
    }
    write_atomic(&dir.join("layer.txt"), txt.as_bytes())?;
    write_atomic(&undo_dir.join("undo.json"), &serde_json::to_vec_pretty(&undo).unwrap_or_default())?;
    write_atomic(&dir.join(MANIFEST), &serde_json::to_vec_pretty(m).unwrap_or_default())
}

// -------------------------------------------------------------------------------------------- live

fn run_steps(env: &Env, steps: &[Step]) -> Result<(), String> {
    for s in steps {
        match s {
            Step::RestartService { name } => env.live.restart_service(name),
            Step::Remount { path, rw } => env.live.remount(path, *rw),
            Step::DropCaches => env.live.drop_caches(),
            Step::Sync => env.live.sync(),
            Step::Shell { cmd } => env.live.shell(cmd),
        }
        .map_err(|e| format!("step {s:?}: {e}"))?;
    }
    Ok(())
}

fn read_undo(dir: &Path) -> Vec<Undo> {
    std::fs::read(dir.join("undo/undo.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Put the live system back for one applied path.
fn revert_one(env: &Env, dir: &Path, a: &Applied, undo: &[Undo]) -> Result<(), String> {
    let t = env.target(&a.path);
    if a.how == "bind" {
        env.live.unbind(&t)?;
        if a.created {
            let _ = std::fs::remove_file(&t);
        }
        return Ok(());
    }
    let u = undo.iter().find(|u| u.path == a.path);
    let _ = std::fs::remove_file(&t);
    match u {
        Some(u) if u.was == "link" => std::os::unix::fs::symlink(&u.link, &t).map_err(|e| format!("{}: {e}", a.path)),
        Some(u) if u.was == "file" => {
            std::fs::copy(dir.join("undo/files").join(&u.copy), &t).map_err(|e| format!("{}: {e}", a.path))?;
            std::fs::set_permissions(&t, std::fs::Permissions::from_mode(u.mode)).map_err(|e| e.to_string())?;
            chown_best_effort(env.live, &t, u.uid, u.gid)
        }
        _ => Ok(()),
    }
}

fn revert_all(env: &Env, dir: &Path, applied: &[Applied]) -> Vec<String> {
    let undo = read_undo(dir);
    applied.iter().rev().filter_map(|a| revert_one(env, dir, a, &undo).err()).collect()
}

/// Make the patch true in the running system. On the first failure what was done is undone and the error returned.
fn go_live(env: &Env, m: &Manifest, dir: &Path) -> Result<Vec<Applied>, String> {
    let mut done: Vec<Applied> = Vec::new();
    let step = |o: &Op| -> Result<Applied, String> {
        let t = env.target(&o.path);
        let src = dir.join("layer").join(o.path.trim_start_matches('/'));
        let md = std::fs::symlink_metadata(&t).ok();
        let mode = parse_mode(&o.mode, o.kind)?;
        if let Some(p) = t.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        match (o.action, o.kind) {
            (Action::Delete, _) => {
                std::fs::remove_file(&t).map_err(|e| format!("{}: {e}", o.path))?;
                Ok(Applied { path: o.path.clone(), how: "delete".into(), created: false })
            }
            (_, Kind::Symlink) => {
                if md.is_some() {
                    std::fs::remove_file(&t).map_err(|e| format!("{}: {e}", o.path))?;
                }
                std::os::unix::fs::symlink(o.link.as_deref().unwrap_or(""), &t).map_err(|e| format!("{}: {e}", o.path))?;
                Ok(Applied { path: o.path.clone(), how: "swap".into(), created: md.is_none() })
            }
            (_, Kind::File) if md.as_ref().is_some_and(|m| m.file_type().is_symlink()) => {
                // a bind mount on a link would land on the link's TARGET (a wrapper over a link to a big binary would replace the binary)
                std::fs::remove_file(&t).map_err(|e| format!("{}: {e}", o.path))?;
                std::fs::copy(&src, &t).map_err(|e| format!("{}: {e}", o.path))?;
                std::fs::set_permissions(&t, std::fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())?;
                chown_best_effort(env.live, &t, o.uid, o.gid)?;
                Ok(Applied { path: o.path.clone(), how: "swap".into(), created: false })
            }
            (_, Kind::File) => {
                let created = md.is_none();
                if created {
                    std::fs::write(&t, b"").map_err(|e| format!("{}: {e}", o.path))?;
                }
                if let Err(e) = env.live.bind(&src, &t) {
                    if created {
                        let _ = std::fs::remove_file(&t);
                    }
                    return Err(e);
                }
                Ok(Applied { path: o.path.clone(), how: "bind".into(), created })
            }
        }
    };
    for o in &m.ops {
        if path_reach(&o.path) == Ok(Reach::Reboot) {
            continue;
        }
        match step(o) {
            Ok(a) => done.push(a),
            Err(e) => {
                let more = revert_all(env, dir, &done);
                return Err(format!("{e}{}", if more.is_empty() { String::new() } else { format!(" (and undoing failed: {})", more.join("; ")) }));
            }
        }
    }
    Ok(done)
}

// -------------------------------------------------------------------------------------------- apply, remove, list

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Stored, active at the next start, and made true now.
    Live,
    /// Stored and active at the next start only.
    NextBoot,
}

fn clear_guard(data: &Path) {
    let d = patches_dir(data);
    let _ = std::fs::remove_file(d.join("disabled"));
    let _ = std::fs::remove_file(d.join("boot-count"));
}

/// Install a staged patch. The order keeps every step undoable: the folder is built beside the final name and renamed in, the live
/// changes are made (and undone on failure), the `active` list is written last.
pub fn apply(env: &Env, m: &Manifest, mode: Mode, by: &str) -> Result<Value, String> {
    let p = plan(env, m)?;
    let staged = verify_staged(env, m)?;
    let root = patches_dir(env.data());
    let dir = root.join(&m.id);
    if p.replaces {
        // the same id again: the old copy is taken away first (live changes undone), then this one goes in
        remove(env, &m.id, by)?;
    }
    let base = if m.base == "current" { current_root_id(env.data()).unwrap_or_else(|| "*".into()) } else { m.base.clone() };
    let tmp = root.join(format!(".new-{}", m.id));
    let _ = std::fs::remove_dir_all(&tmp);
    if let Err(e) = write_patch_dir(env, m, &staged, &tmp, &base) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, &dir).map_err(|e| e.to_string())?;
    let mut st = State { state: "active".into(), applied_at: env.dm.now(), by: by.into(), ..State::default() };
    let mut warnings = Vec::new();
    if mode == Mode::Live {
        if let Err(e) = run_steps(env, &m.pre) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!("not applied: {e}"));
        }
        match go_live(env, m, &dir) {
            Ok(a) => {
                st.applied = a;
                st.live_boot = env.live.boot_id();
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(format!("not applied (nothing was left changed): {e}"));
            }
        }
    }
    let mut ids = read_active(env.data());
    ids.retain(|i| *i != m.id);
    ids.insert(0, m.id.clone());
    if let Err(e) = write_state(&dir, &st).and_then(|_| write_active(env.data(), &ids)) {
        revert_all(env, &dir, &st.applied);
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!("not applied: {e}"));
    }
    clear_guard(env.data());
    let _ = std::fs::remove_dir_all(staging_dir(env.data(), &m.id));
    if mode == Mode::Live {
        if let Err(e) = run_steps(env, &m.post) {
            warnings.push(format!("the patch is in place but a step after it failed: {e}"));
        }
    }
    let live_files = st.applied.len();
    let message = match (mode, p.next_boot_only.is_empty()) {
        (Mode::NextBoot, _) => format!("Patch {} is stored and takes effect at the next start.", m.id),
        (Mode::Live, true) => format!("Patch {} is live ({live_files} file(s)) and stays after a restart.", m.id),
        (Mode::Live, false) => format!("Patch {} is live for {live_files} file(s); {} take effect at the next start.", m.id, p.next_boot_only.join(", ")),
    };
    Ok(json!({ "id": m.id, "live": mode == Mode::Live, "live_files": live_files, "next_boot_only": p.next_boot_only, "needs_reboot": !p.next_boot_only.is_empty() || mode == Mode::NextBoot, "warnings": warnings, "message": message }))
}

/// Take a patch away: undo what was done live in this start, drop it from `active`. A layer that the running system was started with is
/// still in use (the kernel holds it), so its folder is only marked `removed` and deleted at the next start.
pub fn remove(env: &Env, id: &str, by: &str) -> Result<Value, String> {
    if !iyakki_stage1::valid_id(id) {
        return Err("not a patch id".into());
    }
    let dir = patches_dir(env.data()).join(id);
    let m = read_manifest(&dir).ok_or_else(|| format!("no patch called {id}"))?;
    let mut st = read_state(&dir).unwrap_or_default();
    let mut warnings = Vec::new();
    let live_now = !st.live_boot.is_empty() && st.live_boot == env.live.boot_id();
    if live_now {
        warnings.extend(revert_all(env, &dir, &st.applied));
        let _ = run_steps(env, &m.post);
    }
    let mut ids = read_active(env.data());
    ids.retain(|i| i != id);
    write_active(env.data(), &ids)?;
    let in_boot_layer = booted(env.data()).iter().any(|b| b == id);
    let message = if in_boot_layer && !live_now {
        st.state = "removed".into();
        st.by = by.into();
        write_state(&dir, &st)?;
        format!("Patch {id} is no longer active; the system was started with it, so it goes away completely at the next start.")
    } else {
        let _ = std::fs::remove_dir_all(&dir);
        format!("Patch {id} is removed.")
    };
    Ok(json!({ "id": id, "message": message, "warnings": warnings, "needs_reboot": in_boot_layer }))
}

/// The ids the running system's root was started with (written by the first stage).
fn booted(data: &Path) -> Vec<String> {
    iyakki_stage1::parse_active(&std::fs::read_to_string(patches_dir(data).join("booted")).unwrap_or_default())
}

/// Mark active patches that a newer system makes pointless: made for another root, or `obsolete_from` reached. They leave `active`.
pub fn refresh_obsolete(env: &Env) -> Vec<String> {
    let root_id = current_root_id(env.data());
    let mut gone = Vec::new();
    for id in read_active(env.data()) {
        let dir = patches_dir(env.data()).join(&id);
        let Some(m) = read_manifest(&dir) else { continue };
        let layer_base = std::fs::read_to_string(dir.join("layer.txt")).ok().and_then(|t| iyakki_stage1::parse_layer_info(&t)).map(|l| l.base).unwrap_or_else(star);
        let why = if m.obsolete_from.as_deref().is_some_and(|v| ver_cmp(&env.version(), v) != std::cmp::Ordering::Less) {
            Some(format!("fixed in IYAKKI {}", m.obsolete_from.clone().unwrap_or_default()))
        } else if layer_base != "*" && root_id.as_deref() != Some(layer_base.as_str()) {
            Some(format!("made for root {layer_base}, the system is now {}", root_id.as_deref().unwrap_or("another one")))
        } else {
            None
        };
        if let Some(why) = why {
            let mut st = read_state(&dir).unwrap_or_default();
            st.state = "obsolete".into();
            st.reason = why;
            let _ = write_state(&dir, &st);
            gone.push(id);
        }
    }
    if !gone.is_empty() {
        let keep: Vec<String> = read_active(env.data()).into_iter().filter(|i| !gone.contains(i)).collect();
        let _ = write_active(env.data(), &keep);
    }
    gone
}

/// The patches, the guard and the log lines of the last start.
pub fn list(env: &Env) -> Value {
    refresh_obsolete(env);
    let data = env.data();
    let active = read_active(data);
    let in_boot = booted(data);
    let boot_now = env.live.boot_id();
    let mut items = Vec::new();
    for e in std::fs::read_dir(patches_dir(data)).into_iter().flatten().flatten() {
        let id = e.file_name().to_string_lossy().into_owned();
        let Some(m) = read_manifest(&e.path()) else { continue };
        let st = read_state(&e.path()).unwrap_or_default();
        let is_active = active.contains(&id) && st.state == "active";
        let live = is_active && !st.live_boot.is_empty() && st.live_boot == boot_now;
        let status = match (st.state.as_str(), is_active) {
            ("obsolete", _) => "obsolete",
            ("removed", _) => "removing",
            (_, true) if live => "live",
            (_, true) if in_boot.contains(&id) => "active",
            (_, true) => "next start",
            _ => "inactive",
        };
        items.push(json!({ "id": id, "title": m.title, "status": status, "files": m.ops.len(), "base": m.base, "by": st.by, "applied_at": st.applied_at, "reason": st.reason, "needs_reboot": m.needs_reboot() || (is_active && !live && !in_boot.contains(&id)), "priority": active.iter().position(|a| *a == id) }));
    }
    items.sort_by_key(|v| v["priority"].as_u64().unwrap_or(u64::MAX));
    let d = patches_dir(data);
    json!({
        "patches": items,
        "guard": { "disabled": std::fs::read_to_string(d.join("disabled")).ok(), "boot_count": iyakki_stage1::parse_count(&std::fs::read_to_string(d.join("boot-count")).unwrap_or_default()), "max": iyakki_stage1::PATCH_GUARD_MAX },
        "skipped_at_boot": std::fs::read_to_string(d.join("skipped")).ok(),
        "last_failure": std::fs::read_to_string(d.join("last-failure")).ok(),
    })
}

/// The running system is up: the start is good. Clears the start counter, deletes patches waiting to be removed and the staging area.
/// Called once the system has come up (the first stage counts every start that begins with patches).
pub fn boot_ok(env: &Env) {
    let data = env.data();
    let d = patches_dir(data);
    let _ = std::fs::remove_file(d.join("boot-count"));
    let in_boot = booted(data);
    for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
        let id = e.file_name().to_string_lossy().into_owned();
        if read_state(&e.path()).is_some_and(|s| s.state == "removed") && !in_boot.contains(&id) {
            let _ = std::fs::remove_dir_all(e.path());
        }
        if id.starts_with(".new-") {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let _ = std::fs::remove_dir_all(data.join("iyakki/patch-staging"));
    refresh_obsolete(env);
}

/// The entry point for the running system: it has been up long enough, so this start counts as good ([`boot_ok`] with the real effects).
pub fn mark_boot_good(data: &Path) {
    if !patches_dir(data).is_dir() {
        return;
    }
    let dm = DevMode::open(data);
    let release = std::fs::read_to_string("/etc/iyakki-release").unwrap_or_default();
    boot_ok(&Env { dm: &dm, live: &SysLive, root: PathBuf::from("/"), release, wide: false, data_free: &|| u64::MAX });
}

/// Full-boot installs (no first stage): copy the active layers, lowest priority first, over an unpacked root. Deletions come from
/// `layer.txt`. A layer that does not verify is skipped, like at the first stage.
pub fn overlay_into(data: &Path, root: &Path) -> Vec<(String, String)> {
    let mut skipped = Vec::new();
    let root_id = current_root_id(data);
    let ids = read_active(data);
    let read = |p: &str| std::fs::read_to_string(data.join(p)).ok();
    let stat = |p: &str| std::fs::symlink_metadata(data.join(p)).ok().map(|m| m.len());
    let plan = iyakki_stage1::plan_patches(&ids.join("\n"), 0, root_id.as_deref(), &read, &stat);
    skipped.extend(plan.skipped.clone());
    for layer in plan.layers.iter().rev() {
        let ldir = data.join(layer);
        let del: Vec<String> = std::fs::read_to_string(ldir.with_file_name("layer.txt")).ok().and_then(|t| iyakki_stage1::parse_layer_info(&t)).map(|i| i.entries.into_iter().filter(|e| e.0 == "del").map(|e| e.2).collect()).unwrap_or_default();
        if let Err(e) = copy_tree(&ldir, &ldir, root, &del) {
            skipped.push((layer.clone(), e));
        }
    }
    skipped
}

fn copy_tree(base: &Path, dir: &Path, root: &Path, del: &[String]) -> Result<(), String> {
    for e in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let p = e.path();
        let rel = p.strip_prefix(base).map_err(|e| e.to_string())?;
        let to = root.join(rel);
        let md = std::fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
        let rel_s = format!("/{}", rel.display());
        if del.contains(&rel_s) {
            let _ = std::fs::remove_file(&to);
        } else if md.is_dir() {
            std::fs::create_dir_all(&to).map_err(|e| e.to_string())?;
            copy_tree(base, &p, root, del)?;
        } else if md.file_type().is_symlink() {
            let _ = std::fs::remove_file(&to);
            std::os::unix::fs::symlink(std::fs::read_link(&p).map_err(|e| e.to_string())?, &to).map_err(|e| e.to_string())?;
        } else {
            let _ = std::fs::remove_file(&to);
            std::fs::copy(&p, &to).map_err(|e| format!("{}: {e}", to.display()))?;
        }
    }
    for d in del {
        let _ = std::fs::remove_file(root.join(d.trim_start_matches('/')));
    }
    Ok(())
}
