//! Versioned JSON files on the data volume (`/var/lib/iyakki`, a person's home).
//!
//! The rules (docs/development/DATA-LAYOUT-AND-UPGRADE.md):
//!
//! * A store file is a JSON object with a `schema` number. A file without one is the alpha.1 layout (schema 0).
//! * Every store has a [`Spec`]: the schema number this release writes and a list of [`Migration`]s, each a plain
//!   `fn(&mut Value)` that lifts a file from `to - 1` to `to`. Migrations run in order, in memory.
//! * Before the first change to a file the old bytes are copied to `<file>.pre-v<N>` (N = the schema it is lifted to).
//!   The copy is made once and never overwritten, so it is always the file as the older release left it.
//! * Files are written whole and atomically (temporary file in the same folder, sync, rename) and keep their mode.
//! * A file with a NEWER schema than this release knows is never rewritten: [`open`] returns [`Outcome::Newer`], the store
//!   reads what it can and refuses every save with a clear message. [`save`] checks this again right before it writes.
//! * A file that cannot be read as JSON is moved to `<file>.corrupt-<time>` (nothing is deleted) and the store starts with its
//!   defaults; [`Outcome::note`] is the sentence for the audit log.
//! * Fields this release does not know are kept: typed stores carry a `#[serde(flatten)] extra: Map` and write it back.
//!
//! Nothing here knows any store's fields, so the module can be used by every crate.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub use serde_json::{Map, Value};

/// The field every versioned file carries.
pub const KEY: &str = "schema";

/// Fields a release does not know, kept when the file is written again.
pub type Extra = Map<String, Value>;

/// One step: lifts the file from `to - 1` to `to`. Works on the whole JSON value (it may even change the shape of the root).
#[derive(Clone, Copy)]
pub struct Migration {
    pub to: u32,
    pub what: &'static str,
    pub run: fn(&mut Value),
}

/// What one store file is.
#[derive(Clone, Copy)]
pub struct Spec {
    /// Shown in messages ("settings", "inbox").
    pub name: &'static str,
    /// The schema this release reads and writes.
    pub current: u32,
    /// In ascending `to`; `to` runs from 1 to `current` without gaps.
    pub migrations: &'static [Migration],
}

impl Spec {
    /// Checks the registry itself (used by the tests of every store).
    pub fn check(&self) -> Result<(), String> {
        let mut want = 1;
        for m in self.migrations {
            if m.to != want {
                return Err(format!("{}: migration to {} where {want} was expected", self.name, m.to));
            }
            want += 1;
        }
        if want - 1 != self.current {
            return Err(format!("{}: {} migrations but the current schema is {}", self.name, want - 1, self.current));
        }
        Ok(())
    }
}

/// The schema of a parsed file: its `schema` number, 0 when there is none (alpha.1 layout, or a bare list).
pub fn version_of(v: &Value) -> u32 {
    v.get(KEY).and_then(Value::as_u64).map_or(0, |n| n.min(u32::MAX as u64) as u32)
}

/// Run the migrations from the file's schema up to `spec.current` and stamp the result. A newer schema is an error.
pub fn migrate(spec: &Spec, v: &mut Value) -> Result<u32, String> {
    let from = version_of(v);
    if from > spec.current {
        return Err(format!("{}: written by a newer release (schema {from}, this release reads up to {})", spec.name, spec.current));
    }
    for m in spec.migrations.iter().filter(|m| m.to > from) {
        (m.run)(v);
    }
    if let Value::Object(o) = v {
        o.insert(KEY.into(), Value::from(spec.current));
    }
    Ok(from)
}

/// Move a bare list into the object shape (`{"schema":N,"<key>":[..]}`); a no-op on anything else. For migrations of list files.
pub fn wrap_list(v: &mut Value, key: &str) {
    if v.is_array() {
        let list = std::mem::take(v);
        let mut o = Map::new();
        o.insert(key.into(), list);
        *v = Value::Object(o);
    }
}

#[derive(Debug)]
pub enum Outcome {
    /// There is no file: use the defaults.
    Missing,
    /// Already at this release's schema.
    Current(Value),
    /// Lifted from `from`; the file was rewritten and `backup` is the copy of the old bytes (`None`: that copy existed already).
    Migrated { value: Value, from: u32, backup: Option<PathBuf> },
    /// Written by a newer release: left untouched. `value` is the file as it is, for reading.
    Newer { value: Value, found: u32 },
    /// Not JSON (or unreadable as a store): moved to `moved_to`; use the defaults.
    Corrupt { moved_to: Option<PathBuf>, reason: String },
}

impl Outcome {
    /// The sentence for the audit log, when something happened that the person should be able to find later.
    pub fn note(&self, spec: &Spec, path: &Path) -> Option<String> {
        let f = path.display();
        match self {
            Outcome::Missing | Outcome::Current(_) => None,
            Outcome::Migrated { from, backup, .. } => Some(format!("{} {f}: upgraded from schema {from} to {}{}", spec.name, spec.current, backup.as_ref().map(|b| format!(" (old file kept as {})", b.display())).unwrap_or_default())),
            Outcome::Newer { found, .. } => Some(format!("{} {f}: schema {found} is newer than this release ({}); the file is read-only until IYAKKI is updated", spec.name, spec.current)),
            Outcome::Corrupt { moved_to, reason } => Some(format!("{} {f}: damaged ({reason}); {} and started with the defaults", spec.name, moved_to.as_ref().map(|m| format!("moved to {}", m.display())).unwrap_or_else(|| "could not be moved aside".into()))),
        }
    }
}

static NOTES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Remember a sentence for the audit log (an upgrade happened, a file was moved aside). Stores load before the audit log is
/// reachable, so the shell's audit log writes whatever is waiting here the next time it records anything.
pub fn report(note: String) {
    let mut q = NOTES.lock().unwrap_or_else(|e| e.into_inner());
    if q.len() < 200 {
        q.push(note);
    }
}

/// Take the waiting sentences.
pub fn drain_notes() -> Vec<String> {
    std::mem::take(&mut *NOTES.lock().unwrap_or_else(|e| e.into_inner()))
}

/// `<file>.<suffix>` next to the file.
pub fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut n = path.file_name().map(|s| s.to_os_string()).unwrap_or_default();
    n.push(".");
    n.push(suffix);
    path.with_file_name(n)
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Write `bytes` to `path` atomically. `mode` applies to a new file; an existing file keeps its mode.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = sibling(path, &format!("tmp{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let keep = fs::metadata(path).map(|m| m.permissions().mode() & 0o7777).unwrap_or(mode);
    let res = (|| -> std::io::Result<()> {
        let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(keep).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.map_err(|e| format!("{}: {e}", path.display()))
}

fn backup_once(path: &Path, to: u32) -> Result<Option<PathBuf>, String> {
    let b = sibling(path, &format!("pre-v{to}"));
    if b.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    write_atomic(&b, &bytes, 0o600)?;
    Ok(Some(b))
}

/// Move a file that cannot be used out of the way. Returns where it went.
pub fn move_aside(path: &Path, now: u64) -> Option<PathBuf> {
    let mut to = sibling(path, &format!("corrupt-{now}"));
    let mut n = 1;
    while to.exists() {
        to = sibling(path, &format!("corrupt-{now}-{n}"));
        n += 1;
    }
    fs::rename(path, &to).ok().map(|_| to)
}

/// Read a store file for its schema: migrate it (backup first, then an atomic rewrite), leave a newer one alone, move a damaged
/// one aside. `now` names the `.corrupt-<now>` copy.
pub fn open(spec: &Spec, path: &Path, now: u64) -> Outcome {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Outcome::Missing,
        // unreadable for another reason (permissions, I/O): do not touch it; the store runs on defaults and cannot save over it
        Err(e) => return Outcome::Corrupt { moved_to: None, reason: e.to_string() },
    };
    let mut v: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return Outcome::Corrupt { moved_to: move_aside(path, now), reason: e.to_string() },
    };
    let found = version_of(&v);
    if found > spec.current {
        return Outcome::Newer { value: v, found };
    }
    if found == spec.current && v.is_object() {
        return Outcome::Current(v);
    }
    let backup = match backup_once(path, found + 1) {
        Ok(b) => b,
        // no backup, no change: the file stays as it is and is read in the old layout
        Err(_) => {
            let _ = migrate(spec, &mut v);
            return Outcome::Current(v);
        }
    };
    let _ = migrate(spec, &mut v);
    match serde_json::to_vec_pretty(&v).map_err(|e| e.to_string()).and_then(|b| write_atomic(path, &b, 0o600)) {
        Ok(()) => Outcome::Migrated { value: v, from: found, backup },
        Err(_) => Outcome::Current(v),
    }
}

/// True when the file on disk was written by a newer release. Used right before a write.
pub fn on_disk_is_newer(spec: &Spec, path: &Path) -> Option<u32> {
    let v: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    Some(version_of(&v)).filter(|n| *n > spec.current)
}

/// The refusal sentence for a read-only store.
pub fn newer_message(spec: &Spec, found: u32) -> String {
    format!("The {} file was written by a newer IYAKKI (schema {found}; this one reads up to {}). It is kept as it is and cannot be changed here; update IYAKKI first.", spec.name, spec.current)
}

/// Write a store file: refuses when the file on disk is newer, stamps the schema, writes atomically.
/// A bare list is wrapped by the caller before (see [`wrap_list`]); this never changes a value's shape.
pub fn save(spec: &Spec, path: &Path, value: &Value, mode: u32) -> Result<(), String> {
    if let Some(n) = on_disk_is_newer(spec, path) {
        return Err(newer_message(spec, n));
    }
    let mut v = value.clone();
    if let Value::Object(o) = &mut v {
        o.insert(KEY.into(), Value::from(spec.current));
    }
    write_atomic(path, &serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?, mode)
}

/// [`save`] for a typed value.
pub fn save_typed<T: serde::Serialize>(spec: &Spec, path: &Path, value: &T, mode: u32) -> Result<(), String> {
    save(spec, path, &serde_json::to_value(value).map_err(|e| e.to_string())?, mode)
}

/// Read a store file of a typed struct: the outcome, the value (defaults when the file is missing or damaged), and the note for the audit.
pub fn load_typed<T: serde::de::DeserializeOwned + Default>(spec: &Spec, path: &Path, now: u64) -> (Outcome, T, Option<String>) {
    let out = open(spec, path, now);
    let val = match &out {
        Outcome::Current(v) | Outcome::Migrated { value: v, .. } | Outcome::Newer { value: v, .. } => serde_json::from_value::<T>(v.clone()).ok(),
        _ => None,
    };
    // an object that does not fit the struct at all counts as damaged (moved aside), never as a reason to overwrite
    if val.is_none() && matches!(out, Outcome::Current(_) | Outcome::Migrated { .. }) {
        let moved = move_aside(path, now);
        let o = Outcome::Corrupt { moved_to: moved, reason: "the contents do not fit this release".into() };
        let note = o.note(spec, path);
        if let Some(n) = &note {
            report(n.clone());
        }
        return (o, T::default(), note);
    }
    let note = out.note(spec, path);
    if let Some(n) = &note {
        report(n.clone());
    }
    (out, val.unwrap_or_default(), note)
}

#[cfg(test)]
mod tests;
