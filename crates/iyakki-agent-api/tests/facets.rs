//! The hand-written facets answer "unavailable" by default and the fake needs no agent (F-SPLIT-03).

use std::path::Path;
use std::time::Duration;

use iyakki_agent_api::{AgentApi, AgentFacets, FakeAgentApi};
use iyakki_shared::workspace::LockState;

#[test]
fn the_fake_has_every_facet_and_they_answer_unavailable() {
    fn takes(_: &dyn AgentApi) {}
    let f = FakeAgentApi::default();
    takes(&f);
    let run = f.exec().system();
    assert!(!run.has("git"), "the fake runner knows no program");
    let o = f.exec().run_argv(Path::new("/"), &["true".into()], &[], Duration::from_secs(1), 10);
    assert!(!o.ok() && o.stderr.starts_with("unavailable"), "{o:?}");
    assert!(!f.exec().on_path("sh"));
    assert!(f.exec().git_init(Path::new("/nonexistent")).is_err());
}

#[test]
fn a_core_without_locks_lets_every_window_have_the_workspace() {
    let f = FakeAgentApi::default();
    let l = f.workspace_locks();
    assert_eq!(l.acquire(Path::new("/x"), "ann", "s1", 10), LockState::Mine);
    assert!(l.refresh(Path::new("/x"), "ann", "s1", 20));
    assert!(l.live(Path::new("/x"), 20).is_none());
}

#[test]
fn a_core_without_developer_mode_shows_no_sign_and_queues_nothing() {
    let f = FakeAgentApi::default();
    let d = f.devmode();
    let dir = Path::new("/nonexistent");
    assert!(!d.is_on(dir) && d.top_sign(dir).is_none() && !d.update_interrupted(dir) && !d.update_fell_back(dir));
    assert!(d.take_open(dir).is_empty() && d.take_input(dir).is_empty() && d.shell_pending(dir).is_empty());
    assert!(d.boot_confirmed(dir).is_none() && d.compat_lines(dir, true).is_empty());
    assert_eq!(d.scrub("a line"), "a line");
    assert_eq!(d.reload_stamp(dir, "app"), 0);
}
