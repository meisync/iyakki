//! Speed and memory of the browser's own engine: how big the page cache may be on this machine right now, how many fetch and
//! picture workers to run, and the numbers Settings -> Browser shows. The sizes come from `MemAvailable` and the free space of the
//! data volume (`iyakki_web::net::budget`), are re-read every few seconds while the browser is used so that the memory gate can take
//! cache back when memory runs short, and never grow past what the person chose (Auto, Small or Large).

use std::path::Path;
use std::time::{Duration, Instant};

use iyakki_web::net::{self, human, Budget, CacheSize};

use crate::safeclamp::SafeClamp;

/// What the machine has to spare right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Machine {
    pub mem_total: u64,
    pub mem_available: u64,
    /// Free bytes on the data volume (0 = unknown).
    pub disk_free: u64,
    pub cores: usize,
}

/// Read the machine (`/proc/meminfo`, the data volume's free space, the processor count).
pub fn machine(data: &Path) -> Machine {
    let m = crate::memgate::read_mem_now();
    Machine { mem_total: m.total, mem_available: m.available, disk_free: iyakki_agent::sharedsite::statvfs_free(data).unwrap_or(0), cores: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2) }
}

/// The cache sizes for this machine and choice.
pub fn budget_for(size: CacheSize, m: &Machine) -> Budget {
    net::budget(size, m.mem_available, m.disk_free)
}

/// How many early fetches run at once for a page: twice the processors (they mostly wait on the network), between 4 and 12; three
/// when memory is short.
pub fn preload_workers(m: &Machine) -> usize {
    if m.mem_total > 0 && m.mem_available < 384 * net::MIB {
        return 3;
    }
    (m.cores * 2).clamp_safe(4, 12)
}

/// How many pictures are fetched and decoded at once: one per processor, between 4 and 8; two when memory is short (a decoded
/// picture can be several megabytes).
pub fn image_workers(m: &Machine) -> usize {
    if m.mem_total > 0 && m.mem_available < 256 * net::MIB {
        return 2;
    }
    m.cores.clamp_safe(4, 8)
}

/// Re-read the machine at most every `every` and give the transport its cache sizes. Returns the budget when it was refreshed.
pub struct Gate {
    last: Option<Instant>,
    every: Duration,
}

impl Gate {
    pub fn new(every: Duration) -> Gate {
        Gate { last: None, every }
    }

    pub fn refresh(&mut self, web: &dyn iyakki_web::Fetch, data: &Path, size: CacheSize) -> Option<Budget> {
        if self.last.is_some_and(|t| t.elapsed() < self.every) {
            return None;
        }
        self.last = Some(Instant::now());
        let b = budget_for(size, &machine(data));
        web.set_cache_budget(b);
        Some(b)
    }

    /// Next refresh is due at once (the person changed the choice).
    pub fn invalidate(&mut self) {
        self.last = None;
    }
}

static GATE: std::sync::Mutex<Option<Gate>> = std::sync::Mutex::new(None);

/// Give the transport its cache sizes, re-reading the machine at most every five seconds.
pub fn refresh(web: &dyn iyakki_web::Fetch, data: &Path, size: CacheSize) {
    if let Ok(mut g) = GATE.lock() {
        g.get_or_insert_with(|| Gate::new(Duration::from_secs(5))).refresh(web, data, size);
    }
}

/// Make the next [`refresh`] act at once (the person changed the cache size).
pub fn refresh_now() {
    if let Ok(mut g) = GATE.lock() {
        if let Some(g) = g.as_mut() {
            g.invalidate();
        }
    }
}

/// The transport for a page's pictures: the early-fetch store of that page when it is the one just loaded (pictures it already
/// started are used from it), otherwise the person's cache.
pub fn image_fetch(cx: &super::AppCx, pre: Option<&std::sync::Arc<iyakki_web::preload::Preloader>>, page: &iyakki_web::Url) -> std::sync::Arc<dyn iyakki_web::Fetch> {
    if let Some(p) = pre.filter(|p| p.page().without_fragment() == page.without_fragment()) {
        return p.clone();
    }
    cx.web.for_user(&cx.session.user).unwrap_or_else(|| cx.web.clone())
}

/// The lines Settings shows: what is in the cache against what it may hold, and what the cache and early fetches saved.
pub fn numbers(stats: Option<iyakki_web::cache::CacheStats>, c: net::Snapshot) -> Vec<(String, String)> {
    let mut out = Vec::new();
    match stats {
        Some(s) => {
            out.push(("Memory".to_string(), format!("{} used of {} ({} answers)", human(s.mem_bytes), human(s.budget.mem), s.mem_entries)));
            out.push(("Disk".to_string(), format!("{} used of {} ({} files)", human(s.disk_bytes), human(s.budget.disk), s.disk_entries)));
        }
        None => out.push(("Cache".to_string(), "not available on this system".to_string())),
    }
    let hits = c.mem_hits + c.disk_hits + c.revalidated;
    let asked = hits + c.misses;
    let rate = if asked == 0 { "no requests yet".to_string() } else { format!("{}% of {} requests answered without the full download", hits * 100 / asked, asked) };
    out.push(("Since start".to_string(), format!("{rate}; {} files fetched early and used, {} not needed; {} downloaded", c.preload_used, c.preload_wasted, human(c.bytes_in))));
    out
}

// ------------------------------------------------------------------ the full engine's share of memory (2026-11-07)

/// How much memory one full engine (one tab's web process) may use before WebKit frees its caches, in MB. WebKit's own default is a
/// share of the whole machine for every process, so several tabs could add up to all of it; this is a share of what is available
/// NOW: Auto a third, Large a half, Small a fifth, kept between 192 MB and 4 GB and never more than the machine has.
pub fn engine_memory_mb(size: CacheSize, mem_total: u64, mem_available: u64) -> u64 {
    let avail = if mem_available == 0 { 1024 * net::MIB } else { mem_available };
    let share = match size {
        CacheSize::Auto => avail / 3,
        CacheSize::Large => avail / 2,
        CacheSize::Small => avail / 5,
    };
    let cap = if mem_total == 0 { 4096 * net::MIB } else { mem_total.min(4096 * net::MIB) };
    (share.clamp_safe(192 * net::MIB, cap) / net::MIB).max(192)
}

/// WebKit's cache model for the engine: `document` (smaller in-memory caches, no large back-forward cache) for the Small choice and
/// for machines with 4 GB or less; the full browser model (`browser`) otherwise.
pub fn engine_cache_model(size: CacheSize, mem_total: u64) -> &'static str {
    if size == CacheSize::Small || (mem_total > 0 && mem_total <= 4096 * net::MIB) {
        "document"
    } else {
        "browser"
    }
}

/// The two engine variables the helper reads (`IYAKKI_WEBVIEW_MEM_MB`, `IYAKKI_WEBVIEW_CACHE_MODEL`, `dist/webview/wpe-helper/helper.c`).
pub fn engine_env(size: CacheSize, m: &Machine) -> Vec<(String, String)> {
    vec![
        ("IYAKKI_WEBVIEW_MEM_MB".to_string(), engine_memory_mb(size, m.mem_total, m.mem_available).to_string()),
        ("IYAKKI_WEBVIEW_CACHE_MODEL".to_string(), engine_cache_model(size, m.mem_total).to_string()),
    ]
}

#[cfg(test)]
#[path = "browser_speed_tests.rs"]
mod tests;
