//! Runs on disk: one folder per run with `run.json` (written after every transition, by rename) and a rotating text log.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::model::*;

pub const LOG_ROTATE_BYTES: u64 = 512 * 1024;

/// How long finished runs are kept (a tenant's `flows.quota`: `history_keep` and `history_days`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retention {
    /// Finished runs kept per flow (at least 1).
    pub keep: usize,
    /// Finished runs older than this many days go (0: no age limit).
    pub days: u64,
}

/// One run the retention rule would remove, and why (the dry-run listing shows exactly these).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PruneItem {
    pub id: String,
    pub flow_id: String,
    pub reason: String,
}

/// Which finished runs does the rule remove? Only whole runs started by a trigger or a person (not sub-runs on their own: those go with
/// their parent); never a run that is still going, waits for someone or has an open fix card; the newest finished run of each flow is
/// always kept. Nothing is deleted here: this is the list (the dry run), and `RunStore::delete` does the removing.
pub fn plan_retention(runs: &[RunSummary], r: &Retention, now_ms: i64) -> Vec<PruneItem> {
    let keep = r.keep.max(1);
    let cutoff = if r.days == 0 { None } else { Some(now_ms - (r.days.min(36_500) as i64) * 86_400_000) };
    let mut by_flow: std::collections::BTreeMap<&str, Vec<&RunSummary>> = Default::default();
    for s in runs.iter().filter(|s| s.parent.is_none()) {
        by_flow.entry(s.flow_id.as_str()).or_default().push(s);
    }
    let mut out: Vec<PruneItem> = Vec::new();
    for (flow, mut v) in by_flow {
        v.sort_by_key(|s| std::cmp::Reverse((s.created_ms, s.id.len(), s.id.clone())));
        // the rank counts finished runs only: a run that is still going does not push a finished one out early
        let mut rank = 0usize;
        for s in v {
            if !s.status.is_terminal() {
                continue;
            }
            rank += 1;
            if s.cards > 0 || !s.blocked.is_empty() {
                continue;
            }
            let at = if s.ended_ms > 0 { s.ended_ms } else { s.created_ms };
            let reason = if rank > keep {
                format!("more than {keep} finished runs of this flow")
            } else if rank > 1 && cutoff.is_some_and(|c| at < c) {
                format!("finished more than {} days ago", r.days)
            } else {
                continue;
            };
            out.push(PruneItem { id: s.id.clone(), flow_id: flow.to_string(), reason });
        }
    }
    // sub-runs go with their parent
    let parents: std::collections::BTreeSet<String> = out.iter().map(|i| i.id.clone()).collect();
    for s in runs.iter().filter(|s| s.status.is_terminal()) {
        if let Some(p) = &s.parent {
            if parents.contains(p) {
                out.push(PruneItem { id: s.id.clone(), flow_id: s.flow_id.clone(), reason: format!("a part of run {p}") });
            }
        }
    }
    out
}

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

    fn sum(id: &str, flow: &str, created: i64, ended: i64, st: RunStatus) -> RunSummary {
        let mut f = Flow::new(flow, flow);
        f.nodes.push(Node::new("s", "start", serde_json::json!({})));
        let mut r = crate::graph::test_run(f);
        r.id = id.into();
        r.created_ms = created;
        r.ended_ms = ended;
        r.status = st;
        r.summary()
    }

    #[test]
    fn retention_lists_by_count_and_by_age_and_spares_the_newest_and_the_unfinished() {
        let day = 86_400_000i64;
        let now = 100 * day;
        let runs = vec![
            sum("r1", "a", 1 * day, 1 * day + 5, RunStatus::Succeeded),
            sum("r2", "a", 50 * day, 50 * day + 5, RunStatus::Failed),
            sum("r3", "a", 99 * day, 99 * day + 5, RunStatus::Succeeded),
            sum("r4", "a", 0, 0, RunStatus::Running),
            sum("r5", "b", 2 * day, 2 * day + 5, RunStatus::Succeeded),
            sum("r6", "c", 60 * day, 60 * day + 5, RunStatus::Succeeded),
        ];
        // by age (30 days): r1 and r2 are old, r3 is new; flow b's only run is old but it is the newest of its flow, so it stays
        let by_age: Vec<_> = plan_retention(&runs, &Retention { keep: 100, days: 30 }, now).into_iter().map(|i| i.id).collect();
        assert_eq!(by_age, vec!["r2".to_string(), "r1".to_string()]);
        // by count: keep the two newest finished runs of a (r3, r2)
        let by_count = plan_retention(&runs, &Retention { keep: 2, days: 0 }, now);
        assert_eq!(by_count.len(), 1);
        assert_eq!((by_count[0].id.as_str(), by_count[0].flow_id.as_str()), ("r1", "a"));
        assert!(by_count[0].reason.contains("more than 2"));
        // a keep of 0 is read as 1
        assert_eq!(plan_retention(&runs, &Retention { keep: 0, days: 0 }, now).len(), 2);
        // nothing is chosen for a running run, ever
        assert!(plan_retention(&runs, &Retention { keep: 1, days: 1 }, now).iter().all(|i| i.id != "r4"));
    }

    #[test]
    fn retention_spares_runs_with_open_cards_or_waiting_and_takes_sub_runs_with_their_parent() {
        let day = 86_400_000i64;
        let mut waiting = sum("r1", "a", 1 * day, 1 * day, RunStatus::Failed);
        waiting.blocked = "approval".into();
        let mut cards = sum("r2", "a", 2 * day, 2 * day, RunStatus::Failed);
        cards.cards = 1;
        let old = sum("r3", "a", 3 * day, 3 * day, RunStatus::Succeeded);
        let mut sub = sum("r4", "a", 3 * day, 3 * day, RunStatus::Succeeded);
        sub.parent = Some("r3".into());
        let newest = sum("r5", "a", 90 * day, 90 * day, RunStatus::Succeeded);
        let plan = plan_retention(&[waiting, cards, old, sub, newest], &Retention { keep: 100, days: 30 }, 100 * day);
        let ids: Vec<&str> = plan.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["r3", "r4"], "{plan:?}");
        assert!(plan[1].reason.contains("part of run r3"));
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
