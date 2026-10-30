//! Loading a page: any transport, redirects followed, every host checked against the caller's policy, the body turned
//! into a [`Doc`] according to what the server says it is.

use crate::cookies::CookieStore;
use crate::doc::{self, Doc};
use crate::url::Url;

/// One request, as the transport should send it (redirects are followed by the caller, not the transport).
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub url: Url,
    /// `GET`, `HEAD` or `POST`.
    pub method: String,
    pub body: Vec<u8>,
    /// For a body (`application/x-www-form-urlencoded`).
    pub content_type: String,
    /// The `Cookie` header, empty for none.
    pub cookie: String,
    pub referer: String,
    /// How urgent the answer is, for transports that queue requests (see [`crate::net`]). 0 means "not said".
    pub priority: u8,
    /// Revalidation of a kept answer: sent as `If-None-Match` and `If-Modified-Since` (empty for none).
    pub if_none_match: String,
    pub if_modified_since: String,
    /// A `Range` header value such as `bytes=0-0` (empty for none), for transports that send it.
    pub range: String,
    /// Extra request headers the person set for this address (Settings -> Browser -> Custom headers). Values are secret-like: the
    /// `Debug` text shows names only. Not carried over a redirect: the next address is judged again.
    pub extra: ExtraHeaders,
}

/// Header (name, value) pairs whose values must never reach a log: `Debug` prints the names.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ExtraHeaders(pub Vec<(String, String)>);

impl std::fmt::Debug for ExtraHeaders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.iter().map(|(n, _)| format!("{n}: <hidden>"))).finish()
    }
}

impl Request {
    pub fn get(url: &Url) -> Request {
        Request { url: url.clone(), method: "GET".into(), ..Request::default() }
    }
}

/// What a transport brings back for one request (redirects are not followed by the transport).
#[derive(Clone, Debug, Default)]
pub struct Response {
    pub status: u16,
    /// The `Location` header of a redirect.
    pub location: Option<String>,
    /// The `Content-Type` header, as sent.
    pub content_type: String,
    /// Every `Set-Cookie` header.
    pub set_cookies: Vec<String>,
    pub body: Vec<u8>,
    /// The size the server announced, when `body` holds only the start of a large file (video, audio).
    pub content_length: Option<u64>,
    /// Seconds the server says the answer stays fresh (`Cache-Control: max-age`), `None` when it did not say or forbade storing.
    pub max_age: Option<u64>,
    /// The caching headers as the server sent them (empty when absent), so a cache can decide for itself.
    pub cache_control: String,
    pub etag: String,
    pub last_modified: String,
    pub vary: String,
    pub expires: String,
    /// The `Age` header in seconds (how long a shared cache already held the answer).
    pub age: u64,
    /// Every response header as sent, in order, names as the server wrote them (values cut to 1000 characters, at most 80 headers).
    /// Empty for transports that do not keep them. Cookies and credentials are in here too: whoever shows them must scrub them.
    pub headers: Vec<(String, String)>,
}

pub trait Fetch: Send + Sync {
    fn get(&self, url: &Url) -> Result<Response, String>;

    /// A transport that keeps a separate cache per person returns the one for `user` (`None`: this transport has no such split).
    fn for_user(&self, _user: &str) -> Option<std::sync::Arc<dyn Fetch>> {
        None
    }

    /// What the cache of `user` (or this transport's only cache) holds; `None` when there is no cache.
    fn cache_stats(&self, _user: &str) -> Option<crate::cache::CacheStats> {
        None
    }

    /// Empty the cache of `user`.
    fn clear_cache(&self, _user: &str) {}

    /// Set the cache sizes (the browser does this from the machine's memory, so the memory gate can take cache back).
    fn set_cache_budget(&self, _b: crate::net::Budget) {}

    /// Any request. A transport that only reads answers GETs and refuses the rest.
    fn send(&self, req: &Request) -> Result<Response, String> {
        if req.method.eq_ignore_ascii_case("GET") || req.method.eq_ignore_ascii_case("HEAD") {
            self.get(&req.url)
        } else {
            Err("this connection can only read pages (GET)".into())
        }
    }
}

/// A transport that cannot reach anything (builds without HTTPS, tests).
pub struct NoNetwork;

impl Fetch for NoNetwork {
    fn get(&self, _: &Url) -> Result<Response, String> {
        Err("this system has no web access built in".into())
    }
}

#[derive(Clone, Debug)]
pub enum Loaded {
    /// A page to show. `url` is where it ended up after redirects. `notes` are lines for a developer view (script
    /// errors, what was skipped).
    Page { url: Url, status: u16, doc: Doc, notes: Vec<String>, source: String },
    /// Something that is not a web page (an image, a download).
    /// A file the reader cannot show (a download, a video). `bytes` is its size when known; `info` describes what could be
    /// read from its start ("H.264 video, 320×240, 5 s").
    Unsupported { url: Url, content_type: String, bytes: usize, info: String },
}

const MAX_REDIRECTS: usize = 8;

/// Turns the HTML of a page into what is shown, and notes for the developer view. The default is [`doc::from_html`];
/// a browser with scripting on passes one that runs the page's scripts first.
pub type Renderer<'a> = &'a dyn Fn(&Url, &str) -> (Doc, Vec<String>);

fn plain(_: &Url, html: &str) -> (Doc, Vec<String>) {
    (doc::from_html(html), Vec::new())
}

/// Load `url`. `allow` is asked about the host of the address and of every redirect, so a site cannot send the browser
/// somewhere the user's role may not go.
pub fn load(f: &dyn Fetch, url: &Url, allow: &dyn Fn(&str) -> bool) -> Result<Loaded, String> {
    load_with(f, url, allow, &plain)
}

pub fn load_with(f: &dyn Fetch, url: &Url, allow: &dyn Fn(&str) -> bool, render: Renderer) -> Result<Loaded, String> {
    load_request(f, Request::get(url), allow, render, None)
}

/// Cookies for a load: the browser's jar (locked only for the moment each request or answer needs it, because scripts
/// on the page being loaded use the same jar), and "now" in seconds since the epoch.
pub struct Jar<'a> {
    pub store: &'a std::sync::Mutex<CookieStore>,
    pub now: i64,
}

/// Load a request (a GET, or a form POST), following redirects, sending and storing cookies when a jar is given.
/// A `POST` answered by 301, 302 or 303 continues as a GET; 307 and 308 repeat the same request.
pub fn load_request(f: &dyn Fetch, req: Request, allow: &dyn Fn(&str) -> bool, render: Renderer, mut jar: Option<Jar>) -> Result<Loaded, String> {
    let fragment = req.url.fragment.clone();
    let (mut final_req, mut r) = fetch_following(f, req, allow, jar.as_mut().map(|j| Jar { store: j.store, now: j.now }))?;
    // a stub page whose only job is to send the reader on (`<meta http-equiv=refresh>`, also inside `<noscript>`): search
    // engines use them for their result links
    for _ in 0..MAX_REFRESHES {
        let Some(next) = refresh_target(&r, &final_req.url) else { break };
        let next_req = Request { referer: final_req.url.to_string(), ..Request::get(&next) };
        (final_req, r) = fetch_following(f, next_req, allow, jar.as_mut().map(|j| Jar { store: j.store, now: j.now }))?;
    }
    let mut shown = final_req.url.clone();
    shown.fragment = fragment;
    Ok(interpret(shown, r, render))
}

const MAX_REFRESHES: usize = 4;

/// Where a small HTML page that immediately refreshes elsewhere sends the reader.
pub fn refresh_target(r: &Response, base: &Url) -> Option<Url> {
    let ct = r.content_type.to_ascii_lowercase();
    if r.status != 200 || r.body.len() > 8192 || !(ct.contains("html") || ct.is_empty()) {
        return None;
    }
    let text = String::from_utf8_lossy(&r.body);
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find("http-equiv") {
        let at = from + i;
        let tag_start = lower[..at].rfind('<')?;
        let tag_end = lower[at..].find('>').map_or(lower.len(), |k| at + k);
        let tag = &lower[tag_start..tag_end];
        from = at + 10;
        if !tag.contains("refresh") {
            continue;
        }
        let ci = tag.find("content")?;
        let rest = tag[ci + 7..].trim_start().strip_prefix('=')?.trim_start();
        let (q, rest) = match rest.chars().next()? {
            c @ ('"' | '\'') => (Some(c), &rest[1..]),
            _ => (None, rest),
        };
        let end = match q {
            Some(c) => rest.find(c).unwrap_or(rest.len()),
            None => rest.find(char::is_whitespace).unwrap_or(rest.len()),
        };
        // the address keeps its original case
        let value_start = tag_start + (tag.len() - rest.len());
        let value = &text[value_start..value_start + end];
        let (delay, target) = value.split_once([';', ',']).map_or((value, ""), |(d, t)| (d, t));
        if delay.trim().parse::<f32>().ok().is_none_or(|d| d > 5.0) {
            return None;
        }
        let t = target.trim();
        let t = if t.len() >= 3 && t[..3].eq_ignore_ascii_case("url") { t[3..].trim_start().strip_prefix('=').unwrap_or(t).trim() } else { t };
        let t = t.trim_matches(['"', '\'']);
        if t.is_empty() {
            return None;
        }
        return base.join(t).ok().filter(|u| u.without_fragment() != base.without_fragment());
    }
    None
}

/// Send a request and follow its redirects (checking every host against `allow`, sending and storing cookies when a jar is
/// given). The request that finally got an answer, and the answer.
pub fn fetch_following(f: &dyn Fetch, req: Request, allow: &dyn Fn(&str) -> bool, mut jar: Option<Jar>) -> Result<(Request, Response), String> {
    let mut req = Request { url: req.url.without_fragment(), ..req };
    for _ in 0..=MAX_REDIRECTS {
        if !allow(&req.url.host) {
            return Err(format!("your account may not open pages from {}", req.url.host));
        }
        if let Some(j) = &jar {
            req.cookie = j.store.lock().map(|s| s.header_for(&req.url, j.now)).unwrap_or_default();
        }
        let r = f.send(&req)?;
        if let Some(j) = &mut jar {
            if let Ok(mut s) = j.store.lock() {
                for c in &r.set_cookies {
                    s.store_from_header(&req.url, c, j.now);
                }
            }
        }
        if (300..400).contains(&r.status) {
            let Some(loc) = r.location.as_deref().filter(|l| !l.trim().is_empty()) else {
                return Err(format!("{} sent a redirect with nowhere to go", req.url.host));
            };
            let next = req.url.join(loc).map_err(|e| format!("the redirect from {} is not usable: {e}", req.url.host))?.without_fragment();
            let keep = matches!(r.status, 307 | 308);
            req = Request {
                referer: req.url.to_string(),
                method: if keep { req.method.clone() } else { "GET".into() },
                body: if keep { req.body.clone() } else { Vec::new() },
                content_type: if keep { req.content_type.clone() } else { String::new() },
                cookie: String::new(),
                priority: req.priority,
                if_none_match: String::new(),
                range: String::new(),
                if_modified_since: String::new(),
                extra: ExtraHeaders::default(),
                url: next,
            };
            continue;
        }
        return Ok((req, r));
    }
    Err("too many redirects".into())
}

/// Most of a page's source kept for viewing.
const SOURCE_MAX: usize = 1 << 20;

fn interpret(url: Url, r: Response, render: Renderer) -> Loaded {
    let ct = r.content_type.to_ascii_lowercase();
    let mime = ct.split(';').next().unwrap_or("").trim().to_string();
    let is_html = mime == "text/html" || mime == "application/xhtml+xml" || mime.is_empty() && looks_like_html(&r.body);
    let is_text = mime.starts_with("text/") || matches!(mime.as_str(), "application/json" | "application/xml" | "application/javascript" | "application/x-sh");
    if !is_html && !is_text {
        let info = crate::media::describe(&mime, &r.body);
        let bytes = r.content_length.map_or(r.body.len(), |l| l as usize);
        return Loaded::Unsupported { url, content_type: if mime.is_empty() { "unknown".into() } else { mime }, bytes, info };
    }
    let text = decode(&r.body, &ct, is_html);
    let (doc, notes) = if is_html { render(&url, &text) } else { (doc::from_text(&text), Vec::new()) };
    // what the server sent, for "view source" (cut at 1 MB of text)
    let source = if text.len() > SOURCE_MAX { text.chars().take(SOURCE_MAX).collect() } else { text };
    Loaded::Page { url, status: r.status, doc, notes, source }
}

fn looks_like_html(b: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&b[..b.len().min(512)]).to_ascii_lowercase();
    head.contains("<html") || head.contains("<!doctype html") || head.contains("<body") || head.contains("<p>") || head.contains("<h1")
}

/// Text from bytes: UTF-8 unless the header or a `<meta>` says Latin-1 / Windows-1252.
pub fn decode(body: &[u8], content_type: &str, html: bool) -> String {
    let declared = charset_of(content_type).or_else(|| html.then(|| meta_charset(&body[..body.len().min(2048)])).flatten());
    match declared.as_deref() {
        Some("iso-8859-1" | "latin1" | "windows-1252" | "cp1252" | "us-ascii") => body.iter().map(|&b| cp1252(b)).collect(),
        _ => String::from_utf8_lossy(body).into_owned(),
    }
}

fn charset_of(ct: &str) -> Option<String> {
    let i = ct.to_ascii_lowercase().find("charset=")?;
    Some(ct[i + 8..].split(';').next()?.trim().trim_matches('"').to_ascii_lowercase())
}

fn meta_charset(head: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(head).to_ascii_lowercase();
    let i = s.find("charset=")?;
    let v: String = s[i + 8..].trim_start_matches(['"', '\'']).chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    (!v.is_empty()).then_some(v)
}

fn cp1252(b: u8) -> char {
    match b {
        0x80 => '€',
        0x82 => '‚',
        0x85 => '…',
        0x91 => '‘',
        0x92 => '’',
        0x93 => '“',
        0x94 => '”',
        0x95 => '•',
        0x96 => '–',
        0x97 => '—',
        0x99 => '™',
        b => b as char,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Block;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct Fake {
        pages: HashMap<String, Response>,
        asked: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(pages: Vec<(&str, Response)>) -> Fake {
            Fake { pages: pages.into_iter().map(|(u, r)| (u.to_string(), r)).collect(), asked: Mutex::new(vec![]) }
        }
    }

    impl Fetch for Fake {
        fn get(&self, url: &Url) -> Result<Response, String> {
            self.asked.lock().unwrap().push(url.to_string());
            self.pages.get(&url.to_string()).cloned().ok_or_else(|| format!("no route to {}", url.host))
        }
    }

    fn html(body: &str) -> Response {
        Response { status: 200, content_type: "text/html; charset=utf-8".into(), body: body.as_bytes().to_vec(), ..Response::default() }
    }

    fn redirect(to: &str) -> Response {
        Response { status: 302, location: Some(to.into()), ..Response::default() }
    }

    fn any(_: &str) -> bool {
        true
    }

    #[test]
    fn a_stub_page_that_refreshes_elsewhere_is_followed_even_inside_noscript() {
        let stub = "<html><head></head><body><script>window.parent.location.replace('https://b.org/x');</script><noscript><META http-equiv='refresh' content=\"0;URL=https://b.org/Real_Page\"></noscript></body></html>";
        let f = Fake::new(vec![("https://a.org/l/", html(stub)), ("https://b.org/Real_Page", html("<title>Real</title><p>arrived</p>"))]);
        let Loaded::Page { url, doc, .. } = load(&f, &Url::parse("https://a.org/l/").unwrap(), &any).unwrap() else { panic!() };
        assert_eq!((url.to_string().as_str(), doc.title.as_str()), ("https://b.org/Real_Page", "Real"));
        // relative targets, bare numbers, long delays and loops
        let rel = html("<meta http-equiv=refresh content='1; url=/next'>");
        let f = Fake::new(vec![("https://a.org/a", rel), ("https://a.org/next", html("<p>n</p>"))]);
        let Loaded::Page { url, .. } = load(&f, &Url::parse("https://a.org/a").unwrap(), &any).unwrap() else { panic!() };
        assert_eq!(url.to_string(), "https://a.org/next");
        let slow = html("<meta http-equiv=refresh content='30; url=/next'><p>stay</p>");
        let f = Fake::new(vec![("https://a.org/a", slow)]);
        let Loaded::Page { url, .. } = load(&f, &Url::parse("https://a.org/a").unwrap(), &any).unwrap() else { panic!() };
        assert_eq!(url.to_string(), "https://a.org/a", "a slow refresh is a page, not a redirect");
        let looping = html("<meta http-equiv=refresh content='0; url=/b'>");
        let looping2 = html("<meta http-equiv=refresh content='0; url=/a'>");
        let f = Fake::new(vec![("https://a.org/a", looping), ("https://a.org/b", looping2)]);
        assert!(load(&f, &Url::parse("https://a.org/a").unwrap(), &any).is_ok(), "loops end after a few hops");
        assert!(f.asked.lock().unwrap().len() <= 8);
        // every hop obeys the host policy
        let f = Fake::new(vec![("https://a.org/a", html("<meta http-equiv=refresh content='0; url=https://evil.net/'>")), ("https://evil.net/", html("<p>x</p>"))]);
        let only_a = |h: &str| h == "a.org";
        assert!(load(&f, &Url::parse("https://a.org/a").unwrap(), &only_a).is_err());
        assert!(!f.asked.lock().unwrap().iter().any(|u| u.contains("evil")));
    }

    #[test]
    fn a_page_loads_and_keeps_the_fragment() {
        let f = Fake::new(vec![("https://a.org/x", html("<title>T</title><p>hi</p>"))]);
        let Loaded::Page { url, status, doc, .. } = load(&f, &Url::parse("https://a.org/x#part").unwrap(), &any).unwrap() else { panic!() };
        assert_eq!((url.to_string().as_str(), status, doc.title.as_str()), ("https://a.org/x#part", 200, "T"));
        assert_eq!(f.asked.lock().unwrap().clone(), vec!["https://a.org/x"], "the fragment is not sent");
    }

    #[test]
    fn redirects_are_followed_and_the_final_address_is_reported() {
        let f = Fake::new(vec![("http://a.org/", redirect("https://a.org/home")), ("https://a.org/home", redirect("/real")), ("https://a.org/real", html("<p>done</p>"))]);
        let Loaded::Page { url, .. } = load(&f, &Url::parse("http://a.org/").unwrap(), &any).unwrap() else { panic!() };
        assert_eq!(url.to_string(), "https://a.org/real");
    }

    #[test]
    fn redirect_loops_and_dead_ends_stop() {
        let f = Fake::new(vec![("https://a.org/", redirect("/b")), ("https://a.org/b", redirect("/"))]);
        assert_eq!(load(&f, &Url::parse("https://a.org/").unwrap(), &any).unwrap_err(), "too many redirects");
        let f = Fake::new(vec![("https://a.org/", Response { status: 301, ..Response::default() })]);
        assert!(load(&f, &Url::parse("https://a.org/").unwrap(), &any).unwrap_err().contains("nowhere to go"));
        let f = Fake::new(vec![("https://a.org/", redirect("javascript:alert(1)"))]);
        assert!(load(&f, &Url::parse("https://a.org/").unwrap(), &any).unwrap_err().contains("not usable"));
    }

    #[test]
    fn the_host_policy_is_checked_on_every_hop() {
        let f = Fake::new(vec![("https://ok.org/", redirect("https://evil.net/")), ("https://evil.net/", html("<p>x</p>"))]);
        let only_ok = |h: &str| h == "ok.org";
        let e = load(&f, &Url::parse("https://ok.org/").unwrap(), &only_ok).unwrap_err();
        assert!(e.contains("may not open pages from evil.net"), "{e}");
        assert_eq!(f.asked.lock().unwrap().clone(), vec!["https://ok.org/"], "the forbidden host was never contacted");
        assert!(load(&f, &Url::parse("https://evil.net/").unwrap(), &only_ok).is_err());
    }

    #[test]
    fn text_images_and_unknown_types() {
        let f = Fake::new(vec![
            ("https://a.org/t", Response { status: 200, content_type: "text/plain".into(), body: b"a\nb".to_vec(), ..Response::default() }),
            ("https://a.org/i", Response { status: 200, content_type: "image/png".into(), body: vec![0; 10], ..Response::default() }),
            ("https://a.org/n", Response { status: 200, content_type: String::new(), body: b"<html><body><p>no type</p></body></html>".to_vec(), ..Response::default() }),
            ("https://a.org/e", Response { status: 404, content_type: "text/html".into(), body: b"<h1>Not found</h1>".to_vec(), ..Response::default() }),
        ]);
        let get = |p: &str| load(&f, &Url::parse(&format!("https://a.org/{p}")).unwrap(), &any).unwrap();
        assert!(matches!(get("t"), Loaded::Page { doc, .. } if doc.blocks == vec![Block::Pre("a\nb".into())]));
        assert!(matches!(get("i"), Loaded::Unsupported { content_type, bytes: 10, .. } if content_type == "image/png"));
        assert!(matches!(get("n"), Loaded::Page { doc, .. } if doc.blocks.len() == 1));
        assert!(matches!(get("e"), Loaded::Page { status: 404, doc, .. } if doc.blocks.len() == 1), "error pages are shown, with their status");
    }

    #[test]
    fn charsets() {
        assert_eq!(decode("héllo".as_bytes(), "text/html; charset=UTF-8", true), "héllo");
        assert_eq!(decode(&[b'c', b'a', b'f', 0xE9, 0x93, b'x', 0x94], "text/html; charset=iso-8859-1", true), "café“x”");
        assert_eq!(decode(b"<meta charset=\"windows-1252\"><p>\xe9", "text/html", true), "<meta charset=\"windows-1252\"><p>é");
        assert_eq!(decode(&[0xff, b'a'], "text/plain", false), "\u{fffd}a", "bad UTF-8 is replaced, not fatal");
    }

    #[test]
    fn no_network_says_so() {
        assert!(load(&NoNetwork, &Url::parse("https://a.org/").unwrap(), &any).unwrap_err().contains("no web access"));
    }

    /// A server that answers by script and remembers what it was asked.
    struct Scripted {
        answers: Mutex<Vec<Response>>,
        seen: Mutex<Vec<Request>>,
    }

    impl Fetch for Scripted {
        fn get(&self, url: &Url) -> Result<Response, String> {
            self.send(&Request::get(url))
        }
        fn send(&self, req: &Request) -> Result<Response, String> {
            self.seen.lock().unwrap().push(req.clone());
            let mut a = self.answers.lock().unwrap();
            if a.is_empty() { Err("no more answers".into()) } else { Ok(a.remove(0)) }
        }
    }

    fn scripted(answers: Vec<Response>) -> Scripted {
        Scripted { answers: Mutex::new(answers), seen: Mutex::new(vec![]) }
    }

    fn redirect_to(code: u16, to: &str, cookies: &[&str]) -> Response {
        Response { status: code, location: Some(to.into()), set_cookies: cookies.iter().map(|c| c.to_string()).collect(), ..Response::default() }
    }

    fn page_with(cookies: &[&str]) -> Response {
        Response { status: 200, content_type: "text/html".into(), body: b"<p>ok</p>".to_vec(), set_cookies: cookies.iter().map(|c| c.to_string()).collect(), ..Response::default() }
    }

    #[test]
    fn cookies_are_kept_between_requests_and_carried_along_redirects() {
        let f = scripted(vec![redirect_to(302, "/second", &["sid=abc; Path=/", "t=1; Max-Age=60"]), page_with(&["later=2"]), page_with(&[])]);
        let store = Mutex::new(CookieStore::new());
        load_request(&f, Request::get(&Url::parse("https://a.org/first").unwrap()), &any, &plain, Some(Jar { store: &store, now: 100 })).unwrap();
        load_request(&f, Request::get(&Url::parse("https://a.org/third").unwrap()), &any, &plain, Some(Jar { store: &store, now: 100 })).unwrap();
        let seen = f.seen.lock().unwrap();
        assert_eq!(seen[0].cookie, "", "nothing to send yet");
        assert_eq!(seen[1].cookie, "sid=abc; t=1", "the cookie set by the redirect went with the next hop");
        assert_eq!(seen[2].cookie, "sid=abc; t=1; later=2", "and with the next page load");
        assert_eq!(seen[1].referer, "https://a.org/first");
    }

    #[test]
    fn a_form_post_becomes_a_get_after_302_303_and_stays_a_post_after_307() {
        for (code, method, keeps_body) in [(302, "GET", false), (303, "GET", false), (301, "GET", false), (307, "POST", true), (308, "POST", true)] {
            let f = scripted(vec![redirect_to(code, "/done", &[]), page_with(&[])]);
            let post = Request { url: Url::parse("https://a.org/submit").unwrap(), method: "POST".into(), body: b"q=1".to_vec(), content_type: "application/x-www-form-urlencoded".into(), ..Request::default() };
            load_request(&f, post, &any, &plain, None).unwrap();
            let seen = f.seen.lock().unwrap();
            assert_eq!((seen[1].method.as_str(), !seen[1].body.is_empty(), !seen[1].content_type.is_empty()), (method, keeps_body, keeps_body), "after a {code}");
            assert_eq!(seen[1].url.path, "/done");
        }
    }

    #[test]
    fn a_get_only_transport_refuses_a_post_with_a_clear_message() {
        let f = Fake::new(vec![("https://a.org/", html("<p>x</p>"))]);
        let post = Request { url: Url::parse("https://a.org/").unwrap(), method: "POST".into(), ..Request::default() };
        assert!(load_request(&f, post, &any, &plain, None).unwrap_err().contains("only read pages"));
    }

    #[test]
    fn the_policy_and_cookies_work_together_a_cookie_never_reaches_a_forbidden_host() {
        let f = scripted(vec![redirect_to(302, "https://evil.net/", &["sid=1"]), page_with(&[])]);
        let store = Mutex::new(CookieStore::new());
        let only_a = |h: &str| h == "a.org";
        let e = load_request(&f, Request::get(&Url::parse("https://a.org/").unwrap()), &only_a, &plain, Some(Jar { store: &store, now: 0 })).unwrap_err();
        assert!(e.contains("may not open pages from evil.net"));
        assert_eq!(f.seen.lock().unwrap().len(), 1, "evil.net was never contacted");
        assert_eq!(store.lock().unwrap().header_for(&Url::parse("https://evil.net/").unwrap(), 0), "", "the cookie belongs to a.org, not to where it redirected");
    }
}
