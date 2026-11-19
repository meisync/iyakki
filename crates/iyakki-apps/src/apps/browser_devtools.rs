//! The developer panel's network list: every request a page load made (the page, its style sheets, pictures and the
//! requests its scripts sent), with the answer's status, size and time. Recorded by wrapping the browser's transport.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use iyakki_web::{Fetch, Request, Response, Url};

/// Most lines kept for one page.
const MAX_LINES: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetLine {
    pub method: String,
    pub url: String,
    /// `None` when the request failed before an answer came.
    pub status: Option<u16>,
    pub bytes: usize,
    pub ms: u128,
    pub error: String,
}

impl NetLine {
    pub fn text(&self) -> String {
        match self.status {
            Some(s) => format!("{:<4} {s} {:>8} {:>5} ms  {}", self.method, human(self.bytes), self.ms, self.url),
            None => format!("{:<4} --- {:>8} {:>5} ms  {}  ({})", self.method, "", self.ms, self.url, self.error),
        }
    }
}

fn human(n: usize) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

pub type Log = Arc<Mutex<Vec<NetLine>>>;

/// A transport that notes what passes through it.
pub struct Recording {
    inner: Arc<dyn Fetch>,
    log: Log,
}

impl Recording {
    pub fn new(inner: Arc<dyn Fetch>, log: Log) -> Recording {
        Recording { inner, log }
    }

    fn note(&self, method: &str, url: &Url, started: Instant, r: &Result<Response, String>) {
        let line = match r {
            Ok(r) => NetLine { method: method.into(), url: url.to_string(), status: Some(r.status), bytes: r.content_length.map_or(r.body.len(), |l| l as usize), ms: started.elapsed().as_millis(), error: String::new() },
            Err(e) => NetLine { method: method.into(), url: url.to_string(), status: None, bytes: 0, ms: started.elapsed().as_millis(), error: e.clone() },
        };
        if let Ok(mut l) = self.log.lock() {
            if l.len() < MAX_LINES {
                l.push(line);
            }
        }
    }
}

impl Fetch for Recording {
    fn get(&self, url: &Url) -> Result<Response, String> {
        let t = Instant::now();
        let r = self.inner.get(url);
        self.note("GET", url, t, &r);
        r
    }

    fn send(&self, req: &Request) -> Result<Response, String> {
        let t = Instant::now();
        let r = self.inner.send(req);
        self.note(if req.method.is_empty() { "GET" } else { &req.method }, &req.url, t, &r);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed;
    impl Fetch for Fixed {
        fn get(&self, url: &Url) -> Result<Response, String> {
            if url.path.contains("bad") { Err("refused".into()) } else { Ok(Response { status: 200, body: vec![0; 2048], ..Response::default() }) }
        }
    }

    #[test]
    fn requests_are_listed_with_status_size_and_failures() {
        let log: Log = Default::default();
        let r = Recording::new(Arc::new(Fixed), log.clone());
        r.get(&Url::parse("https://a.org/x").unwrap()).unwrap();
        assert!(r.get(&Url::parse("https://a.org/bad").unwrap()).is_err());
        let l = log.lock().unwrap();
        assert_eq!(l.len(), 2);
        assert!(l[0].text().contains("200") && l[0].text().contains("2.0 KB") && l[0].text().contains("https://a.org/x"), "{}", l[0].text());
        assert!(l[1].text().contains("---") && l[1].text().contains("refused"));
    }
}
