//! Declarative data migrations of an installed app (F-CORE-28): a file in the app's own package, `migrations/NAME.json`, lists plain steps that
//! change the files in the app's data folder. The wizard step `migration` names it; the real wizard environment runs it from a snapshot with
//! rollback (`bundlesnap::run_with_rollback`), so a step that fails puts the data back as it was.
//!
//! Steps (all paths are inside the data folder: relative, no `..`, no links followed):
//!   {"op": "rename", "from": "a.json", "to": "b.json"}
//!   {"op": "delete", "file": "cache.tmp"}
//!   {"op": "json_rename_key", "file": "settings.json", "from": "colour", "to": "color"}
//!   {"op": "json_set_default", "file": "settings.json", "key": "theme", "value": "dark"}
//!   {"op": "json_remove_key", "file": "settings.json", "key": "old"}
//! The file is `{"ops": [...]}`. At most 64 steps and 2 MB per JSON file; an unknown step is refused before anything runs. No code of the app runs.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

pub const MAX_OPS: usize = 64;
pub const MAX_JSON_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    Rename { from: String, to: String },
    Delete { file: String },
    JsonRenameKey { file: String, from: String, to: String },
    JsonSetDefault { file: String, key: String, value: Value },
    JsonRemoveKey { file: String, key: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    ops: Vec<Op>,
}

/// Read the steps of a migration file. Nothing is touched.
pub fn parse(text: &str) -> Result<Vec<Op>, String> {
    let f: File = serde_json::from_str(text).map_err(|e| format!("the migration file is not in the agreed form: {e}"))?;
    if f.ops.is_empty() || f.ops.len() > MAX_OPS {
        return Err(format!("a migration has 1 to {MAX_OPS} steps"));
    }
    for op in &f.ops {
        for p in paths(op) {
            safe(p)?;
        }
    }
    Ok(f.ops)
}

fn paths(op: &Op) -> Vec<&str> {
    match op {
        Op::Rename { from, to } => vec![from, to],
        Op::Delete { file } | Op::JsonRenameKey { file, .. } | Op::JsonSetDefault { file, .. } | Op::JsonRemoveKey { file, .. } => vec![file],
    }
}

/// A path inside the data folder: not empty, relative, no `..`, plain names.
fn safe(p: &str) -> Result<(), String> {
    let path = Path::new(p);
    if p.is_empty() || p.len() > 200 || path.is_absolute() || path.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(format!("{p:?} is not a file inside the app's data folder"));
    }
    Ok(())
}

fn inside(data: &Path, rel: &str) -> Result<PathBuf, String> {
    safe(rel)?;
    let full = data.join(rel);
    // no link on the way may lead out of the folder
    let mut cur = data.to_path_buf();
    for c in Path::new(rel).components() {
        cur.push(c);
        if std::fs::symlink_metadata(&cur).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!("{rel:?} goes through a link, which a migration does not follow"));
        }
    }
    Ok(full)
}

fn read_json(path: &Path) -> Result<Value, String> {
    let md = std::fs::metadata(path).map_err(|_| format!("{} is not there", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))?;
    if md.len() > MAX_JSON_BYTES {
        return Err("a JSON file is larger than the migration limit".into());
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not JSON: {e}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
}

fn write_json(path: &Path, v: &Value) -> Result<(), String> {
    let tmp = path.with_extension("migrating");
    std::fs::write(&tmp, serde_json::to_string_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Run the steps in `data`, one after the other; the first that fails ends it (the caller puts the snapshot back). Returns a plain line.
pub fn apply(data: &Path, ops: &[Op]) -> Result<String, String> {
    let mut done = 0;
    for op in ops {
        match op {
            Op::Rename { from, to } => {
                let (a, b) = (inside(data, from)?, inside(data, to)?);
                if !a.exists() {
                    return Err(format!("{from} is not there"));
                }
                if b.exists() {
                    return Err(format!("{to} is already there"));
                }
                if let Some(d) = b.parent() {
                    std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
                }
                std::fs::rename(&a, &b).map_err(|e| e.to_string())?;
            }
            Op::Delete { file } => {
                let p = inside(data, file)?;
                if p.is_dir() {
                    return Err(format!("{file} is a folder; a migration deletes files"));
                }
                // a file that is already gone is what the step wanted
                let _ = std::fs::remove_file(&p);
            }
            Op::JsonRenameKey { file, from, to } => {
                let p = inside(data, file)?;
                let mut v = read_json(&p)?;
                let o = v.as_object_mut().ok_or_else(|| format!("{file} is not a JSON object"))?;
                if let Some(x) = o.remove(from) {
                    if o.contains_key(to) {
                        return Err(format!("{file} already has {to}"));
                    }
                    o.insert(to.clone(), x);
                    write_json(&p, &v)?;
                }
            }
            Op::JsonSetDefault { file, key, value } => {
                let p = inside(data, file)?;
                let mut v = read_json(&p)?;
                let o = v.as_object_mut().ok_or_else(|| format!("{file} is not a JSON object"))?;
                if !o.contains_key(key) {
                    o.insert(key.clone(), value.clone());
                    write_json(&p, &v)?;
                }
            }
            Op::JsonRemoveKey { file, key } => {
                let p = inside(data, file)?;
                let mut v = read_json(&p)?;
                let o = v.as_object_mut().ok_or_else(|| format!("{file} is not a JSON object"))?;
                if o.remove(key).is_some() {
                    write_json(&p, &v)?;
                }
            }
        }
        done += 1;
    }
    Ok(format!("{done} migration step{} done", if done == 1 { "" } else { "s" }))
}

/// The text of the app's migration file `migrations/NAME.json`, from its installed package. `None`: no such file (or the name is not a name).
pub fn migration_text(store: &crate::store::Store, user: &str, app: &str, reference: &str) -> Option<String> {
    if reference.is_empty() || reference.len() > 60 || !reference.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) || reference.contains("..") {
        return None;
    }
    let file = format!("migrations/{reference}.json");
    store.content("tile", user).into_iter().filter(|c| c.name == app).find_map(|c| crate::store::tile::Bundle::from_payload(&c.payload).ok().and_then(|b| b.text(&file).map(str::to_string)))
}

/// The app's WASM migrator `migrations/NAME.wasm` (binary) or `NAME.wat` (text), if its package has one.
pub fn migration_module(store: &crate::store::Store, user: &str, app: &str, reference: &str) -> Option<Vec<u8>> {
    if reference.is_empty() || reference.len() > 60 || !reference.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) || reference.contains("..") {
        return None;
    }
    store.content("tile", user).into_iter().filter(|c| c.name == app).find_map(|c| {
        let b = crate::store::tile::Bundle::from_payload(&c.payload).ok()?;
        ["wasm", "wat"].iter().find_map(|e| b.files.get(&format!("migrations/{reference}.{e}")).map(|f| f.bytes().to_vec()))
    })
}

/// Run the app's migration `reference` over its data folder from a snapshot, rolled back when a step fails. A declarative file runs no code of
/// the app; a WASM migrator runs only inside the tile sandbox (`datamigrate_wasm`).
pub fn run_for_app(store: &crate::store::Store, user: &str, app: &str, reference: &str) -> Result<String, String> {
    if migration_text(store, user, app, "").is_none() && (reference.is_empty() || reference.len() > 60 || !reference.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) || reference.contains("..")) {
        return Err(format!("{reference:?} is not a migration name"));
    }
    let module = if migration_text(store, user, app, reference).is_none() { migration_module(store, user, app, reference) } else { None };
    let text = if module.is_some() { String::new() } else { migration_text(store, user, app, reference).ok_or_else(|| format!("the app has no migration called {reference} (migrations/{reference}.json, .wasm or .wat is not in its package); its data was not touched"))? };
    let ops = if module.is_some() { Vec::new() } else { parse(&text).map_err(|e| format!("{e}; the app's data was not touched"))? };
    let key = store.records().into_iter().find(|(_, r)| r.name == app).map(|(k, _)| k).ok_or("that app is not installed")?;
    let Some(data) = store.app_data_dir(&key) else { return Ok("the app keeps no data yet, so there was nothing to move".into()) };
    let now = crate::clock::now_unix();
    // a counter beside the second: two updates within one second must not meet the same snapshot name
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = format!("migration-{}-{now}-{}", reference.replace('.', "-"), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    crate::bundlesnap::run_with_rollback(&data, &store.snapshot_root(&key), &id, now, crate::store::Store::SNAPSHOT_MAX_BYTES, &|d| match &module {
        Some(m) => crate::datamigrate_wasm::apply(d, m),
        None => apply(d, &ops),
    })
}

/// The migration an update runs by itself: `migrations/on-update.json` of the NEW package. Returns the words for the update's answer, or
/// `None` when the package has none.
pub fn after_update(store: &crate::store::Store, user: &str, app: &str, from: &str, to: &str) -> Option<String> {
    if migration_text(store, user, app, "on-update").is_none() && migration_module(store, user, app, "on-update").is_none() {
        return None;
    }
    Some(match run_for_app(store, user, app, "on-update") {
        Ok(m) => format!(" Its data was moved to the layout of {to} ({m})."),
        Err(e) => format!(" The data move that goes with {to} failed and the data was put back as it was in {from}: {e}"),
    })
}

#[cfg(test)]
#[path = "datamigrate_tests.rs"]
mod tests;
