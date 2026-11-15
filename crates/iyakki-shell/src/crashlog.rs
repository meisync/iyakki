//! What the last panic was and where: the shell catches a panicking app (draw, input and tick are each wrapped), closes only
//! that app and writes this note to the audit log, so a crash on a real device can be traced after the fact.

use std::sync::Mutex;

static LAST: Mutex<String> = Mutex::new(String::new());

/// The frames of our own crates (`iyakki_*`) in a rendered backtrace, "#3 iyakki_shell::textsel::drag at src/textsel.rs:439:21",
/// at most `max`.
pub fn our_frames(bt: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut lines = bt.lines().peekable();
    while let Some(l) = lines.next() {
        let Some((idx, name)) = l.trim().split_once(": ") else { continue };
        if idx.is_empty() || !idx.bytes().all(|b| b.is_ascii_digit()) || !name.contains("iyakki_") || name.contains("crashlog::") {
            continue;
        }
        let at = lines.peek().map(|n| n.trim()).and_then(|n| n.strip_prefix("at ")).map(str::to_string);
        out.push(match at {
            Some(a) => format!("#{idx} {name} at {a}"),
            None => format!("#{idx} {name}"),
        });
        if out.len() >= max {
            break;
        }
    }
    out
}

/// Remember every panic (message, place, our frames) for [`take_last`]. Chains to the hook that was there (which prints and logs).
/// Safe to call more than once.
pub fn install() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let frames = our_frames(&std::backtrace::Backtrace::force_capture().to_string(), 12);
            put(&LAST, format!("{info} | {}", frames.join(" | ")));
            prev(info);
        }));
    });
}

/// The note of the last panic since the previous call (empty when none was recorded).
pub fn take_last() -> String {
    take(&LAST)
}

/// Keep a note. A lock that another thread poisoned (it panicked while holding it) is used all the same: the crash notes must work
/// exactly when things are going wrong.
fn put(slot: &Mutex<String>, note: String) {
    let mut l = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *l = note;
    l.truncate(1800);
}

fn take(slot: &Mutex<String>) -> String {
    std::mem::take(&mut *slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_frames_are_kept_with_their_places() {
        let bt = "   0: std::backtrace::Backtrace::force_capture\n   1: iyakki_shell::textsel::drag\n             at ./crates/iyakki-shell/src/textsel.rs:439:21\n   2: core::panicking::panic_fmt\n   3: <iyakki_shell::apps::ide::IdeApp as iyakki_shell::apps::App>::render\n";
        assert_eq!(our_frames(bt, 20), vec!["#1 iyakki_shell::textsel::drag at ./crates/iyakki-shell/src/textsel.rs:439:21", "#3 <iyakki_shell::apps::ide::IdeApp as iyakki_shell::apps::App>::render"]);
        assert_eq!(our_frames(bt, 1).len(), 1);
    }

    #[test]
    fn the_note_is_kept_and_taken_even_when_a_thread_panicked_holding_the_lock() {
        let slot = std::sync::Arc::new(Mutex::new(String::new()));
        let s2 = slot.clone();
        let _ = std::thread::spawn(move || {
            let _g = s2.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            panic!("an app thread fails under the lock");
        })
        .join();
        assert!(slot.is_poisoned(), "the setup: the lock is poisoned");
        put(&slot, "x".repeat(5000));
        let n = take(&slot);
        assert_eq!(n.len(), 1800, "kept and cut to the limit");
        assert_eq!(take(&slot), "", "taken once");
    }

    #[test]
    fn a_caught_panic_leaves_its_note() {
        install();
        let _ = take_last();
        let r = std::panic::catch_unwind(|| {
            let v: Vec<u8> = Vec::new();
            v[3]
        });
        assert!(r.is_err());
        // other tests may panic at the same time; the note is whichever came last, but it is never empty
        assert!(!take_last().is_empty());
    }
}
