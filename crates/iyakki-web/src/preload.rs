//! The preload scanner: as soon as the page's HTML is in hand, find the style sheets, scripts and pictures it names and start
//! fetching them all at once, most urgent first, instead of one after the other as the page asks for them. The page's own
//! loading code does not change: a request for something already on its way joins it (or takes the finished answer), a
//! request for something not yet started simply goes on its own. Answers nobody asked for are dropped when the page is left.
//!
//! The answers kept here are the transport's raw first answers (a redirect is an answer too, cookies are applied when the page
//! consumes it), keyed by address and the `Cookie` header the request would carry, so a page whose cookies changed meanwhile
//! asks again rather than seeing a stale answer. Nothing is shared between pages or people: the store lives as long as one
//! page load.

use crate::cookies::CookieStore;
use crate::fetch::{Fetch, Request, Response};
use crate::net::{self, COUNTERS};
use crate::url::Url;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Style,
    Script,
    Image,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub url: Url,
    pub kind: Kind,
}

/// What the page will actually use (a page with scripts off does not need its scripts).
#[derive(Clone, Copy, Debug)]
pub struct Wants {
    pub styles: bool,
    pub scripts: bool,
    pub images: bool,
}

/// Most style sheets and scripts started early, and most pictures.
pub const MAX_HINTS: usize = 120;
pub const MAX_IMAGE_HINTS: usize = 16;
/// Most bytes of unconsumed answers held.
pub const MAX_HELD: usize = 48 << 20;
/// How long a page waits for an answer already on its way before asking for itself.
const JOIN_WAIT: Duration = Duration::from_secs(30);

fn script_type_ok(t: Option<&str>) -> bool {
    match t.map(|t| t.trim().to_ascii_lowercase()) {
        None => true,
        Some(t) => t.is_empty() || t == "module" || t.contains("javascript") || t.contains("ecmascript"),
    }
}

/// The things `html` asks for, in the order the page will want them (style sheets first, then scripts, then pictures).
pub fn scan(html: &str, base: &Url, wants: Wants) -> Vec<Hint> {
    let tree = crate::html::parse(html);
    let mut base = base.clone();
    let page_https = base.https;
    let mut order = Vec::new();
    let mut todo = vec![0usize];
    while let Some(n) = todo.pop() {
        order.push(n);
        todo.extend(tree.nodes[n].children.iter().rev());
    }
    if let Some(b) = order.iter().copied().find(|&n| tree.tag(n) == Some("base")) {
        if let Some(h) = tree.attr(b, "href").and_then(|h| base.join(h).ok()) {
            base = h;
        }
    }
    let (mut styles, mut scripts, mut images): (Vec<Hint>, Vec<Hint>, Vec<Hint>) = (vec![], vec![], vec![]);
    let push = |v: &mut Vec<Hint>, href: Option<&str>, kind: Kind| {
        let Some(h) = href.map(str::trim).filter(|h| !h.is_empty() && !h.starts_with("data:") && !h.starts_with("javascript:") && !h.starts_with('#')) else { return };
        let Ok(u) = base.join(h) else { return };
        let u = u.without_fragment();
        // a secure page does not load unprotected files (the page refuses them), so they are not fetched early either
        if (page_https && !u.https) || v.iter().any(|x| x.url == u) {
            return;
        }
        v.push(Hint { url: u, kind });
    };
    for n in order {
        match tree.tag(n) {
            Some("link") => {
                let rel = tree.attr(n, "rel").unwrap_or("").to_ascii_lowercase();
                let rels: Vec<&str> = rel.split_whitespace().collect();
                let media = tree.attr(n, "media").unwrap_or("").to_ascii_lowercase();
                let print_only = media.trim() == "print";
                let as_ = tree.attr(n, "as").unwrap_or("").to_ascii_lowercase();
                if rels.contains(&"stylesheet") && !rels.contains(&"alternate") && tree.attr(n, "disabled").is_none() && !print_only {
                    if wants.styles {
                        push(&mut styles, tree.attr(n, "href"), Kind::Style);
                    }
                } else if rels.contains(&"preload") && as_ == "style" && wants.styles {
                    push(&mut styles, tree.attr(n, "href"), Kind::Style);
                } else if (rels.contains(&"modulepreload") || rels.contains(&"preload") && as_ == "script") && wants.scripts {
                    push(&mut scripts, tree.attr(n, "href"), Kind::Script);
                }
            }
            Some("script") if wants.scripts && tree.attr(n, "nomodule").is_none() && script_type_ok(tree.attr(n, "type")) => {
                push(&mut scripts, tree.attr(n, "src"), Kind::Script);
            }
            Some("img") if wants.images && images.len() < MAX_IMAGE_HINTS && !tree.attr(n, "loading").is_some_and(|l| l.eq_ignore_ascii_case("lazy")) => {
                push(&mut images, tree.attr(n, "src"), Kind::Image);
            }
            _ => {}
        }
    }
    let mut out = styles;
    out.extend(scripts);
    out.extend(images);
    out.truncate(MAX_HINTS + MAX_IMAGE_HINTS);
    out
}

#[derive(Clone, Hash, PartialEq, Eq, Debug)]
struct Key {
    url: String,
    cookie: String,
}

enum Slot {
    Queued,
    /// A page load asked for it first and is fetching it itself.
    Claimed,
    Running,
    Done(Response, Kind),
    /// A picture that could not be fetched: the page is told the same, once, instead of asking the server again.
    Failed(String),
}

#[derive(Default)]
struct St {
    slots: HashMap<Key, Slot>,
    queue: VecDeque<(Key, Request, Kind)>,
    held: usize,
    workers: usize,
}

struct Shared {
    st: Mutex<St>,
    cv: Condvar,
    cancelled: AtomicBool,
}

/// Started fetches for one page load. Hand it to the page's loading code as its transport.
pub struct Preloader {
    inner: Arc<dyn Fetch>,
    allow: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    jar: Option<Arc<Mutex<CookieStore>>>,
    page: Url,
    workers: usize,
    shared: Arc<Shared>,
}

impl Preloader {
    /// `workers`: how many fetches run at once for this page (the transport's own limits still apply below).
    pub fn new(inner: Arc<dyn Fetch>, page: &Url, allow: Arc<dyn Fn(&str) -> bool + Send + Sync>, jar: Option<Arc<Mutex<CookieStore>>>, workers: usize) -> Arc<Preloader> {
        Arc::new(Preloader { inner, allow, jar, page: page.clone(), workers: workers.max(1).min(16), shared: Arc::new(Shared { st: Mutex::new(St::default()), cv: Condvar::new(), cancelled: AtomicBool::new(false) }) })
    }

    fn now() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }

    /// The address of the page these fetches belong to.
    pub fn page(&self) -> &Url {
        &self.page
    }

    /// Queue these fetches and start the workers. Hints for hosts the account may not open are ignored.
    pub fn start(&self, hints: &[Hint]) {
        if self.shared.cancelled.load(Ordering::Relaxed) {
            return;
        }
        let mut jobs = Vec::new();
        for h in hints {
            if !(self.allow)(&h.url.host) {
                continue;
            }
            // what the consumer will send: scripts are fetched without cookies or referrer, sheets and pictures with both
            let (cookie, referer, priority) = match h.kind {
                Kind::Script => (String::new(), String::new(), net::BLOCKING),
                Kind::Style => (self.cookie_for(&h.url), self.page.to_string(), net::BLOCKING),
                Kind::Image => (self.cookie_for(&h.url), self.page.to_string(), net::IMAGE),
            };
            let req = Request { cookie: cookie.clone(), referer, priority, ..Request::get(&h.url) };
            jobs.push((Key { url: h.url.to_string(), cookie }, req, h.kind));
        }
        let n_workers = {
            let mut st = self.shared.st.lock().unwrap_or_else(|p| p.into_inner());
            for (k, req, kind) in jobs {
                if st.slots.contains_key(&k) {
                    continue;
                }
                st.slots.insert(k.clone(), Slot::Queued);
                st.queue.push_back((k, req, kind));
            }
            let want = self.workers.min(st.queue.len());
            let add = want.saturating_sub(st.workers);
            st.workers += add;
            add
        };
        for _ in 0..n_workers {
            let (shared, inner) = (self.shared.clone(), self.inner.clone());
            let spawned = std::thread::Builder::new().name("iyakki-preload".into()).stack_size(2 << 20).spawn(move || worker(shared, inner));
            if spawned.is_err() {
                if let Ok(mut st) = self.shared.st.lock() {
                    st.workers = st.workers.saturating_sub(1);
                }
            }
        }
    }

    fn cookie_for(&self, url: &Url) -> String {
        self.jar.as_ref().and_then(|j| j.lock().ok().map(|s| s.header_for(url, Self::now()))).unwrap_or_default()
    }

    /// Stop: nothing queued is started, answers that arrive later are dropped (the page was left).
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::Relaxed);
        let mut st = self.shared.st.lock().unwrap_or_else(|p| p.into_inner());
        let wasted = st.slots.values().filter(|s| matches!(s, Slot::Done(..))).count() as u64;
        net::bump(&COUNTERS.preload_wasted, wasted);
        st.queue.clear();
        st.slots.clear();
        st.held = 0;
        self.shared.cv.notify_all();
    }

    /// The page has loaded: answers for style sheets and scripts that nothing asked for are let go (pictures stay: they are asked
    /// for after the page is shown).
    pub fn release_unused(&self) {
        let mut st = self.shared.st.lock().unwrap_or_else(|p| p.into_inner());
        let mut freed = 0usize;
        let mut dropped = 0u64;
        st.slots.retain(|_, s| match s {
            Slot::Done(r, k) if *k != Kind::Image => {
                freed += r.body.len();
                dropped += 1;
                false
            }
            _ => true,
        });
        st.held = st.held.saturating_sub(freed);
        net::bump(&COUNTERS.preload_wasted, dropped);
    }

    /// How many answers are waiting to be used, how many are queued, how many bytes are held (for tests and the timing example).
    pub fn pending(&self) -> (usize, usize, usize) {
        let st = self.shared.st.lock().unwrap_or_else(|p| p.into_inner());
        (st.slots.values().filter(|s| matches!(s, Slot::Done(..))).count(), st.queue.len(), st.held)
    }
}

impl Drop for Preloader {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn worker(shared: Arc<Shared>, inner: Arc<dyn Fetch>) {
    loop {
        let (key, req, kind) = {
            let mut st = shared.st.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                if shared.cancelled.load(Ordering::Relaxed) {
                    st.workers = st.workers.saturating_sub(1);
                    return;
                }
                match st.queue.pop_front() {
                    Some((k, r, kind)) => {
                        // a page load that got there first is fetching it itself
                        if matches!(st.slots.get(&k), Some(Slot::Queued)) {
                            st.slots.insert(k.clone(), Slot::Running);
                            break (k, r, kind);
                        }
                    }
                    None => {
                        st.workers = st.workers.saturating_sub(1);
                        return;
                    }
                }
            }
        };
        let result = inner.send(&req);
        let mut st = shared.st.lock().unwrap_or_else(|p| p.into_inner());
        if shared.cancelled.load(Ordering::Relaxed) || !matches!(st.slots.get(&key), Some(Slot::Running)) {
            net::bump(&COUNTERS.preload_wasted, 1);
            continue;
        }
        match result {
            Ok(r) if st.held + r.body.len() <= MAX_HELD => {
                st.held += r.body.len();
                st.slots.insert(key, Slot::Done(r, kind));
            }
            // too much held, or the fetch failed: the page asks for itself and sees the real error
            Err(e) if kind == Kind::Image => {
                st.slots.insert(key, Slot::Failed(e));
            }
            _ => {
                st.slots.remove(&key);
            }
        }
        shared.cv.notify_all();
    }
}

impl Fetch for Preloader {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.send(&Request::get(url))
    }

    fn send(&self, req: &Request) -> Result<Response, String> {
        let plain = req.method.eq_ignore_ascii_case("GET") && req.body.is_empty() && req.if_none_match.is_empty() && req.if_modified_since.is_empty();
        if !plain {
            return self.inner.send(req);
        }
        let key = Key { url: req.url.without_fragment().to_string(), cookie: req.cookie.clone() };
        let mut st = self.shared.st.lock().unwrap_or_else(|p| p.into_inner());
        let deadline = std::time::Instant::now() + JOIN_WAIT;
        loop {
            match st.slots.get(&key) {
                Some(Slot::Done(..)) => {
                    if let Some(Slot::Done(r, _)) = st.slots.remove(&key) {
                        st.held = st.held.saturating_sub(r.body.len());
                        net::bump(&COUNTERS.preload_used, 1);
                        return Ok(r);
                    }
                }
                Some(Slot::Failed(_)) => {
                    if let Some(Slot::Failed(e)) = st.slots.remove(&key) {
                        return Err(e);
                    }
                }
                Some(Slot::Queued) => {
                    // not started yet: go now rather than wait for a turn in the queue
                    st.slots.insert(key, Slot::Claimed);
                    drop(st);
                    return self.inner.send(req);
                }
                Some(Slot::Running) => {
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    if left.is_zero() || self.shared.cancelled.load(Ordering::Relaxed) {
                        drop(st);
                        return self.inner.send(req);
                    }
                    st = self.shared.cv.wait_timeout(st, left).unwrap_or_else(|p| p.into_inner()).0;
                }
                Some(Slot::Claimed) | None => {
                    drop(st);
                    return self.inner.send(req);
                }
            }
        }
    }

    fn for_user(&self, user: &str) -> Option<Arc<dyn Fetch>> {
        self.inner.for_user(user)
    }
}

#[cfg(test)]
#[path = "preload_tests.rs"]
mod tests;
