//! Host files: in window mode (iyakki-desktop on a normal computer) the person's real files live in the host's `$HOME`,
//! while the IYAKKI account has a home and workspaces of its own. This module is the whole bridge, and it is deliberately
//! small: nothing of the host is reachable until the person grants it.
//!
//! * **Import** copies host files into the account (an Uploads folder, a workspace, DATA or the chat). The host side is
//!   confined to the host `$HOME`: `..`, links that lead out and dot-folders (`~/.ssh`) are refused. Nothing is overwritten
//!   without asking, one file may be at most [`MAX_FILE_BYTES`] and one import [`MAX_IMPORT_BYTES`], and the copy reports
//!   progress so the screen can stay alive.
//! * **Mounts** grant one host folder to the account as `host:<name>`, read-only unless the person turns writing on. They are
//!   kept per person (`host-mounts-<user>.json`) and checked again on every access: the folder must still exist and still
//!   be inside the host `$HOME`. Removing a mount takes effect at the next access, open data handles included.
//! * **Export** copies an account file out to the host `$HOME` or a writable mount, never replacing a file silently.
//!
//! Everything is off unless [`set_enabled`] was called: the OS image and the browser front end never turn it on.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// The largest single file imported or exported.
pub const MAX_FILE_BYTES: u64 = 512 << 20;
/// The most one import (all its files) may copy.
pub const MAX_IMPORT_BYTES: u64 = 2 << 30;
/// The most files one import may hold (a folder is walked up to this).
pub const MAX_IMPORT_FILES: usize = 5000;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Tests that turn host files on or off (a process-wide switch) take this so they do not trip over each other.
#[doc(hidden)]
pub fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}

/// The desktop window turns host files on (window mode only).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Why host files are unavailable, in a sentence for the screen.
pub const UNAVAILABLE: &str = "Host files are only available in the desktop window, which runs on a normal computer with its own files. This system keeps your files inside your account, so there is no host to reach.";

/// The host's home folder (`$HOME`, real path), unless it is `/` or `/root`.
pub fn host_home() -> Option<PathBuf> {
    if let Some(h) = HOME_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Some(h);
    }
    host_home_from(std::env::var("HOME").ok().as_deref())
}

static HOME_OVERRIDE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Name another folder as the host home (the desktop can start with `IYAKKI_HOST_HOME`; tests use it).
pub fn set_host_home(p: Option<PathBuf>) {
    *HOME_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = p.and_then(|p| std::fs::canonicalize(p).ok());
}

pub fn host_home_from(home_env: Option<&str>) -> Option<PathBuf> {
    let h = home_env.filter(|h| !h.is_empty() && *h != "/" && *h != "/root")?;
    std::fs::canonicalize(h).ok().filter(|p| p.is_dir())
}

/// A host path (`~`, relative to `home`, `..`, links all resolved) that is inside `home` and has no dot-folder on the way.
pub fn host_resolve(home: &Path, input: &str) -> Result<PathBuf, String> {
    let t = input.trim();
    let joined = if t == "~" {
        home.to_path_buf()
    } else if let Some(r) = t.strip_prefix("~/") {
        home.join(r)
    } else if t.starts_with('/') {
        PathBuf::from(t)
    } else {
        home.join(t)
    };
    let real = crate::scope::resolve(home, &joined.to_string_lossy())?;
    inside_home(home, &real)?;
    Ok(real)
}

/// `real` (already a real path) must be inside `home`, and no part below `home` may start with a dot.
pub fn inside_home(home: &Path, real: &Path) -> Result<(), String> {
    let Ok(rel) = real.strip_prefix(home) else { return Err(format!("{} is outside your host home folder ({}).", real.display(), home.display())) };
    if rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')) {
        return Err(format!("{} is in a hidden folder, which host files never reach.", real.display()));
    }
    Ok(())
}

// ------------------------------------------------------------------ mounts

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostMount {
    /// Used as `host:<name>`.
    pub name: String,
    /// The real path of the folder.
    pub path: String,
    /// Writing is allowed (read-only by default).
    #[serde(default)]
    pub write: bool,
}

#[derive(Debug)]
pub struct HostMounts {
    file: PathBuf,
    pub items: Vec<HostMount>,
}

fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 40 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A mount name from a folder name: letters, digits, `-` and `_`.
pub fn mount_name_for(p: &Path) -> String {
    let n: String = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let n = n.trim_matches('-').to_string();
    if n.is_empty() { "folder".into() } else { n.chars().take(40).collect() }
}

impl HostMounts {
    pub fn load(dir: &Path, user: &str) -> HostMounts {
        let key = if iyakki_sec::policy::valid_username(user) { user } else { "invalid" };
        let file = dir.join(format!("host-mounts-{key}.json"));
        let items = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        HostMounts { file, items }
    }

    fn save(&self) -> Result<(), String> {
        if self.file.to_string_lossy().ends_with("-invalid.json") {
            return Err("invalid user".into());
        }
        let tmp = self.file.with_extension("tmp");
        if let Some(d) = self.file.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.items).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.file).map_err(|e| e.to_string())
    }

    /// Grant a host folder. It must be a real folder inside the host home (not the home itself), with no hidden folder on
    /// the way. `name` is made unique; the stored path is the real one.
    pub fn add(&mut self, home: &Path, folder: &str, name: Option<&str>, write: bool) -> Result<HostMount, String> {
        if !enabled() {
            return Err(UNAVAILABLE.into());
        }
        let real = host_resolve(home, folder)?;
        if !real.is_dir() {
            return Err(format!("{} is not a folder.", real.display()));
        }
        if real == home {
            return Err("Choose a folder inside your home folder, not the whole of it.".into());
        }
        if self.items.iter().any(|m| Path::new(&m.path) == real) {
            return Err(format!("{} is already shared.", real.display()));
        }
        let base = name.map(str::to_string).filter(|n| valid_name(n)).unwrap_or_else(|| mount_name_for(&real));
        let mut n = base.clone();
        let mut k = 2;
        while self.items.iter().any(|m| m.name == n) {
            n = format!("{base}-{k}");
            k += 1;
        }
        let m = HostMount { name: n, path: real.to_string_lossy().into_owned(), write };
        self.items.push(m.clone());
        self.save()?;
        Ok(m)
    }

    pub fn remove(&mut self, name: &str) -> Result<HostMount, String> {
        let i = self.items.iter().position(|m| m.name == name).ok_or_else(|| format!("no host folder named {name} is shared"))?;
        let m = self.items.remove(i);
        self.save()?;
        Ok(m)
    }

    pub fn set_write(&mut self, name: &str, write: bool) -> Result<(), String> {
        self.items.iter_mut().find(|m| m.name == name).ok_or_else(|| format!("no host folder named {name} is shared"))?.write = write;
        self.save()
    }

    /// The mounts that are still good: the folder exists and is still inside the host home (a link swapped in since the
    /// grant leads nowhere). Real paths.
    pub fn valid(&self, home: &Path) -> Vec<HostMount> {
        self.items
            .iter()
            .filter_map(|m| {
                let real = std::fs::canonicalize(&m.path).ok().filter(|p| p.is_dir())?;
                inside_home(home, &real).ok()?;
                Some(HostMount { name: m.name.clone(), path: real.to_string_lossy().into_owned(), write: m.write })
            })
            .collect()
    }
}

/// The valid mounts of a person when host files are on and a host home exists (else none).
pub fn active_mounts(dir: &Path, user: &str) -> Vec<HostMount> {
    if !enabled() {
        return Vec::new();
    }
    match host_home() {
        Some(h) => HostMounts::load(dir, user).valid(&h),
        None => Vec::new(),
    }
}

// ------------------------------------------------------------------ copying

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnExists {
    /// Stop (the caller asks the person).
    Fail,
    /// Copy beside it as `name (2).ext`.
    KeepBoth,
    Replace,
}

/// One file to copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub src: PathBuf,
    /// The path inside the destination folder (a folder dropped on the window keeps its shape).
    pub rel: PathBuf,
    pub size: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub items: Vec<Item>,
    pub total: u64,
    /// Paths (inside the destination) that already exist.
    pub conflicts: Vec<PathBuf>,
    /// Files refused, with the reason in a sentence.
    pub refused: Vec<String>,
}

fn walk(src: &Path, rel: &Path, depth: usize, plan: &mut Plan) {
    let Ok(meta) = std::fs::metadata(src) else { return plan.refused.push(format!("{}: cannot be read", src.display())) };
    if meta.is_dir() {
        if depth > 8 {
            return plan.refused.push(format!("{}: folders nested deeper than 8 levels are skipped", src.display()));
        }
        let Ok(rd) = std::fs::read_dir(src) else { return plan.refused.push(format!("{}: cannot be read", src.display())) };
        let mut names: Vec<_> = rd.flatten().map(|e| e.file_name()).collect();
        names.sort();
        for n in names {
            if n.to_string_lossy().starts_with('.') {
                continue;
            }
            let child = src.join(&n);
            // a link inside a folder is not followed: it could lead anywhere
            if std::fs::symlink_metadata(&child).is_ok_and(|m| m.file_type().is_symlink()) {
                continue;
            }
            if plan.items.len() >= MAX_IMPORT_FILES {
                plan.refused.push(format!("more than {MAX_IMPORT_FILES} files: the rest of {} is skipped", src.display()));
                return;
            }
            walk(&child, &rel.join(&n), depth + 1, plan);
        }
    } else if meta.is_file() {
        if meta.len() > MAX_FILE_BYTES {
            return plan.refused.push(format!("{} is {} MB, more than the {} MB limit for one file", src.display(), meta.len() >> 20, MAX_FILE_BYTES >> 20));
        }
        plan.items.push(Item { src: src.to_path_buf(), rel: rel.to_path_buf(), size: meta.len() });
    }
}

/// What copying `srcs` (host files and folders, already checked to be inside the host home or a mount) into `dest` would do.
/// Nothing is copied yet.
pub fn plan_import(srcs: &[PathBuf], dest: &Path) -> Plan {
    let mut plan = Plan::default();
    for s in srcs {
        let name = PathBuf::from(s.file_name().unwrap_or_default());
        walk(s, &name, 0, &mut plan);
    }
    plan.total = plan.items.iter().map(|i| i.size).sum();
    if plan.total > MAX_IMPORT_BYTES {
        plan.refused.push(format!("together {} MB, more than the {} MB limit for one import", plan.total >> 20, MAX_IMPORT_BYTES >> 20));
        plan.items.clear();
        plan.total = 0;
    }
    plan.conflicts = plan.items.iter().filter(|i| dest.join(&i.rel).exists()).map(|i| i.rel.clone()).collect();
    plan
}

fn keep_both(p: &Path) -> PathBuf {
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for n in 2..1000 {
        let c = p.with_file_name(format!("{stem} ({n}){ext}"));
        if !c.exists() {
            return c;
        }
    }
    p.to_path_buf()
}

/// Copy one file in chunks, adding to `done` as it goes. The file is written under a temporary name and moved into place, so
/// a half copy never looks like the real file.
pub fn copy_file(src: &Path, dst: &Path, done: &AtomicU64) -> Result<(), String> {
    use std::io::{Read, Write};
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = dst.with_file_name(format!(".{}.importing", dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    let r = (|| {
        let mut i = std::fs::File::open(src).map_err(|e| format!("{}: {e}", src.display()))?;
        let mut o = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = i.read(&mut buf).map_err(|e| format!("{}: {e}", src.display()))?;
            if n == 0 {
                break;
            }
            o.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", tmp.display()))?;
            done.fetch_add(n as u64, Ordering::Relaxed);
        }
        o.flush().map_err(|e| e.to_string())
    })();
    match r {
        Ok(()) => std::fs::rename(&tmp, dst).map_err(|e| format!("{}: {e}", dst.display())),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Carry out a plan into `dest`. `Fail` with a conflict copies nothing. Returns the files written.
pub fn run_import(plan: &Plan, dest: &Path, on_exists: OnExists, done: &AtomicU64) -> Result<Vec<PathBuf>, String> {
    if on_exists == OnExists::Fail && !plan.conflicts.is_empty() {
        return Err(format!("{} already exists; nothing was copied.", plan.conflicts[0].display()));
    }
    let mut out = Vec::new();
    for it in &plan.items {
        let mut to = dest.join(&it.rel);
        if to.exists() {
            match on_exists {
                OnExists::Fail => return Err(format!("{} already exists", to.display())),
                OnExists::KeepBoth => to = keep_both(&to),
                OnExists::Replace => {}
            }
        }
        // the destination must stay inside `dest` (a name with .. cannot get here, but the check is cheap)
        if !to.starts_with(dest) {
            return Err(format!("{} is not inside the destination", to.display()));
        }
        copy_file(&it.src, &to, done)?;
        out.push(to);
    }
    Ok(out)
}

/// Copy one account file out to `dest` (a file path on the host: inside the host home or a writable mount, checked by the
/// caller). Never replaces without `Replace`.
pub fn export_file(src: &Path, dest: &Path, on_exists: OnExists) -> Result<PathBuf, String> {
    let meta = std::fs::metadata(src).map_err(|e| format!("{}: {e}", src.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a file", src.display()));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("{} is {} MB, more than the {} MB limit", src.display(), meta.len() >> 20, MAX_FILE_BYTES >> 20));
    }
    let mut to = dest.to_path_buf();
    if to.exists() {
        match on_exists {
            OnExists::Fail => return Err(format!("{} already exists", to.display())),
            OnExists::KeepBoth => to = keep_both(&to),
            OnExists::Replace => {}
        }
    }
    copy_file(src, &to, &AtomicU64::new(0))?;
    Ok(to)
}


// ------------------------------------------------------------------ link or copy

/// What a folder holds, counted without reading the files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FolderStats {
    pub files: u64,
    pub bytes: u64,
    /// Audio, video and picture files among them.
    pub media: u64,
    /// The count stopped at the walk limit.
    pub capped: bool,
}

const MEDIA_EXT: &[&str] = &["mp3", "flac", "wav", "ogg", "opus", "m4a", "aac", "wma", "mp4", "mkv", "mov", "avi", "webm", "m4v", "jpg", "jpeg", "png", "gif", "webp", "heic", "bmp", "tif", "tiff"];

pub fn is_media_name(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|e| MEDIA_EXT.contains(&e.to_string_lossy().to_ascii_lowercase().as_str()))
}

/// Count a folder (hidden entries and links are not followed). Stops at 200,000 files.
pub fn folder_stats(dir: &Path) -> FolderStats {
    fn go(d: &Path, depth: usize, st: &mut FolderStats) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            if st.files >= 200_000 {
                st.capped = true;
                return;
            }
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() && depth < 12 {
                go(&e.path(), depth + 1, st);
            } else if t.is_file() {
                st.files += 1;
                st.bytes += e.metadata().map_or(0, |m| m.len());
                if is_media_name(&e.file_name().to_string_lossy()) {
                    st.media += 1;
                }
            }
        }
    }
    let mut st = FolderStats::default();
    go(dir, 0, &mut st);
    st
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Share it read-only where it is (no copy).
    Link,
    /// Copy it into the account.
    Copy,
}

/// The suggested way to bring a folder in: link when it is big (over 200 MB or 500 files) or mostly media, else copy.
pub fn default_choice(st: &FolderStats) -> Choice {
    if st.capped || st.bytes > 200 << 20 || st.files > 500 || (st.files > 0 && st.media * 2 > st.files) { Choice::Link } else { Choice::Copy }
}

fn choices_file(dir: &Path, user: &str) -> PathBuf {
    dir.join(format!("host-choices-{}.json", if iyakki_sec::policy::valid_username(user) { user } else { "invalid" }))
}

/// The way this person last brought this folder in.
pub fn remembered_choice(dir: &Path, user: &str, folder: &Path) -> Option<Choice> {
    let m: std::collections::BTreeMap<String, String> = std::fs::read(choices_file(dir, user)).ok().and_then(|b| serde_json::from_slice(&b).ok())?;
    match m.get(&folder.to_string_lossy().into_owned())?.as_str() {
        "link" => Some(Choice::Link),
        "copy" => Some(Choice::Copy),
        _ => None,
    }
}

pub fn remember_choice(dir: &Path, user: &str, folder: &Path, c: Choice) {
    let f = choices_file(dir, user);
    let mut m: std::collections::BTreeMap<String, String> = std::fs::read(&f).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    m.insert(folder.to_string_lossy().into_owned(), if c == Choice::Link { "link" } else { "copy" }.into());
    if m.len() > 200 {
        let first = m.keys().next().cloned();
        if let Some(k) = first {
            m.remove(&k);
        }
    }
    if let Ok(t) = serde_json::to_vec(&m) {
        let _ = std::fs::write(f, t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-hostfiles-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn host_paths_stay_inside_the_host_home_and_out_of_dot_folders() {
        let base = tmp("paths");
        let home = base.join("host");
        std::fs::create_dir_all(home.join("Documents")).unwrap();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join("Documents/a.csv"), "x").unwrap();
        std::fs::write(home.join(".ssh/id"), "x").unwrap();
        std::fs::write(base.join("outside.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&base, home.join("up")).unwrap();
        std::os::unix::fs::symlink("/etc", home.join("etc")).unwrap();
        assert_eq!(host_resolve(&home, "~/Documents/a.csv").unwrap(), home.join("Documents/a.csv"));
        assert_eq!(host_resolve(&home, "Documents/a.csv").unwrap(), home.join("Documents/a.csv"));
        for bad in ["/etc/passwd", "../outside.txt", "up/outside.txt", "etc/passwd", "~/../outside.txt", "~/.ssh/id", "/"] {
            assert!(host_resolve(&home, bad).is_err(), "{bad}");
        }
        assert!(host_resolve(&home, ".ssh/id").unwrap_err().contains("hidden"));
        assert_eq!(host_home_from(Some("/")), None);
        assert_eq!(host_home_from(Some("/root")), None);
        assert_eq!(host_home_from(Some(home.to_str().unwrap())), Some(home));
    }

    #[test]
    fn mounts_are_per_person_read_only_by_default_checked_again_and_revocable() {
        let _g = test_guard();
        set_enabled(true);
        let base = tmp("mounts");
        let (home, data) = (base.join("host"), base.join("data"));
        std::fs::create_dir_all(home.join("Documents/data")).unwrap();
        std::fs::create_dir_all(home.join(".secret")).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let mut m = HostMounts::load(&data, "amy");
        assert!(m.add(&home, "~", None, false).unwrap_err().contains("inside your home"));
        assert!(m.add(&home, "/etc", None, false).is_err());
        assert!(m.add(&home, "~/.secret", None, false).unwrap_err().contains("hidden"));
        assert!(m.add(&home, "~/nothing", None, false).is_err());
        let a = m.add(&home, "~/Documents/data", None, false).unwrap();
        assert_eq!((a.name.as_str(), a.write), ("data", false), "read-only unless turned on");
        assert!(m.add(&home, "~/Documents/data", None, false).unwrap_err().contains("already"));
        let again = m.add(&home, "~/Documents", Some("data"), true).unwrap();
        assert_eq!(again.name, "data-2", "names stay unique");
        // kept per person
        assert_eq!(HostMounts::load(&data, "amy").items.len(), 2);
        assert!(HostMounts::load(&data, "bob").items.is_empty());
        m.set_write("data", true).unwrap();
        assert!(HostMounts::load(&data, "amy").items[0].write);
        // checked again at every access: a folder that vanished or became a link out is dropped
        assert_eq!(m.valid(&home).len(), 2);
        std::fs::remove_dir_all(home.join("Documents/data")).unwrap();
        std::os::unix::fs::symlink("/etc", home.join("Documents/data")).unwrap();
        let v = m.valid(&home);
        assert_eq!(v.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["data-2"], "{v:?}");
        // revoking
        m.remove("data-2").unwrap();
        assert!(m.valid(&home).is_empty());
        assert!(m.remove("data-2").is_err());
        // nothing is reachable when host files are off
        set_enabled(false);
        assert!(m.add(&home, "~/Documents", None, false).unwrap_err().contains("desktop window"));
        assert!(active_mounts(&data, "amy").is_empty());
        set_enabled(false);
    }

    #[test]
    fn import_copies_without_overwriting_keeps_folder_shape_and_skips_hidden_and_links() {
        let base = tmp("import");
        let (src, dest) = (base.join("src"), base.join("Uploads"));
        std::fs::create_dir_all(src.join("proj/sub")).unwrap();
        std::fs::create_dir_all(src.join("proj/.git")).unwrap();
        std::fs::write(src.join("a.csv"), "id\n1\n").unwrap();
        std::fs::write(src.join("proj/b.txt"), "b").unwrap();
        std::fs::write(src.join("proj/sub/c.txt"), "c").unwrap();
        std::fs::write(src.join("proj/.git/config"), "no").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", src.join("proj/link")).unwrap();
        let plan = plan_import(&[src.join("a.csv"), src.join("proj")], &dest);
        let rels: Vec<String> = plan.items.iter().map(|i| i.rel.display().to_string()).collect();
        assert_eq!(rels, vec!["a.csv", "proj/b.txt", "proj/sub/c.txt"], "{plan:?}");
        assert!(plan.conflicts.is_empty() && plan.refused.is_empty());
        let done = AtomicU64::new(0);
        let out = run_import(&plan, &dest, OnExists::Fail, &done).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(done.load(Ordering::Relaxed), plan.total);
        assert_eq!(std::fs::read_to_string(dest.join("proj/sub/c.txt")).unwrap(), "c");
        assert!(!dest.join("proj/.git").exists() && !dest.join("proj/link").exists());
        // the same again: a conflict, and nothing is touched unless asked
        std::fs::write(src.join("a.csv"), "id\n2\n").unwrap();
        let plan = plan_import(&[src.join("a.csv")], &dest);
        assert_eq!(plan.conflicts, vec![PathBuf::from("a.csv")]);
        assert!(run_import(&plan, &dest, OnExists::Fail, &done).unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read_to_string(dest.join("a.csv")).unwrap(), "id\n1\n", "not overwritten");
        let out = run_import(&plan, &dest, OnExists::KeepBoth, &done).unwrap();
        assert!(out[0].ends_with("a (2).csv"), "{out:?}");
        assert_eq!(std::fs::read_to_string(dest.join("a.csv")).unwrap(), "id\n1\n");
        run_import(&plan, &dest, OnExists::Replace, &done).unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("a.csv")).unwrap(), "id\n2\n", "only when the person said replace");
        assert!(std::fs::read_dir(&dest).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().ends_with(".importing")), "no temporary files left");
    }

    #[test]
    fn size_limits_are_plain_sentences() {
        let base = tmp("caps");
        let big = base.join("big.bin");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_FILE_BYTES + 1).unwrap();
        std::fs::write(base.join("ok.txt"), "x").unwrap();
        let plan = plan_import(&[big.clone(), base.join("ok.txt")], &base.join("dest"));
        assert_eq!(plan.items.len(), 1);
        assert!(plan.refused[0].contains("more than the 512 MB limit for one file"), "{:?}", plan.refused);
        // export refuses it too
        assert!(export_file(&big, &base.join("out.bin"), OnExists::Fail).unwrap_err().contains("limit"));
    }

    #[test]
    fn export_never_replaces_silently() {
        let base = tmp("export");
        std::fs::write(base.join("r.csv"), "new").unwrap();
        std::fs::create_dir_all(base.join("host")).unwrap();
        std::fs::write(base.join("host/r.csv"), "old").unwrap();
        assert!(export_file(&base.join("r.csv"), &base.join("host/r.csv"), OnExists::Fail).unwrap_err().contains("already exists"));
        assert_eq!(std::fs::read_to_string(base.join("host/r.csv")).unwrap(), "old");
        let to = export_file(&base.join("r.csv"), &base.join("host/r.csv"), OnExists::KeepBoth).unwrap();
        assert!(to.ends_with("r (2).csv"));
        export_file(&base.join("r.csv"), &base.join("host/r.csv"), OnExists::Replace).unwrap();
        assert_eq!(std::fs::read_to_string(base.join("host/r.csv")).unwrap(), "new");
    }
    #[test]
    fn a_folder_is_linked_when_big_or_media_and_copied_when_small_and_the_choice_is_remembered() {
        let base = tmp("choice");
        let songs = base.join("Songs");
        let docs = base.join("Docs");
        std::fs::create_dir_all(songs.join("album")).unwrap();
        std::fs::create_dir_all(&docs).unwrap();
        for i in 0..6 {
            std::fs::write(songs.join(format!("album/t{i}.mp3")), "x").unwrap();
        }
        std::fs::write(songs.join("cover.JPG"), "x").unwrap();
        std::fs::write(songs.join(".hidden.mp3"), "x").unwrap();
        std::fs::write(docs.join("a.txt"), "x").unwrap();
        std::fs::write(docs.join("b.csv"), "x").unwrap();
        let s = folder_stats(&songs);
        assert_eq!((s.files, s.media), (7, 7), "hidden files are not counted");
        assert_eq!(default_choice(&s), Choice::Link, "mostly media");
        let d = folder_stats(&docs);
        assert_eq!((d.files, d.media), (2, 0));
        assert_eq!(default_choice(&d), Choice::Copy, "small and not media");
        assert_eq!(default_choice(&FolderStats { files: 10, bytes: 201 << 20, media: 0, capped: false }), Choice::Link, "over 200 MB");
        assert_eq!(default_choice(&FolderStats { files: 501, bytes: 1000, media: 0, capped: false }), Choice::Link, "over 500 files");
        assert_eq!(default_choice(&FolderStats { files: 500, bytes: 199 << 20, media: 100, capped: false }), Choice::Copy);
        let data = base.join("data");
        std::fs::create_dir_all(&data).unwrap();
        assert_eq!(remembered_choice(&data, "amy", &docs), None);
        remember_choice(&data, "amy", &docs, Choice::Link);
        assert_eq!(remembered_choice(&data, "amy", &docs), Some(Choice::Link));
        assert_eq!(remembered_choice(&data, "bob", &docs), None, "per person");
    }
}
