//! The browser's HTTP cache: a memory tier in front of a folder on disk, both bounded, one cache per person.
//!
//! What is kept: plain GETs that answered 200 and that the server allows to keep (`Cache-Control: max-age`, or an `ETag` /
//! `Last-Modified` to ask "has it changed?" with). What is never kept: `no-store`, `private`, answers that set cookies, answers
//! whose `Vary` names the cookie or something this browser changes between requests, and answers to requests that carried
//! cookies unless they are style sheets, scripts, pictures or fonts (or the server said `public`): a page that was made for
//! a signed-in person is not stored. A kept answer that has gone stale is asked about again with `If-None-Match` /
//! `If-Modified-Since`; a `304` renews it without sending the body again, and when the network is down a stale answer is
//! better than none. See `docs/BROWSER-SPEED.md`.

use crate::fetch::{Fetch, Request, Response};
use crate::net::{self, Budget, COUNTERS};
use crate::url::Url;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Longest freshness trusted, whatever the server asks for (a week).
const MAX_FRESH: u64 = 7 * 86_400;
/// Per-entry overhead counted against the memory budget.
const ENTRY_COST: u64 = 256;

/// Freshness in seconds from a `Cache-Control` value, `None` when the answer must not be stored.
pub fn max_age(value: &str) -> Option<u64> {
    let mut age = None;
    for part in value.split(',') {
        let part = part.trim().to_ascii_lowercase();
        match part.as_str() {
            "no-store" | "no-cache" | "private" => return None,
            _ => {
                if let Some(n) = part.strip_prefix("max-age=").or_else(|| part.strip_prefix("s-maxage=")) {
                    age = n.trim_matches('"').parse::<u64>().ok().or(age);
                }
            }
        }
    }
    age.filter(|a| *a > 0)
}

/// What the cache may do with one answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub store: bool,
    /// Seconds the answer stays fresh (0: ask again every time, which needs a validator).
    pub lifetime: u64,
}

const NO: Policy = Policy { store: false, lifetime: 0 };

/// The kinds of file that are the same for everyone who may ask for them, even when the request carried cookies.
fn is_static(content_type: &str, url: &Url) -> bool {
    let ct = content_type.to_ascii_lowercase();
    let path = url.path.to_ascii_lowercase();
    ct.starts_with("image/")
        || ct.contains("css")
        || ct.contains("javascript")
        || ct.contains("ecmascript")
        || ct.contains("font")
        || ct.contains("wasm")
        || [".css", ".js", ".mjs", ".woff2", ".woff", ".ttf", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".ico", ".wasm"].iter().any(|e| path.ends_with(e)) && !ct.contains("html")
}

/// Decide from the answer's headers. `had_cookie`: the request carried cookies.
pub fn policy(r: &Response, url: &Url, had_cookie: bool) -> Policy {
    if r.status != 200 || !r.set_cookies.is_empty() || r.content_length.is_some() || r.content_type.contains(['\t', '\n']) || r.etag.contains(['\t', '\n']) || r.last_modified.contains(['\t', '\n']) {
        return NO;
    }
    // what the transport reduced to a number, when it did not hand over the header itself
    let cc = if r.cache_control.is_empty() { r.max_age.map(|a| format!("max-age={a}")).unwrap_or_default() } else { r.cache_control.to_ascii_lowercase() };
    let (mut max, mut public, mut no_cache) = (None, false, false);
    for part in cc.split(',') {
        let part = part.trim();
        match part {
            "no-store" | "private" => return NO,
            "no-cache" => no_cache = true,
            "public" => public = true,
            _ => {
                if let Some(n) = part.strip_prefix("max-age=") {
                    max = n.trim_matches('"').parse::<u64>().ok().or(max);
                }
            }
        }
    }
    for v in r.vary.to_ascii_lowercase().split(',') {
        let v = v.trim();
        if v.is_empty() {
            continue;
        }
        // headers this browser sends the same every time cannot make two answers differ
        if !matches!(v, "accept-encoding" | "accept-language" | "user-agent" | "accept" | "origin") {
            return NO;
        }
    }
    if had_cookie && !public && !is_static(&r.content_type, url) {
        return NO;
    }
    let has_validator = !r.etag.is_empty() || !r.last_modified.is_empty();
    let lifetime = if no_cache { 0 } else { max.unwrap_or(0).saturating_sub(r.age).min(MAX_FRESH) };
    if lifetime == 0 && !has_validator {
        return NO;
    }
    Policy { store: true, lifetime }
}

fn real_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn key(url: &Url) -> String {
    // FNV-1a, twice with different seeds: a file name, not a security boundary (the url is stored and compared)
    let text = url.to_string();
    let (mut a, mut b) = (0xcbf29ce484222325u64, 0x84222325cbf29ce4u64);
    for byte in text.bytes() {
        a = (a ^ byte as u64).wrapping_mul(0x100000001b3);
        b = (b ^ byte as u64).wrapping_mul(0x1000193).rotate_left(5);
    }
    format!("{a:016x}{b:016x}")
}

fn is_key(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Clone)]
struct Entry {
    url: String,
    status: u16,
    content_type: String,
    etag: String,
    last_modified: String,
    stored: u64,
    lifetime: u64,
    body: Vec<u8>,
}

impl Entry {
    fn fresh(&self, now: u64) -> bool {
        now < self.stored.saturating_add(self.lifetime)
    }
    fn validator(&self) -> bool {
        !self.etag.is_empty() || !self.last_modified.is_empty()
    }
    fn cost(&self) -> u64 {
        self.body.len() as u64 + ENTRY_COST
    }
    fn response(&self) -> Response {
        Response { status: self.status, content_type: self.content_type.clone(), body: self.body.clone(), etag: self.etag.clone(), last_modified: self.last_modified.clone(), ..Response::default() }
    }
    fn encode(&self) -> Vec<u8> {
        let mut out = format!("V2\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", self.stored, self.lifetime, self.status, self.url, self.content_type, self.etag, self.last_modified).into_bytes();
        out.extend_from_slice(&self.body);
        out
    }
    fn decode(raw: Vec<u8>) -> Option<Entry> {
        let nl = raw.iter().position(|b| *b == b'\n')?;
        let head = std::str::from_utf8(&raw[..nl]).ok()?;
        let mut f = head.split('\t');
        if f.next()? != "V2" {
            return None;
        }
        let (stored, lifetime, status) = (f.next()?.parse().ok()?, f.next()?.parse().ok()?, f.next()?.parse().ok()?);
        let (url, content_type, etag, last_modified) = (f.next()?.to_string(), f.next()?.to_string(), f.next()?.to_string(), f.next()?.to_string());
        let mut raw = raw;
        let body = raw.split_off(nl + 1);
        Some(Entry { url, status, content_type, etag, last_modified, stored, lifetime, body })
    }
}

#[derive(Default)]
struct Mem {
    map: HashMap<String, (Arc<Entry>, u64)>,
    total: u64,
    tick: u64,
}

impl Mem {
    fn evict_to(&mut self, limit: u64) {
        if self.total <= limit {
            return;
        }
        // down to 90% so that the next insert does not evict again at once
        let goal = limit / 10 * 9;
        let mut order: Vec<(u64, String)> = self.map.iter().map(|(k, v)| (v.1, k.clone())).collect();
        order.sort();
        for (_, k) in order {
            if self.total <= goal {
                break;
            }
            if let Some((e, _)) = self.map.remove(&k) {
                self.total -= e.cost();
            }
        }
    }
}

#[derive(Default)]
struct Disk {
    loaded: bool,
    files: HashMap<String, (u64, u64)>,
    total: u64,
    tick: u64,
}

/// What the cache holds now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub mem_bytes: u64,
    pub mem_entries: usize,
    pub disk_bytes: u64,
    pub disk_entries: usize,
    pub budget: Budget,
}

/// Wraps a transport with the cache.
pub struct CachedFetch<F: Fetch> {
    inner: F,
    dir: PathBuf,
    mem: Mutex<Mem>,
    disk: Mutex<Disk>,
    budget: Mutex<Budget>,
    /// Seconds added to the clock (tests move time forward).
    skew: AtomicU64,
    seq: AtomicU64,
}

impl<F: Fetch> CachedFetch<F> {
    /// A cache in `dir` with a modest default budget; the browser sets the real one from the machine (see [`net::budget`]).
    pub fn new(inner: F, dir: impl Into<PathBuf>) -> CachedFetch<F> {
        CachedFetch::with_budget(inner, dir, Budget { mem: 32 << 20, disk: 64 << 20, max_entry: 2 << 20 })
    }

    pub fn with_budget(inner: F, dir: impl Into<PathBuf>, budget: Budget) -> CachedFetch<F> {
        CachedFetch { inner, dir: dir.into(), mem: Mutex::new(Mem::default()), disk: Mutex::new(Disk::default()), budget: Mutex::new(budget), skew: AtomicU64::new(0), seq: AtomicU64::new(0) }
    }

    fn now(&self) -> u64 {
        real_now() + self.skew.load(Ordering::Relaxed)
    }

    /// Move this cache's clock forward (tests).
    pub fn advance_clock(&self, secs: u64) {
        self.skew.fetch_add(secs, Ordering::Relaxed);
    }

    pub fn budget(&self) -> Budget {
        *self.budget.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Change the sizes (the memory gate shrinks them when memory runs short); the excess goes at once.
    pub fn set_budget(&self, b: Budget) {
        *self.budget.lock().unwrap_or_else(|p| p.into_inner()) = b;
        if let Ok(mut m) = self.mem.lock() {
            m.evict_to(b.mem);
        }
        self.trim(b.disk);
    }

    fn path(&self, k: &str) -> PathBuf {
        self.dir.join(k)
    }

    fn load_index(&self, d: &mut Disk) {
        if d.loaded {
            return;
        }
        d.loaded = true;
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return };
        let mut v: Vec<(SystemTime, String, u64)> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if !is_key(&name) {
                    return None;
                }
                let m = e.metadata().ok()?;
                Some((m.modified().unwrap_or(UNIX_EPOCH), name, m.len()))
            })
            .collect();
        v.sort();
        for (_, name, len) in v {
            d.tick += 1;
            d.total += len;
            let t = d.tick;
            d.files.insert(name, (len, t));
        }
    }

    fn read_disk(&self, k: &str) -> Option<Entry> {
        let raw = std::fs::read(self.path(k)).ok();
        let Some(raw) = raw else {
            // gone from under us: forget it
            if let Ok(mut d) = self.disk.lock() {
                if let Some((len, _)) = d.files.remove(k) {
                    d.total = d.total.saturating_sub(len);
                }
            }
            return None;
        };
        let e = Entry::decode(raw);
        if let Ok(mut d) = self.disk.lock() {
            d.tick += 1;
            let t = d.tick;
            if let Some(f) = d.files.get_mut(k) {
                f.1 = t;
            }
        }
        e
    }

    fn lookup(&self, k: &str, url: &Url) -> Option<(Arc<Entry>, bool)> {
        if let Ok(mut m) = self.mem.lock() {
            m.tick += 1;
            let t = m.tick;
            if let Some((e, last)) = m.map.get_mut(k) {
                *last = t;
                if e.url == url.to_string() {
                    return Some((e.clone(), true));
                }
            }
        }
        let e = self.read_disk(k)?;
        if e.url != url.to_string() {
            return None;
        }
        let e = Arc::new(e);
        self.put_mem(k, &e);
        Some((e, false))
    }

    fn put_mem(&self, k: &str, e: &Arc<Entry>) {
        let b = self.budget();
        if b.mem == 0 || e.body.len() as u64 > b.max_entry {
            return;
        }
        if let Ok(mut m) = self.mem.lock() {
            if let Some((old, _)) = m.map.remove(k) {
                m.total -= old.cost();
            }
            m.tick += 1;
            let t = m.tick;
            m.total += e.cost();
            m.map.insert(k.to_string(), (e.clone(), t));
            m.evict_to(b.mem);
        }
    }

    fn write_disk(&self, k: &str, e: &Entry) {
        let b = self.budget();
        let raw = e.encode();
        if b.disk == 0 || raw.len() as u64 > (b.disk / 8).min(32 << 20) {
            return;
        }
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let tmp = self.dir.join(format!("{k}.{}.tmp", self.seq.fetch_add(1, Ordering::Relaxed)));
        if std::fs::write(&tmp, &raw).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        if std::fs::rename(&tmp, self.path(k)).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        if let Ok(mut d) = self.disk.lock() {
            self.load_index(&mut d);
            d.tick += 1;
            let t = d.tick;
            if let Some((old, _)) = d.files.insert(k.to_string(), (raw.len() as u64, t)) {
                d.total = d.total.saturating_sub(old);
            }
            d.total += raw.len() as u64;
        }
        self.trim(b.disk);
    }

    /// Delete the least recently used files until the folder is under its limit.
    fn trim(&self, limit: u64) {
        let Ok(mut d) = self.disk.lock() else { return };
        self.load_index(&mut d);
        if d.total <= limit {
            return;
        }
        let goal = limit / 10 * 9;
        let mut order: Vec<(u64, String, u64)> = d.files.iter().map(|(k, v)| (v.1, k.clone(), v.0)).collect();
        order.sort();
        for (_, k, len) in order {
            if d.total <= goal {
                break;
            }
            let _ = std::fs::remove_file(self.path(&k));
            d.files.remove(&k);
            d.total = d.total.saturating_sub(len);
            if let Ok(mut m) = self.mem.lock() {
                if let Some((e, _)) = m.map.remove(&k) {
                    m.total -= e.cost();
                }
            }
        }
    }

    fn store(&self, k: &str, url: &Url, r: &Response, pol: Policy) -> Arc<Entry> {
        let e = Arc::new(Entry { url: url.to_string(), status: r.status, content_type: r.content_type.clone(), etag: r.etag.clone(), last_modified: r.last_modified.clone(), stored: self.now(), lifetime: pol.lifetime, body: r.body.clone() });
        self.put_mem(k, &e);
        self.write_disk(k, &e);
        e
    }

    /// What is held now (reads the folder's list the first time).
    pub fn stats(&self) -> CacheStats {
        let (mem_bytes, mem_entries) = self.mem.lock().map(|m| (m.total, m.map.len())).unwrap_or_default();
        let (disk_bytes, disk_entries) = match self.disk.lock() {
            Ok(mut d) => {
                self.load_index(&mut d);
                (d.total, d.files.len())
            }
            Err(_) => (0, 0),
        };
        CacheStats { mem_bytes, mem_entries, disk_bytes, disk_entries, budget: self.budget() }
    }

    /// Remove everything kept.
    pub fn clear(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        if let Ok(mut m) = self.mem.lock() {
            *m = Mem::default();
        }
        if let Ok(mut d) = self.disk.lock() {
            *d = Disk { loaded: true, ..Disk::default() };
        }
    }
}

impl<F: Fetch> Fetch for CachedFetch<F> {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.send(&Request::get(url))
    }

    fn cache_stats(&self, _user: &str) -> Option<CacheStats> {
        Some(self.stats())
    }

    fn clear_cache(&self, _user: &str) {
        self.clear();
    }

    fn set_cache_budget(&self, b: Budget) {
        self.set_budget(b);
    }

    fn send(&self, req: &Request) -> Result<Response, String> {
        net::bump(&COUNTERS.requests, 1);
        let cacheable = req.method.eq_ignore_ascii_case("GET") && req.if_none_match.is_empty() && req.if_modified_since.is_empty() && self.budget() != Budget::OFF;
        if !cacheable {
            net::bump(&COUNTERS.misses, 1);
            return self.inner.send(req);
        }
        let k = key(&req.url);
        let had_cookie = !req.cookie.is_empty();
        let held = self.lookup(&k, &req.url);
        if let Some((e, in_mem)) = &held {
            if e.fresh(self.now()) {
                net::bump(if *in_mem { &COUNTERS.mem_hits } else { &COUNTERS.disk_hits }, 1);
                return Ok(e.response());
            }
        }
        // stale: ask whether it changed (when there is something to ask with)
        let mut ask = req.clone();
        if let Some((e, _)) = &held {
            if e.validator() {
                ask.if_none_match = e.etag.clone();
                ask.if_modified_since = if e.etag.is_empty() { e.last_modified.clone() } else { String::new() };
            }
        }
        let r = match self.inner.send(&ask) {
            Ok(r) => r,
            Err(err) => {
                // offline or the server is down: an old copy beats an error page
                if let Some((e, _)) = &held {
                    return Ok(e.response());
                }
                return Err(err);
            }
        };
        if r.status == 304 {
            if let Some((e, _)) = &held {
                net::bump(&COUNTERS.revalidated, 1);
                let fresh = if r.cache_control.is_empty() { None } else { Some(policy(&Response { status: 200, content_type: e.content_type.clone(), etag: e.etag.clone(), last_modified: e.last_modified.clone(), ..r.clone() }, &req.url, had_cookie)) };
                let lifetime = fresh.map(|p| p.lifetime).unwrap_or(e.lifetime);
                let renewed = Arc::new(Entry { stored: self.now(), lifetime, ..(**e).clone() });
                self.put_mem(&k, &renewed);
                self.write_disk(&k, &renewed);
                let mut out = renewed.response();
                out.set_cookies = r.set_cookies;
                return Ok(out);
            }
        }
        net::bump(&COUNTERS.misses, 1);
        let pol = policy(&r, &req.url, had_cookie);
        if pol.store {
            self.store(&k, &req.url, &r, pol);
        } else if held.is_some() && r.status == 200 {
            // the new answer must not be kept: neither may the old one
            let _ = std::fs::remove_file(self.path(&k));
            if let Ok(mut m) = self.mem.lock() {
                if let Some((e, _)) = m.map.remove(&k) {
                    m.total -= e.cost();
                }
            }
            if let Ok(mut d) = self.disk.lock() {
                if let Some((len, _)) = d.files.remove(&k) {
                    d.total = d.total.saturating_sub(len);
                }
            }
        }
        Ok(r)
    }
}

/// A plain shared handle to a transport, so that several caches can sit on one.
#[derive(Clone)]
pub struct ArcFetch(pub Arc<dyn Fetch>);

impl Fetch for ArcFetch {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.0.get(url)
    }
    fn send(&self, req: &Request) -> Result<Response, String> {
        self.0.send(req)
    }
}

/// One cache folder per person under `base`: `base/<user>`. Requests that do not say who they are go to the network directly,
/// so nothing one person fetched can be handed to another.
pub struct UserCaches {
    inner: Arc<dyn Fetch>,
    base: PathBuf,
    budget: Mutex<Budget>,
    users: Mutex<HashMap<String, Arc<CachedFetch<ArcFetch>>>>,
}

fn safe_name(user: &str) -> String {
    let s: String = user.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect();
    if s.is_empty() || s.starts_with('.') {
        format!("u{s}")
    } else {
        s
    }
}

impl UserCaches {
    pub fn new(inner: Arc<dyn Fetch>, base: impl Into<PathBuf>, budget: Budget) -> UserCaches {
        UserCaches { inner, base: base.into(), budget: Mutex::new(budget), users: Mutex::new(HashMap::new()) }
    }

    /// The cache of `user` (made on first use).
    pub fn cache(&self, user: &str) -> Arc<CachedFetch<ArcFetch>> {
        let mut u = self.users.lock().unwrap_or_else(|p| p.into_inner());
        let b = *self.budget.lock().unwrap_or_else(|p| p.into_inner());
        u.entry(safe_name(user)).or_insert_with(|| Arc::new(CachedFetch::with_budget(ArcFetch(self.inner.clone()), self.base.join(safe_name(user)), b))).clone()
    }

    /// Remove the flat files an older version left directly in `base` (they belonged to nobody).
    pub fn remove_legacy_files(&self) {
        if let Ok(rd) = std::fs::read_dir(&self.base) {
            for e in rd.flatten() {
                if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

impl Fetch for UserCaches {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.inner.get(url)
    }
    fn send(&self, req: &Request) -> Result<Response, String> {
        self.inner.send(req)
    }
    fn for_user(&self, user: &str) -> Option<Arc<dyn Fetch>> {
        Some(self.cache(user))
    }
    fn cache_stats(&self, user: &str) -> Option<CacheStats> {
        Some(self.cache(user).stats())
    }
    fn clear_cache(&self, user: &str) {
        self.cache(user).clear();
    }
    fn set_cache_budget(&self, b: Budget) {
        *self.budget.lock().unwrap_or_else(|p| p.into_inner()) = b;
        let all: Vec<_> = self.users.lock().map(|u| u.values().cloned().collect()).unwrap_or_default();
        for c in all {
            c.set_budget(b);
        }
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
