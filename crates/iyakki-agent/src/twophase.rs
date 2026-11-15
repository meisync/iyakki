//! Two-phase data migration with the boot-confirm commit point (F-CORE-20; docs/development/CORE-COMPATIBILITY-DESIGN.md section 7).
//!
//! A migration that is destructive or one-way is split in two so that going back to the previous version always reads the data it knows:
//!
//! 1. **Stage** (`stage`): the new layout of each file is written NEXT TO the old one, as `<file>.next`. The old file is not touched.
//!    The new version reads `<file>.next` ([`path_to_read`]); the old version keeps reading `<file>`.
//! 2. **Confirm** ([`reconcile`]): when the new version has booted and the boot loader was told (the shell calls [`reconcile`] right after
//!    `bootconfirm` writes its variable), the migration becomes `Confirmed` and a grace period starts.
//!    If the OLD version starts instead (the person rolled back, or the new version never confirmed), the staged files are moved
//!    aside (`<file>.next.rolled-back`) and the migration is `RolledBack`; nothing is deleted.
//! 3. **Commit** ([`commit`]): only after `Confirmed` and the grace period: each old file is moved to `<file>.pre-<id>` (kept) and
//!    `<file>.next` takes its place. A crash half way is resumed by calling `commit` again.
//!
//! The state is one small JSON file per migration, `migrations/<id>.json` in the data folder, written atomically.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const DIR: &str = "migrations";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Staged,
    Confirmed,
    Committing,
    Committed,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub id: String,
    pub from_version: String,
    pub to_version: String,
    pub phase: Phase,
    pub staged_at: i64,
    #[serde(default)]
    pub confirmed_at: Option<i64>,
    /// Files (relative to the data folder) that move to the new layout.
    pub files: Vec<String>,
    /// Fields a newer release adds are kept.
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') && !id.starts_with('.')
}

fn file_ok(f: &str) -> bool {
    !f.is_empty() && !f.starts_with('/') && !f.split('/').any(|p| p.is_empty() || p == ".." || p == ".")
}

fn state_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(DIR).join(format!("{id}.json"))
}

fn next_of(dir: &Path, file: &str) -> PathBuf {
    iyakki_schema::sibling(&dir.join(file), "next")
}

fn save(dir: &Path, s: &State) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?;
    iyakki_schema::write_atomic(&state_path(dir, &s.id), &bytes, 0o600)
}

pub fn load(dir: &Path, id: &str) -> Option<State> {
    serde_json::from_slice(&std::fs::read(state_path(dir, id)).ok()?).ok()
}

/// Every migration that is not finished (staged, confirmed or committing).
pub fn pending(dir: &Path) -> Vec<State> {
    let Ok(rd) = std::fs::read_dir(dir.join(DIR)) else { return vec![] };
    let mut v: Vec<State> = rd.flatten().filter_map(|e| serde_json::from_slice::<State>(&std::fs::read(e.path()).ok()?).ok()).filter(|s| matches!(s.phase, Phase::Staged | Phase::Confirmed | Phase::Committing)).collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

/// Phase 1. `convert(file, old_bytes)` gives the new layout of one file (a missing old file is passed as empty bytes and may stay empty).
/// Nothing changes on a failure: files already written next to the old ones are removed again. Calling it again for a migration that is
/// already staged (or later) returns the kept state and writes nothing, so a restart is safe.
pub fn stage(dir: &Path, id: &str, from: &str, to: &str, files: &[&str], now: i64, convert: &dyn Fn(&str, &[u8]) -> Result<Vec<u8>, String>) -> Result<State, String> {
    if !id_ok(id) {
        return Err(format!("migration name {id:?} is not allowed"));
    }
    if let Some(s) = load(dir, id) {
        return Ok(s);
    }
    if let Some(f) = files.iter().find(|f| !file_ok(f)) {
        return Err(format!("{f:?} is not a path inside the data folder"));
    }
    let mut written: Vec<PathBuf> = vec![];
    for f in files {
        let old = std::fs::read(dir.join(f)).unwrap_or_default();
        let res = convert(f, &old).and_then(|new| {
            let p = next_of(dir, f);
            iyakki_schema::write_atomic(&p, &new, 0o600)?;
            written.push(p);
            Ok(())
        });
        if let Err(e) = res {
            for p in &written {
                let _ = std::fs::remove_file(p);
            }
            return Err(format!("{f}: {e}; nothing was changed"));
        }
    }
    let s = State { id: id.into(), from_version: from.into(), to_version: to.into(), phase: Phase::Staged, staged_at: now, confirmed_at: None, files: files.iter().map(|f| f.to_string()).collect(), extra: Default::default() };
    if let Err(e) = save(dir, &s) {
        for p in &written {
            let _ = std::fs::remove_file(p);
        }
        return Err(e);
    }
    Ok(s)
}

/// Which file a running version should read for `file`: the staged new layout for the version it was staged for, the original otherwise.
pub fn path_to_read(dir: &Path, id: &str, file: &str, running_version: &str) -> PathBuf {
    match load(dir, id) {
        Some(s) if matches!(s.phase, Phase::Staged | Phase::Confirmed) && s.to_version == running_version && s.files.iter().any(|f| f == file) && next_of(dir, file).is_file() => next_of(dir, file),
        _ => dir.join(file),
    }
}

/// Called when a start is confirmed (after the sign-in screen is on the display), with the version that is running. A staged migration
/// for this version becomes confirmed; a staged or confirmed one for ANOTHER version (the old one started again) is rolled back.
pub fn reconcile(dir: &Path, running_version: &str, now: i64) -> Vec<(String, Phase)> {
    let mut out = vec![];
    for mut s in pending(dir) {
        if s.phase == Phase::Committing {
            continue;
        }
        if s.to_version == running_version {
            if s.phase == Phase::Staged {
                s.phase = Phase::Confirmed;
                s.confirmed_at = Some(now);
                if save(dir, &s).is_ok() {
                    out.push((s.id.clone(), s.phase));
                }
            }
        } else if s.from_version == running_version {
            for f in &s.files {
                let n = next_of(dir, f);
                if n.is_file() {
                    let _ = std::fs::rename(&n, iyakki_schema::sibling(&n, "rolled-back"));
                }
            }
            s.phase = Phase::RolledBack;
            if save(dir, &s).is_ok() {
                out.push((s.id.clone(), s.phase));
            }
        }
    }
    out
}

/// Phase 2: only for a confirmed migration whose grace period (seconds) is over. The old files are kept as `<file>.pre-<id>`.
pub fn commit(dir: &Path, id: &str, now: i64, grace_secs: i64) -> Result<State, String> {
    let mut s = load(dir, id).ok_or_else(|| format!("no migration called {id}"))?;
    match s.phase {
        Phase::Committed => return Ok(s),
        Phase::RolledBack => return Err("this migration was rolled back; stage it again with the new version".into()),
        Phase::Staged => return Err("the new version has not confirmed a good start yet, so the old data stays".into()),
        Phase::Confirmed | Phase::Committing => {}
    }
    if s.phase == Phase::Confirmed {
        let at = s.confirmed_at.unwrap_or(now);
        if now - at < grace_secs {
            return Err(format!("the grace period is not over: {} more seconds", grace_secs - (now - at)));
        }
        s.phase = Phase::Committing;
        save(dir, &s)?;
    }
    for f in &s.files {
        let (cur, next) = (dir.join(f), next_of(dir, f));
        let keep = iyakki_schema::sibling(&cur, &format!("pre-{id}"));
        if next.is_file() {
            if cur.is_file() && !keep.exists() {
                std::fs::rename(&cur, &keep).map_err(|e| format!("{f}: {e}"))?;
            }
            std::fs::rename(&next, &cur).map_err(|e| format!("{f}: {e}"))?;
        }
    }
    s.phase = Phase::Committed;
    save(dir, &s)?;
    Ok(s)
}

/// One plain line per unfinished migration, for the compat report.
pub fn status_lines(dir: &Path) -> Vec<String> {
    pending(dir)
        .iter()
        .map(|s| {
            let what = match s.phase {
                Phase::Staged => format!("waiting for {} to confirm a good start; the old data is untouched", s.to_version),
                Phase::Confirmed => "confirmed, waiting for the grace period before the old data is put aside".to_string(),
                _ => "finishing".to_string(),
            };
            format!("data migration {}: {} ({} file(s))", s.id, what, s.files.len())
        })
        .collect()
}

#[cfg(test)]
#[path = "twophase_tests.rs"]
mod tests;
