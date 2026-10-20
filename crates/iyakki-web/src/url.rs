//! Web addresses.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub https: bool,
    /// Lowercase.
    pub host: String,
    pub port: Option<u16>,
    /// Always starts with `/`.
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
}

const MAX_LEN: usize = 4096;

impl Default for Url {
    /// An address that is never requested; only here so that structs holding one can derive `Default`.
    fn default() -> Url {
        Url { https: true, host: "localhost".into(), port: None, path: "/".into(), query: None, fragment: None }
    }
}

impl Url {
    /// Parse an absolute `http://` or `https://` address. Anything else is refused, as are addresses with a user name
    /// or password in them (they are used to make one site look like another).
    pub fn parse(s: &str) -> Result<Url, String> {
        let s = s.trim();
        if s.len() > MAX_LEN {
            return Err("that address is too long".into());
        }
        if s.chars().any(|c| c.is_control()) {
            return Err("that address has control characters in it".into());
        }
        let (https, rest) = if let Some(r) = strip_prefix_ci(s, "https://") {
            (true, r)
        } else if let Some(r) = strip_prefix_ci(s, "http://") {
            (false, r)
        } else {
            return Err(format!("only http and https addresses can be opened ({})", s.split(':').next().filter(|p| p.len() < 12 && s.contains(':')).unwrap_or("no scheme")));
        };
        let (rest, fragment) = match rest.split_once('#') {
            Some((a, f)) => (a, Some(f.to_string())),
            None => (rest, None),
        };
        let (rest, query) = match rest.split_once('?') {
            Some((a, q)) => (a, Some(q.to_string())),
            None => (rest, None),
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        if authority.contains('@') {
            return Err("addresses with a user name or password in them are not opened".into());
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => match p.parse::<u16>() {
                Ok(p) => (h, Some(p)),
                Err(_) if p.is_empty() => (h, None),
                Err(_) => return Err("the port in that address is not a number".into()),
            },
            _ => (authority, None),
        };
        let host = host.to_ascii_lowercase();
        if host.is_empty() || host.len() > 253 || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '[' | ']' | ':')) {
            return Err("that is not a valid host name".into());
        }
        Ok(Url { https, host, port, path: normalize_path(path), query, fragment })
    }

    /// What a person typed into the address bar: `example.org/news` means `https://example.org/news`.
    pub fn from_input(s: &str) -> Result<Url, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("type a web address".into());
        }
        if s.contains("://") {
            return Url::parse(s);
        }
        // `javascript:...`, `mailto:...`: say they cannot be opened (but `localhost:8080` is a host and a port)
        if let Some((scheme, rest)) = s.split_once(':') {
            if !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphabetic()) && !rest.starts_with(|c: char| c.is_ascii_digit()) && !scheme.eq_ignore_ascii_case("localhost") {
                return Url::parse(s);
            }
        }
        if s.contains(char::is_whitespace) || !(s.contains('.') || s.starts_with("localhost")) {
            // not an address: words to search for
            return Ok(Url::search(s));
        }
        Url::parse(&format!("https://{s}"))
    }

    /// A web search for words typed into the address bar, on the default engine.
    pub fn search(words: &str) -> Url {
        SearchEngine::default().url(words)
    }

    /// `google.com/search?q=…` becomes the same search on the default engine, since Google's results page cannot be
    /// loaded without a full browser (it sends a JavaScript check page). Returns the replacement address.
    pub fn reroute_google_search(&self) -> Option<Url> {
        self.reroute_google_search_on(SearchEngine::default())
    }

    pub fn reroute_google_search_on(&self, engine: SearchEngine) -> Option<Url> {
        let h = self.host.strip_prefix("www.").unwrap_or(&self.host);
        let is_google = h == "google.com" || (h.starts_with("google.") && h.len() <= 12 && h[7..].split('.').all(|p| p.len() <= 3 && !p.is_empty()));
        if !is_google || self.path != "/search" {
            return None;
        }
        let q = self.query.as_deref()?.split('&').find_map(|kv| kv.strip_prefix("q="))?;
        (!q.is_empty()).then(|| engine.url_encoded(q))
    }

    /// A Bing result link (`bing.com/ck/a?...&u=a1<base64 of the address>`) names the real address inside it; its own page only
    /// redirects with a script. Returns the address it points to.
    pub fn unwrap_search_link(&self) -> Option<Url> {
        if !(self.host == "www.bing.com" || self.host == "bing.com") || self.path != "/ck/a" {
            return None;
        }
        let u = self.query.as_deref()?.split('&').find_map(|kv| kv.strip_prefix("u=a1"))?;
        let bytes = crate::image::base64_decode(u)?;
        let target = Url::parse(std::str::from_utf8(&bytes).ok()?).ok()?;
        (target.host != self.host).then_some(target)
    }

    /// Like [`Url::from_input`], but words that are not an address are searched for on `engine`.
    pub fn from_input_on(s: &str, engine: SearchEngine) -> Result<Url, String> {
        let u = Url::from_input(s)?;
        Ok(if u.host == SearchEngine::default().host() && u.path == SearchEngine::default().path() && engine != SearchEngine::default() { engine.url_encoded(u.query.as_deref().and_then(|q| q.strip_prefix("q=")).unwrap_or("")) } else { u })
    }

    /// Resolve a link found on a page whose address is `self`.
    pub fn join(&self, reference: &str) -> Result<Url, String> {
        let r = reference.trim();
        if r.is_empty() {
            return Ok(self.clone());
        }
        if r.contains("://") || r.starts_with("//") {
            return Url::parse(&if r.starts_with("//") { format!("{}:{r}", if self.https { "https" } else { "http" }) } else { r.to_string() });
        }
        let scheme_end = r.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')));
        if scheme_end.is_some_and(|i| r[i..].starts_with(':') && i > 0) {
            return Err(format!("links of the kind {} cannot be opened here", &r[..scheme_end.unwrap_or(0) + 1]));
        }
        let mut out = self.clone();
        out.fragment = None;
        if let Some(f) = r.strip_prefix('#') {
            out.fragment = Some(f.to_string());
            return Ok(out);
        }
        let (r, fragment) = match r.split_once('#') {
            Some((a, f)) => (a, Some(f.to_string())),
            None => (r, None),
        };
        let (r, query) = match r.split_once('?') {
            Some((a, q)) => (a, Some(q.to_string())),
            None => (r, None),
        };
        out.fragment = fragment;
        out.query = query;
        if !r.is_empty() {
            let path = if r.starts_with('/') {
                r.to_string()
            } else {
                let dir = &self.path[..self.path.rfind('/').map_or(0, |i| i + 1)];
                format!("{dir}{r}")
            };
            out.path = normalize_path(&path);
        }
        Ok(out)
    }

    /// The address without its `#fragment` (what is requested from the server).
    pub fn request_target(&self) -> String {
        format!("{}{}", self.path, self.query.as_ref().map_or(String::new(), |q| format!("?{q}")))
    }

    pub fn without_fragment(&self) -> Url {
        Url { fragment: None, ..self.clone() }
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let default = if self.https { 443 } else { 80 };
        write!(f, "{}://{}", if self.https { "https" } else { "http" }, self.host)?;
        if let Some(p) = self.port.filter(|p| *p != default) {
            write!(f, ":{p}")?;
        }
        write!(f, "{}", self.path)?;
        if let Some(q) = &self.query {
            write!(f, "?{q}")?;
        }
        if let Some(fr) = &self.fragment {
            write!(f, "#{fr}")?;
        }
        Ok(())
    }
}

/// Search pages that answer an ordinary request with results (checked live: Google now answers every request without
/// JavaScript with "enable JavaScript", and Startpage, Mojeek, Yandex and Ecosia show a picture check or refuse, so
/// those are not offered). Bing's plain results page works without scripts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SearchEngine {
    /// Brave Search: server-rendered results with direct links. It answers 429 to HTTP/1.1 clients (checked live), which is
    /// all this browser speaks, so it is only tried after the others.
    Brave,
    /// DuckDuckGo's plain-HTML page; it sometimes asks for a check and then shows nothing useful here.
    DuckDuckGo,
    /// Bing: server-rendered results; its result links are unwrapped to the real address ([`Url::unwrap_search_link`]).
    #[default]
    Bing,
    /// Wikipedia's own search, for looking something up rather than finding sites.
    Wikipedia,
}

impl SearchEngine {
    pub const ALL: [SearchEngine; 4] = [SearchEngine::Bing, SearchEngine::DuckDuckGo, SearchEngine::Brave, SearchEngine::Wikipedia];

    pub fn name(self) -> &'static str {
        match self {
            SearchEngine::Brave => "Brave Search",
            SearchEngine::DuckDuckGo => "DuckDuckGo",
            SearchEngine::Bing => "Bing",
            SearchEngine::Wikipedia => "Wikipedia",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            SearchEngine::Brave => "brave",
            SearchEngine::DuckDuckGo => "duckduckgo",
            SearchEngine::Bing => "bing",
            SearchEngine::Wikipedia => "wikipedia",
        }
    }

    pub fn from_id(s: &str) -> Option<SearchEngine> {
        SearchEngine::ALL.into_iter().find(|e| e.id() == s)
    }

    pub fn next(self) -> SearchEngine {
        SearchEngine::ALL[(SearchEngine::ALL.iter().position(|e| *e == self).unwrap_or(0) + 1) % SearchEngine::ALL.len()]
    }

    fn host(self) -> &'static str {
        match self {
            SearchEngine::Brave => "search.brave.com",
            SearchEngine::DuckDuckGo => "html.duckduckgo.com",
            SearchEngine::Bing => "www.bing.com",
            SearchEngine::Wikipedia => "en.wikipedia.org",
        }
    }

    fn path(self) -> &'static str {
        match self {
            SearchEngine::Brave => "/search",
            SearchEngine::DuckDuckGo => "/html/",
            SearchEngine::Bing => "/search",
            SearchEngine::Wikipedia => "/w/index.php",
        }
    }

    /// The name of the query field in the engine's address.
    fn param(self) -> &'static str {
        match self {
            SearchEngine::Wikipedia => "search",
            _ => "q",
        }
    }

    pub fn url(self, words: &str) -> Url {
        self.url_encoded(&form_urlencode(&[("q".to_string(), words.trim().to_string())])[2..])
    }

    /// The engine whose results page `url` is, and the search words in it (still form-encoded).
    pub fn of_url(url: &Url) -> Option<(SearchEngine, String)> {
        let e = SearchEngine::ALL.into_iter().find(|e| e.host() == url.host && e.path() == url.path)?;
        let q = url.query.as_deref()?.split('&').find_map(|kv| kv.strip_prefix(e.param()).and_then(|r| r.strip_prefix('=')))?;
        Some((e, q.to_string()))
    }

    /// The same words on the next engine in the list that is not in `tried`.
    pub fn fallback(tried: &[SearchEngine], words: &str) -> Option<(SearchEngine, Url)> {
        let e = SearchEngine::ALL.into_iter().find(|e| !tried.contains(e))?;
        Some((e, e.url_encoded(words)))
    }

    /// `q` is already form-encoded.
    fn url_encoded(self, q: &str) -> Url {
        Url { https: true, host: self.host().into(), port: None, path: self.path().into(), query: Some(format!("{}={q}", self.param())), fragment: None }
    }
}

/// `application/x-www-form-urlencoded`: what a form sends and what follows the `?` of a GET form.
pub fn form_urlencode(pairs: &[(String, String)]) -> String {
    fn enc(s: &str, out: &mut String) {
        for b in s.bytes() {
            match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => out.push(b as char),
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
    }
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        enc(k, &mut out);
        out.push('=');
        enc(v, &mut out);
    }
    out
}

fn strip_prefix_ci<'a>(s: &'a str, p: &str) -> Option<&'a str> {
    (s.len() >= p.len() && s.is_char_boundary(p.len()) && s[..p.len()].eq_ignore_ascii_case(p)).then(|| &s[p.len()..])
}

/// Remove `.` and `..` segments and doubled slashes (a trailing slash stays).
fn normalize_path(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let trailing = p.ends_with('/') || p.ends_with("/.") || p.ends_with("/..");
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    let mut s = format!("/{}", out.join("/"));
    if trailing && !s.ends_with('/') {
        s.push('/');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn parses_and_prints_addresses() {
        let a = u("HTTPS://Example.ORG:8443/a/b?x=1#top");
        assert_eq!((a.https, a.host.as_str(), a.port, a.path.as_str(), a.query.as_deref(), a.fragment.as_deref()), (true, "example.org", Some(8443), "/a/b", Some("x=1"), Some("top")));
        assert_eq!(a.to_string(), "https://example.org:8443/a/b?x=1#top");
        assert_eq!(u("http://example.org").to_string(), "http://example.org/");
        assert_eq!(u("https://example.org:443/").to_string(), "https://example.org/", "the default port is not printed");
        assert_eq!(a.request_target(), "/a/b?x=1");
        assert_eq!(a.without_fragment().to_string(), "https://example.org:8443/a/b?x=1");
    }

    #[test]
    fn refuses_what_a_reader_must_not_open() {
        for bad in ["javascript:alert(1)", "file:///etc/passwd", "ftp://example.org/", "data:text/html,hi", "mailto:a@b.c", "https://user:pw@example.org/", "https://exa mple.org/", "https://", "https://example.org:99999/", "https://example.org:abc/", "http://a\\u{0}b/", ""] {
            assert!(Url::parse(bad).is_err(), "{bad:?} must be refused");
        }
        assert!(Url::parse(&format!("https://example.org/{}", "a".repeat(5000))).is_err(), "very long addresses are refused");
    }

    #[test]
    fn typed_addresses_get_a_scheme_and_other_words_are_searched() {
        assert_eq!(Url::from_input("example.org/news").unwrap().to_string(), "https://example.org/news");
        assert_eq!(Url::from_input(" http://example.org ").unwrap().to_string(), "http://example.org/");
        assert_eq!(Url::from_input("localhost:8080/x").unwrap().to_string(), "https://localhost:8080/x");
        assert_eq!(Url::from_input("what is rust").unwrap().to_string(), "https://www.bing.com/search?q=what+is+rust");
        assert_eq!(Url::from_input("rust").unwrap().host, "www.bing.com");
        assert!(Url::from_input("   ").is_err());
        assert!(Url::from_input("javascript:alert(1)").is_err());
    }

    #[test]
    fn form_data_is_encoded_the_way_a_browser_does() {
        let p = |k: &str, v: &str| (k.to_string(), v.to_string());
        assert_eq!(form_urlencode(&[p("q", "rust lang"), p("a&b", "x=y"), p("é", "ü€"), p("empty", "")]), "q=rust+lang&a%26b=x%3Dy&%C3%A9=%C3%BC%E2%82%AC&empty=");
        assert_eq!(form_urlencode(&[]), "");
    }

    #[test]
    fn links_resolve_against_the_page() {
        let base = u("https://example.org/docs/guide/intro.html?v=2#old");
        assert_eq!(base.join("next.html").unwrap().to_string(), "https://example.org/docs/guide/next.html");
        assert_eq!(base.join("../index.html").unwrap().to_string(), "https://example.org/docs/index.html");
        assert_eq!(base.join("/root").unwrap().to_string(), "https://example.org/root");
        assert_eq!(base.join("//cdn.example.net/x.png").unwrap().to_string(), "https://cdn.example.net/x.png");
        assert_eq!(base.join("http://other.org/").unwrap().to_string(), "http://other.org/");
        assert_eq!(base.join("#sec").unwrap().to_string(), "https://example.org/docs/guide/intro.html?v=2#sec");
        assert_eq!(base.join("?q=1").unwrap().to_string(), "https://example.org/docs/guide/intro.html?q=1");
        assert_eq!(base.join("").unwrap(), base);
        assert_eq!(base.join("./a/./b/../c").unwrap().to_string(), "https://example.org/docs/guide/a/c");
        assert_eq!(base.join("../../../../x").unwrap().to_string(), "https://example.org/x", "cannot climb above the root");
        for bad in ["javascript:void(0)", "mailto:x@y.z", "tel:123", "data:text/plain,a", "JavaScript:alert(1)"] {
            assert!(base.join(bad).is_err(), "{bad}");
        }
        assert_eq!(base.join("dir/").unwrap().to_string(), "https://example.org/docs/guide/dir/");
        assert_eq!(u("https://example.org/a//b/./c/..").path, "/a/b/");
    }

    #[test]
    fn bing_result_links_are_unwrapped_to_the_real_address() {
        let u = Url::parse("https://www.bing.com/ck/a?!&&p=abc&ptn=3&ver=2&u=a1aHR0cHM6Ly9lbi53aWtpcGVkaWEub3JnL3dpa2kvQ2F0&ntb=1").unwrap();
        assert_eq!(u.unwrap_search_link().unwrap().to_string(), "https://en.wikipedia.org/wiki/Cat");
        assert!(Url::parse("https://www.bing.com/search?q=a").unwrap().unwrap_search_link().is_none());
        assert!(Url::parse("https://www.bing.com/ck/a?u=a1bm90IGEgdXJs").unwrap().unwrap_search_link().is_none());
        assert!(Url::parse("https://example.org/ck/a?u=a1aHR0cHM6Ly9lbi53aWtpcGVkaWEub3JnLw").unwrap().unwrap_search_link().is_none());
    }

    #[test]
    fn words_become_a_search_and_google_searches_are_rerouted() {
        assert_eq!(Url::from_input("tamil language").unwrap().to_string(), "https://www.bing.com/search?q=tamil+language");
        assert_eq!(Url::from_input("weather").unwrap().host, "www.bing.com");
        assert_eq!(Url::from_input("example.org").unwrap().host, "example.org");
        assert_eq!(Url::search("a&b=c d").query.as_deref(), Some("q=a%26b%3Dc+d"));
        let g = |s: &str| Url::parse(s).unwrap().reroute_google_search().map(|u| u.to_string());
        assert_eq!(g("https://www.google.com/search?q=tamil+language&hl=en").as_deref(), Some("https://www.bing.com/search?q=tamil+language"));
        assert_eq!(g("https://google.co.in/search?hl=en&q=a%26b").as_deref(), Some("https://www.bing.com/search?q=a%26b"));
        let ddg = Url::from_input_on("tamil language", SearchEngine::DuckDuckGo).unwrap().to_string();
        assert_eq!(ddg, "https://html.duckduckgo.com/html/?q=tamil+language");
        assert_eq!(Url::from_input_on("example.org", SearchEngine::DuckDuckGo).unwrap().host, "example.org");
        assert_eq!(SearchEngine::from_id("duckduckgo"), Some(SearchEngine::DuckDuckGo));
        assert_eq!(SearchEngine::Bing.next().next().next().next(), SearchEngine::Bing);
        // an engine's own results page is recognised, and the next one gets the same words
        let page = Url::parse("https://search.brave.com/search?q=a%26b+c&source=web").unwrap();
        let (e, q) = SearchEngine::of_url(&page).unwrap();
        assert_eq!((e, q.as_str()), (SearchEngine::Brave, "a%26b+c"));
        assert_eq!(SearchEngine::fallback(&[e], &q).unwrap().1.to_string(), "https://www.bing.com/search?q=a%26b+c");
        assert_eq!(SearchEngine::fallback(&[SearchEngine::Bing, e], &q).unwrap().0, SearchEngine::DuckDuckGo);
        assert!(SearchEngine::fallback(&SearchEngine::ALL, &q).is_none());
        let wp = Url::parse("https://en.wikipedia.org/w/index.php?search=cats").unwrap();
        assert_eq!(SearchEngine::of_url(&wp).map(|(e, q)| (e, q)), Some((SearchEngine::Wikipedia, "cats".to_string())));
        assert!(SearchEngine::of_url(&Url::parse("https://example.org/search?q=x").unwrap()).is_none());
        assert_eq!(Url::from_input_on("cats and dogs", SearchEngine::Bing).unwrap().to_string(), "https://www.bing.com/search?q=cats+and+dogs");
        assert_eq!(Url::from_input_on("cats", SearchEngine::Wikipedia).unwrap().to_string(), "https://en.wikipedia.org/w/index.php?search=cats");
        // a typed Google results address follows the chosen engine, whatever its field is called
        let gw = Url::parse("https://www.google.com/search?q=a%26b").unwrap().reroute_google_search_on(SearchEngine::Wikipedia).unwrap();
        assert_eq!(gw.to_string(), "https://en.wikipedia.org/w/index.php?search=a%26b");
        for e in SearchEngine::ALL {
            assert_eq!(SearchEngine::from_id(e.id()), Some(e));
            assert!(Url::from_input_on("x y", e).unwrap().to_string().starts_with(&format!("https://{}", e.host())));
        }
        assert_eq!(g("https://www.google.com/"), None);
        assert_eq!(g("https://www.google.com/search"), None);
        assert_eq!(g("https://www.google.com/search?q="), None);
        assert_eq!(g("https://notgoogle.com/search?q=x"), None);
        assert_eq!(g("https://google.evil.example/search?q=x"), None);
    }
}
