use super::*;
use iyakki_shared::geminilive_host::{Card, Screen};

#[test]
fn a_core_without_gemini_live_starts_nothing_and_says_so() {
    struct S;
    impl ServicesB6 for S {}
    let live = Arc::new(S).gemini_live();
    let r = Running::new(Arc::new(Mutex::new(RunState::Connecting)), Arc::new(AtomicBool::new(false)), None, Arc::new(|| {}));
    assert!(r.finished(), "no thread: finished");
    assert_eq!(r.state(), RunState::Connecting);
    drop(live);
    let none = Running::new(Arc::new(Mutex::new(RunState::Fallback(UNAVAILABLE_B6.into()))), Arc::new(AtomicBool::new(false)), None, Arc::new(|| {}));
    assert_eq!(none.state(), RunState::Fallback(UNAVAILABLE_B6.into()));
}

#[test]
fn running_stop_calls_the_cores_stop_and_finish_turn_sets_the_flag() {
    let stopped = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let s2 = stopped.clone();
    let r = Running::new(Arc::new(Mutex::new(RunState::Open)), done.clone(), None, Arc::new(move || s2.store(true, std::sync::atomic::Ordering::SeqCst)));
    r.finish_turn();
    r.stop();
    assert!(done.load(std::sync::atomic::Ordering::SeqCst) && stopped.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn the_board_answers_only_what_the_person_pressed() {
    let b = Board::new();
    b.show(&Card { id: "x".into(), name: "n".into(), summary: "s".into(), detail: String::new() });
    assert_eq!(b.answer("x"), None);
    assert!(b.press("x", true));
    assert_eq!(b.answer("x"), Some(true));
    assert!(!b.press("x", false), "answered once");
    assert!(b.pending().is_empty());
}
