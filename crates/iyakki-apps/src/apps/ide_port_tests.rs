//! The IDE port: the real one answers from the shell's context and writes to its audit log; the fake keeps the same promises in memory.

use std::sync::{Arc, Mutex};

use iyakki_core_fake::FakeCore;

use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};

fn fake(tag: &str) -> FakeIdePort {
    let dir = temp_dir(tag);
    let store = Arc::new(Mutex::new(iyakki_shared::settings::SettingsStore::open(&dir.join("settings.json")).unwrap()));
    FakeIdePort::new("ada", Role::Operator, &dir, Arc::new(FakeCore::new()), SettingsRef::of_store(&store))
}

#[test]
fn the_real_port_answers_from_the_context_and_audits_for_the_signed_in_person() {
    let cx = cx_with_admin_store("ideport-real");
    let p = CxIde(&cx);
    assert_eq!(p.user(), "root");
    assert_eq!(p.role(), Role::Admin);
    assert!(p.policy().manage_settings);
    assert_eq!(p.data_dir(), cx.data_dir());
    p.audit("ide.port.test", "one");
    (p.audit_sink())("ide.port.sink", "two");
    let seen = p.audit_recent(5);
    assert!(seen.iter().any(|l| l.contains("ide.port.test") && l.contains("root")), "{seen:?}");
    assert!(seen.iter().any(|l| l.contains("ide.port.sink") && l.contains("root")), "the sink writes for the same person: {seen:?}");
}

#[test]
fn the_real_port_hands_out_the_facets_the_context_holds() {
    let cx = cx_with_admin_store("ideport-real-facets");
    let p = CxIde(&cx);
    assert!(Arc::ptr_eq(&p.core(), &cx.core));
    // each facet is reachable and answers (no panic, no shell state needed beyond the context)
    let _ = (p.browser(), p.git_kit(), p.clis(), p.storeapi(), p.tasks_facet(), p.browser_jobs());
}

#[test]
fn the_fake_records_audit_lines_through_both_paths_newest_last() {
    let p = fake("ideport-fake-audit");
    p.audit("a.one", "x");
    (p.audit_sink())("a.two", "y");
    assert_eq!(p.audited(), vec![("ada".to_string(), "a.one".to_string(), "x".to_string()), ("ada".to_string(), "a.two".to_string(), "y".to_string())]);
    let last = p.audit_recent(1);
    assert_eq!(last.len(), 1);
    assert!(last[0].contains("a.two"));
    assert!(p.audit_recent(0).is_empty());
}

#[test]
fn the_fake_has_the_role_and_policy_of_its_role_and_a_manual_clock() {
    let p = fake("ideport-fake-clock");
    assert_eq!(p.role(), Role::Operator);
    assert_eq!(p.policy(), &Role::Operator.template());
    let t0 = p.now_secs();
    assert_eq!(p.now_secs(), t0);
    p.advance(30);
    assert_eq!(p.now_secs(), t0 + 30);
}

#[test]
#[should_panic(expected = "give it a Git kit")]
fn a_fake_port_never_falls_back_to_the_real_git_kit() {
    let p = fake("ideport-no-git");
    let _ = p.git_kit();
}

#[test]
fn the_real_port_hands_out_the_slice_three_facets_and_the_persons_run_store() {
    let cx = cx_with_admin_store("ideport-real-slice3");
    let p = CxIde(&cx);
    let _ = (p.kb_source(), p.quick_upstream());
    assert_eq!(p.agents_cfg(), cx.agents_cfg());
    assert_eq!(p.runs().list("root"), cx.runs().list("root"), "the same run store, not a copy");
}

#[test]
fn the_fake_serves_the_agent_config_it_was_given_and_refuses_what_it_was_not() {
    let mut p = fake("ideport-fake-agents");
    assert_eq!(p.agents_cfg(), iyakki_shared::extagentcfg::AgentsCfg::default());
    p.agents.default = Some("claude".into());
    assert_eq!(p.agents_cfg().default.as_deref(), Some("claude"));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| p.runs()));
    assert!(r.is_err(), "a run store that was not given is a stop, never the real one");
}
