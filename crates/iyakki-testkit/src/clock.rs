//! A manual clock: the test moves time, nothing waits. Code that takes a time source (a closure or a trait) is given `clock.reader()`;
//! the test calls [`ManualClock::advance`] instead of sleeping.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Milliseconds since an arbitrary start, shared between clones.
#[derive(Clone, Debug, Default)]
pub struct ManualClock(Arc<AtomicU64>);

impl ManualClock {
    /// A clock at `start_ms`.
    pub fn new(start_ms: u64) -> ManualClock {
        ManualClock(Arc::new(AtomicU64::new(start_ms)))
    }
    /// Now, in milliseconds.
    pub fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    /// Now, in whole seconds.
    pub fn now_secs(&self) -> u64 {
        self.now_ms() / 1000
    }
    /// Moves the clock forward.
    pub fn advance(&self, by: Duration) {
        self.0.fetch_add(by.as_millis() as u64, Ordering::SeqCst);
    }
    /// Sets the clock.
    pub fn set_ms(&self, ms: u64) {
        self.0.store(ms, Ordering::SeqCst);
    }
    /// A closure for code that takes `Fn() -> u64` (milliseconds).
    pub fn reader(&self) -> impl Fn() -> u64 + Send + Sync + 'static {
        let c = self.clone();
        move || c.now_ms()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_moves_only_when_told_and_clones_share_it() {
        let c = ManualClock::new(5_000);
        let r = c.reader();
        c.advance(Duration::from_millis(2_500));
        assert_eq!((r(), c.now_secs()), (7_500, 7));
        let d = c.clone();
        d.set_ms(1);
        assert_eq!(c.now_ms(), 1);
    }
}
