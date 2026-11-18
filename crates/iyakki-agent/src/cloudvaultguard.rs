//! What stands between the network and the vault's signature check (`cloudvaultd.rs`): refusing replays and limiting rates.
//!
//! **Replay.** A signed request (`IYAKKI-SIG id,ts,nonce,sig`) is valid for [`CLOCK_SKEW`] seconds on either side of the service's
//! clock, and only once: the pair (id, nonce) is remembered until the request could no longer pass the clock check anyway
//! (`ts + CLOCK_SKEW < now`), so a copy of a captured request is refused for as long as the original would have been accepted. Only
//! requests whose signature verified are remembered, so an anonymous caller cannot fill the cache. The cache is bounded
//! ([`MAX_REMEMBERED`]); when it is full of live entries new requests are refused (429), never accepted unchecked. It is **persistent**:
//! every accepted pair is appended to `replay.log` (mode 0600) in the service's data folder, which is read again at start (expired lines
//! are dropped and the file rewritten). If that file cannot be read or written the service falls back to a safe rule: nothing signed
//! before the moment of the start is accepted (`not_before`), which is stricter but cannot be replayed. A power cut can lose the last
//! unsynced lines of the log (the lines are written, not fsynced, per request), so every restart (a start that finds an earlier log) also sets `not_before` to the moment of
//! the start: a request signed before it is refused even if its line was lost. The cost is that a request signed just before a restart
//! has to be signed again (the client does that on a 401).
//!
//! **Rates.** Per key (the id that signed, counted only after the signature verified) and per source address (counted before anything is
//! checked). Both are token buckets. A source that fails authentication [`Limits::fail_max`] times inside [`Limits::fail_window`]
//! seconds is *locked*: from then on its requests are still checked, but a request that fails answers 429 at once and is not counted
//! further. **The lockout never refuses a request that verifies**, and failures are counted against the source, never against the
//! name a request claims, so nobody can lock the owner out by sending bad requests in the owner's name or from another address. The
//! owner's own key is limited only by the owner's own traffic. A stolen key that floods the service is handled by the operator, not by
//! the limit: replace the key (`iyakki vault enroll`, file access is the authority) and restart the service, which also clears every
//! counter (they are in memory). Behind a proxy or NAT every user shares one source address; the lockout still lets signed requests
//! through, the source bucket is generous ([`Limits::default`]).

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

pub const CLOCK_SKEW: u64 = 120;
pub const MAX_REMEMBERED: usize = 50_000;
const MAX_TRACKED: usize = 4096;

#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    pub key_burst: f64,
    pub key_per_sec: f64,
    pub source_burst: f64,
    pub source_per_sec: f64,
    pub fail_max: u32,
    pub fail_window: u64,
    pub lock_secs: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        // a rotation reads every secret of a scope (up to 5000), so a key's burst is large; it is a brake on abuse, not on use
        Limits { key_burst: 300.0, key_per_sec: 50.0, source_burst: 1000.0, source_per_sec: 200.0, fail_max: 10, fail_window: 60, lock_secs: 60 }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Not a fresh, new, signed request: the ordinary 401 (the reason is for the log).
    Rejected(&'static str),
    /// Too fast: 429, try again after this many seconds.
    Slow(u64),
}

#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: u64,
}

impl Bucket {
    fn take(&mut self, now: u64, burst: f64, per_sec: f64) -> bool {
        self.tokens = (self.tokens + now.saturating_sub(self.last) as f64 * per_sec).min(burst);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

struct Source {
    bucket: Bucket,
    fails: u32,
    window_start: u64,
    locked_until: u64,
}

pub struct Guard {
    limits: Limits,
    seen: BTreeMap<String, u64>,
    log: Option<std::fs::File>,
    log_path: PathBuf,
    /// Nothing signed before this is accepted (set when the log could not be trusted).
    not_before: u64,
    keys: HashMap<String, Bucket>,
    sources: HashMap<IpAddr, Source>,
}

fn entry(id: &str, nonce: &str) -> String {
    format!("{id} {nonce}")
}

impl Guard {
    pub fn open(data: &Path, now: u64, limits: Limits) -> Guard {
        let log_path = data.join("replay.log");
        let mut g = Guard { limits, seen: BTreeMap::new(), log: None, log_path, not_before: 0, keys: HashMap::new(), sources: HashMap::new() };
        let restart = g.log_path.exists(); // a first start has nothing to lose
        match g.load(now) {
            // a power cut can lose the last unsynced lines of the log, so a request signed before this start is never trusted to be new
            // again: a replay of one that was accepted just before a cut is refused, the owner's next request (signed now) passes
            Ok(()) => {
                if restart {
                    g.not_before = now;
                }
            }
            Err(e) => {
                log::warn!("vault service: replay log: {e}; refusing everything signed before now");
                g.not_before = now;
                g.seen.clear();
                g.log = None;
            }
        }
        g
    }

    fn load(&mut self, now: u64) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        match std::fs::read_to_string(&self.log_path) {
            Ok(t) => {
                for l in t.lines() {
                    let Some((ts, rest)) = l.split_once(' ') else { return Err("a line is not understood".into()) };
                    let ts: u64 = ts.parse().map_err(|_| "a time is not a number".to_string())?;
                    if ts + CLOCK_SKEW >= now {
                        self.seen.insert(rest.to_string(), ts);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        // rewrite without the expired lines, then keep the file open for appending
        let tmp = self.log_path.with_extension("log.tmp");
        let text: String = self.seen.iter().map(|(k, ts)| format!("{ts} {k}\n")).collect();
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
        f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.log_path).map_err(|e| e.to_string())?;
        self.log = Some(std::fs::OpenOptions::new().append(true).mode(0o600).open(&self.log_path).map_err(|e| e.to_string())?);
        Ok(())
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub fn set_limits(&mut self, l: Limits) {
        self.limits = l;
    }

    /// Before anything is checked: one token of the source's bucket. `Err(seconds)` when it is empty.
    pub fn source_allowed(&mut self, ip: IpAddr, now: u64) -> Result<(), u64> {
        let (burst, per_sec) = (self.limits.source_burst, self.limits.source_per_sec);
        let s = self.source(ip, now);
        if s.bucket.take(now, burst, per_sec) {
            Ok(())
        } else {
            Err(1)
        }
    }

    fn source(&mut self, ip: IpAddr, now: u64) -> &mut Source {
        if !self.sources.contains_key(&ip) && self.sources.len() >= MAX_TRACKED {
            self.sources.retain(|_, s| s.locked_until > now || now.saturating_sub(s.bucket.last) < 600);
            if self.sources.len() >= MAX_TRACKED {
                self.sources.clear(); // tracking is dropped, never the service
            }
        }
        let burst = self.limits.source_burst;
        self.sources.entry(ip).or_insert(Source { bucket: Bucket { tokens: burst, last: now }, fails: 0, window_start: now, locked_until: 0 })
    }

    /// Seconds left of the source's lockout (0 when it is not locked).
    pub fn locked_for(&mut self, ip: IpAddr, now: u64) -> u64 {
        self.sources.get(&ip).map_or(0, |s| s.locked_until.saturating_sub(now))
    }

    /// A request from `ip` failed authentication. Counts against the address only.
    pub fn failed(&mut self, ip: IpAddr, now: u64) {
        let (max, window, lock) = (self.limits.fail_max, self.limits.fail_window, self.limits.lock_secs);
        let s = self.source(ip, now);
        if now.saturating_sub(s.window_start) > window {
            s.window_start = now;
            s.fails = 0;
        }
        s.fails += 1;
        if s.fails >= max {
            s.locked_until = now + lock;
            s.fails = 0;
            s.window_start = now;
        }
    }

    /// A request from `ip` verified: its failures are forgiven (a lock does not outlive the owner getting through).
    pub fn succeeded(&mut self, ip: IpAddr, now: u64) {
        if let Some(s) = self.sources.get_mut(&ip) {
            s.fails = 0;
            s.window_start = now;
            s.locked_until = 0;
        }
    }

    /// A verified request: fresh, not seen before, remembered. Then the key's bucket.
    pub fn accept(&mut self, id: &str, nonce: &str, ts: u64, now: u64) -> Result<(), Refused> {
        if ts.abs_diff(now) > CLOCK_SKEW {
            return Err(Refused::Rejected(if ts < now { "too old" } else { "too far in the future" }));
        }
        if ts < self.not_before {
            return Err(Refused::Rejected("signed before the service started"));
        }
        self.seen.retain(|_, t| *t + CLOCK_SKEW >= now);
        let k = entry(id, nonce);
        if self.seen.contains_key(&k) {
            return Err(Refused::Rejected("replayed"));
        }
        if self.seen.len() >= MAX_REMEMBERED {
            return Err(Refused::Slow(CLOCK_SKEW));
        }
        if let Some(f) = self.log.as_mut() {
            // not remembered on disk means not accepted: a request that cannot be recorded could be replayed after a restart
            if f.write_all(format!("{ts} {k}\n").as_bytes()).is_err() {
                log::warn!("vault service: replay log not writable");
                return Err(Refused::Slow(5));
            }
        }
        self.seen.insert(k, ts);
        let (burst, per_sec) = (self.limits.key_burst, self.limits.key_per_sec);
        if self.keys.len() >= MAX_TRACKED && !self.keys.contains_key(id) {
            self.keys.retain(|_, b| now.saturating_sub(b.last) < 600);
        }
        let b = self.keys.entry(id.to_string()).or_insert(Bucket { tokens: burst, last: now });
        if b.take(now, burst, per_sec) {
            Ok(())
        } else {
            Err(Refused::Slow(1))
        }
    }

    #[cfg(test)]
    pub fn remembered(&self) -> usize {
        self.seen.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-guard-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn quiet() -> Limits {
        Limits { key_burst: 1e9, key_per_sec: 1e9, source_burst: 1e9, source_per_sec: 1e9, ..Limits::default() }
    }

    #[test]
    fn a_full_replay_cache_refuses_new_requests_and_frees_up_when_entries_expire() {
        let d = dir("full");
        let mut g = Guard::open(&d, NOW, quiet());
        for i in 0..MAX_REMEMBERED {
            assert!(g.accept(&format!("acme/u{}", i % 977), &format!("n{i}"), NOW, NOW).is_ok(), "entry {i}");
        }
        assert_eq!(g.remembered(), MAX_REMEMBERED);
        // never accepted unchecked: a new request waits, a remembered one is still a replay
        assert_eq!(g.accept("acme/u1", "fresh", NOW, NOW), Err(Refused::Slow(CLOCK_SKEW)));
        assert_eq!(g.accept("acme/u0", "n0", NOW, NOW), Err(Refused::Rejected("replayed")));
        assert_eq!(g.remembered(), MAX_REMEMBERED);
        // once the entries are older than the window they are dropped and the service takes requests again
        let later = NOW + CLOCK_SKEW + 1;
        assert!(g.accept("acme/u1", "fresh", later, later).is_ok());
        assert_eq!(g.remembered(), 1);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn a_start_refuses_anything_signed_before_it_even_when_its_log_line_was_lost() {
        let d = dir("cut");
        {
            let mut g = Guard::open(&d, NOW, quiet());
            assert!(g.accept("acme/alice", "n1", NOW, NOW).is_ok());
            assert!(g.accept("acme/alice", "old", NOW - 100, NOW).is_ok(), "a first start has nothing to lose");
        }
        // simulate the power cut: the unsynced line never reached the disk
        std::fs::write(d.join("replay.log"), "").unwrap();
        let mut g = Guard::open(&d, NOW + 5, quiet());
        assert_eq!(g.accept("acme/alice", "n1", NOW, NOW + 5), Err(Refused::Rejected("signed before the service started")));
        assert!(g.accept("acme/alice", "n2", NOW + 5, NOW + 5).is_ok(), "a request signed after the start still passes");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn an_unwritable_log_refuses_requests_instead_of_forgetting_them() {
        let d = dir("ro");
        let mut g = Guard::open(&d, NOW, quiet());
        // replace the open log handle with a read-only one so that every append fails
        g.log = Some(std::fs::File::open(d.join("replay.log")).unwrap());
        assert_eq!(g.accept("acme/alice", "n1", NOW, NOW), Err(Refused::Slow(5)));
        assert_eq!(g.remembered(), 0);
        std::fs::remove_dir_all(&d).ok();
    }
}
