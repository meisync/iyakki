//! Snapshot before an update and the migration runner with rollback (F-CORE-25; docs/development/CORE-COMPATIBILITY-DESIGN.md section 7).
//!
//! * [`snapshot`] copies the files of an app's data folder into `<root>/<id>/data/` with a manifest of sizes and SHA-256 sums. Links are
//!   not followed (they are listed as skipped), and a snapshot over the byte cap is refused before anything is copied.
//! * [`restore`] checks every file of the snapshot against the manifest first, then puts the snapshot back. The folder it replaces is
//!   kept next to it as `<folder>.rolled-back-<id>`; nothing is deleted.
//! * [`run_with_rollback`] takes a snapshot, runs a migration and restores the snapshot when the migration fails.
//! * [`RollbackEnv`] wraps the wizard engine's [`Env`] so that every migration step of a wizard runs that way.
//!
//! WASM migrators in the tile sandbox are not built (KI-722).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iyakki_bindelta::bindelta::{hex, sha256};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::wizardrun::Env;

pub const MANIFEST: &str = "manifest.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSum {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub created: i64,
    pub files: Vec<FileSum>,
    pub skipped: Vec<String>,
    pub bytes: u64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') && !id.starts_with('.')
}

fn walk(base: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>, skipped: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(base.join(rel)) else { return };
    let mut names: Vec<_> = rd.flatten().collect();
    names.sort_by_key(|e| e.file_name());
    for e in names {
        let r = if rel.is_empty() { e.file_name().to_string_lossy().into_owned() } else { format!("{rel}/{}", e.file_name().to_string_lossy()) };
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(base, &r, out, skipped),
            Ok(t) if t.is_file() => out.push((r, e.path())),
            _ => skipped.push(r),
        }
    }
}

fn dir_of(root: &Path, id: &str) -> PathBuf {
    root.join(id)
}

/// Copy `src` into `<root>/<id>`. `max_bytes` caps the total size.
pub fn snapshot(src: &Path, root: &Path, id: &str, now: i64, max_bytes: u64) -> Result<Snapshot, String> {
    if !id_ok(id) {
        return Err(format!("snapshot name {id:?} is not allowed"));
    }
    let dir = dir_of(root, id);
    if dir.exists() {
        return Err(format!("a snapshot called {id} already exists"));
    }
    let (mut list, mut skipped) = (vec![], vec![]);
    walk(src, "", &mut list, &mut skipped);
    let total: u64 = list.iter().map(|(_, p)| std::fs::metadata(p).map_or(0, |m| m.len())).sum();
    if total > max_bytes {
        return Err(format!("the data is {total} bytes, over the {max_bytes} byte limit for a snapshot; nothing was copied"));
    }
    let tmp = root.join(format!(".{id}.partial"));
    let _ = std::fs::remove_dir_all(&tmp);
    let res = (|| -> Result<Snapshot, String> {
        let mut files = vec![];
        for (rel, p) in &list {
            let bytes = std::fs::read(p).map_err(|e| format!("{rel}: {e}"))?;
            iyakki_schema::write_atomic(&tmp.join("data").join(rel), &bytes, 0o600)?;
            files.push(FileSum { path: rel.clone(), size: bytes.len() as u64, sha256: hex(&sha256(&bytes)) });
        }
        std::fs::create_dir_all(tmp.join("data")).map_err(|e| e.to_string())?;
        let s = Snapshot { id: id.into(), created: now, files, skipped, bytes: total, extra: Default::default() };
        iyakki_schema::write_atomic(&tmp.join(MANIFEST), &serde_json::to_vec_pretty(&s).map_err(|e| e.to_string())?, 0o600)?;
        std::fs::rename(&tmp, &dir).map_err(|e| e.to_string())?;
        Ok(s)
    })();
    if res.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    res
}

pub fn load(root: &Path, id: &str) -> Result<Snapshot, String> {
    if !id_ok(id) {
        return Err(format!("snapshot name {id:?} is not allowed"));
    }
    let b = std::fs::read(dir_of(root, id).join(MANIFEST)).map_err(|_| format!("no snapshot called {id}"))?;
    serde_json::from_slice(&b).map_err(|_| format!("the manifest of snapshot {id} is damaged"))
}

/// Every snapshot under `root`, oldest first.
pub fn list(root: &Path) -> Vec<Snapshot> {
    let Ok(rd) = std::fs::read_dir(root) else { return vec![] };
    let mut v: Vec<Snapshot> = rd.flatten().filter_map(|e| load(root, &e.file_name().to_string_lossy()).ok()).collect();
    v.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
    v
}

/// Check every file of a snapshot against its manifest.
pub fn verify(root: &Path, id: &str) -> Result<Snapshot, String> {
    let s = load(root, id)?;
    for f in &s.files {
        let b = std::fs::read(dir_of(root, id).join("data").join(&f.path)).map_err(|_| format!("snapshot {id}: {} is missing", f.path))?;
        if b.len() as u64 != f.size || hex(&sha256(&b)) != f.sha256 {
            return Err(format!("snapshot {id}: {} does not match its checksum; it is damaged", f.path));
        }
    }
    Ok(s)
}

/// Put a snapshot back as `dst`. The folder it replaces is kept as `<dst>.rolled-back-<id>`.
pub fn restore(root: &Path, id: &str, dst: &Path) -> Result<PathBuf, String> {
    let s = verify(root, id)?;
    let mut kept = dst.as_os_str().to_os_string();
    kept.push(format!(".rolled-back-{id}"));
    let kept = PathBuf::from(kept);
    if kept.exists() {
        return Err(format!("{} is already there; move it away first", kept.display()));
    }
    let staged = dst.with_file_name(format!(".{}.restoring", dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    let _ = std::fs::remove_dir_all(&staged);
    for f in &s.files {
        let b = std::fs::read(dir_of(root, id).join("data").join(&f.path)).map_err(|e| format!("{}: {e}", f.path))?;
        iyakki_schema::write_atomic(&staged.join(&f.path), &b, 0o600)?;
    }
    std::fs::create_dir_all(&staged).map_err(|e| e.to_string())?;
    if dst.exists() {
        std::fs::rename(dst, &kept).map_err(|e| format!("{}: {e}", dst.display()))?;
    }
    std::fs::rename(&staged, dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    Ok(kept)
}

/// Keep the newest `keep` snapshots; the others are moved to `<root>/.old` (never deleted). Returns the ids moved.
pub fn prune(root: &Path, keep: usize) -> Vec<String> {
    let all = list(root);
    let cut = all.len().saturating_sub(keep);
    let mut moved = vec![];
    for s in &all[..cut] {
        let old = root.join(".old");
        if std::fs::create_dir_all(&old).is_ok() && std::fs::rename(dir_of(root, &s.id), old.join(&s.id)).is_ok() {
            moved.push(s.id.clone());
        }
    }
    moved
}

/// Snapshot `data`, run `migrate`, and put the snapshot back when it fails. The error says which happened.
pub fn run_with_rollback(data: &Path, root: &Path, id: &str, now: i64, max_bytes: u64, migrate: &dyn Fn(&Path) -> Result<String, String>) -> Result<String, String> {
    snapshot(data, root, id, now, max_bytes).map_err(|e| format!("no snapshot could be taken, so the migration did not run: {e}"))?;
    match migrate(data) {
        Ok(s) => Ok(s),
        Err(e) => match restore(root, id, data) {
            Ok(kept) => Err(format!("{e}; the data was put back (the half-migrated copy is at {})", kept.display())),
            Err(r) => Err(format!("{e}; putting the data back also failed: {r}")),
        },
    }
}

/// The wizard engine's environment with every migration step run from a snapshot with rollback. Checks pass straight through.
pub struct RollbackEnv<'a> {
    pub inner: &'a dyn Env,
    pub data: PathBuf,
    pub root: PathBuf,
    pub now: i64,
    pub max_bytes: u64,
    /// The migration itself: reference, from, to, answers (no secrets), the data folder.
    pub migrate: &'a dyn Fn(&str, Option<&str>, Option<&str>, &BTreeMap<String, Value>, &Path) -> Result<String, String>,
    pub wizard: String,
}

impl Env for RollbackEnv<'_> {
    fn check(&self, check: &str, param: &str) -> Result<(), String> {
        self.inner.check(check, param)
    }

    fn migrate(&self, reference: &str, from: Option<&str>, to: Option<&str>, answers: &BTreeMap<String, Value>) -> Result<String, String> {
        let safe: String = reference.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).take(24).collect();
        let id = format!("{}-{}-{}", self.wizard, safe, self.now);
        run_with_rollback(&self.data, &self.root, &id, self.now, self.max_bytes, &|d| (self.migrate)(reference, from, to, answers, d))
    }
}

#[cfg(test)]
#[path = "bundlesnap_tests.rs"]
mod tests;
