//! A log of what the assistant's tools did, for looking back: who, which tool, what exactly (the command line or the
//! address, never a header value or a file's contents), whether it worked and the start of the result. Entries older
//! than the retention (three days unless settings say otherwise) are dropped each time one is written, so the log
//! stays short. The tamper-evident audit log of the account system is separate and keeps its own history.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const DEFAULT_DAYS: u32 = 3;
pub const MAX_DAYS: u32 = 365;
const DAY: u64 = 86_400;
const MAX_FIELD: usize = 400;
const MAX_ENTRIES: usize = 5000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub ts: u64,
    pub user: String,
    pub tool: String,
    pub what: String,
    pub ok: bool,
    pub result: String,
}

pub struct ToolLog {
    path: PathBuf,
    lock: Mutex<()>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn cut(s: &str) -> String {
    let mut t: String = s.chars().take(MAX_FIELD).collect();
    if s.chars().count() > MAX_FIELD {
        t.push('…');
    }
    t
}

/// 0 means "not chosen": the default applies.
pub fn effective_days(chosen: u32) -> u32 {
    if chosen == 0 { DEFAULT_DAYS } else { chosen.min(MAX_DAYS) }
}

impl ToolLog {
    pub fn new(path: &Path) -> ToolLog {
        ToolLog { path: path.into(), lock: Mutex::new(()) }
    }

    fn load(&self) -> Vec<Entry> {
        std::fs::read_to_string(&self.path).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// Adds an entry and drops the ones older than `days`. Best effort: logging never stops a tool.
    pub fn record(&self, days: u32, user: &str, tool: &str, what: &str, ok: bool, result: &str) {
        self.record_at(now(), days, user, tool, what, ok, result);
    }

    pub fn record_at(&self, ts: u64, days: u32, user: &str, tool: &str, what: &str, ok: bool, result: &str) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let keep_from = ts.saturating_sub(effective_days(days) as u64 * DAY);
        let mut all: Vec<Entry> = self.load().into_iter().filter(|e| e.ts >= keep_from).collect();
        all.push(Entry { ts, user: user.into(), tool: tool.into(), what: cut(what), ok, result: cut(result) });
        if all.len() > MAX_ENTRIES {
            all.drain(..all.len() - MAX_ENTRIES);
        }
        let text: String = all.iter().filter_map(|e| serde_json::to_string(e).ok()).map(|l| l + "\n").collect();
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = self.path.with_extension("tmp");
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp) {
            if f.write_all(text.as_bytes()).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    /// Newest first; `user` limits it to one person's entries.
    pub fn list(&self, days: u32, user: Option<&str>) -> Vec<Entry> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let keep_from = now().saturating_sub(effective_days(days) as u64 * DAY);
        let mut v: Vec<Entry> = self.load().into_iter().filter(|e| e.ts >= keep_from && user.is_none_or(|u| e.user == u)).collect();
        v.reverse();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(tag: &str) -> ToolLog {
        let d = std::env::temp_dir().join(format!("iyakki-toollog-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        ToolLog::new(&d.join("tools.jsonl"))
    }

    #[test]
    fn entries_older_than_the_retention_are_dropped_three_days_by_default() {
        let l = log("ret");
        let t = now();
        l.record_at(t - 4 * DAY, 0, "alice", "fetch_url", "fetch http://a", true, "old");
        l.record_at(t - 2 * DAY, 0, "alice", "fetch_url", "fetch http://b", true, "recent"); // writing this drops the 4-day-old one
        l.record(0, "bob", "run_command", "aws s3 ls", false, "exit status 1");
        let all = l.list(0, None);
        assert_eq!(all.iter().map(|e| e.what.as_str()).collect::<Vec<_>>(), vec!["aws s3 ls", "fetch http://b"], "newest first, the old one gone");
        assert_eq!(l.list(0, Some("alice")).len(), 1);
        // a longer retention keeps more from now on
        l.record_at(t - 2 * DAY, 7, "alice", "x", "kept for a week", true, "");
        assert!(l.list(7, None).iter().any(|e| e.what == "kept for a week"));
        assert_eq!(effective_days(0), 3);
        assert_eq!(effective_days(9999), MAX_DAYS);
    }

    #[test]
    fn long_text_is_cut_and_the_file_is_private() {
        let l = log("cut");
        l.record(3, "alice", "read", &"x".repeat(5000), true, &"y".repeat(5000));
        let e = &l.list(3, None)[0];
        assert!(e.what.chars().count() <= MAX_FIELD + 1 && e.result.chars().count() <= MAX_FIELD + 1);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&l.path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
