//! Where the app events of this machine are kept: one JSON file per person and app under `<data>/appevents/` (`<user>.<app>.json`: the
//! registrations, the queue, the next sequence number and the last delivered events). The engine is `iyakki_core_api::appevents`; this is
//! its store and its clock. Names are checked (letters, digits, `-`, `_`), so a name never walks out of the folder. A file that does not
//! parse is ignored (the app registers again) and never stops the machine.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_core_api::appevents::{EventStore, FnScopes, SharedHub};

fn name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 48 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    pub fn new(data_dir: &Path) -> FileStore {
        FileStore { dir: data_dir.join("appevents") }
    }
}

impl EventStore for FileStore {
    fn load_all(&self) -> Vec<(String, String)> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut out = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(stem) = name.strip_suffix(".json") else { continue };
            let Some((u, a)) = stem.split_once('.') else { continue };
            if !name_ok(u) || !name_ok(a) {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(e.path()) {
                out.push((format!("{u}/{a}"), text));
            }
        }
        out
    }
    fn save(&self, key: &str, json: &str) {
        let Some((u, a)) = key.split_once('/') else { return };
        if !name_ok(u) || !name_ok(a) || std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let path = self.dir.join(format!("{u}.{a}.json"));
        let tmp = self.dir.join(format!("{u}.{a}.tmp"));
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The app events of this machine: the engine over a file store and the system clock. `holds(user, app, scope)` is the core's capability
/// check (the built-in table, the person's approval record).
pub fn local_app_events(data_dir: &Path, holds: impl Fn(&str, &str, &str) -> bool + Send + Sync + 'static) -> Arc<SharedHub> {
    local_app_events_with(data_dir, Box::new(FnScopes(holds)))
}

/// Like [`local_app_events`] with a scope check of its own (which also answers the file policy for `files.changed`).
pub fn local_app_events_with(data_dir: &Path, scopes: Box<dyn iyakki_core_api::appevents::ScopeCheck>) -> Arc<SharedHub> {
    local_app_events_clocked(data_dir, scopes, Box::new(now_ms))
}

/// Like [`local_app_events_with`] with a clock of the caller's (a test moves it).
pub fn local_app_events_clocked(data_dir: &Path, scopes: Box<dyn iyakki_core_api::appevents::ScopeCheck>, clock: Box<dyn Fn() -> u64 + Send + Sync>) -> Arc<SharedHub> {
    Arc::new(SharedHub::new(Box::new(FileStore::new(data_dir)), scopes, clock))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::appevents::{payload, AppEvents, Val};

    fn dir(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-appevents-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn registrations_and_the_queue_are_kept_in_files_and_come_back() {
        let d = dir("keep");
        let a = local_app_events(&d, |_, _, _| true);
        a.register("pat", "notes", "connectivity.changed", "").unwrap();
        a.emit("connectivity.changed", None, payload(&[("state", Val::Text("offline".into()))])).unwrap();
        assert!(d.join("appevents/pat.notes.json").exists());
        let b = local_app_events(&d, |_, _, _| true);
        assert_eq!(b.list("pat", "notes").len(), 1);
        let got = b.start("pat", "notes");
        assert_eq!(got.len(), 1);
        assert!(got[0].late && got[0].seq == 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bad_names_are_not_written_and_a_bad_file_is_ignored() {
        let d = dir("bad");
        let a = local_app_events(&d, |_, _, _| true);
        a.register("pat", "../evil", "connectivity.changed", "").unwrap();
        assert!(!d.join("evil.json").exists());
        std::fs::create_dir_all(d.join("appevents")).unwrap();
        std::fs::write(d.join("appevents/pat.broken.json"), "{not json").unwrap();
        let b = local_app_events(&d, |_, _, _| true);
        assert!(b.apps("pat").is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
