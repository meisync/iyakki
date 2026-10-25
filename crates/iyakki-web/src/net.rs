//! Keeping the network busy without flooding it: a limit of connections per host and overall, with the more urgent request
//! going first when several wait (document, then style sheets and blocking scripts, then fonts, then everything else, pictures
//! last), plus the counters Settings shows and the sizes of the caches.
//!
//! The limits are held only while one request is on the wire and a request never asks for another permit while it holds one,
//! so exhausting the pool cannot deadlock: every waiter gets its turn when a running request ends (the transport has a time limit).

use crate::fetch::{Fetch, Request, Response};
use crate::url::Url;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// Request priorities (lower number: more urgent). `0` in [`Request::priority`] means "not said" and ranks as [`DEFAULT`].
pub const DOCUMENT: u8 = 1;
/// Style sheets and scripts the page waits for.
pub const BLOCKING: u8 = 2;
pub const FONT: u8 = 3;
pub const DEFAULT: u8 = 4;
pub const IMAGE: u8 = 6;

fn rank(p: u8) -> u8 {
    if p == 0 {
        DEFAULT
    } else {
        p
    }
}

/// HTTP/1.1 browsers open at most six connections to one host; so do we.
pub const PER_HOST: usize = 6;
/// Open requests overall.
pub const GLOBAL: usize = 32;

#[derive(Default)]
struct LimState {
    host: HashMap<String, usize>,
    running: usize,
    /// Waiting requests: (rank, ticket, host).
    waiting: Vec<(u8, u64, String)>,
    ticket: u64,
    peak_running: usize,
    peak_host: usize,
}

/// The connection limits, shared by every request of a transport.
pub struct Limiter {
    per_host: usize,
    global: usize,
    st: Mutex<LimState>,
    cv: Condvar,
}

/// Held while a request is on the wire; releasing it lets the next waiter go.
pub struct Permit<'a> {
    lim: &'a Limiter,
    host: String,
}

impl Limiter {
    pub fn new(per_host: usize, global: usize) -> Limiter {
        Limiter { per_host: per_host.max(1), global: global.max(1), st: Mutex::new(LimState::default()), cv: Condvar::new() }
    }

    fn can_run(&self, s: &LimState, host: &str) -> bool {
        s.running < self.global && s.host.get(host).copied().unwrap_or(0) < self.per_host
    }

    /// Wait until `host` may take another connection and no more urgent request that could run is waiting.
    pub fn acquire(&self, host: &str, priority: u8) -> Permit<'_> {
        let mut s = self.st.lock().unwrap_or_else(|p| p.into_inner());
        let mine = (rank(priority), s.ticket);
        s.ticket += 1;
        s.waiting.push((mine.0, mine.1, host.to_string()));
        loop {
            let better_ready = s.waiting.iter().any(|(r, t, h)| (*r, *t) < mine && self.can_run(&s, h));
            if self.can_run(&s, host) && !better_ready {
                break;
            }
            s = self.cv.wait(s).unwrap_or_else(|p| p.into_inner());
        }
        s.waiting.retain(|(_, t, _)| *t != mine.1);
        s.running += 1;
        let n = {
            let n = s.host.entry(host.to_string()).or_insert(0);
            *n += 1;
            *n
        };
        s.peak_running = s.peak_running.max(s.running);
        s.peak_host = s.peak_host.max(n);
        // others may now be first in line
        self.cv.notify_all();
        Permit { lim: self, host: host.to_string() }
    }

    /// The most requests that ran at once, and the most to one host (for tests and the timing example).
    pub fn peaks(&self) -> (usize, usize) {
        let s = self.st.lock().unwrap_or_else(|p| p.into_inner());
        (s.peak_running, s.peak_host)
    }

    /// Requests running and waiting now.
    pub fn load(&self) -> (usize, usize) {
        let s = self.st.lock().unwrap_or_else(|p| p.into_inner());
        (s.running, s.waiting.len())
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut s = self.lim.st.lock().unwrap_or_else(|p| p.into_inner());
        s.running = s.running.saturating_sub(1);
        if let Some(n) = s.host.get_mut(&self.host) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                s.host.remove(&self.host);
            }
        }
        self.lim.cv.notify_all();
    }
}

/// A transport whose requests go through a [`Limiter`].
pub struct LimitedFetch<F: Fetch> {
    inner: F,
    limiter: Arc<Limiter>,
}

impl<F: Fetch> LimitedFetch<F> {
    pub fn new(inner: F, limiter: Arc<Limiter>) -> LimitedFetch<F> {
        LimitedFetch { inner, limiter }
    }
    pub fn limiter(&self) -> &Arc<Limiter> {
        &self.limiter
    }
}

impl<F: Fetch> Fetch for LimitedFetch<F> {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.send(&Request::get(url))
    }

    fn send(&self, req: &Request) -> Result<Response, String> {
        let _permit = self.limiter.acquire(&req.url.host, req.priority);
        self.inner.send(req)
    }
}

/// What the browser has done with the network and its caches since it started (shown in Settings, Browser).
#[derive(Default)]
pub struct Counters {
    pub requests: AtomicU64,
    pub bytes_in: AtomicU64,
    pub mem_hits: AtomicU64,
    pub disk_hits: AtomicU64,
    pub revalidated: AtomicU64,
    pub misses: AtomicU64,
    pub preload_used: AtomicU64,
    pub preload_wasted: AtomicU64,
}

pub static COUNTERS: Counters = Counters {
    requests: AtomicU64::new(0),
    bytes_in: AtomicU64::new(0),
    mem_hits: AtomicU64::new(0),
    disk_hits: AtomicU64::new(0),
    revalidated: AtomicU64::new(0),
    misses: AtomicU64::new(0),
    preload_used: AtomicU64::new(0),
    preload_wasted: AtomicU64::new(0),
};

/// A copy of the counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub requests: u64,
    pub bytes_in: u64,
    pub mem_hits: u64,
    pub disk_hits: u64,
    pub revalidated: u64,
    pub misses: u64,
    pub preload_used: u64,
    pub preload_wasted: u64,
}

pub fn snapshot() -> Snapshot {
    let c = &COUNTERS;
    let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
    Snapshot { requests: g(&c.requests), bytes_in: g(&c.bytes_in), mem_hits: g(&c.mem_hits), disk_hits: g(&c.disk_hits), revalidated: g(&c.revalidated), misses: g(&c.misses), preload_used: g(&c.preload_used), preload_wasted: g(&c.preload_wasted) }
}

pub(crate) fn bump(a: &AtomicU64, n: u64) {
    a.fetch_add(n, Ordering::Relaxed);
}

/// How much the person lets the cache use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CacheSize {
    /// From the memory and free space the machine has (the default).
    #[default]
    Auto,
    Small,
    Large,
}

impl CacheSize {
    pub fn id(self) -> &'static str {
        match self {
            CacheSize::Auto => "auto",
            CacheSize::Small => "small",
            CacheSize::Large => "large",
        }
    }
    pub fn from_id(s: &str) -> CacheSize {
        match s.trim() {
            "small" => CacheSize::Small,
            "large" => CacheSize::Large,
            _ => CacheSize::Auto,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            CacheSize::Auto => "Auto",
            CacheSize::Small => "Small",
            CacheSize::Large => "Large",
        }
    }
}

pub const MIB: u64 = 1 << 20;
pub const GIB: u64 = 1 << 30;

/// The sizes the caches are held to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Budget {
    /// Answers kept in memory.
    pub mem: u64,
    /// Answers kept in the person's cache folder.
    pub disk: u64,
    /// The largest single answer kept in memory (disk takes larger ones up to `disk / 8`).
    pub max_entry: u64,
}

impl Budget {
    /// No caching at all (a machine with no memory to spare, or unknown facts and a cautious caller).
    pub const OFF: Budget = Budget { mem: 0, disk: 0, max_entry: 0 };
}

/// The cache sizes for this machine, from `MemAvailable` and the free space of the data volume (both in bytes, 0 = unknown).
/// Auto: 6% of the available memory (at least 8 MB, at most 256 MB), a disk tier of a tenth of the free space (at least 64 MB,
/// at most 1 GB); Large doubles both (memory up to 512 MB, disk up to 2 GB); Small is 2% (4 to 32 MB) and 64 MB. Under 256 MB of
/// available memory only the disk tier is used, and under 96 MB nothing at all, so the memory gate can always take it back.
pub fn budget(size: CacheSize, mem_available: u64, disk_free: u64) -> Budget {
    let avail = if mem_available == 0 { 512 * MIB } else { mem_available };
    let free = if disk_free == 0 { 640 * MIB } else { disk_free };
    let (mem_pct, mem_lo, mem_hi, disk_div, disk_lo, disk_hi) = match size {
        CacheSize::Auto => (6, 8 * MIB, 256 * MIB, 10, 64 * MIB, GIB),
        CacheSize::Large => (12, 16 * MIB, 512 * MIB, 5, 128 * MIB, 2 * GIB),
        CacheSize::Small => (2, 4 * MIB, 32 * MIB, 64, 32 * MIB, 64 * MIB),
    };
    let mut mem = bound(avail / 100 * mem_pct, mem_lo, mem_hi);
    // never into the last 96 MB of free memory; a machine that is short gets no memory tier
    if avail < 256 * MIB {
        mem = 0;
    }
    if avail < 96 * MIB {
        return Budget::OFF;
    }
    mem = mem.min(avail.saturating_sub(96 * MIB));
    let disk = bound(free / disk_div, disk_lo, disk_hi).min(free.saturating_sub(256 * MIB).max(32 * MIB));
    Budget { mem, disk, max_entry: bound(mem / 16, 256 * 1024, 8 * MIB) }
}

/// `v` held between `lo` and `hi` (the lower bound wins if they are inverted): never panics, unlike `clamp`.
fn bound(v: u64, lo: u64, hi: u64) -> u64 {
    v.max(lo).min(hi.max(lo))
}

/// A short size, "12.3 MB".
pub fn human(n: u64) -> String {
    if n >= GIB {
        format!("{:.1} GB", n as f64 / GIB as f64)
    } else if n >= MIB {
        format!("{:.1} MB", n as f64 / MIB as f64)
    } else if n >= 1024 {
        format!("{} KB", n / 1024)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    /// A server that takes `ms` per request and records how many ran at once.
    struct Slow {
        ms: u64,
        now: AtomicUsize,
        peak: AtomicUsize,
        order: Mutex<Vec<String>>,
    }
    impl Slow {
        fn new(ms: u64) -> Slow {
            Slow { ms, now: AtomicUsize::new(0), peak: AtomicUsize::new(0), order: Mutex::new(vec![]) }
        }
    }
    impl Fetch for Arc<Slow> {
        fn get(&self, u: &Url) -> Result<Response, String> {
            let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(n, Ordering::SeqCst);
            self.order.lock().unwrap().push(u.path.clone());
            std::thread::sleep(Duration::from_millis(self.ms));
            self.now.fetch_sub(1, Ordering::SeqCst);
            Ok(Response { status: 200, body: vec![1], ..Response::default() })
        }
    }

    fn run_all(f: &Arc<LimitedFetch<Arc<Slow>>>, urls: Vec<(String, u8)>) {
        let hs: Vec<_> = urls
            .into_iter()
            .map(|(u, p)| {
                let f = f.clone();
                std::thread::spawn(move || {
                    let mut r = Request::get(&Url::parse(&u).unwrap());
                    r.priority = p;
                    f.send(&r).unwrap();
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
    }

    #[test]
    fn one_host_never_has_more_than_six_requests_at_once() {
        let slow = Arc::new(Slow::new(20));
        let f = Arc::new(LimitedFetch::new(slow.clone(), Arc::new(Limiter::new(PER_HOST, GLOBAL))));
        run_all(&f, (0..24).map(|i| (format!("https://a.org/{i}"), 0)).collect());
        assert_eq!(slow.peak.load(Ordering::SeqCst), 6, "six to one host, no more");
        assert_eq!(f.limiter().peaks().1, 6);
    }

    #[test]
    fn the_global_limit_holds_across_hosts() {
        let slow = Arc::new(Slow::new(20));
        let f = Arc::new(LimitedFetch::new(slow.clone(), Arc::new(Limiter::new(6, 10))));
        run_all(&f, (0..40).map(|i| (format!("https://h{}.org/{i}", i % 20), 0)).collect());
        assert!(slow.peak.load(Ordering::SeqCst) <= 10);
        assert!(slow.peak.load(Ordering::SeqCst) >= 5, "and the hosts really ran side by side");
    }

    #[test]
    fn urgent_requests_go_before_pictures_when_the_pool_is_full() {
        let slow = Arc::new(Slow::new(30));
        let f = Arc::new(LimitedFetch::new(slow.clone(), Arc::new(Limiter::new(1, 1))));
        // one running request fills the pool; then pictures and a style sheet queue up (the sheet last)
        let first = {
            let f = f.clone();
            std::thread::spawn(move || f.get(&Url::parse("https://a.org/first").unwrap()).unwrap())
        };
        std::thread::sleep(Duration::from_millis(10));
        let mut hs = Vec::new();
        for (name, p) in [("img1", IMAGE), ("img2", IMAGE), ("css", BLOCKING)] {
            let f = f.clone();
            hs.push(std::thread::spawn(move || {
                let mut r = Request::get(&Url::parse(&format!("https://a.org/{name}")).unwrap());
                r.priority = p;
                f.send(&r).unwrap();
            }));
            std::thread::sleep(Duration::from_millis(3));
        }
        first.join().unwrap();
        for h in hs {
            h.join().unwrap();
        }
        let order = slow.order.lock().unwrap().clone();
        assert_eq!(order, ["/first", "/css", "/img1", "/img2"], "the sheet jumps the queue, pictures keep their order");
    }

    #[test]
    fn an_exhausted_pool_does_not_deadlock() {
        // 64 requests through a pool of 2: everything finishes
        let slow = Arc::new(Slow::new(1));
        let f = Arc::new(LimitedFetch::new(slow.clone(), Arc::new(Limiter::new(2, 2))));
        let t = Instant::now();
        run_all(&f, (0..64).map(|i| (format!("https://a.org/{i}"), (i % 7) as u8)).collect());
        assert!(t.elapsed() < Duration::from_secs(10));
        assert_eq!(f.limiter().load(), (0, 0));
    }

    #[test]
    fn budgets_follow_memory_and_can_shrink_to_nothing() {
        let auto = budget(CacheSize::Auto, 4 * GIB, 100 * GIB);
        assert_eq!(auto.mem, (4 * GIB / 100 * 6).clamp(8 * MIB, 256 * MIB));
        assert_eq!(auto.disk, GIB, "a tenth of 100 GB is capped at 1 GB");
        let large = budget(CacheSize::Large, 8 * GIB, 100 * GIB);
        assert_eq!((large.mem, large.disk), (512 * MIB, 2 * GIB));
        let small = budget(CacheSize::Small, 4 * GIB, 100 * GIB);
        assert_eq!((small.mem, small.disk), (32 * MIB, 64 * MIB));
        // a 1 GB machine with 700 MB available
        let tight = budget(CacheSize::Auto, 700 * MIB, 8 * GIB);
        assert!(tight.mem >= 8 * MIB && tight.mem <= 42 * MIB, "{tight:?}");
        // the gate takes it back: short of memory the memory tier goes first, then everything
        assert_eq!(budget(CacheSize::Auto, 200 * MIB, 8 * GIB).mem, 0);
        assert!(budget(CacheSize::Auto, 200 * MIB, 8 * GIB).disk > 0);
        assert_eq!(budget(CacheSize::Large, 50 * MIB, 8 * GIB), Budget::OFF);
        // small disks: the cache never takes the last 256 MB
        assert!(budget(CacheSize::Large, 8 * GIB, 600 * MIB).disk <= 344 * MIB);
        // bigger settings never give less
        for avail in [128 * MIB, 512 * MIB, 2 * GIB, 16 * GIB] {
            let (s, a, l) = (budget(CacheSize::Small, avail, 20 * GIB), budget(CacheSize::Auto, avail, 20 * GIB), budget(CacheSize::Large, avail, 20 * GIB));
            assert!(s.mem <= a.mem && a.mem <= l.mem && s.disk <= a.disk && a.disk <= l.disk, "{avail}");
        }
    }

    #[test]
    fn size_ids_round_trip() {
        for s in [CacheSize::Auto, CacheSize::Small, CacheSize::Large] {
            assert_eq!(CacheSize::from_id(s.id()), s);
        }
        assert_eq!(CacheSize::from_id("nonsense"), CacheSize::Auto);
        assert_eq!(human(1536), "1 KB");
        assert_eq!(human(5 * MIB), "5.0 MB");
    }
}
