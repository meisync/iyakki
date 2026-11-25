//! Browser sessions: a random token in a cookie, a second random token (the CSRF token) that the page has to repeat, an idle
//! limit, a cap on how many there can be, and a limit on sign-in tries per address.

use crate::screen::Screen;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const IDLE: Duration = Duration::from_secs(8 * 3600);
pub const MAX_SESSIONS: usize = 32;
pub const COOKIE: &str = "iyakki_session";

pub fn random_token() -> Result<String, String> {
    let mut b = [0u8; 32];
    iyakki_sec::random_fill(&mut b).map_err(|e| format!("no randomness: {e:?}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Seconds of wall-clock time (the monotonic `Instant` does not run while the machine sleeps).
fn wall_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Has the session been idle for [`IDLE`]? By the monotonic clock, or by the wall clock (a machine that slept for a day has been idle
/// for a day: LIFECYCLE-AUDIT L-07). A wall clock that went back counts as no time at all, never as a huge one.
pub(crate) fn idle_over(mono: Duration, last_wall: u64, now_wall: u64) -> bool {
    mono >= IDLE || now_wall.saturating_sub(last_wall) >= IDLE.as_secs()
}

pub struct Session {
    last_wall: u64,
    pub user: String,
    pub csrf: String,
    pub created: Instant,
    pub last_seen: Instant,
    /// The fingerprint of the device key that opened this session, when it was opened with one (revoking the key ends it).
    pub via_key: Option<String>,
    /// The person's screen; whoever is connected holds the lock.
    pub screen: Arc<Mutex<Box<dyn Screen>>>,
}

#[derive(Default)]
pub struct Sessions {
    map: Mutex<HashMap<String, Session>>,
}

/// What a request with a session cookie learns about it.
pub struct Found {
    pub user: String,
    pub csrf: String,
    pub screen: Arc<Mutex<Box<dyn Screen>>>,
}

impl Sessions {
    pub fn new() -> Sessions {
        Sessions::default()
    }

    /// Start a session; returns (cookie token, csrf token). Refuses when too many people are signed in.
    pub fn create(&self, user: &str, screen: Box<dyn Screen>) -> Result<(String, String), String> {
        self.create_via(user, screen, None)
    }

    /// [`Sessions::create`] for a session opened with a device key (`via_key` is the key's fingerprint).
    pub fn create_via(&self, user: &str, screen: Box<dyn Screen>, via_key: Option<String>) -> Result<(String, String), String> {
        let mut m = self.map.lock().map_err(|_| "busy".to_string())?;
        let wall = wall_secs();
        m.retain(|_, s| !idle_over(s.last_seen.elapsed(), s.last_wall, wall));
        if m.len() >= MAX_SESSIONS {
            return Err("too many people are signed in".into());
        }
        let (token, csrf) = (random_token()?, random_token()?);
        let now = Instant::now();
        m.insert(token.clone(), Session { last_wall: wall, user: user.into(), csrf: csrf.clone(), created: now, last_seen: now, via_key, screen: Arc::new(Mutex::new(screen)) });
        Ok((token, csrf))
    }

    pub fn find(&self, token: &str) -> Option<Found> {
        let mut m = self.map.lock().ok()?;
        let s = m.get_mut(token)?;
        let wall = wall_secs();
        if idle_over(s.last_seen.elapsed(), s.last_wall, wall) {
            m.remove(token);
            return None;
        }
        s.last_seen = Instant::now();
        s.last_wall = wall;
        Some(Found { user: s.user.clone(), csrf: s.csrf.clone(), screen: s.screen.clone() })
    }

    pub fn end(&self, token: &str) {
        if let Ok(mut m) = self.map.lock() {
            m.remove(token);
        }
    }

    /// End every session of `user` that a device key with this fingerprint opened; returns how many.
    pub fn end_via_key(&self, user: &str, fingerprint: &str) -> usize {
        let Ok(mut m) = self.map.lock() else { return 0 };
        let before = m.len();
        m.retain(|_, s| !(s.user == user && s.via_key.as_deref() == Some(fingerprint)));
        before - m.len()
    }

    pub fn count(&self) -> usize {
        self.map.lock().map(|m| m.len()).unwrap_or(0)
    }
}

/// At most `max` sign-in tries per address in a minute.
pub struct RateLimit {
    max: usize,
    tries: Mutex<HashMap<String, Vec<Instant>>>,
}

impl RateLimit {
    pub fn new(max: usize) -> RateLimit {
        RateLimit { max, tries: Mutex::new(HashMap::new()) }
    }

    /// Counts a try; false means "too many, refuse".
    pub fn allow(&self, who: &str) -> bool {
        let Ok(mut t) = self.tries.lock() else { return false };
        let now = Instant::now();
        t.retain(|_, v| {
            v.retain(|i| now.duration_since(*i) < Duration::from_secs(60));
            !v.is_empty()
        });
        if t.len() > 10_000 && !t.contains_key(who) {
            return false; // a flood from many addresses: refuse newcomers rather than grow without bound
        }
        let v = t.entry(who.to_string()).or_default();
        if v.len() >= self.max {
            return false;
        }
        v.push(now);
        true
    }
}

/// Constant-time comparison for tokens.
pub fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Input;
    use crate::screen::Frame;

    struct Dummy;
    impl Screen for Dummy {
        fn user(&self) -> String {
            "u".into()
        }
        fn input(&mut self, _: Input) {}
        fn viewing(&mut self, _: bool) {}
        fn step(&mut self) -> Option<Frame> {
            None
        }
        fn take_copy(&mut self) -> Option<String> {
            None
        }
        fn finished(&self) -> bool {
            false
        }
    }

    #[test]
    fn tokens_are_long_random_and_different_each_time() {
        let (a, b) = (random_token().unwrap(), random_token().unwrap());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn a_session_is_found_by_its_token_and_ended_on_request() {
        let s = Sessions::new();
        let (tok, csrf) = s.create("ann", Box::new(Dummy)).unwrap();
        let f = s.find(&tok).expect("found");
        assert_eq!((f.user.as_str(), f.csrf.as_str()), ("ann", csrf.as_str()));
        assert!(s.find("0000").is_none());
        s.end(&tok);
        assert!(s.find(&tok).is_none());
    }

    #[test]
    fn only_a_limited_number_of_people_can_be_signed_in_at_once() {
        let s = Sessions::new();
        for i in 0..MAX_SESSIONS {
            s.create(&format!("u{i}"), Box::new(Dummy)).unwrap();
        }
        assert!(s.create("one more", Box::new(Dummy)).is_err());
        assert_eq!(s.count(), MAX_SESSIONS);
    }

    #[test]
    fn the_rate_limit_counts_per_address() {
        let r = RateLimit::new(3);
        assert!(r.allow("1.1.1.1") && r.allow("1.1.1.1") && r.allow("1.1.1.1"));
        assert!(!r.allow("1.1.1.1"));
        assert!(r.allow("2.2.2.2"), "another address is not affected");
    }

    #[test]
    fn tokens_compare_without_stopping_at_the_first_difference() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
    }

    #[test]
    fn a_session_is_idle_by_either_clock_and_a_wall_clock_set_back_is_no_time() {
        let hour = 3600;
        assert!(!idle_over(Duration::from_secs(10), 1_000, 1_000 + 7 * hour), "just under eight hours by both clocks");
        assert!(idle_over(Duration::from_secs(10), 1_000, 1_000 + 8 * hour), "the machine slept: the wall clock says eight hours");
        assert!(idle_over(IDLE, 1_000, 1_000), "the monotonic clock says so");
        assert!(!idle_over(Duration::from_secs(10), 5_000_000, 1_000), "a clock set back is not a long idle time");
    }

    #[test]
    fn a_session_that_slept_past_the_limit_is_gone_at_the_next_request() {
        let s = Sessions::new();
        let (tok, _) = s.create("ann", Box::new(Dummy)).unwrap();
        assert!(s.find(&tok).is_some());
        s.map.lock().unwrap().get_mut(&tok).unwrap().last_wall = wall_secs().saturating_sub(IDLE.as_secs() + 1);
        assert!(s.find(&tok).is_none());
        assert_eq!(s.count(), 0);
    }

    #[test]
    fn a_session_whose_wall_clock_was_set_back_stays_and_one_just_under_the_limit_stays_too() {
        let s = Sessions::new();
        let (tok, _) = s.create("ann", Box::new(Dummy)).unwrap();
        // the clock was corrected backwards: the last use is "in the future", which counts as no time at all
        s.map.lock().unwrap().get_mut(&tok).unwrap().last_wall = wall_secs() + 5_000_000;
        assert!(s.find(&tok).is_some(), "a clock set back does not end a session");
        assert_eq!(s.count(), 1);
        // just under the limit by the wall clock: still there; the use refreshes it
        s.map.lock().unwrap().get_mut(&tok).unwrap().last_wall = wall_secs().saturating_sub(IDLE.as_secs() - 60);
        assert!(s.find(&tok).is_some());
        assert!(wall_secs().saturating_sub(s.map.lock().unwrap().get(&tok).unwrap().last_wall) < 60, "a request counts as use");
    }
}
