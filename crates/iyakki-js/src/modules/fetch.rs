//! `fetch()` and `XMLHttpRequest`, for reading (GET and HEAD). Every request, and every redirect it follows, is checked
//! against the host policy of the account that opened the page; the number and total size of requests per page is capped.

use std::any::Any;
use std::sync::Arc;

use std::sync::Mutex;

use iyakki_web::{CookieStore, Fetch, Request, Url};

use crate::engine::WebModule;

pub struct FetchModule {
    base: Url,
    web: Arc<dyn Fetch>,
    allow: Box<dyn Fn(&str) -> bool + Send>,
    requests: u32,
    bytes: usize,
    /// The browser's cookies (shared with its own requests) and "now", for requests to the page's own host.
    jar: Option<(Arc<Mutex<CookieStore>>, i64)>,
    /// Every address requested, for the browser's developer view.
    pub log: Vec<String>,
}

const MAX_REQUESTS: u32 = 40;
const MAX_BYTES: usize = 8 << 20;
const MAX_REDIRECTS: usize = 5;

impl FetchModule {
    /// `base` is the page's own address (relative requests resolve against it).
    pub fn new(base: Url, web: Arc<dyn Fetch>, allow: Box<dyn Fn(&str) -> bool + Send>) -> FetchModule {
        FetchModule { base, web, allow, requests: 0, bytes: 0, jar: None, log: Vec::new() }
    }

    /// Send cookies with requests to the page's own host, and keep the ones the answers set.
    pub fn with_cookies(mut self, jar: Arc<Mutex<CookieStore>>, now: i64) -> FetchModule {
        self.jar = Some((jar, now));
        self
    }

    fn get(&mut self, target: &str, method: &str, body: &str, content_type: &str) -> Result<serde_json::Value, String> {
        if self.requests >= MAX_REQUESTS {
            return Err("this page has made too many requests".into());
        }
        self.requests += 1;
        let head = method == "HEAD";
        let mut req = Request { url: self.base.join(target)?.without_fragment(), method: method.to_string(), body: body.as_bytes().to_vec(), content_type: content_type.to_string(), cookie: String::new(), referer: self.base.to_string() };
        for _ in 0..=MAX_REDIRECTS {
            let url = req.url.clone();
            if !(self.allow)(&url.host) {
                return Err(format!("your account may not open pages from {}", url.host));
            }
            if let Some((jar, now)) = &self.jar {
                req.cookie = if url.host == self.base.host { jar.lock().map(|j| j.header_for(&url, *now)).unwrap_or_default() } else { String::new() };
            }
            self.log.push(url.to_string());
            let r = self.web.send(&req)?;
            if let Some((jar, now)) = &self.jar {
                if url.host == self.base.host {
                    if let Ok(mut j) = jar.lock() {
                        for c in &r.set_cookies {
                            j.store_from_header(&url, c, *now);
                        }
                    }
                }
            }
            if (300..400).contains(&r.status) {
                let loc = r.location.as_deref().filter(|l| !l.trim().is_empty()).ok_or("a redirect with nowhere to go")?;
                let keep = matches!(r.status, 307 | 308);
                req = Request { url: url.join(loc)?.without_fragment(), method: if keep { req.method.clone() } else { "GET".into() }, body: if keep { req.body.clone() } else { Vec::new() }, content_type: if keep { req.content_type.clone() } else { String::new() }, cookie: String::new(), referer: url.to_string() };
                continue;
            }
            self.bytes += r.body.len();
            if self.bytes > MAX_BYTES {
                return Err("this page has downloaded too much".into());
            }
            let html = r.content_type.to_ascii_lowercase().contains("html");
            let body = if head { String::new() } else { iyakki_web::fetch::decode(&r.body, &r.content_type, html) };
            return Ok(serde_json::json!({
                "status": r.status,
                "status_text": status_text(r.status),
                "url": req.url.to_string(),
                "headers": { "content-type": r.content_type },
                "body": body,
            }));
        }
        Err("too many redirects".into())
    }
}

fn status_text(s: u16) -> &'static str {
    match s {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "",
    }
}

impl WebModule for FetchModule {
    fn name(&self) -> &'static str {
        "fetch"
    }

    fn prelude(&self) -> &'static str {
        r#"
(function (g) {
  'use strict';
  class Headers {
    constructor(o) { this._m = new Map(Object.entries(o || {}).map(([k, v]) => [String(k).toLowerCase(), String(v)])); }
    get(k) { const v = this._m.get(String(k).toLowerCase()); return v === undefined ? null : v; }
    has(k) { return this._m.has(String(k).toLowerCase()); }
    set(k, v) { this._m.set(String(k).toLowerCase(), String(v)); }
    append(k, v) { this.set(k, v); }
    delete(k) { this._m.delete(String(k).toLowerCase()); }
    forEach(f, t) { this._m.forEach((v, k) => f.call(t, v, k, this)); }
    entries() { return this._m.entries(); }
    keys() { return this._m.keys(); }
    values() { return this._m.values(); }
    [Symbol.iterator]() { return this._m.entries(); }
  }
  class Response {
    constructor(body, init) {
      init = init || {};
      this._body = body == null ? '' : String(body);
      this.status = init.status === undefined ? 200 : init.status;
      this.ok = this.status >= 200 && this.status < 300;
      this.statusText = init.statusText || '';
      this.url = init.url || '';
      this.headers = new Headers(init.headers);
      this.type = 'basic'; this.redirected = false; this.bodyUsed = false;
    }
    text() { this.bodyUsed = true; return Promise.resolve(this._body); }
    json() { return this.text().then((t) => JSON.parse(t)); }
    arrayBuffer() { return this.text().then((t) => (typeof TextEncoder === 'function' ? new TextEncoder().encode(t).buffer : new ArrayBuffer(0))); }
    clone() { return new Response(this._body, { status: this.status, statusText: this.statusText, url: this.url, headers: Object.fromEntries(this.headers._m) }); }
    static error() { return new Response('', { status: 0 }); }
  }
  class Request {
    constructor(input, init) {
      init = init || {};
      const from = input instanceof Request ? input : null;
      this.url = from ? from.url : String(input && input.href !== undefined ? input.href : input);
      this.method = String(init.method || (from && from.method) || 'GET').toUpperCase();
      this.headers = new Headers(init.headers !== undefined ? (init.headers && typeof init.headers.forEach === 'function' && !(init.headers instanceof Headers) ? Object.fromEntries(init.headers) : init.headers instanceof Headers ? Object.fromEntries(init.headers._m) : init.headers) : from ? Object.fromEntries(from.headers._m) : {});
      this.body = init.body !== undefined ? init.body : from ? from.body : null;
      this.signal = init.signal || null; this.credentials = init.credentials || 'same-origin'; this.mode = init.mode || 'cors'; this.cache = init.cache || 'default';
      this.redirect = init.redirect || 'follow'; this.referrer = init.referrer || ''; this.bodyUsed = false;
    }
    clone() { return new Request(this); }
    text() { this.bodyUsed = true; return Promise.resolve(this.body == null ? '' : String(this.body)); }
    json() { return this.text().then((t) => JSON.parse(t)); }
  }
  const request = (url, method, body, content_type) => JSON.parse(__host('fetch', 'get', JSON.stringify({ url: String(url), method, body, content_type })));
  const bodyText = (b) => (b === undefined || b === null ? '' : typeof b === 'string' ? b : (typeof URLSearchParams === 'function' && b instanceof URLSearchParams) ? b.toString() : typeof b === 'object' && b.toString !== Object.prototype.toString ? String(b) : JSON.stringify(b));
  const typeOf = (init) => { const h = init && init.headers; if (!h) return ''; if (typeof h.get === 'function') return h.get('content-type') || ''; for (const k of Object.keys(h)) if (k.toLowerCase() === 'content-type') return String(h[k]); return ''; };
  g.fetch = function (input, init) {
    return new Promise((resolve, reject) => {
      try {
        const url = typeof input === 'string' ? input : (input && input.url) || String(input);
        const method = ((init && init.method) || (input && input.method) || 'GET').toUpperCase();
        if (method !== 'GET' && method !== 'HEAD' && method !== 'POST') throw new TypeError('only GET, HEAD and POST requests are supported');
        const rawBody = init && init.body !== undefined ? init.body : (input instanceof Request ? input.body : undefined);
        const body = method === 'POST' ? bodyText(rawBody) : '';
        const ct = method === 'POST' ? typeOf(init) || (input instanceof Request ? typeOf(input) : '') || (typeof rawBody === 'string' && /^\s*[{\[]/.test(rawBody) ? 'application/json' : 'application/x-www-form-urlencoded;charset=UTF-8') : '';
        const r = request(url, method, body, ct);
        resolve(new Response(r.body, { status: r.status, statusText: r.status_text, url: r.url, headers: r.headers }));
      } catch (e) { reject(new TypeError('Failed to fetch: ' + ((e && e.message) || e))); }
    });
  };
  class XMLHttpRequest {
    constructor() { this.readyState = 0; this.status = 0; this.statusText = ''; this.responseText = ''; this.response = ''; this.responseURL = ''; this.responseType = ''; this.onreadystatechange = null; this.onload = null; this.onerror = null; this.onloadend = null; this._h = {}; this._l = {}; }
    open(method, url) { this._method = String(method).toUpperCase(); this._url = String(url); this.readyState = 1; this._fire('readystatechange'); }
    setRequestHeader(k, v) { if (String(k).toLowerCase() === 'content-type') this._ct = String(v); }
    getResponseHeader(k) { return this._h[String(k).toLowerCase()] === undefined ? null : this._h[String(k).toLowerCase()]; }
    getAllResponseHeaders() { return Object.entries(this._h).map(([k, v]) => k + ': ' + v).join('\r\n'); }
    addEventListener(t, f) { (this._l[t] = this._l[t] || []).push(f); }
    removeEventListener(t, f) { this._l[t] = (this._l[t] || []).filter((x) => x !== f); }
    abort() { this._aborted = true; }
    overrideMimeType() {}
    _fire(t) { const h = this['on' + t]; try { if (typeof h === 'function') h.call(this, { type: t, target: this }); for (const f of this._l[t] || []) f.call(this, { type: t, target: this }); } catch (e) {} }
    send(body) {
      this._body = body;
      Promise.resolve().then(() => {
        if (this._aborted) return;
        try {
          if (this._method !== 'GET' && this._method !== 'HEAD' && this._method !== 'POST') throw new Error('only GET, HEAD and POST requests are supported');
          const r = request(this._url, this._method, this._method === 'POST' ? bodyText(this._body) : '', this._ct || 'application/x-www-form-urlencoded;charset=UTF-8');
          this.status = r.status; this.statusText = r.status_text; this.responseURL = r.url; this._h = r.headers;
          this.responseText = r.body;
          this.response = this.responseType === 'json' ? (() => { try { return JSON.parse(r.body); } catch (e) { return null; } })() : r.body;
          this.readyState = 4;
          this._fire('readystatechange'); this._fire('load'); this._fire('loadend');
        } catch (e) { this.readyState = 4; this._fire('readystatechange'); this._fire('error'); this._fire('loadend'); }
      });
    }
  }
  g.Headers = Headers; g.Request = Request; g.Response = Response; g.XMLHttpRequest = XMLHttpRequest;
})(globalThis);
"#
    }

    fn call(&mut self, op: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        match op {
            "get" => {
                let url = args["url"].as_str().ok_or("no address")?.to_string();
                let method = args["method"].as_str().unwrap_or("GET").to_ascii_uppercase();
                if !matches!(method.as_str(), "GET" | "HEAD" | "POST") {
                    return Err("only GET, HEAD and POST requests are supported".into());
                }
                let body = args["body"].as_str().unwrap_or("").to_string();
                if body.len() > 1 << 20 {
                    return Err("the request body is too large".into());
                }
                let ct = args["content_type"].as_str().unwrap_or("").to_string();
                self.get(&url, &method, &body, &ct)
            }
            other => Err(format!("fetch has no operation {other}")),
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{ConsoleModule, TimersModule};
    use crate::{modules::run_timers, JsEngine, Limits};
    use iyakki_web::Response;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct Site(Mutex<HashMap<String, Response>>);
    impl Fetch for Site {
        fn get(&self, url: &Url) -> Result<Response, String> {
            self.0.lock().unwrap().get(&url.to_string()).cloned().ok_or_else(|| "connection refused".to_string())
        }
    }

    fn page(pages: Vec<(&str, Response)>, allow: impl Fn(&str) -> bool + Send + 'static) -> JsEngine {
        let web: Arc<dyn Fetch> = Arc::new(Site(Mutex::new(pages.into_iter().map(|(u, r)| (u.to_string(), r)).collect())));
        JsEngine::new(Limits::default(), vec![Box::new(ConsoleModule::new()), Box::new(TimersModule), Box::new(FetchModule::new(Url::parse("https://a.org/app/index.html").unwrap(), web, Box::new(allow)))]).unwrap()
    }

    fn json(body: &str) -> Response {
        Response { status: 200, content_type: "application/json".into(), body: body.as_bytes().to_vec(), ..Response::default() }
    }

    fn logged(e: &mut JsEngine) -> Vec<String> {
        e.module::<ConsoleModule>("console").unwrap().lines.iter().map(|l| l.text.clone()).collect()
    }

    #[test]
    fn fetch_returns_a_promise_of_a_response_with_text_and_json() {
        let mut e = page(vec![("https://a.org/app/data.json", json("{\"n\": 42}")), ("https://a.org/other", Response { status: 404, content_type: "text/plain".into(), body: b"gone".to_vec(), ..Response::default() })], |_| true);
        e.eval("t", "fetch('data.json').then(r => r.json()).then(d => console.log('n=' + d.n)); fetch('/other').then(r => { console.log(r.status + ' ' + r.ok + ' ' + r.statusText + ' ' + r.headers.get('Content-Type')); return r.text(); }).then(t => console.log(t))").unwrap();
        e.run_jobs().unwrap();
        let mut got = logged(&mut e);
        got.sort();
        assert_eq!(got, vec!["404 false Not Found text/plain", "gone", "n=42"], "every promise settled (their order depends on how many steps each chain takes)");
        let m = e.module::<FetchModule>("fetch").unwrap();
        assert_eq!(m.log, vec!["https://a.org/app/data.json", "https://a.org/other"], "relative addresses resolve against the page");
    }

    #[test]
    fn request_objects_exist_can_be_cloned_and_are_accepted_by_fetch() {
        let mut e = page(vec![("https://a.org/data.json", json("{\"n\": 7}"))], |_| true);
        e.eval("t", "const r = new Request('/data.json', { method: 'get', headers: { 'X-A': '1' } }); const c = r.clone(); console.log(typeof Request + ' ' + r.method + ' ' + c.headers.get('x-a') + ' ' + (r instanceof Request)); fetch(r).then(x => x.json()).then(d => console.log('n=' + d.n)); fetch(new Request(r, { method: 'GET' })).then(x => console.log(x.status))").unwrap();
        e.run_jobs().unwrap();
        let mut l = logged(&mut e);
        assert_eq!(l.remove(0), "function GET 1 true");
        l.sort();
        assert_eq!(l, vec!["200", "n=7"]);
    }

    #[test]
    fn failures_and_unsupported_methods_reject_like_a_browser() {
        let mut e = page(vec![], |_| true);
        e.eval("t", "fetch('https://nowhere.org/').catch(err => console.log(err.name + ': ' + err.message)); fetch('/x', { method: 'POST', body: 'a' }).catch(err => console.log('post: ' + err.message)); fetch('javascript:alert(1)').catch(err => console.log('js: ' + err.message))").unwrap();
        e.run_jobs().unwrap();
        let l = logged(&mut e);
        assert!(l[0].starts_with("TypeError: Failed to fetch") && l[0].contains("connection refused"), "{l:?}");
        assert!(l[1].contains("only read pages"), "a GET-only connection refuses a POST: {l:?}");
        assert!(l[2].contains("cannot be opened") || l[2].contains("only http"), "{l:?}");
    }

    #[test]
    fn the_host_policy_covers_requests_and_their_redirects() {
        let redirect = Response { status: 302, location: Some("https://evil.net/steal".into()), ..Response::default() };
        let mut e = page(vec![("https://a.org/go", redirect), ("https://evil.net/steal", json("{}"))], |h| h == "a.org");
        e.eval("t", "fetch('/go').catch(err => console.log(err.message)); fetch('https://evil.net/direct').catch(err => console.log(err.message))").unwrap();
        e.run_jobs().unwrap();
        let l = logged(&mut e);
        assert!(l[0].contains("may not open pages from evil.net") && l[1].contains("may not open pages from evil.net"), "{l:?}");
        assert_eq!(e.module::<FetchModule>("fetch").unwrap().log, vec!["https://a.org/go"], "the forbidden host was never contacted");
    }

    #[test]
    fn a_page_cannot_make_endless_requests() {
        let mut e = page(vec![("https://a.org/ping", json("1"))], |_| true);
        let r = e.eval("t", "let ok = 0, no = 0; for (let i = 0; i < 60; i++) fetch('/ping').then(() => ok++, () => no++); 0").unwrap();
        assert_eq!(r, "0");
        e.run_jobs().unwrap();
        assert_eq!(e.eval("t", "ok + ',' + no").unwrap(), "40,20");
    }

    #[test]
    fn xml_http_request_works_for_simple_reads() {
        let mut e = page(vec![("https://a.org/app/x.txt", Response { status: 200, content_type: "text/plain".into(), body: b"hello".to_vec(), ..Response::default() })], |_| true);
        e.eval("t", "const x = new XMLHttpRequest(); x.open('GET', 'x.txt'); x.onload = () => console.log(x.status + ':' + x.responseText + ':' + x.getResponseHeader('content-type') + ':' + x.readyState); x.send(); const y = new XMLHttpRequest(); y.open('GET', 'https://down.example/'); y.onerror = () => console.log('error'); y.send()").unwrap();
        run_timers(&mut e, 10, 10).unwrap();
        assert_eq!(logged(&mut e), vec!["200:hello:text/plain:4", "error"]);
    }

    struct Recorder(Mutex<Vec<iyakki_web::Request>>, Mutex<Vec<Response>>);
    impl Fetch for Recorder {
        fn get(&self, url: &Url) -> Result<Response, String> {
            self.send(&iyakki_web::Request::get(url))
        }
        fn send(&self, req: &iyakki_web::Request) -> Result<Response, String> {
            self.0.lock().unwrap().push(req.clone());
            let mut a = self.1.lock().unwrap();
            if a.is_empty() { Ok(Response { status: 200, content_type: "text/plain".into(), body: b"ok".to_vec(), ..Response::default() }) } else { Ok(a.remove(0)) }
        }
    }

    #[test]
    fn posts_carry_their_body_and_type_and_cookies_flow_both_ways_for_the_pages_own_host() {
        let rec = Arc::new(Recorder(Mutex::new(vec![]), Mutex::new(vec![Response { status: 200, content_type: "application/json".into(), body: b"{}".to_vec(), set_cookies: vec!["token=xyz; Path=/".into(), "hid=1; HttpOnly".into()], ..Response::default() }])));
        let jar = Arc::new(Mutex::new(CookieStore::new()));
        jar.lock().unwrap().store_from_header(&Url::parse("https://a.org/").unwrap(), "sid=1", 100);
        jar.lock().unwrap().store_from_header(&Url::parse("https://other.org/").unwrap(), "o=2", 100);
        let web: Arc<dyn Fetch> = rec.clone();
        let mut e = JsEngine::new(Limits::default(), vec![Box::new(ConsoleModule::new()), Box::new(crate::modules::UrlModule), Box::new(FetchModule::new(Url::parse("https://a.org/app/").unwrap(), web, Box::new(|_| true)).with_cookies(jar.clone(), 100))]).unwrap();
        e.eval("t", "fetch('/api', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ q: 'rust' }) }).then(r => r.json()); fetch('/form', { method: 'POST', body: new URLSearchParams({ a: '1', b: 'x y' }) }); fetch('https://other.org/x');").unwrap();
        e.run_jobs().unwrap();
        let seen = rec.0.lock().unwrap();
        assert_eq!((seen[0].method.as_str(), String::from_utf8_lossy(&seen[0].body).to_string(), seen[0].content_type.as_str(), seen[0].cookie.as_str(), seen[0].referer.as_str()), ("POST", "{\"q\":\"rust\"}".to_string(), "application/json", "sid=1", "https://a.org/app/"));
        assert_eq!((String::from_utf8_lossy(&seen[1].body).to_string(), seen[1].content_type.as_str()), ("a=1&b=x+y".to_string(), "application/x-www-form-urlencoded;charset=UTF-8"), "a string body defaults to a form post");
        assert_eq!(seen[2].cookie, "", "cookies are not sent to other hosts");
        assert!(jar.lock().unwrap().header_for(&Url::parse("https://a.org/").unwrap(), 100).contains("token=xyz"), "the answer's Set-Cookie went into the browser's jar");
        assert!(!jar.lock().unwrap().script_view(&Url::parse("https://a.org/").unwrap(), 100).contains("hid"), "HttpOnly stays hidden from scripts");
    }
}
