//! The fake of the pending-work facet (F-SPLIT-03): a waker a test controls, a strip line it sets, every ask recorded.

use iyakki_agent_api::pending::{AgentPending, FakePending, NoPending};
use iyakki_shared::pending::Wake;

#[test]
fn a_core_without_a_ledger_has_no_waker_and_no_strip() {
    assert!(NoPending.pending_waker("amy", "", None).is_none());
    assert_eq!(NoPending.pending_strip_line("amy", "", 0), None);
}

#[test]
fn the_fake_waker_is_ready_when_a_test_sets_a_wake_and_gives_it_once() {
    let f = FakePending::default();
    let w = f.pending_waker("amy", "proj", None).unwrap();
    assert!(!w.ready() && w.take().is_none());
    *f.wake.lock().unwrap() = Some(Wake::Turn { text: "the build finished".into(), ids: vec!["t1".into()] });
    assert!(w.ready());
    match w.take() {
        Some(Wake::Turn { text, ids }) => assert_eq!((text.as_str(), ids), ("the build finished", vec!["t1".to_string()])),
        other => panic!("{other:?}"),
    }
    assert!(!w.ready(), "a wake is given once");
    *f.strip.lock().unwrap() = Some("2 running".into());
    assert_eq!(f.pending_strip_line("amy", "proj", 5).as_deref(), Some("2 running"));
    assert_eq!(f.asked.lock().unwrap().as_slice(), ["waker(amy,proj)", "strip(amy,proj)"]);
}
