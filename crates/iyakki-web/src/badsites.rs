//! The known-bad site list (F-BROW-98): sites the person (or a list they chose to download) says are dangerous, such as phishing
//! or malware hosts. A request to a listed site, or to any host under it, is refused before anything is sent, and a **page** gets an
//! explanation instead of an error so the person knows why it did not open. The list is the person's: nothing is shipped with the
//! system and nothing is fetched until they ask (the shell's download goes through the same transport as pages, from an address
//! they give or one of the [`SUGGESTED`] lists they pick).
//!
//! Text format, one site per line, all of these are read: `example.org`, `example.org  why it is listed`, the hosts-file form
//! `0.0.0.0 example.org`, and comment lines starting with `#`. A line with a full address (`http://x/y`) is skipped on purpose:
//! it names one file, and blocking a whole site for it would be wrong. Hosts are matched as written or as a parent domain
//! (`example.org` covers `a.b.example.org`); a single label such as `com` or `localhost` is never accepted.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::fetch::{Fetch, Request, Response};
use crate::url::Url;

pub const MAX_ENTRIES: usize = 400_000;
pub const MAX_BYTES: usize = 16 << 20;
/// Title of the explanation page.
pub const INTERSTITIAL_TITLE: &str = "Site on your known-bad list";

/// A list the program can suggest (the person still has to click; the text is third-party data with its own licence).
#[derive(Clone, Copy, Debug)]
pub struct Suggested {
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    pub licence: &'static str,
    pub what: &'static str,
}

pub const SUGGESTED: &[Suggested] = &[Suggested { id: "urlhaus", name: "URLhaus host list", url: "https://urlhaus.abuse.ch/downloads/hostfile/", licence: "CC0 as the publisher states it (check on its site)", what: "hosts that serve malware, from abuse.ch" }];

/// A host written as a person or a list wrote it, made ready for matching; `None` when it is not a usable host.
pub fn normalise_host(s: &str) -> Option<String> {
    let t = s.trim().trim_end_matches('.').to_ascii_lowercase();
    if t.is_empty() || t.len() > 253 || t.contains(['/', '?', '#', '@', ' ', ':', '*']) {
        return None;
    }
    let labels: Vec<&str> = t.split('.').collect();
    if labels.len() < 2 || labels.iter().any(|l| l.is_empty() || l.len() > 63 || l.starts_with('-') || l.ends_with('-') || !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
        return None;
    }
    Some(t)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BadSites {
    map: HashMap<String, String>,
}

/// What reading a text found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub added: usize,
    pub skipped: usize,
    /// More lines than [`MAX_ENTRIES`] allow were left unread.
    pub truncated: bool,
}

impl BadSites {
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Reads list text and adds what it can use.
    pub fn read_text(&mut self, text: &str) -> Parsed {
        let mut p = Parsed::default();
        for line in text.lines().take(4_000_000) {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // a trailing comment of a hosts file
            let line = line.split(" #").next().unwrap_or(line).trim();
            let mut it = line.split_whitespace();
            let first = it.next().unwrap_or("");
            let (host, reason) = if matches!(first, "0.0.0.0" | "127.0.0.1" | "::" | "::1") {
                (it.next().unwrap_or(""), it.collect::<Vec<_>>().join(" "))
            } else {
                (first, it.collect::<Vec<_>>().join(" "))
            };
            match normalise_host(host) {
                Some(h) => {
                    if self.map.len() >= MAX_ENTRIES && !self.map.contains_key(&h) {
                        p.truncated = true;
                        break;
                    }
                    self.map.insert(h, reason.chars().take(200).collect());
                    p.added += 1;
                }
                None => p.skipped += 1,
            }
        }
        p
    }

    pub fn add(&mut self, host: &str, reason: &str) -> Result<String, String> {
        let h = normalise_host(host).ok_or_else(|| format!("{host:?} is not a site name (write it like example.org, without http:// or a path)"))?;
        if self.map.len() >= MAX_ENTRIES && !self.map.contains_key(&h) {
            return Err("the list is full".into());
        }
        self.map.insert(h.clone(), reason.chars().take(200).collect());
        Ok(h)
    }

    pub fn remove(&mut self, host: &str) -> bool {
        normalise_host(host).is_some_and(|h| self.map.remove(&h).is_some())
    }

    /// The listed site that `host` is or is under, with the reason (empty when none was given).
    pub fn check_host(&self, host: &str) -> Option<(&str, &str)> {
        if self.map.is_empty() {
            return None;
        }
        let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
        let mut rest = h.as_str();
        loop {
            if let Some((k, v)) = self.map.get_key_value(rest) {
                return Some((k.as_str(), v.as_str()));
            }
            let (_, parent) = rest.split_once('.')?;
            if !parent.contains('.') {
                return None;
            }
            rest = parent;
        }
    }

    /// The list as text (sorted), to be saved and read again.
    pub fn to_text(&self) -> String {
        let mut v: Vec<(&String, &String)> = self.map.iter().collect();
        v.sort();
        v.iter().map(|(h, r)| if r.is_empty() { format!("{h}\n") } else { format!("{h}\t{r}\n") }).collect()
    }

    pub fn entries(&self, limit: usize) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self.map.iter().map(|(h, r)| (h.clone(), r.clone())).collect();
        v.sort();
        v.truncate(limit);
        v
    }
}

/// The list in use and its switch, shared by every transport that was given it.
#[derive(Default)]
pub struct Guard {
    list: RwLock<Arc<BadSites>>,
    off: std::sync::atomic::AtomicBool,
}

impl Guard {
    pub fn global() -> Arc<Guard> {
        static G: std::sync::OnceLock<Arc<Guard>> = std::sync::OnceLock::new();
        G.get_or_init(|| Arc::new(Guard::default())).clone()
    }

    pub fn set(&self, list: BadSites, enabled: bool) {
        if let Ok(mut w) = self.list.write() {
            *w = Arc::new(list);
        }
        self.off.store(!enabled, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn list(&self) -> Arc<BadSites> {
        self.list.read().map(|l| l.clone()).unwrap_or_default()
    }

    pub fn enabled(&self) -> bool {
        !self.off.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The listed site and reason for `host`, when the guard is on.
    pub fn check(&self, host: &str) -> Option<(String, String)> {
        if !self.enabled() {
            return None;
        }
        self.list().check_host(host).map(|(a, b)| (a.to_string(), b.to_string()))
    }
}

fn esc(s: &str) -> String {
    s.chars().map(|c| match c { '<' => "&lt;".to_string(), '>' => "&gt;".to_string(), '&' => "&amp;".to_string(), '"' => "&quot;".to_string(), c => c.to_string() }).collect()
}

/// The page shown instead of a listed site.
pub fn interstitial_html(host: &str, listed: &str, reason: &str) -> String {
    let why = if reason.is_empty() { String::new() } else { format!("<p>Why it is listed: {}</p>", esc(reason)) };
    format!(
        "<html><head><title>{INTERSTITIAL_TITLE}</title></head><body><h1>{INTERSTITIAL_TITLE}</h1><p>{} was not opened: {} is on your known-bad site list.</p>{why}<p>Nothing was sent to the site. To open it anyway, take it off the list (ask the assistant to remove it from the known-bad list), or switch the list off.</p></body></html>",
        esc(host),
        esc(listed)
    )
}

/// A transport that refuses listed sites before anything is sent.
pub struct BadSiteFetch {
    inner: Arc<dyn Fetch>,
    guard: Arc<Guard>,
}

impl BadSiteFetch {
    pub fn new(inner: Arc<dyn Fetch>, guard: Arc<Guard>) -> BadSiteFetch {
        BadSiteFetch { inner, guard }
    }

    fn gate(&self, req: &Request) -> Option<Result<Response, String>> {
        let (listed, reason) = self.guard.check(&req.url.host)?;
        if req.priority == crate::net::DOCUMENT {
            Some(Ok(Response { status: 200, content_type: "text/html; charset=utf-8".into(), body: interstitial_html(&req.url.host, &listed, &reason).into_bytes(), ..Response::default() }))
        } else {
            Some(Err(format!("{} is on your known-bad site list", req.url.host)))
        }
    }
}

impl Fetch for BadSiteFetch {
    fn get(&self, url: &Url) -> Result<Response, String> {
        self.send(&Request::get(url))
    }

    fn for_user(&self, user: &str) -> Option<Arc<dyn Fetch>> {
        self.inner.for_user(user).map(|i| Arc::new(BadSiteFetch { inner: i, guard: self.guard.clone() }) as Arc<dyn Fetch>)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counting(AtomicUsize);
    impl Fetch for Counting {
        fn get(&self, _: &Url) -> Result<Response, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Response { status: 200, body: b"real page".to_vec(), content_type: "text/html".into(), ..Response::default() })
        }
    }

    fn list(text: &str) -> BadSites {
        let mut b = BadSites::default();
        b.read_text(text);
        b
    }

    #[test]
    fn every_line_form_is_read_and_urls_and_nonsense_are_skipped() {
        let mut b = BadSites::default();
        let p = b.read_text("# a comment\n\nevil.example\nphish.example  stole logins\n0.0.0.0 bad.example\n127.0.0.1 worse.example malware # trailing\nhttp://x.example/payload.exe\ncom\nlocalhost\nUPPER.Example.\nspace here.example\n*.wild.example\n");
        assert_eq!(p, Parsed { added: 5, skipped: 5, truncated: false });
        assert_eq!(b.check_host("evil.example"), Some(("evil.example", "")));
        assert_eq!(b.check_host("phish.example").unwrap().1, "stole logins");
        assert_eq!(b.check_host("worse.example").unwrap().1, "malware");
        assert!(b.check_host("upper.example").is_some(), "names are lower-cased and a final dot is dropped");
        assert!(b.check_host("x.example").is_none(), "the URL line was not turned into a whole-site entry");
    }

    #[test]
    fn a_site_covers_its_sub_domains_but_not_look_alikes_or_the_parent() {
        let b = list("evil.example\nsub.deep.example\n");
        assert_eq!(b.check_host("a.b.evil.example").unwrap().0, "evil.example");
        assert!(b.check_host("EVIL.example.").is_some());
        assert!(b.check_host("notevil.example").is_none());
        assert!(b.check_host("evil.example.org").is_none());
        assert!(b.check_host("deep.example").is_none(), "a child's listing does not cover its parent");
        assert!(b.check_host("x.sub.deep.example").is_some());
        assert!(b.check_host("example").is_none());
        assert!(BadSites::default().check_host("evil.example").is_none());
    }

    #[test]
    fn add_remove_and_save_round_trip() {
        let mut b = BadSites::default();
        assert_eq!(b.add("Evil.Example", "phishing").unwrap(), "evil.example");
        assert!(b.add("http://evil.example/x", "").is_err());
        assert!(b.add("com", "").is_err());
        b.add("a.example", "").unwrap();
        let again = list(&b.to_text());
        assert_eq!(again, b);
        assert!(b.remove("EVIL.example"));
        assert!(!b.remove("evil.example"));
        assert_eq!(b.len(), 1);
        assert_eq!(b.entries(10), vec![("a.example".to_string(), String::new())]);
    }

    #[test]
    fn the_size_cap_holds_and_hostile_text_is_only_skipped() {
        let mut b = BadSites::default();
        let big: String = (0..MAX_ENTRIES + 50).map(|i| format!("h{i}.example\n")).collect();
        let p = b.read_text(&big);
        assert!(p.truncated && b.len() == MAX_ENTRIES);
        let mut c = BadSites::default();
        let p = c.read_text(&"\u{0}\u{ffff}....\n..\n-a.example\na-.example\n".repeat(100));
        assert_eq!((p.added, c.len()), (0, 0));
    }

    #[test]
    fn a_listed_page_gets_an_explanation_and_nothing_is_sent_while_other_requests_fail_plainly() {
        let inner = Arc::new(Counting(AtomicUsize::new(0)));
        let g = Arc::new(Guard::default());
        g.set(list("evil.example\tcredential theft <b>\n"), true);
        let f = BadSiteFetch::new(inner.clone(), g.clone());
        let page = f.send(&Request { priority: crate::net::DOCUMENT, ..Request::get(&Url::parse("https://login.evil.example/a").unwrap()) }).unwrap();
        let html = String::from_utf8(page.body).unwrap();
        assert!(html.contains(INTERSTITIAL_TITLE) && html.contains("login.evil.example") && html.contains("credential theft &lt;b&gt;"), "{html}");
        let e = f.get(&Url::parse("https://evil.example/pic.png").unwrap()).unwrap_err();
        assert!(e.contains("known-bad"), "{e}");
        assert_eq!(inner.0.load(Ordering::SeqCst), 0, "nothing reached the network");
        assert_eq!(f.get(&Url::parse("https://fine.example/").unwrap()).unwrap().body, b"real page");
        assert_eq!(inner.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn switching_the_list_off_lets_everything_through_and_a_redirect_is_judged_at_every_hop() {
        let inner = Arc::new(Counting(AtomicUsize::new(0)));
        let g = Arc::new(Guard::default());
        g.set(list("evil.example\n"), false);
        let f = BadSiteFetch::new(inner.clone(), g.clone());
        assert!(f.get(&Url::parse("https://evil.example/").unwrap()).is_ok());
        g.set(list("evil.example\n"), true);
        assert!(f.get(&Url::parse("https://evil.example/").unwrap()).is_err());
        // a redirect to a listed host: fetch_following sends every hop through the transport
        struct Redirect;
        impl Fetch for Redirect {
            fn get(&self, u: &Url) -> Result<Response, String> {
                if u.host == "start.example" {
                    Ok(Response { status: 302, location: Some("https://evil.example/landing".into()), ..Response::default() })
                } else {
                    Ok(Response { status: 200, body: b"ok".to_vec(), ..Response::default() })
                }
            }
        }
        let f = BadSiteFetch::new(Arc::new(Redirect), g);
        let r = crate::fetch::fetch_following(&f, Request::get(&Url::parse("https://start.example/").unwrap()), &|_| true, None);
        assert!(r.unwrap_err().contains("known-bad"));
    }
}
