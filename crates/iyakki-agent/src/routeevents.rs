//! The way out of this computer came back (F-LIFE-15, F-LIFE-19; lifecycle audit P-06, P-10, P-11): one watcher looks at the network link
//! every few seconds (an interface that is up and a default route through it, `link_state_routed`) and, when the link was down and is up
//! again, rings a bell. Things that wait for the network sleep on the bell instead of a plain timer, or register a hook:
//!
//! * downloads that failed on the network start again ([`crate::mediaqueue`]),
//! * the clock is set again soon instead of in four hours ([`crate::ntp`]),
//! * the back-off of the SSH tunnels starts over ([`crate::sshtunnel_mgr`]).
//!
//! The watcher is a pure step ([`Watcher::step`]) so a test drives it with made-up looks; the bell and the hooks are plain data with no
//! clock of their own.

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

/// Called when the link returns; `false` takes the hook off the list (its owner is gone).
pub type Hook = Arc<dyn Fn() -> bool + Send + Sync>;

/// Whether the machine has a way out now: an interface that is up and a default route through an interface that is up.
pub fn route_up() -> bool {
    crate::appevent_sources::link_state_routed(&crate::browserdev::sys_root(), &iyakki_core_local::net::ipv4_routes_text()) == "online"
}

/// The bell and the list of hooks.
#[derive(Default)]
pub struct RouteEvents {
    rung: Mutex<u64>,
    cv: Condvar,
    hooks: Mutex<Vec<(String, Hook)>>,
}

impl RouteEvents {
    pub fn new() -> RouteEvents {
        RouteEvents::default()
    }

    /// How many times the link has returned since this object was made.
    pub fn count(&self) -> u64 {
        *self.rung.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Register a hook under a name (a hook of the same name is replaced).
    pub fn on_return(&self, name: &str, h: Hook) {
        let mut v = self.hooks.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        v.retain(|(n, _)| n != name);
        v.push((name.to_string(), h));
    }

    /// The link returned: ring the bell and call every hook (outside the lock, a hook may register another).
    pub fn announce_return(&self) {
        *self.rung.lock().unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
        self.cv.notify_all();
        let hooks: Vec<(String, Hook)> = self.hooks.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let mut gone = Vec::new();
        for (n, h) in hooks {
            if !h() {
                gone.push(n);
            }
        }
        if !gone.is_empty() {
            self.hooks.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|(n, _)| !gone.contains(n));
        }
    }

    /// Wait up to `d` for the link to return after the `seen`th ring; true (and `seen` updated) when it did. A ring that came before this
    /// call is not lost: the count is compared, not the moment.
    pub fn sleep_unless_returned(&self, seen: &mut u64, d: Duration) -> bool {
        let g = self.rung.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (g, _) = self.cv.wait_timeout_while(g, d, |c| *c == *seen).unwrap_or_else(std::sync::PoisonError::into_inner);
        let returned = *g != *seen;
        *seen = *g;
        returned
    }
}

/// The machine's own bell.
pub fn global() -> &'static RouteEvents {
    static G: OnceLock<RouteEvents> = OnceLock::new();
    G.get_or_init(RouteEvents::new)
}

/// What the looks at the link mean: the first only remembers; a look that finds it up after a look that found it down is a return.
#[derive(Default)]
pub struct Watcher {
    last: Option<bool>,
}

impl Watcher {
    pub fn step(&mut self, up: bool) -> bool {
        let returned = up && self.last == Some(false);
        self.last = Some(up);
        returned
    }
}

/// How often the link is looked at.
pub const LOOK_EVERY: Duration = Duration::from_secs(5);

/// Start the one thread that looks at the link and rings the machine's bell (the program calls this once; never in a test).
pub fn spawn_watcher() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = std::thread::Builder::new().name("iyakki-route-watch".into()).spawn(|| {
            let mut w = Watcher::default();
            loop {
                if w.step(route_up()) {
                    log::info!("network: the way out is back");
                    global().announce_return();
                }
                std::thread::sleep(LOOK_EVERY);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn only_down_then_up_is_a_return_and_the_first_look_only_remembers() {
        let mut w = Watcher::default();
        assert!(!w.step(true), "the first look: no event");
        assert!(!w.step(true));
        assert!(!w.step(false));
        assert!(!w.step(false));
        assert!(w.step(true), "down, then up");
        assert!(!w.step(true), "still up");
        let mut booted_offline = Watcher::default();
        assert!(!booted_offline.step(false));
        assert!(booted_offline.step(true), "a machine that boots without a route and gets one");
    }

    #[test]
    fn a_ring_wakes_a_sleeper_and_a_ring_before_the_sleep_is_not_lost() {
        let ev = Arc::new(RouteEvents::new());
        // rung before the sleeper looks: it returns at once, whatever the timeout
        let mut seen = ev.count();
        ev.announce_return();
        assert!(ev.sleep_unless_returned(&mut seen, Duration::from_secs(600)));
        assert_eq!(seen, 1);
        // rung while it sleeps
        let e2 = ev.clone();
        let sleeper = std::thread::spawn(move || {
            let mut seen = 1;
            e2.sleep_unless_returned(&mut seen, Duration::from_secs(600))
        });
        // the sleeper may not have started yet: the count makes it work either way
        ev.announce_return();
        assert!(sleeper.join().unwrap());
    }

    #[test]
    fn a_sleep_without_a_ring_ends_with_the_timeout_and_says_so() {
        let ev = RouteEvents::new();
        let mut seen = ev.count();
        assert!(!ev.sleep_unless_returned(&mut seen, Duration::ZERO));
    }

    #[test]
    fn hooks_are_called_on_a_return_replaced_by_name_and_dropped_when_they_say_so() {
        let ev = RouteEvents::new();
        let a = Arc::new(AtomicUsize::new(0));
        let b = Arc::new(AtomicUsize::new(0));
        let (a1, b1) = (a.clone(), b.clone());
        ev.on_return("x", Arc::new(move || {
            a1.fetch_add(1, Ordering::SeqCst);
            true
        }));
        ev.on_return("y", Arc::new(move || {
            b1.fetch_add(1, Ordering::SeqCst);
            false
        }));
        ev.announce_return();
        ev.announce_return();
        assert_eq!((a.load(Ordering::SeqCst), b.load(Ordering::SeqCst)), (2, 1), "the second hook asked to go after one call");
        let a2 = a.clone();
        ev.on_return("x", Arc::new(move || {
            a2.fetch_add(10, Ordering::SeqCst);
            true
        }));
        ev.announce_return();
        assert_eq!(a.load(Ordering::SeqCst), 12, "a hook of the same name replaced the first");
    }
}
