//! Bearer tokens and rate limits. A token is 256 random bits (`iyk_` + hex) handed out once at sign-in; only its SHA-256 is kept,
//! in memory and (when `token_store` is set) in a private file, so a copy of that file signs nobody in. Tokens have an idle and
//! an absolute lifetime, are limited in number, and end at once when the account goes (the caller re-checks the account on every
//! request).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MAX_TOKENS: usize = 256;
pub const PREFIX: &str = "iyk_";

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn hash_token(t: &str) -> String {
    hex(&Sha256::digest(t.as_bytes()))
}

pub fn new_token() -> Result<String, String> {
    let mut b = [0u8; 32];
    iyakki_sec::random_fill(&mut b).map_err(|e| format!("no randomness: {e:?}"))?;
    Ok(format!("{PREFIX}{}", hex(&b)))
}

/// Constant-time comparison.
pub fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    h: String,
    user: String,
    created: u64,
}

struct Entry {
    user: String,
    created: u64,
    last: Instant,
}

pub struct Tokens {
    map: Mutex<HashMap<String, Entry>>,
    path: Option<PathBuf>,
    idle: Duration,
    lifetime: u64,
    now: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl Tokens {
    pub fn new(path: Option<PathBuf>, idle: Duration, lifetime_secs: u64) -> Tokens {
        Tokens::with_clock(path, idle, lifetime_secs, Box::new(|| crate::clock::now_unix().max(0) as u64))
    }

    pub fn with_clock(path: Option<PathBuf>, idle: Duration, lifetime_secs: u64, now: Box<dyn Fn() -> u64 + Send + Sync>) -> Tokens {
        let t = Tokens { map: Mutex::new(HashMap::new()), path, idle, lifetime: lifetime_secs, now };
        t.load();
        t
    }

    fn load(&self) {
        let Some(p) = &self.path else { return };
        let Ok(text) = std::fs::read_to_string(p) else { return };
        let Ok(list) = serde_json::from_str::<Vec<Saved>>(&text) else {
            log::warn!("service: the token store is unreadable; everybody signs in again");
            return;
        };
        let now = (self.now)();
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        for s in list.into_iter().filter(|s| now.saturating_sub(s.created) < self.lifetime).take(MAX_TOKENS) {
            m.insert(s.h, Entry { user: s.user, created: s.created, last: Instant::now() });
        }
    }

    fn save(&self, m: &HashMap<String, Entry>) {
        let Some(p) = &self.path else { return };
        let list: Vec<Saved> = m.iter().map(|(h, e)| Saved { h: h.clone(), user: e.user.clone(), created: e.created }).collect();
        let Ok(text) = serde_json::to_string(&list) else { return };
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = p.with_extension("tmp");
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let ok = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| f.write_all(text.as_bytes())).and_then(|_| std::fs::rename(&tmp, p));
        if let Err(e) = ok {
            log::warn!("service: the token store could not be written: {e}");
        }
    }

    fn expired(&self, e: &Entry) -> bool {
        e.last.elapsed() >= self.idle || (self.now)().saturating_sub(e.created) >= self.lifetime
    }

    /// A new token for `user`; refuses when too many are out.
    pub fn issue(&self, user: &str) -> Result<String, String> {
        let mut m = self.map.lock().map_err(|_| "busy".to_string())?;
        let dead: Vec<String> = m.iter().filter(|(_, e)| self.expired(e)).map(|(h, _)| h.clone()).collect();
        for h in dead {
            m.remove(&h);
        }
        if m.len() >= MAX_TOKENS {
            return Err("too many people are signed in".into());
        }
        let t = new_token()?;
        m.insert(hash_token(&t), Entry { user: user.into(), created: (self.now)(), last: Instant::now() });
        self.save(&m);
        Ok(t)
    }

    /// The person a token belongs to; touches its idle clock.
    pub fn find(&self, token: &str) -> Option<String> {
        if !token.starts_with(PREFIX) || token.len() != PREFIX.len() + 64 {
            return None;
        }
        let h = hash_token(token);
        let mut m = self.map.lock().ok()?;
        let e = m.get_mut(&h)?;
        if self.expired(e) {
            m.remove(&h);
            self.save(&m);
            return None;
        }
        e.last = Instant::now();
        Some(e.user.clone())
    }

    pub fn revoke(&self, token: &str) {
        if let Ok(mut m) = self.map.lock() {
            if m.remove(&hash_token(token)).is_some() {
                self.save(&m);
            }
        }
    }

    /// Every token of a person (the account was removed or changed).
    pub fn revoke_user(&self, user: &str) {
        if let Ok(mut m) = self.map.lock() {
            let before = m.len();
            m.retain(|_, e| e.user != user);
            if m.len() != before {
                self.save(&m);
            }
        }
    }

    pub fn count(&self) -> usize {
        self.map.lock().map(|m| m.len()).unwrap_or(0)
    }
}

/// At most `max` events per key in a sliding minute (keys: client addresses, people).
pub struct Limiter {
    max: u32,
    hits: Mutex<HashMap<String, Vec<Instant>>>,
}

impl Limiter {
    pub fn new(max: u32) -> Limiter {
        Limiter { max, hits: Mutex::new(HashMap::new()) }
    }

    /// Counts one event; false means refuse. The second value is how many seconds until a slot frees up.
    pub fn allow(&self, key: &str) -> (bool, u64) {
        let Ok(mut h) = self.hits.lock() else { return (false, 60) };
        let now = Instant::now();
        let minute = Duration::from_secs(60);
        h.retain(|_, v| {
            v.retain(|i| now.duration_since(*i) < minute);
            !v.is_empty()
        });
        if h.len() > 10_000 && !h.contains_key(key) {
            return (false, 60); // a flood from many keys: refuse newcomers rather than grow without bound
        }
        let v = h.entry(key.to_string()).or_default();
        if v.len() >= self.max as usize {
            let wait = v.first().map(|f| 60u64.saturating_sub(now.duration_since(*f).as_secs())).unwrap_or(60).max(1);
            return (false, wait);
        }
        v.push(now);
        (true, 0)
    }
}
