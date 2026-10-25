//! Keep the desktop alive: if the shell's main loop panics, run it again on the SAME shell (so the signed-in person, the open
//! app and the settings survive) after a short pause. Only when the same state crashes again straight away is it thrown away and
//! rebuilt from scratch, and only after `max_attempts` crashes in a row does the supervisor give up on the desktop.

use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// `run` returned normally.
    Finished,
    /// Too many crashes in a row.
    GaveUp,
    /// `make` could not build the state.
    NoState,
}

/// `make` builds the state (the shell); `run` runs it until it returns (a normal end) or panics. `on_crash(attempt, kept)` is
/// told about each crash (`kept`: whether the next round reuses the state).
pub fn run_kept<S>(mut make: impl FnMut() -> Option<S>, mut run: impl FnMut(&mut S), max_attempts: usize, pause: Duration, mut on_crash: impl FnMut(usize, bool)) -> Outcome {
    let mut state: Option<S> = None;
    // did the previous round crash without the state having run for a while in between?
    let mut crashed_straight_after_reuse = false;
    for attempt in 1..=max_attempts {
        if state.is_none() {
            state = make();
        }
        let Some(s) = state.as_mut() else { return Outcome::NoState };
        let started = std::time::Instant::now();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(s)));
        if r.is_ok() {
            return Outcome::Finished;
        }
        // a state that ran for half a minute before it crashed is healthy again: the count starts over for it
        let healthy = started.elapsed() > Duration::from_secs(30);
        let reuse = !crashed_straight_after_reuse || healthy;
        crashed_straight_after_reuse = !healthy && state.is_some() && reuse;
        if !reuse {
            state = None;
            crashed_straight_after_reuse = false;
        }
        on_crash(attempt, reuse);
        std::thread::sleep(pause);
    }
    Outcome::GaveUp
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Session {
        user: &'static str,
        runs: u32,
    }

    #[test]
    fn a_panic_in_the_loop_restarts_it_on_the_same_session() {
        let made = Cell::new(0);
        let crashes = Cell::new(Vec::new());
        let out = run_kept(
            || {
                made.set(made.get() + 1);
                Some(Session { user: "root", runs: 0 })
            },
            |s| {
                s.runs += 1;
                assert_eq!(s.user, "root");
                if s.runs == 1 {
                    panic!("injected");
                }
                // the second round sees the first one's state: the session was kept
                assert_eq!(s.runs, 2);
            },
            5,
            Duration::from_millis(1),
            |a, kept| {
                let mut v = crashes.take();
                v.push((a, kept));
                crashes.set(v);
            },
        );
        assert_eq!(out, Outcome::Finished);
        assert_eq!(made.get(), 1, "the shell was not rebuilt");
        assert_eq!(crashes.take(), vec![(1, true)]);
    }

    #[test]
    fn a_state_that_crashes_again_at_once_is_rebuilt_and_endless_crashes_give_up() {
        let made = Cell::new(0);
        let out = run_kept(
            || {
                made.set(made.get() + 1);
                Some(0u32)
            },
            |_| panic!("always"),
            5,
            Duration::from_millis(1),
            |_, _| {},
        );
        assert_eq!(out, Outcome::GaveUp);
        assert!(made.get() >= 2 && made.get() <= 4, "rebuilt after a repeated crash: {}", made.get());
        assert_eq!(run_kept(|| None::<u8>, |_| {}, 3, Duration::ZERO, |_, _| {}), Outcome::NoState);
    }
}
