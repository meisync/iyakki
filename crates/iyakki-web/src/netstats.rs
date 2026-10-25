//! Measuring a page load: a transport wrapper that notes every request (when it started and ended on the page's clock,
//! how many bytes, how many requests to the same host were in flight at that moment) so a timeline can be printed.
//! Used by the `browser_timing` example and the tests; it adds two clock reads and a short lock per request.

use crate::fetch::{Fetch, Request, Response};
use crate::url::Url;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// One finished request.
#[derive(Clone, Debug)]
pub struct Entry {
    pub url: String,
    pub host: String,
    pub method: String,
    pub status: u16,
    pub content_type: String,
    pub bytes: usize,
    /// Milliseconds since the log was created.
    pub start_ms: f64,
    pub end_ms: f64,
    /// Requests to this host in flight when this one started (itself included).
    pub host_concurrency: usize,
    pub error: String,
}

impl Entry {
    pub fn ms(&self) -> f64 {
        self.end_ms - self.start_ms
    }

    /// `html`, `css`, `js`, `image`, `font`, `json`, `other`, from the content type (or the address when the server did not say).
    pub fn kind(&self) -> &'static str {
        let ct = self.content_type.to_ascii_lowercase();
        let path = self.url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
        if ct.contains("html") {
            "html"
        } else if ct.contains("css") || path.ends_with(".css") {
            "css"
        } else if ct.contains("javascript") || ct.contains("ecmascript") || path.ends_with(".js") || path.ends_with(".mjs") {
            "js"
        } else if ct.starts_with("image/") {
            "image"
        } else if ct.contains("font") || path.ends_with(".woff2") || path.ends_with(".woff") {
            "font"
        } else if ct.contains("json") {
            "json"
        } else {
            "other"
        }
    }
}

#[derive(Default)]
struct State {
    entries: Vec<Entry>,
    in_flight: HashMap<String, usize>,
    in_flight_all: usize,
    max_all: usize,
}

/// A transport that records what passes through it.
pub struct TimedFetch<F: Fetch> {
    inner: F,
    t0: Instant,
    state: Mutex<State>,
}

impl<F: Fetch> TimedFetch<F> {
    pub fn new(inner: F) -> TimedFetch<F> {
        TimedFetch { inner, t0: Instant::now(), state: Mutex::new(State::default()) }
    }

    pub fn inner(&self) -> &F {
        &self.inner
    }

    /// Milliseconds on this log's clock.
    pub fn now_ms(&self) -> f64 {
        self.t0.elapsed().as_secs_f64() * 1000.0
    }

    /// Everything recorded so far, in the order the requests finished.
    pub fn entries(&self) -> Vec<Entry> {
        self.state.lock().map(|s| s.entries.clone()).unwrap_or_default()
    }

    /// The most requests that were in flight at once, over all hosts.
    pub fn max_concurrency(&self) -> usize {
        self.state.lock().map(|s| s.max_all).unwrap_or(0)
    }

    /// Forget the entries and start the clock again.
    pub fn reset(&mut self) {
        self.t0 = Instant::now();
        if let Ok(mut s) = self.state.lock() {
            *s = State::default();
        }
    }
}

impl<F: Fetch> Fetch for TimedFetch<F> {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.send(&Request::get(url))
    }

    fn send(&self, req: &Request) -> Result<Response, String> {
        let host = req.url.host.clone();
        let start_ms = self.now_ms();
        let concurrency = {
            let mut s = match self.state.lock() {
                Ok(s) => s,
                Err(_) => return self.inner.send(req),
            };
            let n = s.in_flight.entry(host.clone()).or_insert(0);
            *n += 1;
            let n = *n;
            s.in_flight_all += 1;
            s.max_all = s.max_all.max(s.in_flight_all);
            n
        };
        let r = self.inner.send(req);
        let end_ms = self.now_ms();
        if let Ok(mut s) = self.state.lock() {
            if let Some(n) = s.in_flight.get_mut(&host) {
                *n = n.saturating_sub(1);
            }
            s.in_flight_all = s.in_flight_all.saturating_sub(1);
            let (status, content_type, bytes, error) = match &r {
                Ok(r) => (r.status, r.content_type.clone(), r.content_length.map_or(r.body.len(), |l| l as usize), String::new()),
                Err(e) => (0, String::new(), 0, e.clone()),
            };
            s.entries.push(Entry { url: req.url.to_string(), host, method: req.method.clone(), status, content_type, bytes, start_ms, end_ms, host_concurrency: concurrency, error });
        }
        r
    }
}

/// Highest value of `host_concurrency` per host.
pub fn max_per_host(entries: &[Entry]) -> Vec<(String, usize)> {
    let mut m: HashMap<&str, usize> = HashMap::new();
    for e in entries {
        let v = m.entry(&e.host).or_insert(0);
        *v = (*v).max(e.host_concurrency);
    }
    let mut v: Vec<(String, usize)> = m.into_iter().map(|(h, n)| (h.to_string(), n)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Slow;
    impl Fetch for Slow {
        fn get(&self, _: &Url) -> Result<Response, String> {
            std::thread::sleep(std::time::Duration::from_millis(30));
            Ok(Response { status: 200, content_type: "text/css".into(), body: vec![0; 10], ..Response::default() })
        }
    }

    #[test]
    fn records_requests_and_the_concurrency_they_reached() {
        let t = Arc::new(TimedFetch::new(Slow));
        let hs: Vec<_> = (0..4)
            .map(|i| {
                let t = t.clone();
                std::thread::spawn(move || t.get(&Url::parse(&format!("https://a.org/{i}.css")).unwrap()).unwrap())
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        let e = t.entries();
        assert_eq!(e.len(), 4);
        assert!(e.iter().all(|x| x.kind() == "css" && x.bytes == 10 && x.ms() >= 25.0));
        assert!(t.max_concurrency() >= 2, "overlapping requests are seen as overlapping");
        assert!(max_per_host(&e)[0].1 >= 2);
    }
}
