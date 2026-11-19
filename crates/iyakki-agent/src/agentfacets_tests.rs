//! The real implementation of the hand-written facets (F-SPLIT-03): `AgentApiImpl` answers from this crate.

use std::path::{Path, PathBuf};
use std::time::Duration;

use iyakki_agent_api::AgentFacets;
use iyakki_shared::workspace::LockState;

use crate::agentapi_gen::AgentApiImpl;

fn tmp(tag: &str) -> PathBuf {
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-facets-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_real_exec_facet_starts_programs_with_the_agents_runner() {
    let a = AgentApiImpl;
    assert!(a.exec().on_path("sh"));
    assert!(!a.exec().on_path("no-such-program-xyz"));
    let o = a.exec().run_argv(&std::env::temp_dir(), &["echo".into(), "hi".into()], &[], Duration::from_secs(5), 1000);
    assert!(o.ok() && o.stdout == "hi\n", "{o:?}");
    let r = a.exec().system();
    assert!(r.has("sh"));
}

#[test]
fn the_real_lock_facet_keeps_a_workspace_to_one_window() {
    let a = AgentApiImpl;
    let root = tmp("lock");
    let now = 1_000_000u64;
    assert_eq!(a.workspace_locks().acquire(&root, "ann", "s1", now), LockState::Mine);
    assert!(a.workspace_locks().live(&root, now).is_some());
    match a.workspace_locks().acquire(&root, "bob", "s2", now + 5) {
        LockState::Held(l) => assert_eq!(l.session, "s1"),
        s => panic!("{s:?}"),
    }
    assert!(!a.workspace_locks().refresh(&root, "bob", "s2", now + 6), "s1 holds it");
    a.workspace_locks().take_over(&root, "bob", "s2", now + 7);
    assert!(a.workspace_locks().refresh(&root, "bob", "s2", now + 8));
    assert!(!a.workspace_locks().refresh(&root, "ann", "s1", now + 9), "s2 took it over");
}

#[test]
fn the_real_developer_mode_facet_is_quiet_on_a_machine_that_never_used_it() {
    let a = AgentApiImpl;
    let data = tmp("dev");
    let d = a.devmode();
    assert!(!d.is_on(&data));
    assert_eq!(d.top_sign(&data), None);
    assert!(!d.update_interrupted(&data) && !d.update_fell_back(&data));
    assert!(d.shell_pending(&data).is_empty() && d.take_open(&data).is_empty() && d.take_input(&data).is_empty());
    assert_eq!(d.update_interrupted_text(&data), None);
    let _ = Path::new("");
}
