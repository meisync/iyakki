//! FLOWS, a failed run reaches the INBOX. When a flow's newest top-level run has failed (manual, scheduled or triggered) and the
//! person has not turned the offer off for it (general switch or per flow, `failfix::FailBook`), one INBOX note says which flow
//! failed at which step and why, and links to the run. A run is noted once (a small file of run ids, separate from the failure
//! cards' book so the two writers never overwrite each other); a step that waits for a permission or a choice already has its own
//! note and is skipped; only failures from the last day are noted, so signing in after a long time does not flood the INBOX.
//! The note carries the redacted failure line of `failfix` (no input value, no credential) and never starts the assistant.

use std::collections::BTreeSet;
use std::path::Path;

use iyakki_flow::RunStatus;
use serde::{Deserialize, Serialize};

use super::failfix::{failure_of, FailBook};
use super::FlowEnv;
use crate::inbox::{Kind, Link};

/// Only failures younger than this are noted.
const FRESH_MS: i64 = 24 * 3600 * 1000;
/// Run ids remembered.
const KEEP: usize = 300;

#[derive(Default, Serialize, Deserialize)]
struct Noted {
    #[serde(default)]
    runs: Vec<String>,
}

fn file(dir: &Path) -> std::path::PathBuf {
    dir.join("failure_notes.json")
}

fn load(dir: &Path) -> Noted {
    std::fs::read_to_string(file(dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

impl FlowEnv {
    /// One pass: INBOX notes for flows whose newest run failed and was not noted yet. Returns a line per note made.
    pub fn notify_new_failures(&self) -> Vec<String> {
        let runs = self.engine.summaries();
        let now = super::now_ms();
        let mut flows: BTreeSet<String> = BTreeSet::new();
        for r in &runs {
            flows.insert(r.flow_id.clone());
        }
        let book = FailBook::open(&self.data);
        let mut noted = load(&self.data);
        let mut out = Vec::new();
        for flow in flows {
            // the newest finished top-level run decides, as for the failure card
            let newest = runs.iter().filter(|r| r.flow_id == flow && r.parent.is_none() && r.status.is_terminal() && r.status != RunStatus::Cancelled).max_by_key(|r| (r.created_ms, r.id.clone()));
            let Some(newest) = newest else { continue };
            if newest.status != RunStatus::Failed || noted.runs.contains(&newest.id) || now - newest.created_ms > FRESH_MS || !book.offer_for(&flow) {
                continue;
            }
            let Some(run) = self.engine.get_run(&newest.id) else { continue };
            let Some(f) = failure_of(&run) else { continue };
            noted.runs.push(newest.id.clone());
            if f.needs_person || f.choice {
                continue;
            }
            let title = format!("Flow failed: {}", f.flow_name.chars().take(60).collect::<String>());
            let body = format!("{} Open FLOWS to look at the log or ask the assistant for a fix; it changes nothing without your approval.", f.headline());
            if self.svc.notify(&self.user, Kind::Error, &title, &body.chars().take(600).collect::<String>(), Some(Link::new("flows", &f.run_id))).is_ok() {
                out.push(format!("{}: INBOX note for run {}", f.flow_id, f.run_id));
            }
        }
        if !out.is_empty() || noted.runs.len() > KEEP {
            let skip = noted.runs.len().saturating_sub(KEEP);
            noted.runs.drain(..skip);
            let _ = std::fs::create_dir_all(&self.data);
            let _ = std::fs::write(file(&self.data), serde_json::to_string(&noted).unwrap_or_default());
        }
        out
    }
}
