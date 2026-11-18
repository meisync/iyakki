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

pub struct Session {
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
        m.retain(|_, s| s.last_seen.elapsed() < IDLE);
        if m.len() >= MAX_SESSIONS {
            return Err("too many people are signed in".into());
        }
        let (token, csrf) = (random_token()?, random_token()?);
        let now = Instant::now();
        m.insert(token.clone(), Session { user: user.into(), csrf: csrf.clone(), created: now, last_seen: now, via_key, screen: Arc::new(Mutex::new(screen)) });
        Ok((token, csrf))
    }

    pub fn find(&self, token: &str) -> Option<Found> {
        let mut m = self.map.lock().ok()?;
        let s = m.get_mut(token)?;
        if s.last_seen.elapsed() >= IDLE {
            m.remove(token);
            return None;
        }
        s.last_seen = Instant::now();
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
}
