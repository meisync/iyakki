//! Who is working on what: the agents of a workspace (parallel tasks, Autopilot, external coding agents), their rows, their state and
//! the files they hold. Kept in `.iyakki/board-assignments.json`, not in FEATURES.md, so the markdown stays clean; the Board's Agents
//! strip and the drill-down panel read it, and a restart keeps it (an agent whose run is gone is shown as interrupted, never as working).
//!
//! * Each agent has a stable display name (`Agent 1`, `Agent 2`, or the name the person gave it) and an engine (`Claude Code (sonnet)`).
//! * One row has one active agent: [`Doc::claim_row`] refuses a second.
//! * Files are soft locks: an agent that touches a file another active agent already touched is recorded as a [`Conflict`]; the person
//!   chooses to queue the second agent or let it continue (default: continue, with a visible badge).
//! * The timeline ([`Step`]) keeps the plan, tool calls with short outputs, tests, board moves and notes, capped.


pub use iyakki_shared::assign::*;

/// Helpers of other modules' tests.
#[cfg(test)]
pub mod tests_support {
    use crate::agent::{ToolDef, Tools};
    use serde_json::Value;

    /// Tools that have nothing.
    pub struct NoTools;
    impl Tools for NoTools {
        fn defs(&self) -> Vec<ToolDef> {
            vec![]
        }
        fn call(&self, name: &str, _args: &Value) -> Result<String, String> {
            Err(format!("no tool {name}"))
        }
        fn describe(&self, name: &str, _args: &Value) -> String {
            name.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-assign-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn rows(r: &[&str]) -> Vec<String> {
        r.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn names_are_stable_numbered_and_renamable() {
        let mut d = Doc::default();
        assert_eq!(d.register("t1", "task", "Claude Code (sonnet)", &rows(&["F-A-1"]), "t1", 10), "Agent 1");
        assert_eq!(d.register("t2", "task", "built-in (qwen)", &rows(&["F-A-2"]), "t2", 11), "Agent 2");
        assert_eq!(d.register("t1", "task", "Claude Code (sonnet)", &rows(&["F-A-1"]), "t1", 12), "Agent 1", "same run, same name");
        assert_eq!(d.get("t1").unwrap().title(), "Agent 1 · Claude Code (sonnet)");
        d.rename("t2", "Reviewer").unwrap();
        assert_eq!(d.get("t2").unwrap().name, "Reviewer");
        assert!(d.rename("t1", "reviewer").is_err());
        d.rename("t2", "").unwrap();
        assert_eq!(d.get("t2").unwrap().name, "Agent 2");
        d.set_state("t1", AState::Done, "", 20);
        assert_eq!(d.register("t3", "task", "", &rows(&[]), "t3", 21), "Agent 3", "a finished agent's name is not reused");
    }

    #[test]
    fn one_row_one_active_agent() {
        let mut d = Doc::default();
        d.register("t1", "task", "", &rows(&["F-A-1"]), "t1", 1);
        d.register("t2", "task", "", &rows(&[]), "t2", 1);
        let e = d.claim_row("t2", "F-A-1").unwrap_err();
        assert!(e.contains("already being worked on by Agent 1"), "{e}");
        assert_eq!(d.assignee("F-A-1"), Some(("Agent 1", AState::Working)));
        d.set_state("t1", AState::Failed, "gave up", 5);
        d.claim_row("t2", "F-A-1").unwrap();
        assert_eq!(d.assignee("F-A-1"), Some(("Agent 2", AState::Working)));
        assert!(d.assignee("F-Z-9").is_none());
    }

    #[test]
    fn two_agents_on_one_file_make_one_conflict_and_the_person_chooses() {
        let mut d = Doc::default();
        d.register("t1", "task", "", &rows(&["F-A-1"]), "t1", 1);
        d.register("t2", "task", "", &rows(&["F-A-2"]), "t2", 1);
        assert!(d.touch_file("t1", "src/lib.rs", 3, 1, 2).is_none());
        assert!(d.touch_file("t2", "docs/a.md", 1, 0, 3).is_none());
        let c = d.touch_file("t2", "./src/lib.rs", 1, 0, 4).expect("a conflict");
        assert_eq!((c.holder_name.as_str(), c.other_name.as_str(), c.choice), ("Agent 1", "Agent 2", Choice::Pending));
        assert!(d.touch_file("t2", "src/lib.rs", 2, 0, 5).is_none(), "once per pair and file");
        assert_eq!(d.live_conflicts().len(), 1);
        assert!(d.must_wait("t2", "src/lib.rs").is_none(), "by default the second agent continues");
        assert!(d.resolve_conflict("src/lib.rs", "t2", Choice::Queue));
        assert_eq!(d.must_wait("t2", "src/lib.rs").as_deref(), Some("Agent 1"));
        assert!(d.must_wait("t1", "src/lib.rs").is_none());
        assert!(d.get("t2").unwrap().steps.iter().any(|s| s.text.contains("also being changed by Agent 1")));
        d.set_state("t1", AState::Done, "", 9);
        assert!(d.must_wait("t2", "src/lib.rs").is_none(), "the holder finished: the queue is over");
        assert!(d.live_conflicts().is_empty());
    }

    #[test]
    fn the_timeline_is_capped_deduplicated_and_survives_the_file() {
        let root = dir("file");
        update(&root, |d| {
            d.register("t1", "task", "Claude Code (sonnet)", &rows(&["F-A-1", "F-A-2"]), "t1", 100);
            for i in 0..350 {
                d.step("t1", StepKind::Tool, &format!("call {i}"), 100 + i);
            }
            d.step("t1", StepKind::Tool, "call 349", 500);
            d.touch_file("t1", "a.rs", 4, 2, 101);
            d.note_added("t1", "N-004", "Use sqlite", 102);
            d.set_usage("t1", 1234, 1.5);
            d.set_progress("t1", 40);
        });
        let d = load(&root);
        let a = d.get("t1").unwrap();
        assert_eq!(a.steps.len(), MAX_STEPS);
        assert_eq!(a.steps.last().unwrap().kind, StepKind::Note);
        assert_eq!((a.tokens, a.progress, a.notes.clone(), a.files[0].adds), (1234, 40, vec!["N-004".to_string()], 4));
        assert_eq!(a.rows, ["F-A-1", "F-A-2"]);
        // a restart: nobody is alive, so the working agent is interrupted, never shown as working
        update(&root, |d| d.reconcile(&[], 900));
        let d = load(&root);
        assert_eq!(d.get("t1").unwrap().state, AState::Interrupted);
        assert!(d.assignee("F-A-1").is_none());
        assert!(d.get("t1").unwrap().steps.last().unwrap().text.contains("the window closed"));
        // a hand-broken file is an empty document
        std::fs::write(path_of(&root), "{ nope").unwrap();
        assert!(load(&root).agents.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tool_results_become_timeline_lines() {
        let (k, t) = classify("run_command", true, &json!({"program": "cargo", "args": ["test", "-q"]}), "test result: ok. 12 passed; 0 failed");
        assert_eq!(k, StepKind::Test);
        assert!(t.contains("cargo test -q") && t.contains("12 passed"));
        assert_eq!(classify("write_file", true, &json!({"path": "src/a.rs"}), "ok").1, "wrote src/a.rs");
        assert_eq!(classify("board_update", true, &json!({"id": "F-A-1"}), "updated F-A-1").0, StepKind::Board);
        assert_eq!(classify("read_file", true, &json!({}), "text").0, StepKind::Tool);
    }
}

#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use std::path::PathBuf;
