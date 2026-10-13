//! A small disk cache for pages and pictures: only plain GETs without cookies, only answers the server said may be
//! kept (`Cache-Control: max-age`), never anything private, never a large file.

use crate::fetch::{Fetch, Request, Response};
use crate::url::Url;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Largest single answer kept.
const MAX_ENTRY: usize = 2 << 20;
/// Longest freshness trusted, whatever the server asks for (one day).
const MAX_FRESH: u64 = 86_400;
/// Total size the folder is kept under.
const MAX_TOTAL: u64 = 64 << 20;

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

fn now() -> u64 {
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

/// Wraps a transport with a cache folder.
pub struct CachedFetch<F: Fetch> {
    inner: F,
    dir: PathBuf,
}

impl<F: Fetch> CachedFetch<F> {
    pub fn new(inner: F, dir: impl Into<PathBuf>) -> CachedFetch<F> {
        CachedFetch { inner, dir: dir.into() }
    }

    fn read(&self, url: &Url) -> Option<Response> {
        let raw = std::fs::read(self.dir.join(key(url))).ok()?;
        let nl = raw.iter().position(|b| *b == b'\n')?;
        let head = std::str::from_utf8(&raw[..nl]).ok()?;
        let mut f = head.split('\t');
        let (expires, status, stored_url, ct) = (f.next()?.parse::<u64>().ok()?, f.next()?.parse::<u16>().ok()?, f.next()?, f.next()?);
        if expires < now() || stored_url != url.to_string() {
            return None;
        }
        Some(Response { status, content_type: ct.to_string(), body: raw[nl + 1..].to_vec(), ..Response::default() })
    }

    fn write(&self, url: &Url, r: &Response, age: u64) {
        if r.status != 200 || r.body.len() > MAX_ENTRY || r.content_length.is_some() || !r.set_cookies.is_empty() || r.content_type.contains(['\t', '\n']) {
            return;
        }
        if std::fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let mut out = format!("{}\t{}\t{}\t{}\n", now() + age.min(MAX_FRESH), r.status, url, r.content_type).into_bytes();
        out.extend_from_slice(&r.body);
        let tmp = self.dir.join(format!("{}.tmp", key(url)));
        if std::fs::write(&tmp, out).is_ok() && std::fs::rename(&tmp, self.dir.join(key(url))).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        self.trim();
    }

    /// Delete the oldest entries until the folder is under its limit.
    fn trim(&self) {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return };
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> =
            rd.flatten().filter_map(|e| e.metadata().ok().map(|m| (m.modified().unwrap_or(UNIX_EPOCH), m.len(), e.path()))).collect();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort();
        for (_, len, path) in files {
            if total <= MAX_TOTAL {
                break;
            }
            if std::fs::remove_file(path).is_ok() {
                total -= len;
            }
        }
    }

    /// Remove everything kept.
    pub fn clear(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl<F: Fetch> Fetch for CachedFetch<F> {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.send(&Request::get(url))
    }

    fn send(&self, req: &Request) -> Result<Response, String> {
        let plain = req.method.eq_ignore_ascii_case("GET") && req.cookie.is_empty();
        if plain {
            if let Some(hit) = self.read(&req.url) {
                return Ok(hit);
            }
        }
        let r = self.inner.send(req)?;
        if plain {
            if let Some(age) = r.max_age {
                self.write(&req.url, &r, age);
            }
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct Counting(Arc<AtomicUsize>, Option<u64>);
    impl Fetch for Counting {
        fn get(&self, _: &Url) -> Result<Response, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Response { status: 200, content_type: "text/plain".into(), body: b"hello".to_vec(), max_age: self.1, ..Response::default() })
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn max_age_reads_the_header() {
        assert_eq!(max_age("public, max-age=300"), Some(300));
        assert_eq!(max_age("max-age=0"), None);
        assert_eq!(max_age("no-store, max-age=60"), None);
        assert_eq!(max_age("private, max-age=60"), None);
        assert_eq!(max_age("s-maxage=\"90\""), Some(90));
        assert_eq!(max_age(""), None);
    }

    #[test]
    fn second_get_comes_from_disk() {
        let n = Arc::new(AtomicUsize::new(0));
        let dir = tmp("hit");
        let c = CachedFetch::new(Counting(n.clone(), Some(60)), &dir);
        let url = Url::parse("https://a.org/x.txt").unwrap();
        let a = c.get(&url).unwrap();
        let b = c.get(&url).unwrap();
        assert_eq!((a.body, b.body, n.load(Ordering::SeqCst)), (b"hello".to_vec(), b"hello".to_vec(), 1));
        assert_eq!(b.content_type, "text/plain");
        c.clear();
        c.get(&url).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 2);
        c.clear();
    }

    #[test]
    fn answers_without_a_max_age_are_not_kept() {
        let n = Arc::new(AtomicUsize::new(0));
        let dir = tmp("miss");
        let c = CachedFetch::new(Counting(n.clone(), None), &dir);
        let url = Url::parse("https://a.org/y").unwrap();
        c.get(&url).unwrap();
        c.get(&url).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 2);
        c.clear();
    }

    #[test]
    fn requests_with_cookies_skip_the_cache() {
        let n = Arc::new(AtomicUsize::new(0));
        let dir = tmp("cookie");
        let c = CachedFetch::new(Counting(n.clone(), Some(60)), &dir);
        let url = Url::parse("https://a.org/z").unwrap();
        let req = Request { cookie: "s=1".into(), ..Request::get(&url) };
        c.send(&req).unwrap();
        c.send(&req).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 2);
        c.clear();
    }

    #[test]
    fn expired_entries_are_refetched() {
        let n = Arc::new(AtomicUsize::new(0));
        let dir = tmp("old");
        let c = CachedFetch::new(Counting(n.clone(), Some(60)), &dir);
        let url = Url::parse("https://a.org/old").unwrap();
        c.get(&url).unwrap();
        let path = dir.join(key(&url));
        let raw = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).replacen(char::is_numeric, "0", 1);
        let _ = raw;
        let mut bytes = std::fs::read(&path).unwrap();
        let nl = bytes.iter().position(|b| *b == b'\n').unwrap();
        let head = String::from_utf8(bytes[..nl].to_vec()).unwrap();
        let rest: Vec<&str> = head.splitn(2, '\t').collect();
        let mut out = format!("1\t{}\n", rest[1]).into_bytes();
        out.extend_from_slice(&bytes.split_off(nl + 1));
        std::fs::write(&path, out).unwrap();
        c.get(&url).unwrap();
        assert_eq!(n.load(Ordering::SeqCst), 2);
        c.clear();
    }
}
