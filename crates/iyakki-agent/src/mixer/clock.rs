//! The mixer's time source. The product runs on the real clock: the mixer thread sleeps to the card's period clock. A test builds
//! the mixer on a manual clock instead (`Mixer::start_with_clock(.., MixClock::manual())`): time stands still until the test calls
//! [`Mixer::advance`](super::Mixer::advance), the mixer thread mixes exactly the periods that time covers and the test continues
//! when it has gone back to sleep. What a test then asserts is what was mixed, never how fast a thread ran, so a starved machine
//! cannot make a gap in what the fake card heard.

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Microseconds since the first call of the process (the real clock).
fn real_us() -> u64 {
    static BASE: OnceLock<Instant> = OnceLock::new();
    BASE.get_or_init(Instant::now).elapsed().as_micros() as u64
}

struct Parked {
    target_us: u64,
    /// A nap also ends when the event counter moves past this value (a source arrived, was written or finished).
    events_seen: Option<u64>,
}

#[derive(Default)]
struct State {
    now_us: u64,
    events: u64,
    stopped: bool,
    parked: Option<Parked>,
}

struct Manual {
    st: Mutex<State>,
    cv: Condvar,
}

/// The clock of one mixer. Cheap to clone; clones share the time.
#[derive(Clone, Default)]
pub struct MixClock {
    manual: Option<Arc<Manual>>,
}

impl std::fmt::Debug for MixClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MixClock({})", if self.manual.is_some() { "manual" } else { "real" })
    }
}

impl MixClock {
    /// The wall clock (what the product uses).
    pub fn real() -> MixClock {
        MixClock { manual: None }
    }

    /// A clock that moves only when [`MixClock::advance`] says so. Starts at 0.
    pub fn manual() -> MixClock {
        MixClock { manual: Some(Arc::new(Manual { st: Mutex::new(State::default()), cv: Condvar::new() })) }
    }

    pub fn is_manual(&self) -> bool {
        self.manual.is_some()
    }

    fn lock(m: &Manual) -> std::sync::MutexGuard<'_, State> {
        m.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn now_us(&self) -> u64 {
        match &self.manual {
            Some(m) => Self::lock(m).now_us,
            None => real_us(),
        }
    }

    pub fn now(&self) -> Duration {
        Duration::from_micros(self.now_us())
    }

    /// Moves a manual clock forward (and wakes the mixer thread). Does nothing on the real clock.
    pub fn advance(&self, by: Duration) {
        if let Some(m) = &self.manual {
            Self::lock(m).now_us += by.as_micros() as u64;
            m.cv.notify_all();
        }
    }

    /// Something happened that a napping mixer thread must look at (a source arrived or got audio).
    pub(crate) fn poke(&self) {
        if let Some(m) = &self.manual {
            Self::lock(m).events += 1;
            m.cv.notify_all();
        }
    }

    /// The event counter (0 on the real clock): taken before the thread looks at its sources, given to [`MixClock::nap`].
    pub(crate) fn events(&self) -> u64 {
        self.manual.as_ref().map_or(0, |m| Self::lock(m).events)
    }

    /// The mixer is being shut down: nothing waits on this clock any more.
    pub(crate) fn stop(&self) {
        if let Some(m) = &self.manual {
            Self::lock(m).stopped = true;
            m.cv.notify_all();
        }
    }

    fn park(&self, m: &Manual, d: Duration, events_seen: Option<u64>) {
        let mut g = Self::lock(m);
        let target_us = g.now_us + d.as_micros() as u64;
        g.parked = Some(Parked { target_us, events_seen });
        m.cv.notify_all(); // a test waiting in `settle` looks again
        while !g.stopped && g.now_us < target_us && events_seen.is_none_or(|e| g.events == e) {
            g = m.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        g.parked = None;
    }

    /// The mixer thread's pacing sleep: `d` on this clock (a manual clock waits until the test has advanced that far).
    pub(crate) fn sleep(&self, d: Duration) {
        match &self.manual {
            Some(m) => self.park(m, d, None),
            None => std::thread::sleep(d),
        }
    }

    /// The idle nap of a manual clock: until `d` has passed on the clock or the event counter is no longer `events_seen`.
    pub(crate) fn nap(&self, d: Duration, events_seen: u64) {
        if let Some(m) = &self.manual {
            self.park(m, d, Some(events_seen));
        }
    }

    /// Waits until the mixer thread has done everything the time so far allows and sleeps again (or was stopped). Panics after
    /// `budget` with what the thread was doing.
    pub(crate) fn settle(&self, budget: Duration) {
        let Some(m) = &self.manual else { return };
        let end = Instant::now() + budget;
        let mut g = Self::lock(m);
        loop {
            let asleep = match &g.parked {
                Some(p) => g.now_us < p.target_us && p.events_seen.is_none_or(|e| e == g.events),
                None => false,
            };
            if g.stopped || asleep {
                return;
            }
            let left = end.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the mixer thread did not go back to sleep within {budget:?} of real time (clock at {} us)", g.now_us);
            g = m.cv.wait_timeout(g, left.min(Duration::from_millis(50))).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manual_clock_moves_only_when_told_and_a_sleeping_thread_wakes_at_its_time() {
        let c = MixClock::manual();
        assert_eq!((c.now_us(), c.is_manual()), (0, true));
        let c2 = c.clone();
        let t = std::thread::spawn(move || {
            c2.sleep(Duration::from_millis(50));
            c2.now_us()
        });
        // the thread parks; nothing wakes it before its time
        c.settle(Duration::from_secs(30));
        c.advance(Duration::from_millis(49));
        c.settle(Duration::from_secs(30));
        assert!(!t.is_finished());
        c.advance(Duration::from_millis(1));
        assert_eq!(t.join().unwrap(), 50_000);
    }

    #[test]
    fn a_nap_ends_at_an_event_and_stop_frees_everything() {
        let c = MixClock::manual();
        let seen = c.events();
        let c2 = c.clone();
        let t = std::thread::spawn(move || c2.nap(Duration::from_secs(10), seen));
        c.settle(Duration::from_secs(30));
        c.poke();
        t.join().unwrap();
        let c3 = c.clone();
        let t = std::thread::spawn(move || c3.sleep(Duration::from_secs(10)));
        c.settle(Duration::from_secs(30));
        c.stop();
        t.join().unwrap();
        assert_eq!(c.now_us(), 0, "time never moved");
    }

    #[test]
    fn the_real_clock_runs_by_itself() {
        let c = MixClock::real();
        let a = c.now_us();
        c.advance(Duration::from_secs(5)); // no effect
        assert!(c.now_us() < a + 5_000_000);
    }
}
