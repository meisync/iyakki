//! Runs on disk: one folder per run with `run.json` (written after every transition, by rename) and a rotating text log.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::model::*;

pub const LOG_ROTATE_BYTES: u64 = 512 * 1024;

pub struct RunStore {
    root: PathBuf,
}

impl RunStore {
    pub fn new(root: &Path) -> RunStore {
        RunStore { root: root.join("runs") }
    }

    fn dir(&self, id: &str) -> Option<PathBuf> {
        if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return None;
        }
        Some(self.root.join(id))
    }

    pub fn save(&self, run: &Run) -> Result<(), String> {
        let dir = self.dir(&run.id).ok_or("bad run id")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let tmp = dir.join(format!("run.json.tmp{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let text = serde_json::to_string(run).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, dir.join("run.json")).map_err(|e| e.to_string())
    }

    pub fn load(&self, id: &str) -> Option<Run> {
        serde_json::from_str(&std::fs::read_to_string(self.dir(id)?.join("run.json")).ok()?).ok()
    }

    pub fn ids(&self) -> Vec<String> {
        std::fs::read_dir(&self.root).map(|rd| rd.flatten().filter(|e| e.path().join("run.json").is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
    }

    pub fn delete(&self, id: &str) {
        if let Some(d) = self.dir(id) {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// Append a line to the run's log; at 512 KB the log becomes `log.1.txt` (one older generation is kept).
    pub fn append_log(&self, id: &str, line: &str) {
        let Some(dir) = self.dir(id) else { return };
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("log.txt");
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) >= LOG_ROTATE_BYTES {
            let _ = std::fs::rename(&path, dir.join("log.1.txt"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{line}");
        }
    }

    /// The log as text: the older generation, then the current one.
    pub fn read_log(&self, id: &str) -> String {
        let Some(dir) = self.dir(id) else { return String::new() };
        let mut s = std::fs::read_to_string(dir.join("log.1.txt")).unwrap_or_default();
        s.push_str(&std::fs::read_to_string(dir.join("log.txt")).unwrap_or_default());
        s
    }

    /// Keep the newest `keep` finished runs of each flow; `unfinished` runs are never removed. Returns the ids removed.
    pub fn prune(&self, runs: &[RunSummary], keep: usize) -> Vec<String> {
        let mut by_flow: std::collections::BTreeMap<&str, Vec<&RunSummary>> = Default::default();
        for r in runs.iter().filter(|r| r.status.is_terminal()) {
            by_flow.entry(r.flow_id.as_str()).or_default().push(r);
        }
        let mut gone = Vec::new();
        for (_, mut v) in by_flow {
            v.sort_by_key(|r| std::cmp::Reverse(r.created_ms));
            for r in v.into_iter().skip(keep.max(1)) {
                self.delete(&r.id);
                gone.push(r.id.clone());
            }
        }
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("flow-runs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn logs_rotate_and_ids_are_safe() {
        let d = tmp("log");
        let s = RunStore::new(&d);
        for i in 0..3000 {
            s.append_log("r1", &format!("line {i} {}", "x".repeat(300)));
        }
        let dir = d.join("runs/r1");
        assert!(dir.join("log.1.txt").exists(), "rotated");
        assert!(std::fs::metadata(dir.join("log.txt")).unwrap().len() < LOG_ROTATE_BYTES + 1000);
        assert!(!dir.join("log.2.txt").exists(), "one older generation only");
        let all = s.read_log("r1");
        assert!(all.contains("line 2999") && all.lines().count() > 1000);
        s.append_log("../evil", "x");
        assert!(!d.join("evil").exists() && !d.join("runs/../evil").exists());
        assert!(s.load("../x").is_none());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn pruning_keeps_the_newest_finished_and_every_unfinished() {
        let d = tmp("prune");
        let s = RunStore::new(&d);
        let mk = |id: &str, flow: &str, at: i64, st: RunStatus| {
            let mut f = Flow::new(flow, flow);
            f.nodes.push(Node::new("s", "start", serde_json::json!({})));
            let mut r = crate::graph::test_run(f);
            r.id = id.into();
            r.created_ms = at;
            r.status = st;
            s.save(&r).unwrap();
            r.summary()
        };
        let sums = vec![mk("r1", "a", 1, RunStatus::Succeeded), mk("r2", "a", 2, RunStatus::Failed), mk("r3", "a", 3, RunStatus::Succeeded), mk("r4", "a", 0, RunStatus::Running), mk("r5", "b", 1, RunStatus::Succeeded)];
        let gone = s.prune(&sums, 2);
        assert_eq!(gone, vec!["r1"]);
        assert!(s.load("r1").is_none() && s.load("r2").is_some() && s.load("r4").is_some() && s.load("r5").is_some());
        let _ = std::fs::remove_dir_all(d);
    }
}
