//! Moving an existing plain home into an encrypted one (design section 9). OPT-IN ONLY: nothing here runs unless the person asked,
//! and nothing is ever converted in place.
//!
//! Order: **plan** (what is copied, how big, space needed) -> make the container and open it at a staging folder -> **copy** -> **verify**
//! (SHA-256 of every file read back through the decrypting path, plus modes, link targets and counts) -> **switch** (the plain home is
//! renamed to `<name>.plain-before-encrypt`, the container is mounted at the home) -> the old copy stays until the person confirms its
//! removal. A failure before the switch undoes everything (the container is deleted, the plain home was never touched). A power cut at any
//! point is handled by [`recover_interrupted`].
//!
//! Honest limit, said to the person: deleting the plain copy on flash storage does not erase the old blocks; the drive may keep them
//! readable until it overwrites or trims them. A stolen drive is not protected by a migration after the fact; moving to a freshly
//! formatted drive is the strong form.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{human, EncHome, HomeError, RecoveryText, FREE_MARGIN, MIN_PAYLOAD, SECTOR};

pub const FLASH_NOTE: &str = "Deleting the old copy on a USB stick, SSD or other flash storage does not erase its old blocks: the drive may keep the plain data readable until it overwrites or trims it. If this computer or drive was already lost or stolen, encrypting afterwards protects nothing. The strong form is to copy to a freshly formatted drive and then erase the old one completely.";

const MAX_DEPTH: u32 = 64;

/// What a migration would do, found without changing anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub files: u64,
    pub dirs: u64,
    pub links: u64,
    pub bytes: u64,
    /// Sockets, pipes and device files: not copied (they are not data), listed so nothing disappears silently.
    pub skipped: Vec<String>,
    pub free: u64,
    /// The private folder's size.
    pub payload: u64,
    pub problems: Vec<String>,
}

impl Plan {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }

    /// The words shown before anything is done.
    pub fn describe(&self) -> String {
        let mut s = format!(
            "Move my existing files: {} files, {} folders and {} links, {} in all, will be COPIED into a new encrypted folder of up to {}. Every file is then checked against the original with SHA-256, and only after that does your home switch over. The old folder is kept (as \"{}\") until you agree to remove it. Free space on the drive: {}.",
            self.files,
            self.dirs,
            self.links,
            human(self.bytes),
            human(self.payload),
            format_args!("<name>{}", crate::PLAIN_SUFFIX),
            human(self.free)
        );
        if !self.skipped.is_empty() {
            s.push_str(&format!(" Not copied (sockets, pipes, devices): {}.", self.skipped.join(", ")));
        }
        for p in &self.problems {
            s.push_str(&format!(" PROBLEM: {p}"));
        }
        s.push(' ');
        s.push_str(FLASH_NOTE);
        s
    }
}

struct Walk {
    files: u64,
    dirs: u64,
    links: u64,
    bytes: u64,
    skipped: Vec<String>,
}

fn walk(dir: &Path, depth: u32, w: &mut Walk) -> Result<(), HomeError> {
    if depth > MAX_DEPTH {
        return Err(HomeError::Io(format!("the folder {} is nested too deeply", dir.display())));
    }
    for e in fs::read_dir(dir).map_err(|e| HomeError::Io(format!("{}: {e}", dir.display())))? {
        let e = e.map_err(|e| HomeError::Io(e.to_string()))?;
        let md = fs::symlink_metadata(e.path()).map_err(|e2| HomeError::Io(format!("{}: {e2}", e.path().display())))?;
        let ft = md.file_type();
        if ft.is_dir() {
            w.dirs += 1;
            walk(&e.path(), depth + 1, w)?;
        } else if ft.is_symlink() {
            w.links += 1;
        } else if ft.is_file() {
            w.files += 1;
            w.bytes += md.len();
        } else if w.skipped.len() < 20 {
            w.skipped.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    Ok(())
}

/// Look at the home and the drive; change nothing.
pub fn plan(env: &EncHome, user: &str) -> Result<Plan, HomeError> {
    if !crate::valid_account(user) {
        return Err(HomeError::BadAccount);
    }
    if env.exists(user) {
        return Err(HomeError::AlreadyExists);
    }
    let src = env.mount_point(user);
    let mut w = Walk { files: 0, dirs: 0, links: 0, bytes: 0, skipped: vec![] };
    if src.is_dir() {
        walk(&src, 0, &mut w)?;
    }
    let free = (env.free)(&env.data_dir).unwrap_or(0);
    // ext4 inside: block rounding, inode tables, journal; a quarter on top of the data plus a fixed part is generous
    let need = (w.bytes + (w.files + w.dirs + w.links) * SECTOR as u64) / 4 * 5 + (128 << 20);
    let want = need.max(env.default_payload().unwrap_or(0)).max(MIN_PAYLOAD);
    let room = free.saturating_sub(FREE_MARGIN);
    let payload = want.min(room.max(need)) / SECTOR as u64 * SECTOR as u64;
    let mut problems = vec![];
    if need + FREE_MARGIN > free {
        problems.push(format!("not enough free space: {} needed for the copy, {} free (the old home stays as it is, so both exist together for a while).", human(need + FREE_MARGIN), human(free)));
    }
    if env.plain_copy_path(user).exists() {
        problems.push(format!("{} already exists from an earlier move; remove it first.", env.plain_copy_path(user).display()));
    }
    if env.is_open(user) {
        problems.push("the home folder is already a mounted drive.".into());
    }
    Ok(Plan { files: w.files, dirs: w.dirs, links: w.links, bytes: w.bytes, skipped: w.skipped, free, payload, problems })
}

/// The steps, reported as they start (and where a test cuts the power).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Planned,
    ContainerMade,
    StagingMounted,
    Copied,
    Verified,
    StagingUnmounted,
    PlainRenamed,
    Switched,
}

#[derive(Debug)]
pub struct Report {
    pub files: u64,
    pub bytes: u64,
    pub plain_copy: PathBuf,
    pub recovery: RecoveryText,
}

fn staging(env: &EncHome, user: &str) -> PathBuf {
    env.dir().join(format!("{user}.staging"))
}
fn building_marker(env: &EncHome, user: &str) -> PathBuf {
    env.dir().join(format!("{user}.building"))
}

fn sha_of(p: &Path) -> std::io::Result<[u8; 32]> {
    let mut f = fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().into())
}

fn copy_tree(src: &Path, dst: &Path, depth: u32, n: &mut u64) -> Result<(), HomeError> {
    if depth > MAX_DEPTH {
        return Err(HomeError::Io("a folder is nested too deeply".into()));
    }
    for e in fs::read_dir(src).map_err(|e| HomeError::Io(format!("{}: {e}", src.display())))? {
        let e = e.map_err(|e| HomeError::Io(e.to_string()))?;
        let (from, to) = (e.path(), dst.join(e.file_name()));
        let md = fs::symlink_metadata(&from).map_err(|e2| HomeError::Io(format!("{}: {e2}", from.display())))?;
        let ft = md.file_type();
        if ft.is_dir() {
            fs::create_dir(&to).map_err(|e2| HomeError::Io(format!("{}: {e2}", to.display())))?;
            copy_tree(&from, &to, depth + 1, n)?;
            let _ = fs::set_permissions(&to, fs::Permissions::from_mode(md.mode() & 0o7777));
            let _ = std::os::unix::fs::lchown(&to, Some(md.uid()), Some(md.gid()));
        } else if ft.is_symlink() {
            let target = fs::read_link(&from).map_err(|e2| HomeError::Io(e2.to_string()))?;
            std::os::unix::fs::symlink(target, &to).map_err(|e2| HomeError::Io(format!("{}: {e2}", to.display())))?;
            let _ = std::os::unix::fs::lchown(&to, Some(md.uid()), Some(md.gid()));
        } else if ft.is_file() {
            let mut r = fs::File::open(&from).map_err(|e2| HomeError::Io(format!("{}: {e2}", from.display())))?;
            let mut w = fs::File::create(&to).map_err(|e2| HomeError::Io(format!("{}: {e2}", to.display())))?;
            std::io::copy(&mut r, &mut w).map_err(|e2| HomeError::Io(format!("{}: {e2}", to.display())))?;
            w.flush().map_err(|e2| HomeError::Io(e2.to_string()))?;
            let _ = fs::set_permissions(&to, fs::Permissions::from_mode(md.mode() & 0o7777));
            let _ = std::os::unix::fs::lchown(&to, Some(md.uid()), Some(md.gid()));
            if let Ok(t) = md.modified() {
                let _ = w.set_times(fs::FileTimes::new().set_modified(t));
            }
            *n += 1;
        }
    }
    Ok(())
}

/// Compare the copy with the original, file by file, reading both again. Returns the number of files checked.
pub fn verify(src: &Path, dst: &Path) -> Result<u64, HomeError> {
    fn go(src: &Path, dst: &Path, depth: u32, n: &mut u64) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("nested too deeply".into());
        }
        let mut names: Vec<_> = fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))?.flatten().map(|e| e.file_name()).collect();
        names.sort();
        // nothing extra may exist in the copy either
        // (a new ext4 file system has a lost+found folder of its own at its top: that one is not extra. Found on the Mac mini 2018:
        // "holds 3 entries, the original 2" after a copy that was right.)
        let own_top_folder = depth == 0 && !names.iter().any(|nm| nm == "lost+found");
        let extra = fs::read_dir(dst).map_err(|e| format!("{}: {e}", dst.display()))?.flatten().filter(|e| !(own_top_folder && e.file_name() == "lost+found")).count();
        let expected = names.iter().filter(|nm| fs::symlink_metadata(src.join(nm)).map(|m| { let t = m.file_type(); t.is_dir() || t.is_file() || t.is_symlink() }).unwrap_or(false)).count();
        if extra != expected {
            return Err(format!("{} holds {extra} entries, the original {expected}", dst.display()));
        }
        for nm in names {
            let (a, b) = (src.join(&nm), dst.join(&nm));
            let ma = fs::symlink_metadata(&a).map_err(|e| format!("{}: {e}", a.display()))?;
            let ft = ma.file_type();
            if !(ft.is_dir() || ft.is_file() || ft.is_symlink()) {
                continue;
            }
            let mb = fs::symlink_metadata(&b).map_err(|_| format!("missing in the copy: {}", b.display()))?;
            if ft.is_dir() {
                if !mb.file_type().is_dir() || ma.mode() & 0o7777 != mb.mode() & 0o7777 {
                    return Err(format!("folder differs: {}", b.display()));
                }
                go(&a, &b, depth + 1, n)?;
            } else if ft.is_symlink() {
                if !mb.file_type().is_symlink() || fs::read_link(&a).ok() != fs::read_link(&b).ok() {
                    return Err(format!("link differs: {}", b.display()));
                }
            } else {
                if !mb.file_type().is_file() || ma.len() != mb.len() || ma.mode() & 0o7777 != mb.mode() & 0o7777 {
                    return Err(format!("file differs in size or mode: {}", b.display()));
                }
                let (ha, hb) = (sha_of(&a).map_err(|e| format!("{}: {e}", a.display()))?, sha_of(&b).map_err(|e| format!("{}: {e}", b.display()))?);
                if ha != hb {
                    return Err(format!("file content differs (SHA-256): {}", b.display()));
                }
                *n += 1;
            }
        }
        Ok(())
    }
    let mut n = 0;
    go(src, dst, 0, &mut n).map_err(|e| HomeError::Io(format!("verification failed: {e}")))?;
    Ok(n)
}

/// Undo a migration that did not reach the switch: unmount the staging folder, close the mapping, delete the half-built container.
fn undo(env: &EncHome, user: &str) {
    let st = staging(env, user);
    if env.kernel.is_mounted(&st) {
        let _ = env.kernel.umount(&st);
    }
    let _ = env.deactivate(user);
    let _ = fs::remove_dir(&st);
    let _ = fs::remove_file(env.container_path(user));
    let _ = fs::remove_file(env.meta_path(user));
    let _ = fs::remove_file(building_marker(env, user));
}

/// Run the migration. `password` is the sign-in password. `step` is told each step as it is reached; returning `false` stops dead there
/// (a test's power cut; real callers always return true).
pub fn run(env: &EncHome, user: &str, password: &str, step: &mut dyn FnMut(Step) -> bool) -> Result<Report, HomeError> {
    let p = plan(env, user)?;
    if !p.ok() {
        return Err(HomeError::NotConfirmed(p.problems.join(" ")));
    }
    if !step(Step::Planned) {
        return Err(HomeError::Io("stopped".into()));
    }
    fs::create_dir_all(env.dir()).map_err(|e| HomeError::Io(e.to_string()))?;
    fs::write(building_marker(env, user), b"migration in progress").map_err(|e| HomeError::Io(e.to_string()))?;
    let result = (|| -> Result<Report, HomeError> {
        let recovery = env.make_container(user, password, p.payload, false)?;
        if !step(Step::ContainerMade) {
            return Err(HomeError::Io("stopped".into()));
        }
        let st = staging(env, user);
        fs::create_dir_all(&st).map_err(|e| HomeError::Io(e.to_string()))?;
        env.kernel.mount(&dm_path(user), &st).map_err(|e| HomeError::Kernel(e.to_string()))?;
        if !step(Step::StagingMounted) {
            return Err(HomeError::Io("stopped".into()));
        }
        let src = env.mount_point(user);
        let mut copied = 0;
        if src.is_dir() {
            copy_tree(&src, &st, 0, &mut copied)?;
        }
        if !step(Step::Copied) {
            return Err(HomeError::Io("stopped".into()));
        }
        let checked = if src.is_dir() { verify(&src, &st)? } else { 0 };
        if checked != copied {
            return Err(HomeError::Io(format!("verification failed: {copied} files copied but {checked} checked")));
        }
        if !step(Step::Verified) {
            return Err(HomeError::Io("stopped".into()));
        }
        // ---- the switch: from here the plain home is only renamed, never deleted ----
        env.kernel.umount(&st).map_err(|e| HomeError::Kernel(e.to_string()))?;
        let _ = fs::remove_dir(&st);
        if !step(Step::StagingUnmounted) {
            return Err(HomeError::Io("stopped".into()));
        }
        let plain = env.plain_copy_path(user);
        let had_home = src.is_dir();
        let before = fs::metadata(&src).ok();
        if had_home {
            fs::rename(&src, &plain).map_err(|e| HomeError::Io(format!("could not move the old home aside: {e}")))?;
        }
        if !step(Step::PlainRenamed) {
            return Err(HomeError::Io("stopped".into()));
        }
        if let Err(e) = env.mount_over_home(user) {
            if had_home {
                let _ = fs::remove_dir(&src);
                let _ = fs::rename(&plain, &src);
            }
            return Err(e);
        }
        env.own_mount_root(user, before.as_ref());
        let _ = fs::remove_file(building_marker(env, user));
        let _ = step(Step::Switched);
        Ok(Report { files: copied, bytes: p.bytes, plain_copy: plain, recovery })
    })();
    if let Err(e) = &result {
        // a stop (test power cut) leaves everything where it is; any real failure undoes what was built, the plain home is untouched
        if !matches!(e, HomeError::Io(m) if m == "stopped") {
            undo(env, user);
        }
    }
    result
}

fn dm_path(user: &str) -> String {
    iyakki_storage::dm::map_path(&crate::map_name(user))
}

/// At start-up (or before the next sign-in): finish or undo a migration that was cut short. Returns what was done, for the log.
///
/// * Before the plain home was renamed: the half-built container is deleted; the plain home is exactly as it was.
/// * After the rename: the container is complete and verified, so it stays; the next sign-in mounts it, and the plain copy is just an old folder.
pub fn recover_interrupted(env: &EncHome, user: &str) -> Option<String> {
    let marker = building_marker(env, user);
    if !marker.exists() {
        return None;
    }
    let renamed = env.plain_copy_path(user).is_dir();
    if renamed {
        let _ = fs::remove_file(&marker);
        let _ = fs::remove_dir(staging(env, user));
        return Some("An interrupted move to an encrypted home was completed: the encrypted copy was already checked; your old folder is kept next to it.".into());
    }
    undo(env, user);
    Some("An interrupted move to an encrypted home was undone: the half-made container was deleted and your home folder is as it was.".into())
}

/// The person confirmed: remove the plain copy. Only when the encrypted home is open and its recovery key was confirmed.
pub fn remove_plain_copy(env: &EncHome, user: &str) -> Result<String, HomeError> {
    let plain = env.plain_copy_path(user);
    if !plain.is_dir() {
        return Err(HomeError::NotConfirmed("There is no old plain copy to remove.".into()));
    }
    if !env.is_open(user) {
        return Err(HomeError::NotOpen);
    }
    if !env.meta(user).recovery_confirmed {
        return Err(HomeError::NotConfirmed("Confirm your recovery key first: once the plain copy is gone, the recovery key is the only way back if you forget the password.".into()));
    }
    fs::remove_dir_all(&plain).map_err(|e| HomeError::Io(e.to_string()))?;
    Ok(format!("The old plain copy was removed. {FLASH_NOTE}"))
}
