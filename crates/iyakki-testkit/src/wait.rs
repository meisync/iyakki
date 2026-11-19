//! Waits against a wall-clock budget. A test never counts attempts or beats ("for _ in 0..600") and never sleeps a fixed time and hopes: it
//! polls the condition (every few milliseconds, then slower) until it holds or until a generous budget (30 s) has passed on the clock, and on
//! failure says the last state it saw.

use std::path::Path;
use std::time::{Duration, Instant};

/// The default budget: far above anything a loaded machine needs, so only a real failure reaches it.
pub const BUDGET: Duration = Duration::from_secs(30);

fn pause(spent: Duration) {
    std::thread::sleep(if spent < Duration::from_millis(200) { Duration::from_millis(2) } else if spent < Duration::from_secs(2) { Duration::from_millis(10) } else { Duration::from_millis(30) });
}

/// Polls `cond` until it is true; panics after `budget` of wall-clock time.
pub fn wait_until(budget: Duration, mut cond: impl FnMut() -> bool) {
    wait_for_state(budget, || if cond() { Ok(()) } else { Err("the condition is still false".to_string()) });
}

/// Polls `check` until it returns `Ok(value)`; panics after `budget` with the last `Err` text (the state observed).
pub fn wait_for_state<T>(budget: Duration, mut check: impl FnMut() -> Result<T, String>) -> T {
    let start = Instant::now();
    loop {
        match check() {
            Ok(v) => return v,
            Err(last) => {
                if start.elapsed() >= budget {
                    panic!("testkit wait: not satisfied after {budget:?}; last state: {last}");
                }
            }
        }
        pause(start.elapsed());
    }
}

/// Waits (up to [`BUDGET`]) for a file to exist.
pub fn wait_for_file(path: &Path) {
    wait_for_state(BUDGET, || if path.exists() { Ok(()) } else { Err(format!("{} does not exist", path.display())) });
}

/// Waits (up to [`BUDGET`]) for a line containing `pattern` in the log file `log`; returns that line. The last 400 bytes of the log are
/// in the failure message.
pub fn wait_for_line(log: &Path, pattern: &str) -> String {
    wait_for_state(BUDGET, || {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        match text.lines().find(|l| l.contains(pattern)) {
            Some(l) => Ok(l.to_string()),
            None => Err(format!("no line with {pattern:?} in {}; its end: {:?}", log.display(), text.get(text.len().saturating_sub(400)..).unwrap_or(""))),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_condition_that_becomes_true_in_another_thread_is_seen_and_a_false_one_names_its_state() {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f2 = flag.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            f2.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        wait_until(BUDGET, || flag.load(std::sync::atomic::Ordering::SeqCst));
        t.join().unwrap();
        let e = std::panic::catch_unwind(|| wait_for_state::<()>(Duration::from_millis(30), || Err("stuck at 3".into()))).unwrap_err();
        assert!(e.downcast_ref::<String>().unwrap().contains("stuck at 3"));
    }
}
