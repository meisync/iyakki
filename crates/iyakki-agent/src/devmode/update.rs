//! `update` scope: replace the files of the BOOT partition (kernel, initramfs, boot menu, theme, EFI loader) with a newer IYAKKI,
//! from a payload made by `iyakki dev update-pack IMAGE` (`iyakki dev update PAYLOAD`), WITHOUT touching the data partition's contents
//! except two folders of its own: `boot-next/` (the staged files) and `boot-prev/` (the version being replaced).
//!
//! **The boot partition has no room for two copies** (256 MB, the initramfs alone is about 180 MB; the PC image's 512 MB volume is
//! the reason it exists), so the design is:
//!
//! 1. the payload is **staged on the data partition** (`boot-next/`) and every file is checked against its SHA-256;
//! 2. the **previous kernel and initramfs are copied to the data partition** (`boot-prev/<id>/`) and checked, BEFORE anything on
//!    the boot partition changes; the boot menu gets an entry **IYAKKI (previous version)** that loads them from there
//!    (`search --label IYAKKI-DATA`), so it does not depend on the boot files at all;
//! 3. that entry is added to the CURRENT `grub.cfg` first (a tiny file), then a marker `boot/UPDATING` is written (and mirrored in
//!    `boot-next/UPDATING.json` on the data partition, which the shell reads at start: an interrupted update is told, not guessed);
//! 4. the files are replaced: initramfs first, kernel next, the rest, `grub.cfg` last. A file that fits beside the old one is
//!    written as a new file and renamed over it (atomic); the one that does not (the initramfs) is overwritten in place. Plain
//!    buffered writes with one fsync per file: never a synchronous mount (that ran at 47 KB/s on a USB 3 drive);
//! 5. every file is read back and compared, the marker is removed, the staged files are deleted.
//!
//! **A lean install** (`boot/root.sfs`, a 1 MB initramfs, docs/BOOT-SPEED.md): the system image in the payload never goes to the boot
//! partition. It is written to the data partition as `iyakki/root/<version>-<sha>/root.sfs`, checked, and made the root by the pointer
//! file `iyakki/root/current` (the old pointer is kept as `previous`); the first stage reads the pointers at every start. The kernel and the
//! small initramfs are replaced as above, and the boot menu's previous-version entry starts the old kernel with `iyakki.root=previous`,
//! which makes the first stage take the root the update replaced (or the seed `boot/root.sfs` of the install when there was none). The seed
//! is never touched, so it stays the last resort (`iyakki.root=boot`). Older roots are removed once the new one is in place.
//!
//! An update is refused when the variant differs from the running one (macweb vs pc) unless forced, when the image is older
//! unless forced, when the Secure Boot signing state would change (never forced: reinstall instead), when the data volume is only
//! RAM (the previous version could not be kept), when the data partition cannot hold the copy, and when the NEW files grow beyond
//! what the boot partition has free (`new total - old total <= free`).

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtOrd};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::files::{self, Sandbox};
use super::DevMode;

pub const MANIFEST: &str = "update.json";
pub const FORMAT: u32 = 1;
pub const MAX_FILES: usize = 400;
pub const MAX_TOTAL: u64 = 1 << 30;
/// Room kept on the data partition beyond the copies.
pub const DATA_MARGIN: u64 = 32 << 20;
pub const MARKER: &str = "UPDATING";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    /// `boot/bzImage`, `EFI/BOOT/bootx64.efi`...
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub format: u32,
    pub version: String,
    pub variant: String,
    #[serde(default)]
    pub built: String,
    #[serde(default)]
    pub source: String,
    /// The EFI loader is the signed shim chain (Secure Boot).
    pub secure_boot: bool,
    pub files: Vec<FileEntry>,
}

// -------------------------------------------------------------------------------------------- names and versions

fn clean_component(c: &str) -> bool {
    !c.is_empty() && c.len() <= 100 && c != "." && c != ".." && c.chars().all(|ch| ch.is_ascii_alphanumeric() || " ._-+~@()".contains(ch))
}

/// Is this a path an update may carry: under `boot/` or `EFI/`, clean components, at most eight deep.
pub fn valid_path(p: &str) -> bool {
    let c: Vec<&str> = p.split('/').collect();
    c.len() >= 2 && c.len() <= 8 && matches!(c[0], "boot" | "EFI") && c.iter().all(|x| clean_component(x))
}

fn key(p: &str) -> String {
    p.to_ascii_lowercase()
}

const REQUIRED: [&str; 2] = ["boot/bzimage", "boot/grub/grub.cfg"];

/// The first-stage RAM disk of a lean install (about 1 MB); a payload carries it instead of, or beside, the full `boot/initrd`.
pub const INITRD_LEAN: &str = "boot/initrd-lean";

/// The RAM disk the boot menu reads on this boot volume: the small one when it is there (the lean block of `grub.cfg` prefers it), else the full one.
fn running_initrd(root: &Path) -> &'static str {
    if find_ci(root, INITRD_LEAN).is_some() { "boot/initrd-lean" } else { "boot/initrd" }
}

/// The system image of a lean install.
pub const ROOT_IMAGE: &str = "boot/root.sfs";

fn is_root_image(p: &str) -> bool {
    key(p) == ROOT_IMAGE
}

/// The folder of the data volume that holds the roots of updates (`iyakki/root`).
fn roots_dir(data: &Path) -> PathBuf {
    data.join(iyakki_stage1::ROOTS_DIR)
}

/// The name of the root folder an update makes: the version and the start of the image's SHA-256, so a rebuild of the same version is
/// another root and a repeated update is recognised.
pub fn root_id(m: &Manifest) -> Option<String> {
    let f = m.files.iter().find(|f| is_root_image(&f.path))?;
    let id = format!("{}-{}", sanitize_id(&m.version), f.sha256.get(..8)?);
    iyakki_stage1::valid_id(&id).then_some(id)
}

fn read_pointer(data: &Path, name: &str) -> Option<iyakki_stage1::Pointer> {
    iyakki_stage1::Pointer::parse(&std::fs::read_to_string(roots_dir(data).join(name)).ok()?)
}

/// Is this root already in place and the one that starts (an update repeated, or sent twice)?
fn root_in_place(data: &Path, m: &Manifest) -> bool {
    let (Some(id), Some(f)) = (root_id(m), m.files.iter().find(|f| is_root_image(&f.path))) else { return false };
    read_pointer(data, "current").is_some_and(|c| c.id == id) && std::fs::metadata(roots_dir(data).join(&id).join("root.sfs")).is_ok_and(|md| md.is_file() && md.len() == f.size)
}

/// Write a small file so that a power cut leaves the old one or the new one: a temporary file, fsync, rename, fsync of the folder.
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_file_name(format!("{}.new", path.file_name().unwrap_or_default().to_string_lossy()));
    let mut w = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    w.write_all(text.as_bytes()).and_then(|_| w.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
    sync_dir(path);
    Ok(())
}

/// Split a version into numbers and a pre-release rank: `0.1.0-alpha.1` < `0.1.0-beta.1` < `0.1.0-rc.1` < `0.1.0`.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    fn parts(v: &str) -> (Vec<u64>, (u8, Vec<u64>)) {
        let v = v.trim().trim_start_matches('v');
        let (core, pre) = v.split_once('-').unwrap_or((v, ""));
        let nums = core.split('.').map(|x| x.parse().unwrap_or(0)).collect();
        let rank = if pre.is_empty() {
            (u8::MAX, vec![])
        } else {
            let word = pre.split(['.', '-']).next().unwrap_or("");
            let r = match word {
                "alpha" | "a" => 1,
                "beta" | "b" => 2,
                "rc" => 3,
                _ => 0,
            };
            (r, pre.split(['.', '-']).filter_map(|x| x.parse().ok()).collect())
        };
        (nums, rank)
    }
    let (na, ra) = parts(a);
    let (nb, rb) = parts(b);
    let n = na.len().max(nb.len());
    for i in 0..n {
        let (x, y) = (na.get(i).copied().unwrap_or(0), nb.get(i).copied().unwrap_or(0));
        if x != y {
            return x.cmp(&y);
        }
    }
    ra.cmp(&rb)
}

// -------------------------------------------------------------------------------------------- the payload

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Make the manifest of a folder of extracted boot files. `release` is the image's `boot/iyakki-release` (KEY=value lines) when it
/// has one; `version`/`variant` override it (needed for an image without that file).
pub fn build_manifest(dir: &Path, files: &[(String, u64, String)], version: Option<&str>, variant: Option<&str>, source: &str) -> Result<Manifest, String> {
    let rel_text = std::fs::read_to_string(dir.join("boot/iyakki-release")).ok();
    let rel = rel_text.as_deref().map(crate::relinfo::ReleaseInfo::parse);
    let version = version.map(str::to_string).or_else(|| rel.as_ref().map(|r| r.version.clone()).filter(|v| !v.is_empty())).ok_or("this image does not say which IYAKKI version it is (no boot/iyakki-release): pass --version")?;
    let variant = variant.map(str::to_string).or_else(|| rel.as_ref().map(|r| r.variant.clone()).filter(|v| !v.is_empty())).ok_or("this image does not say its variant (no boot/iyakki-release): pass --variant (pc, mac, web, macweb...)")?;
    let list: Vec<FileEntry> = files.iter().map(|(p, s, h)| FileEntry { path: p.clone(), size: *s, sha256: h.clone() }).collect();
    let lower: Vec<String> = list.iter().map(|f| key(&f.path)).collect();
    for r in REQUIRED {
        if !lower.iter().any(|p| p == r) {
            return Err(format!("the image has no {r}: it is not an IYAKKI boot partition"));
        }
    }
    if !lower.iter().any(|p| p == "boot/initrd" || p == INITRD_LEAN) {
        return Err("the image has no boot/initrd (and no boot/initrd-lean): it is not an IYAKKI boot partition".into());
    }
    let secure_boot = lower.iter().any(|p| p == "efi/boot/mmx64.efi" || p == "efi/boot/grubx64.efi");
    Ok(Manifest { format: FORMAT, version, variant, built: rel.as_ref().map(|r| r.build_date.clone()).unwrap_or_default(), source: source.to_string(), secure_boot, files: list })
}

pub fn write_manifest(dir: &Path, m: &Manifest) -> Result<(), String> {
    std::fs::write(dir.join(MANIFEST), serde_json::to_vec_pretty(m).unwrap_or_default()).map_err(|e| e.to_string())
}

/// Check a manifest on its own terms: format, paths, sizes, the files every update needs, no duplicates.
pub fn check_manifest(m: &Manifest) -> Result<(), String> {
    if m.format != FORMAT {
        return Err(format!("update format {} is not understood (this device reads format {FORMAT})", m.format));
    }
    if m.files.is_empty() || m.files.len() > MAX_FILES {
        return Err(format!("an update carries 1 to {MAX_FILES} files"));
    }
    if m.version.len() > 64 || m.version.is_empty() || m.version.chars().any(|c| !(c.is_ascii_alphanumeric() || ".-+_".contains(c))) {
        return Err("the version is not valid".into());
    }
    if m.variant.is_empty() || m.variant.len() > 32 || !m.variant.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("the variant is not valid".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut total = 0u64;
    for f in &m.files {
        if !valid_path(&f.path) {
            return Err(format!("{:?} is not a path an update may write (boot/... or EFI/..., plain names)", f.path));
        }
        if !seen.insert(key(&f.path)) {
            return Err(format!("{} is listed twice", f.path));
        }
        if f.sha256.len() != 64 || !f.sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("{} has no valid SHA-256", f.path));
        }
        total += f.size;
    }
    if total > MAX_TOTAL {
        return Err(format!("the files add up to {} MB: more than an update carries", total >> 20));
    }
    for r in REQUIRED {
        if !seen.contains(r) {
            return Err(format!("the image has no {r}: it is not an IYAKKI boot partition"));
        }
    }
    if !seen.contains("boot/initrd") && !seen.contains(INITRD_LEAN) {
        return Err("the image has no boot/initrd (and no boot/initrd-lean): it is not an IYAKKI boot partition".into());
    }
    Ok(())
}

/// Check a payload folder against its manifest: every file there, the right size, the right SHA-256.
pub fn verify_payload(dir: &Path, m: &Manifest) -> Result<(), String> {
    check_manifest(m)?;
    for f in &m.files {
        let p = dir.join(&f.path);
        let meta = std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", f.path))?;
        if !meta.is_file() || meta.len() != f.size {
            return Err(format!("{} is not the size the manifest says", f.path));
        }
        if files::sha256_file(&p)? != f.sha256 {
            return Err(format!("{} does not match its SHA-256: the payload is damaged", f.path));
        }
    }
    Ok(())
}

pub fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let t = std::fs::read(dir.join(MANIFEST)).map_err(|e| format!("{}: {e} (make a payload with `iyakki dev update-pack IMAGE -o DIR`)", dir.join(MANIFEST).display()))?;
    serde_json::from_slice(&t).map_err(|e| format!("{MANIFEST}: {e}"))
}

// -------------------------------------------------------------------------------------------- the boot partition

/// The boot partition made available as a folder. Dropping it puts the mount back as it was.
pub struct BootVol {
    pub root: PathBuf,
    /// Bytes free, and bytes the volume holds in all.
    pub free: u64,
    pub total: u64,
    /// Bytes per cluster (every file takes whole clusters).
    pub cluster: u64,
    restore: Option<Box<dyn FnOnce() + Send>>,
}

impl BootVol {
    pub fn new(root: PathBuf, free: u64, total: u64, cluster: u64) -> BootVol {
        BootVol { root, free, total, cluster: cluster.max(512), restore: None }
    }

    pub fn on_drop(mut self, f: Box<dyn FnOnce() + Send>) -> BootVol {
        self.restore = Some(f);
        self
    }
}

impl Drop for BootVol {
    fn drop(&mut self) {
        if let Some(f) = self.restore.take() {
            f();
        }
    }
}

/// How the boot partition is reached. The device implements it with a real mount; tests with a folder.
pub trait BootMount: Send + Sync {
    fn open(&self, write: bool) -> Result<BootVol, String>;
    /// Bytes free right now (changes as files are written).
    fn free(&self, vol: &BootVol) -> u64;
}

/// Bytes a file of `n` bytes takes on a volume with clusters of `c`.
pub fn alloc(n: u64, c: u64) -> u64 {
    n.div_ceil(c.max(1)) * c.max(1)
}

/// A boot partition that is a folder with a size limit (tests, and a mounted partition seen through its mount point).
pub struct DirBoot {
    pub root: PathBuf,
    pub capacity: u64,
    pub cluster: u64,
}

fn tree_alloc(dir: &Path, c: u64) -> u64 {
    let mut n = 0;
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        match e.metadata() {
            Ok(m) if m.is_dir() => n += c + tree_alloc(&p, c),
            Ok(m) => n += alloc(m.len(), c),
            _ => {}
        }
    }
    n
}

impl BootMount for DirBoot {
    fn open(&self, _write: bool) -> Result<BootVol, String> {
        let used = tree_alloc(&self.root, self.cluster);
        Ok(BootVol::new(self.root.clone(), self.capacity.saturating_sub(used), self.capacity, self.cluster))
    }

    fn free(&self, vol: &BootVol) -> u64 {
        self.capacity.saturating_sub(tree_alloc(&vol.root, vol.cluster))
    }
}

/// Find a file under `root` by a path that matches in any letter case (FAT does not care); returns the real path.
pub fn find_ci(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut cur = root.to_path_buf();
    for comp in rel.split('/') {
        let want = comp.to_ascii_lowercase();
        let hit = std::fs::read_dir(&cur).ok()?.flatten().find(|e| e.file_name().to_string_lossy().to_ascii_lowercase() == want)?;
        cur = hit.path();
    }
    Some(cur)
}

// -------------------------------------------------------------------------------------------- what runs now

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Running {
    pub version: String,
    pub variant: String,
    pub built: String,
    /// The EFI loader on the boot partition is the signed shim chain.
    pub secure_boot: bool,
    /// The version the boot partition's files are NOW (an update puts `boot/iyakki-release` there): newer than the running one
    /// after an update that has not been restarted into yet. Empty when the partition has no such file.
    pub installed: String,
}

impl Running {
    /// The version the files on the boot partition are, for comparing with an update and for naming the saved copy.
    pub fn on_disk(&self) -> &str {
        if self.installed.is_empty() { &self.version } else { &self.installed }
    }
}

pub fn running_from(release_text: &str, boot_root: &Path) -> Running {
    let r = crate::relinfo::ReleaseInfo::parse(release_text);
    let secure_boot = find_ci(boot_root, "EFI/BOOT/mmx64.efi").is_some() || find_ci(boot_root, "EFI/BOOT/grubx64.efi").is_some();
    let installed = find_ci(boot_root, "boot/iyakki-release").and_then(|p| std::fs::read_to_string(p).ok()).map(|t| crate::relinfo::ReleaseInfo::parse(&t).version).unwrap_or_default();
    Running { version: if r.version.is_empty() { crate::relinfo::VERSION.to_string() } else { r.version }, variant: r.variant, built: r.build_date, secure_boot, installed }
}

pub struct Env<'a> {
    pub dm: &'a DevMode,
    pub boot: &'a dyn BootMount,
    /// The text of `/etc/iyakki-release`.
    pub release: String,
    /// The data folder is only RAM (no IYAKKI-DATA volume): nothing survives a restart, so the previous version cannot be kept.
    pub volatile: bool,
    /// Bytes free on the data partition.
    pub data_free: &'a dyn Fn() -> u64,
    /// Test knob: stop with an error after this many bytes were written to the boot partition (a power cut). None on a device.
    pub fault_after: Option<&'a AtomicU64>,
}

impl Env<'_> {
    fn staging(&self) -> PathBuf {
        self.dm.data_dir().join("boot-next")
    }

    fn prev_root(&self) -> PathBuf {
        self.dm.data_dir().join("boot-prev")
    }
}

// -------------------------------------------------------------------------------------------- the plan

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub from: String,
    pub to: String,
    /// Files that differ and must be staged.
    pub needs: Vec<String>,
    pub same: usize,
    pub growth: i64,
    pub free: u64,
    pub notes: Vec<String>,
}

fn existing_hash(root: &Path, rel: &str) -> Option<(PathBuf, u64, String)> {
    let p = find_ci(root, rel)?;
    let m = std::fs::metadata(&p).ok().filter(|m| m.is_file())?;
    Some((p.clone(), m.len(), files::sha256_file(&p).ok()?))
}

/// Check everything that can be checked before a byte moves. `force` accepts a different variant and an older version only.
pub fn plan(env: &Env, m: &Manifest, force: bool) -> Result<Plan, String> {
    check_manifest(m)?;
    if env.volatile {
        return Err("this device has no persistent IYAKKI-DATA volume (the session is kept in RAM), so the previous version could not be kept for the boot menu: install IYAKKI to the disk first".into());
    }
    let vol = env.boot.open(false)?;
    let run = running_from(&env.release, &vol.root);
    let mut notes = Vec::new();
    if run.variant.is_empty() {
        if !force {
            return Err("this device does not say which image variant it runs (no /etc/iyakki-release), so an update cannot be checked against it; --force accepts that".into());
        }
        notes.push("the running variant is unknown (forced)".into());
    } else if run.variant != m.variant {
        if !force {
            return Err(format!("this update is for the {} image and this device runs the {} image: they carry different kernels and drivers (--force to install it anyway)", m.variant, run.variant));
        }
        notes.push(format!("variant changes from {} to {} (forced)", run.variant, m.variant));
    }
    match compare_versions(&m.version, run.on_disk()) {
        Ordering::Less => {
            if !force {
                return Err(format!("{} is older than the {} on this device: a downgrade needs --force", m.version, run.on_disk()));
            }
            notes.push(format!("downgrade from {} to {} (forced)", run.on_disk(), m.version));
        }
        Ordering::Equal if !run.built.is_empty() && !m.built.is_empty() && m.built < run.built && !force => {
            return Err(format!("this build ({}) is older than the one that runs now ({}): --force to install it", m.built, run.built));
        }
        _ => {}
    }
    if m.secure_boot != run.secure_boot {
        return Err(format!("the Secure Boot signing would change ({} now, {} in the update): a different boot chain cannot be swapped in place, reinstall from the image instead", if run.secure_boot { "signed" } else { "unsigned" }, if m.secure_boot { "signed" } else { "unsigned" }));
    }
    let mut needs = Vec::new();
    let (mut same, mut new_alloc, mut old_alloc) = (0usize, 0u64, 0u64);
    let mut notes_root = false;
    for f in &m.files {
        // the system image of a lean install goes to the data partition, never to the boot partition (docs/BOOT-SPEED.md)
        if is_root_image(&f.path) {
            notes_root = true;
            let seed_same = existing_hash(&vol.root, &f.path).is_some_and(|(_, _, h)| h == f.sha256) && read_pointer(env.dm.data_dir(), "current").is_none();
            if seed_same || root_in_place(env.dm.data_dir(), m) {
                same += 1;
            } else {
                needs.push(f.path.clone());
            }
            continue;
        }
        let cfg = key(&f.path) == "boot/grub/grub.cfg";
        match existing_hash(&vol.root, &f.path) {
            // grub.cfg is rewritten with the previous-version entry, so it always counts as changed
            Some((_, _, h)) if h == f.sha256 && !cfg => same += 1,
            Some((_, sz, _)) => {
                needs.push(f.path.clone());
                old_alloc += alloc(sz, vol.cluster);
                new_alloc += alloc(f.size + if cfg { 2048 } else { 0 }, vol.cluster);
            }
            None => {
                needs.push(f.path.clone());
                new_alloc += alloc(f.size, vol.cluster);
            }
        }
    }
    let growth = new_alloc as i64 - old_alloc as i64;
    // the marker file and directory entries need a cluster or two
    if growth + 2 * vol.cluster as i64 > vol.free as i64 {
        return Err(format!(
            "the new files need {} MB more room than the old ones leave: the boot partition has {} MB free of {} MB (new files {} MB, old files they replace {} MB){}",
            (growth.max(0) as u64 + 2 * vol.cluster).saturating_sub(vol.free).div_ceil(1 << 20),
            vol.free >> 20,
            vol.total >> 20,
            new_alloc >> 20,
            old_alloc >> 20,
            if vol.total < (400 << 20) { "; the PC image's 512 MB boot volume has the room, a 256 MB one does not" } else { "" }
        ));
    }
    if needs.is_empty() {
        notes.push("this device already has these boot files".into());
    }
    if notes_root && needs.iter().any(|n| is_root_image(n)) {
        notes.push("the system image goes to the data partition (iyakki/root), the boot partition keeps its seed image".into());
    }
    // the data partition: the previous kernel and initramfs, and the staged files
    let prev_bytes: u64 = ["boot/bzImage", running_initrd(&vol.root)].iter().filter_map(|r| existing_hash(&vol.root, r)).map(|(_, s, _)| s).sum();
    let staged: u64 = needs.iter().filter_map(|n| m.files.iter().find(|f| &f.path == n)).map(|f| f.size).sum();
    let free = (env.data_free)();
    let want = prev_bytes + staged + DATA_MARGIN;
    if free < want {
        return Err(format!("the data partition has {} MB free and needs {} MB for the staged files and the copy of the version being replaced", free >> 20, want >> 20));
    }
    Ok(Plan { from: run.on_disk().to_string(), to: m.version.clone(), needs, same, growth, free: vol.free, notes })
}

// -------------------------------------------------------------------------------------------- staging

fn staging_sandbox(env: &Env) -> Result<Sandbox, String> {
    let s = env.staging();
    std::fs::create_dir_all(&s).map_err(|e| format!("{}: {e}", s.display()))?;
    let real = std::fs::canonicalize(&s).map_err(|e| e.to_string())?;
    Ok(Sandbox::with_roots(&real, &[real.clone()], &[], &[]))
}

/// One chunk of a staged file. The path and the SHA-256 must be the ones in the manifest.
pub fn stage_chunk(env: &Env, m: &Manifest, path: &str, offset: u64, chunk: &[u8]) -> Result<Value, (u16, Value)> {
    let f = m.files.iter().find(|f| f.path == path).ok_or((422, json!({ "error": format!("{path} is not in the update") })))?;
    let sb = staging_sandbox(env).map_err(|e| (500, json!({ "error": e })))?;
    // the secret-name rule of the file sandbox must not hide boot files (`*.key`...): the allowed names are the manifest's
    files::put(&sb, path, offset, f.size, &f.sha256, true, None, chunk)
}

pub fn staged_path(env: &Env, rel: &str) -> PathBuf {
    env.staging().join(rel)
}

// -------------------------------------------------------------------------------------------- the boot menu entry

/// The arguments of the normal entry's `linux` line (everything after the kernel path).
fn normal_args(cfg: &str) -> Option<String> {
    let start = cfg.find("menuentry \"IYAKKI\"")?;
    let block = &cfg[start..];
    let block = &block[..block.find("\n}").unwrap_or(block.len())];
    let line = block.lines().find(|l| l.trim_start().starts_with("linux "))?;
    let mut it = line.trim().split_whitespace();
    it.next()?;
    it.next()?;
    Some(it.collect::<Vec<_>>().join(" "))
}

pub const PREV_TITLE: &str = "IYAKKI (previous version)";

/// `cfg` with a menu entry that starts the version saved in `boot-prev/<id>` on the data partition, right after the normal entry.
/// An entry of an earlier update is replaced.
pub fn with_previous_entry(cfg: &str, id: &str, version: &str) -> Result<String, String> {
    with_previous_entry_args(cfg, id, version, "")
}

/// [`with_previous_entry`], with `extra` added to the command line (`iyakki.root=previous` after an update that changed the system image).
pub fn with_previous_entry_args(cfg: &str, id: &str, version: &str, extra: &str) -> Result<String, String> {
    let mut args = normal_args(cfg).ok_or("the boot menu has no normal IYAKKI entry to copy the command line from")?;
    if !extra.is_empty() {
        args = format!("{args} {extra}");
    }
    let mut text = cfg.to_string();
    if let Some(i) = text.find(&format!("menuentry \"{PREV_TITLE}")) {
        // drop the old entry (and the comment line above it)
        let end = text[i..].find("\n}\n").map_or(text.len(), |e| i + e + 3);
        let from = text[..i].rfind("\n# previous version").map_or(i, |c| c + 1);
        text.replace_range(from..end, "");
    }
    let entry = format!(
        "# previous version: the kernel and initramfs saved on the data partition by the last system update\nmenuentry \"{PREV_TITLE} {version}\" {{\n\tsearch --no-floppy --label IYAKKI-DATA --set=iyakki_dp\n\techo \"  Loading the previous IYAKKI from the data partition.\"\n\tlinux ($iyakki_dp)/boot-prev/{id}/bzImage {args}\n\tinitrd ($iyakki_dp)/boot-prev/{id}/initrd\n}}\n"
    );
    let first = text.find("menuentry \"IYAKKI\"").ok_or("no normal entry")?;
    let close = text[first..].find("\n}\n").map(|c| first + c + 3).ok_or("the normal entry is not closed")?;
    text.insert_str(close, &format!("\n{entry}"));
    Ok(text)
}

// -------------------------------------------------------------------------------------------- applying

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Marker {
    pub from: String,
    pub to: String,
    pub started: i64,
    pub prev_id: String,
    pub files: Vec<String>,
}

fn marker_mirror(data: &Path) -> PathBuf {
    data.join("boot-next").join("UPDATING.json")
}

/// A system update that did not finish (read from the data partition: it is always there to read).
pub fn interrupted(data: &Path) -> Option<Marker> {
    serde_json::from_slice(&std::fs::read(marker_mirror(data)).ok()?).ok()
}

/// One sentence for the person, or None.
pub fn interrupted_text(data: &Path) -> Option<String> {
    interrupted(data).map(|m| format!("A system update from {} to {} did not finish. Choose \"{PREV_TITLE}\" in the boot menu, then run `iyakki dev update` again.", m.from, m.to))
}

fn spend(env: &Env, n: usize) -> Result<(), String> {
    if let Some(left) = env.fault_after {
        let cur = left.load(AtOrd::SeqCst);
        if (n as u64) > cur {
            left.store(0, AtOrd::SeqCst);
            return Err("the power was cut here (test)".into());
        }
        left.store(cur - n as u64, AtOrd::SeqCst);
    }
    Ok(())
}

/// Copy `src` to `dst` in chunks (buffered, one fsync at the end).
fn copy_file(env: &Env, src: &Path, dst: &Path) -> Result<(), String> {
    let mut r = std::fs::File::open(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let mut w = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = r.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        spend(env, n)?;
        w.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", dst.display()))?;
    }
    w.sync_all().map_err(|e| format!("{}: {e}", dst.display()))
}

fn write_bytes(env: &Env, dst: &Path, data: &[u8]) -> Result<(), String> {
    spend(env, data.len())?;
    let mut w = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    w.write_all(data).map_err(|e| format!("{}: {e}", dst.display()))?;
    w.sync_all().map_err(|e| format!("{}: {e}", dst.display()))
}

fn sync_dir(p: &Path) {
    if let Some(d) = p.parent() {
        if let Ok(f) = std::fs::File::open(d) {
            let _ = f.sync_all();
        }
    }
}

/// Put `data` (a file of the payload, or the generated `grub.cfg`) at `rel` on the boot partition.
fn replace_file(env: &Env, vol: &BootVol, rel: &str, from: Either<'_>) -> Result<(), String> {
    let size = match &from {
        Either::File(p) => std::fs::metadata(p).map_err(|e| e.to_string())?.len(),
        Either::Bytes(b) => b.len() as u64,
    };
    let dest = find_ci(&vol.root, rel).unwrap_or_else(|| vol.root.join(rel));
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let old = std::fs::metadata(&dest).ok().map(|m| alloc(m.len(), vol.cluster)).unwrap_or(0);
    let free = env.boot.free(vol);
    let put = |to: &Path| match &from {
        Either::File(p) => copy_file(env, p, to),
        Either::Bytes(b) => write_bytes(env, to, b),
    };
    // beside the old file when it fits (atomic rename), else over it
    if dest.exists() && free >= alloc(size, vol.cluster) + 2 * vol.cluster {
        let tmp = dest.with_file_name(format!("{}.upd", dest.file_name().unwrap_or_default().to_string_lossy()));
        put(&tmp)?;
        std::fs::rename(&tmp, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    } else {
        let _ = old;
        put(&dest)?;
    }
    sync_dir(&dest);
    Ok(())
}

enum Either<'a> {
    File(&'a Path),
    Bytes(&'a [u8]),
}

fn read_back(vol: &BootVol, rel: &str, want: &str) -> Result<(), String> {
    let p = find_ci(&vol.root, rel).ok_or_else(|| format!("{rel} is missing after the copy"))?;
    if files::sha256_file(&p)? != want {
        return Err(format!("{rel} reads back different from what was written"));
    }
    Ok(())
}

fn sanitize_id(v: &str) -> String {
    v.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect()
}

/// Which prev folder the boot menu entry points at now (the newest one).
fn latest_prev(env: &Env) -> Option<(String, String)> {
    let t = std::fs::read_to_string(env.prev_root().join("latest")).ok()?;
    let (id, version) = t.trim().split_once('\n')?;
    env.prev_root().join(id).join("bzImage").is_file().then(|| (id.to_string(), version.to_string()))
}

/// Put the staged system image on the data partition and make it the root: the image moves into `iyakki/root/<id>/` (the staging folder is
/// on the same volume, so nothing is copied), its size and SHA-256 are checked, the old `current` becomes `previous`, then `current` is
/// switched through a temporary file. A power cut leaves the old pointer (the old root starts) or the new one (the image is complete).
fn install_root(env: &Env, m: &Manifest) -> Result<(), String> {
    let f = m.files.iter().find(|f| is_root_image(&f.path)).ok_or("internal: the update has no system image")?;
    let id = root_id(m).ok_or("the system image has no usable id")?;
    let dir = roots_dir(env.dm.data_dir());
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = dir.join(format!("{id}.tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let (src, dst) = (env.staging().join(&f.path), tmp.join("root.sfs"));
    if std::fs::rename(&src, &dst).is_err() {
        std::fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    }
    std::fs::File::open(&dst).and_then(|h| h.sync_all()).map_err(|e| e.to_string())?;
    let ok = std::fs::metadata(&dst).is_ok_and(|md| md.len() == f.size) && files::sha256_file(&dst).is_ok_and(|h| h == f.sha256);
    if !ok {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("the system image does not match its SHA-256 after it was moved: nothing was changed".into());
    }
    let fin = dir.join(&id);
    if fin.exists() {
        std::fs::remove_dir_all(&fin).map_err(|e| format!("{}: {e}", fin.display()))?;
    }
    std::fs::rename(&tmp, &fin).map_err(|e| format!("{}: {e}", fin.display()))?;
    sync_dir(&fin);
    if let Some(old) = std::fs::read_to_string(dir.join("current")).ok().filter(|t| iyakki_stage1::Pointer::parse(t).is_some_and(|p| p.id != id)) {
        write_atomic(&dir.join("previous"), &old)?;
    }
    write_atomic(&dir.join("current"), &format!("{id}\nsize={}\n", f.size))?;
    // roots no pointer names any more are removed (the seed on the boot partition is not here and stays)
    let keep: Vec<String> = ["current", "previous"].iter().filter_map(|n| read_pointer(env.dm.data_dir(), n)).map(|p| p.id).collect();
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if e.path().is_dir() && !keep.contains(&name) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    Ok(())
}

/// Do the update. The caller has checked the scope; `plan` is run again here (the device decides, not the computer).
pub fn apply(env: &Env, m: &Manifest, force: bool, who: &str) -> Result<Value, String> {
    apply_with_entry(env, m, force, who, None)
}

/// [`apply`], with the boot menu's "previous version" entry pointing at `entry` (`id` of a folder in `boot-prev/`, version) instead of
/// the copy of the version being replaced. The Update centre's rollback uses it: while the files are replaced, the entry starts the
/// version being restored, so a power cut cannot leave only the version that was being left. The version being replaced is still
/// saved in `boot-prev/`.
pub fn apply_with_entry(env: &Env, m: &Manifest, force: bool, who: &str, entry: Option<(String, String)>) -> Result<Value, String> {
    let p = plan(env, m, force)?;
    let staging = env.staging();
    // 1. the staged files are the right ones
    for n in &p.needs {
        let f = m.files.iter().find(|f| &f.path == n).ok_or("internal: file missing from manifest")?;
        let sp = staging.join(n);
        let ok = std::fs::metadata(&sp).is_ok_and(|md| md.is_file() && md.len() == f.size) && files::sha256_file(&sp).is_ok_and(|h| h == f.sha256);
        if !ok {
            return Err(format!("{n} is not staged completely on the device: send it again"));
        }
    }
    if p.needs.is_empty() {
        return Ok(json!({ "done": true, "changed": 0, "message": "nothing to change: the boot files are already these" }));
    }
    let ro = env.boot.open(false)?;
    let run = running_from(&env.release, &ro.root);
    let root_new = p.needs.iter().any(|n| is_root_image(n));
    let touches_kernel = root_new || p.needs.iter().any(|n| matches!(key(n).as_str(), "boot/bzimage" | "boot/initrd" | "boot/initrd-lean"));
    // after a change of the system image the previous-version entry must start the root that was replaced, not the new one
    let extra = if root_new { "iyakki.root=previous" } else { "" };
    // 2. the version being replaced goes to the data partition, checked, before the boot partition changes
    let mut prev: Option<(String, String)> = None;
    if touches_kernel {
        let base = sanitize_id(run.on_disk());
        let mut id = base.clone();
        let mut n = 2;
        while env.prev_root().join(&id).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        let tmp = env.prev_root().join(format!("{id}.tmp"));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        for (rel, name) in [("boot/bzImage", "bzImage"), (running_initrd(&ro.root), "initrd")] {
            let (src, _, h) = existing_hash(&ro.root, rel).ok_or_else(|| format!("the boot partition has no {rel} to keep"))?;
            // the data partition is written in the ordinary way: it is not under the fault knob (the boot partition is the one at risk)
            std::fs::copy(&src, tmp.join(name)).map_err(|e| format!("keeping {rel}: {e}"))?;
            std::fs::File::open(tmp.join(name)).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
            if files::sha256_file(&tmp.join(name))? != h {
                let _ = std::fs::remove_dir_all(&tmp);
                return Err(format!("the copy of {rel} does not match the original: nothing was changed"));
            }
        }
        std::fs::write(tmp.join("prev.json"), serde_json::to_vec_pretty(&json!({ "version": run.on_disk(), "variant": run.variant, "built": run.built })).unwrap_or_default()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, env.prev_root().join(&id)).map_err(|e| e.to_string())?;
        std::fs::write(env.prev_root().join("latest"), format!("{id}\n{}", run.on_disk())).map_err(|e| e.to_string())?;
        prev = Some((id, run.on_disk().to_string()));
        env.dm.log(who, "", "dev.update.prev", &format!("kept {} in boot-prev/{}", run.on_disk(), prev.as_ref().map(|p| p.0.as_str()).unwrap_or("")), true);
    }
    let prev = prev.or_else(|| latest_prev(env));
    let entry_prev = entry.or_else(|| prev.clone());
    drop(ro);
    // 3. the menu entry for it goes into the CURRENT menu first, then the marker
    let vol = env.boot.open(true)?;
    let cfg_now = find_ci(&vol.root, "boot/grub/grub.cfg").and_then(|p| std::fs::read_to_string(p).ok());
    if let (Some((id, ver)), Some(cfg)) = (&entry_prev, &cfg_now) {
        if touches_kernel {
            let with = with_previous_entry_args(cfg, id, ver, extra)?;
            replace_file(env, &vol, "boot/grub/grub.cfg", Either::Bytes(with.as_bytes()))?;
        }
    }
    let marker = Marker { from: run.on_disk().to_string(), to: m.version.clone(), started: env.dm.now(), prev_id: prev.as_ref().map(|p| p.0.clone()).unwrap_or_default(), files: p.needs.clone() };
    let mjson = serde_json::to_vec_pretty(&marker).unwrap_or_default();
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    std::fs::write(marker_mirror(env.dm.data_dir()), &mjson).map_err(|e| e.to_string())?;
    let marker_path = vol.root.join(MARKER);
    write_bytes(env, &marker_path, &mjson)?;
    // 4a. a lean install's system image goes to the data partition and becomes the root (the kernel is replaced after it, as docs/BOOT-SPEED.md says)
    let mut root_done = false;
    if root_new {
        env.dm.log(who, "", "dev.update.file", "installing the system image on the data partition", true);
        install_root(env, m)?;
        root_done = true;
    }
    // 4. replace: initramfs, kernel, the rest, the menu last
    let mut order: Vec<String> = p.needs.iter().filter(|n| !is_root_image(n)).cloned().collect();
    let rank = |n: &str| match key(n).as_str() {
        "boot/initrd" | "boot/initrd-lean" => 0,
        "boot/bzimage" => 1,
        "boot/grub/grub.cfg" => 9,
        _ => 5,
    };
    order.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.cmp(b)));
    for n in &order {
        let f = m.files.iter().find(|f| &f.path == n).ok_or("internal: file missing from manifest")?;
        env.dm.log(who, "", "dev.update.file", &format!("writing {n} ({} bytes)", f.size), true);
        if key(n) == "boot/grub/grub.cfg" {
            let new_cfg = std::fs::read_to_string(staging.join(n)).map_err(|e| e.to_string())?;
            let out = match &entry_prev {
                Some((id, ver)) => with_previous_entry_args(&new_cfg, id, ver, extra)?,
                None => new_cfg,
            };
            replace_file(env, &vol, n, Either::Bytes(out.as_bytes()))?;
            // the menu is read back as the text that was meant
            let got = std::fs::read(find_ci(&vol.root, n).ok_or("grub.cfg is missing after the copy")?).map_err(|e| e.to_string())?;
            if sha256_hex(&got) != sha256_hex(out.as_bytes()) {
                return Err("grub.cfg reads back different from what was written".into());
            }
        } else {
            replace_file(env, &vol, n, Either::File(&staging.join(n)))?;
            read_back(&vol, n, &f.sha256)?;
        }
    }
    // 5. done: the marker goes, then the staged files
    if let Some(mp) = find_ci(&vol.root, MARKER) {
        let _ = std::fs::remove_file(&mp);
        sync_dir(&mp);
    }
    let _ = std::fs::remove_file(marker_mirror(env.dm.data_dir()));
    let _ = std::fs::remove_dir_all(&staging);
    // keep the version just saved and one older; remove the rest
    if let Some((keep, _)) = &prev {
        let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(env.prev_root()).into_iter().flatten().flatten().filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().ends_with(".tmp")).filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
        dirs.sort();
        let extra = dirs.len().saturating_sub(2);
        for (_, d) in dirs.into_iter().take(extra) {
            if d.file_name().is_some_and(|n| n.to_string_lossy() != *keep) {
                let _ = std::fs::remove_dir_all(d);
            }
        }
    }
    let changed = order.len() + usize::from(root_done);
    Ok(json!({ "done": true, "changed": changed, "from": run.on_disk(), "to": m.version, "previous_entry": prev.is_some(), "system_image": root_done, "message": format!("{changed} file(s) replaced{}: IYAKKI {} -> {}; the boot menu keeps \"{PREV_TITLE}\"", if root_done { " (the system image on the data partition)" } else { "" }, run.on_disk(), m.version) }))
}

/// For the page and the CLI: is an update unfinished, and what is kept.
pub fn status(data: &Path) -> Value {
    let prev = std::fs::read_to_string(data.join("boot-prev").join("latest")).ok().and_then(|t| t.lines().nth(1).map(str::to_string));
    let staged: usize = std::fs::read_dir(data.join("boot-next")).map(|d| d.flatten().count()).unwrap_or(0);
    json!({ "interrupted": interrupted(data), "message": interrupted_text(data), "previous_version": prev, "staged_entries": staged })
}

/// Which boot partition files exist (for `plan` output tests).
pub fn list_boot(root: &Path) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    fn walk(dir: &Path, prefix: &str, out: &mut BTreeMap<String, u64>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() { name } else { format!("{prefix}/{}", name) };
            match e.metadata() {
                Ok(m) if m.is_dir() => walk(&e.path(), &rel, out),
                Ok(m) => {
                    out.insert(rel, m.len());
                }
                _ => {}
            }
        }
    }
    walk(root, "", &mut out);
    out
}
