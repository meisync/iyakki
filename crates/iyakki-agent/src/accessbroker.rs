//! The local side of the broker (docs/development/CORE-ACCESS-MODEL.md sections 6b and 8, slice 2): the approvals read from the store's
//! file, the access log kept in one bounded file on this machine, and the broker made from them. The rules are in
//! `iyakki_core_api::broker`.
//!
//! The access log (`<data>/access/log.jsonl`, mode 0600) is a ring of the latest [`RING`] events, newest last. It holds who (app, person,
//! device name), what capability, which unit by name, when and allowed or refused: no content, no paths, no pictures. It never leaves the
//! machine. When it grows past the ring by a tenth the oldest lines are dropped in one rewrite.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iyakki_core_api::appcaps::Approval;
use iyakki_core_api::broker::{AccessEvent, AccessSink, ApprovalLookup, Broker, CardAsk, NoCard};

/// The shared types, so a caller of this module needs no second dependency.
pub use iyakki_core_api::{appcaps, broker};

/// The most events kept.
pub const RING: usize = 5000;

/// The layout of one log line, written as `"v"` on every line this release adds (a line without it is the alpha.2 layout, version 0).
/// A reader skips what it cannot parse; trimming keeps every kept line as it is on disk, so fields of a newer release survive.
pub const LINE_VERSION: u32 = 1;

fn log_path(data: &Path) -> PathBuf {
    data.join("access").join("log.jsonl")
}

/// The bounded file log.
pub struct FileLog {
    path: PathBuf,
    cap: usize,
    /// Lines in the file, counted on first use.
    lines: Mutex<Option<usize>>,
}

impl FileLog {
    pub fn open(data: &Path) -> FileLog {
        FileLog::with_cap(data, RING)
    }

    pub fn with_cap(data: &Path, cap: usize) -> FileLog {
        FileLog { path: log_path(data), cap: cap.max(1), lines: Mutex::new(None) }
    }

    fn read_all(&self) -> Vec<AccessEvent> {
        let Ok(text) = std::fs::read_to_string(&self.path) else { return vec![] };
        text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// The latest events, newest first.
    pub fn recent(&self, limit: usize) -> Vec<AccessEvent> {
        let mut v = self.read_all();
        v.reverse();
        v.truncate(limit);
        v
    }

    /// Empty the log (the person asked). The caller writes the audit line.
    pub fn clear(&self) {
        let mut n = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        let _ = std::fs::remove_file(&self.path);
        *n = Some(0);
    }
}

impl AccessSink for FileLog {
    fn record(&self, event: AccessEvent) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let Ok(mut value) = serde_json::to_value(&event) else { return };
        if let Some(o) = value.as_object_mut() {
            o.insert("v".into(), LINE_VERSION.into());
        }
        let Ok(mut line) = serde_json::to_string(&value) else { return };
        line.push('\n');
        let mut n = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(d) = self.path.parent() {
            if std::fs::create_dir_all(d).is_err() {
                return;
            }
        }
        let count = *n.get_or_insert_with(|| std::fs::read_to_string(&self.path).map(|t| t.lines().count()).unwrap_or(0));
        let wrote = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&self.path).and_then(|mut f| f.write_all(line.as_bytes())).is_ok();
        if !wrote {
            return;
        }
        *n = Some(count + 1);
        if count + 1 > self.cap + self.cap / 10 {
            // raw lines, not the typed event: a field a newer release added to a line is not lost by a trim
            let text = std::fs::read_to_string(&self.path).unwrap_or_default();
            let all: Vec<&str> = text.lines().filter(|l| serde_json::from_str::<AccessEvent>(l).is_ok()).collect();
            let keep = &all[all.len().saturating_sub(self.cap)..];
            let mut out = String::new();
            for l in keep {
                out.push_str(l);
                out.push('\n');
            }
            let tmp = self.path.with_extension("tmp");
            if std::fs::OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| f.write_all(out.as_bytes())).is_ok() && std::fs::rename(&tmp, &self.path).is_ok() {
                *n = Some(keep.len());
            }
        }
    }
}

/// The approvals of the store, read afresh at every call so a revoke applies at the next call.
pub struct FileLookup(pub PathBuf);

impl FileLookup {
    pub fn for_data(data: &Path) -> FileLookup {
        FileLookup(data.join("store").join("approvals.json"))
    }
}

impl ApprovalLookup for FileLookup {
    fn approval(&self, installation: &str) -> Option<Approval> {
        crate::store::approvals::read_map(&self.0).get(installation).cloned()
    }
}

/// The broker of this machine: approvals from the store, the file log, the given card (none: hard asks are refused with `no_card`).
pub fn local(data: &Path, card: Option<Arc<dyn CardAsk>>) -> (Arc<Broker>, Arc<FileLog>) {
    let log = Arc::new(FileLog::open(data));
    let card: Arc<dyn CardAsk> = card.unwrap_or_else(|| Arc::new(NoCard));
    (Arc::new(Broker::new(Arc::new(FileLookup::for_data(data)), card, log.clone(), Arc::new(crate::clock::now_unix))), log)
}

/// One installation with what it holds, for Settings, Apps access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub installation: String,
    pub app: String,
    pub version: String,
    pub who: String,
    /// Capability names still held (granted and not revoked).
    pub holds: Vec<String>,
    /// Capability names taken away.
    pub revoked: Vec<String>,
}

/// Every stored approval, in a stable order.
pub fn held_list(data: &Path) -> Vec<Held> {
    let all = crate::store::approvals::read_map(&data.join("store").join("approvals.json"));
    all.into_iter()
        .map(|(installation, a)| {
            let mut holds: Vec<String> = a.grants.iter().filter(|g| a.holds(&g.name, g.param.as_deref())).map(|g| g.name.clone()).collect();
            holds.dedup();
            let mut revoked: Vec<String> = a.revoked.iter().map(|g| g.name.clone()).collect();
            revoked.dedup();
            Held { installation, app: a.app, version: a.app_version, who: a.who, holds, revoked }
        })
        .collect()
}

/// Take a capability away from an installation. `Ok(false)`: it did not hold it. The caller writes the audit line (`access.revoke`).
pub fn revoke(data: &Path, installation: &str, capability: &str) -> Result<bool, String> {
    let store = crate::store::Store::open(data);
    let mut book = crate::store::approvals::FileBook(&store);
    use iyakki_core_api::appcaps::ApprovalBook;
    let Some(mut a) = book.get(installation) else { return Err("no such installation".into()) };
    let changed = a.revoke(capability);
    if changed {
        book.put(installation, a);
    }
    Ok(changed)
}

#[cfg(test)]
mod tests;
