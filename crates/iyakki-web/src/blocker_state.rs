//! The blocker in use: one shared [`Filter`] (the compiled lists, the on/off switch, the sites the person allowed, counters and a
//! short log) and [`FilteredFetch`], a transport wrapper that asks it before any request is sent, so a blocked request never
//! reaches the cache or the network. A page is never blocked silently: a blocked **page** gets an explanation page instead.
//! With no lists, a broken list or the switch off, everything passes (the filter fails open).

use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use crate::blocker::{host_of, under, Blocker, ReqInfo, ResType, Verdict};
use crate::fetch::{Fetch, Request, Response};
use crate::url::Url;

/// How many blocked requests the per-page list keeps.
pub const LOG_MAX: usize = 300;
/// Title of the explanation page, so the browser can recognise it.
pub const INTERSTITIAL_TITLE: &str = "Blocked by your filter list";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocked {
    /// Host of the page that asked.
    pub page: String,
    pub url: String,
    pub kind: ResType,
    /// The rule as written in a list.
    pub rule: String,
}

#[derive(Default)]
pub struct Filter {
    enabled: AtomicBool,
    blocker: RwLock<Arc<Blocker>>,
    allow: RwLock<BTreeSet<String>>,
    total: AtomicU64,
    checked: AtomicU64,
    page: Mutex<String>,
    page_count: AtomicU64,
    log: Mutex<VecDeque<Blocked>>,
}

impl Filter {
    pub fn new() -> Arc<Filter> {
        let f = Filter::default();
        f.enabled.store(true, Ordering::Relaxed);
        Arc::new(f)
    }

    /// The process-wide filter the browser's network uses.
    pub fn global() -> Arc<Filter> {
        static G: std::sync::OnceLock<Arc<Filter>> = std::sync::OnceLock::new();
        G.get_or_init(Filter::new).clone()
    }

    pub fn set_blocker(&self, b: Blocker) {
        if let Ok(mut w) = self.blocker.write() {
            *w = Arc::new(b);
        }
    }

    pub fn blocker(&self) -> Arc<Blocker> {
        self.blocker.read().map(|b| b.clone()).unwrap_or_default()
    }

    /// The person's choices: the switch and the hosts where blocking is off.
    pub fn configure(&self, enabled: bool, allow: &BTreeSet<String>) {
        self.enabled.store(enabled, Ordering::Relaxed);
        if let Ok(mut a) = self.allow.write() {
            if *a != *allow {
                *a = allow.clone();
            }
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Blocking is off on `host` (the person allowed it there, or it is under an allowed site).
    pub fn allowed_on(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.allow.read().map(|a| a.iter().any(|d| under(&host, d))).unwrap_or(false)
    }

    /// Blocked requests since start, and requests checked.
    pub fn totals(&self) -> (u64, u64) {
        (self.total.load(Ordering::Relaxed), self.checked.load(Ordering::Relaxed))
    }

    /// Blocked on the page now shown (the last page that was opened).
    pub fn page_blocked(&self) -> (String, u64) {
        (self.page.lock().map(|p| p.clone()).unwrap_or_default(), self.page_count.load(Ordering::Relaxed))
    }

    /// The blocked requests of the page now shown, newest last.
    pub fn page_log(&self) -> Vec<Blocked> {
        let page = self.page.lock().map(|p| p.clone()).unwrap_or_default();
        self.log.lock().map(|l| l.iter().filter(|b| b.page == page).cloned().collect()).unwrap_or_default()
    }

    /// A page is being opened: counters start again.
    pub fn new_page(&self, host: &str) {
        if let (Ok(mut p), Ok(mut l)) = (self.page.lock(), self.log.lock()) {
            *p = host.to_ascii_lowercase();
            l.clear();
        }
        self.page_count.store(0, Ordering::Relaxed);
    }

    /// Check one request. Returns the verdict and the rule text when blocked.
    pub fn check(&self, req: &Request) -> Option<Blocked> {
        if !self.enabled() {
            return None;
        }
        let blocker = self.blocker();
        if blocker.rule_count() == 0 {
            return None;
        }
        let doc = req.priority == crate::net::DOCUMENT;
        let kind = if doc { ResType::Document } else { ResType::guess(&req.url.to_string()) };
        let current = self.page.lock().map(|p| p.clone()).unwrap_or_default();
        // the referrer names the page that asked; without one, the page now shown does
        let page_host = if doc {
            req.url.host.to_ascii_lowercase()
        } else if !req.referer.is_empty() {
            host_of(&req.referer)
        } else {
            current
        };
        if doc {
            self.new_page(&page_host);
        }
        if self.allowed_on(&page_host) || (!doc && self.allowed_on(&req.url.host)) {
            return None;
        }
        self.checked.fetch_add(1, Ordering::Relaxed);
        let url = req.url.to_string();
        let initiator = if doc { "" } else { page_host.as_str() };
        match blocker.check(&ReqInfo { url: &url, initiator, kind }) {
            Verdict::Allow => None,
            Verdict::Block { rule } => {
                self.total.fetch_add(1, Ordering::Relaxed);
                self.page_count.fetch_add(1, Ordering::Relaxed);
                let b = Blocked { page: page_host, url, kind, rule: blocker.describe(rule) };
                if let Ok(mut l) = self.log.lock() {
                    if l.len() >= LOG_MAX {
                        l.pop_front();
                    }
                    l.push_back(b.clone());
                }
                Some(b)
            }
        }
    }
}

fn esc(s: &str) -> String {
    s.chars().map(|c| match c { '<' => "&lt;".to_string(), '>' => "&gt;".to_string(), '&' => "&amp;".to_string(), '"' => "&quot;".to_string(), c if c.is_control() => " ".to_string(), c => c.to_string() }).collect()
}

/// The page shown instead of a page the lists block.
pub fn interstitial_html(b: &Blocked) -> String {
    let host = host_of(&b.url);
    format!(
        "<!doctype html><html><head><title>{INTERSTITIAL_TITLE}</title></head><body><h1>{INTERSTITIAL_TITLE}</h1>\
         <p>{} was not opened because one of your filter lists blocks this address.</p>\
         <p>Rule: <code>{}</code></p>\
         <p>Press A (or choose \"Allow on this site\") to open {} anyway. The choice is kept for this site only; \
         Settings, Browser, Ads and trackers turns blocking off everywhere.</p></body></html>",
        esc(&host),
        esc(&b.rule),
        esc(&host)
    )
}

/// A transport that checks every request against a [`Filter`] first.
pub struct FilteredFetch {
    inner: Arc<dyn Fetch>,
    filter: Arc<Filter>,
}

impl FilteredFetch {
    pub fn new(inner: Arc<dyn Fetch>, filter: Arc<Filter>) -> FilteredFetch {
        FilteredFetch { inner, filter }
    }

    fn gate(&self, req: &Request) -> Option<Result<Response, String>> {
        let b = self.filter.check(req)?;
        if req.priority == crate::net::DOCUMENT {
            Some(Ok(Response { status: 200, content_type: "text/html; charset=utf-8".into(), body: interstitial_html(&b).into_bytes(), ..Response::default() }))
        } else {
            Some(Err(format!("blocked by your filter list: {}", b.rule)))
        }
    }
}

impl Fetch for FilteredFetch {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.send(&Request::get(url))
    }

    fn for_user(&self, user: &str) -> Option<Arc<dyn Fetch>> {
        self.inner.for_user(user).map(|i| Arc::new(FilteredFetch { inner: i, filter: self.filter.clone() }) as Arc<dyn Fetch>)
    }

    fn cache_stats(&self, user: &str) -> Option<crate::cache::CacheStats> {
        self.inner.cache_stats(user)
    }

    fn clear_cache(&self, user: &str) {
        self.inner.clear_cache(user)
    }

    fn set_cache_budget(&self, b: crate::net::Budget) {
        self.inner.set_cache_budget(b)
    }

    fn send(&self, req: &Request) -> Result<Response, String> {
        if let Some(r) = self.gate(req) {
            return r;
        }
        self.inner.send(req)
    }
}
