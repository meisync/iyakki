//! Cookies the way a browser keeps them: by host and path, with expiry, `Secure` and `HttpOnly`. Sent with every request
//! to a matching address, set by `Set-Cookie` answers and by scripts (`document.cookie`, which never sees `HttpOnly`).

use crate::url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// Lowercase host the cookie belongs to.
    pub domain: String,
    /// Sent to subdomains too (the cookie was set with a `Domain` attribute).
    pub subdomains: bool,
    pub path: String,
    /// Seconds since the epoch; `None` is a session cookie.
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<String>,
}

#[derive(Default, Clone, Debug)]
pub struct CookieStore {
    cookies: Vec<Cookie>,
}

const MAX_PER_DOMAIN: usize = 50;
const MAX_TOTAL: usize = 3000;
const MAX_SIZE: usize = 4096;

impl CookieStore {
    pub fn new() -> CookieStore {
        CookieStore::default()
    }

    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    pub fn all(&self) -> &[Cookie] {
        &self.cookies
    }

    pub fn clear(&mut self) {
        self.cookies.clear();
    }

    /// Remove every cookie of a host (and its subdomain cookies).
    pub fn clear_host(&mut self, host: &str) {
        self.cookies.retain(|c| !(c.domain == host || host.ends_with(&format!(".{}", c.domain))));
    }

    /// Restore saved cookies (expired ones are dropped).
    pub fn load(&mut self, saved: Vec<Cookie>, now: i64) {
        for c in saved {
            if c.expires.is_some_and(|e| e > now) {
                self.insert(c);
            }
        }
    }

    /// Cookies that outlive the session: what a vault saves.
    pub fn persistent(&self, now: i64) -> Vec<Cookie> {
        self.cookies.iter().filter(|c| c.expires.is_some_and(|e| e > now)).cloned().collect()
    }

    fn insert(&mut self, c: Cookie) {
        // a cookie that is set again keeps its place in the order (the order they were first set)
        if let Some(i) = self.cookies.iter().position(|x| x.name == c.name && x.domain == c.domain && x.path == c.path) {
            self.cookies[i] = c;
            return;
        }
        if self.cookies.iter().filter(|x| x.domain == c.domain).count() >= MAX_PER_DOMAIN || self.cookies.len() >= MAX_TOTAL {
            return;
        }
        self.cookies.push(c);
    }

    fn parse(url: &Url, text: &str, now: i64, from_script: bool) -> Option<(Cookie, bool)> {
        if text.len() > MAX_SIZE {
            return None;
        }
        let mut parts = text.split(';');
        let (name, value) = parts.next()?.split_once('=')?;
        let (name, value) = (name.trim().to_string(), value.trim().to_string());
        if name.is_empty() || name.contains(|c: char| c.is_whitespace() || c.is_control()) || value.contains(|c: char| c.is_control()) {
            return None;
        }
        let mut c = Cookie { name, value, domain: url.host.clone(), subdomains: false, path: default_path(&url.path), expires: None, secure: false, http_only: false, same_site: None };
        let mut max_age: Option<i64> = None;
        let mut expires: Option<i64> = None;
        for attr in parts {
            let (k, v) = attr.split_once('=').map_or((attr.trim(), ""), |(k, v)| (k.trim(), v.trim()));
            match k.to_ascii_lowercase().as_str() {
                "domain" => {
                    let d = v.trim_start_matches('.').to_ascii_lowercase();
                    // a cookie may widen to a parent domain of its own host, never to a bare suffix such as "com"
                    if d.contains('.') && (url.host == d || url.host.ends_with(&format!(".{d}"))) && !is_ip(&d) {
                        c.domain = d;
                        c.subdomains = true;
                    } else if !d.is_empty() {
                        return None;
                    }
                }
                "path" if v.starts_with('/') => c.path = v.to_string(),
                "max-age" => max_age = v.parse().ok(),
                "expires" => expires = parse_http_date(v),
                "secure" => c.secure = true,
                "httponly" => c.http_only = true,
                "samesite" => c.same_site = Some(v.to_ascii_lowercase()),
                _ => {}
            }
        }
        if from_script && c.http_only {
            return None; // scripts cannot set HttpOnly cookies
        }
        if c.secure && !url.https {
            return None;
        }
        c.expires = max_age.map(|s| now + s).or(expires);
        let deleting = c.expires.is_some_and(|e| e <= now);
        Some((c, deleting))
    }

    /// A `Set-Cookie` header from the answer to a request for `url`.
    pub fn store_from_header(&mut self, url: &Url, set_cookie: &str, now: i64) {
        if let Some((c, deleting)) = Self::parse(url, set_cookie, now, false) {
            if deleting {
                self.cookies.retain(|x| !(x.name == c.name && x.domain == c.domain && x.path == c.path));
            } else {
                self.insert(c);
            }
        }
    }

    /// `document.cookie = "..."` on a page at `url`.
    pub fn store_from_script(&mut self, url: &Url, text: &str, now: i64) {
        if let Some((c, deleting)) = Self::parse(url, text, now, true) {
            if deleting {
                self.cookies.retain(|x| !(x.name == c.name && x.domain == c.domain && x.path == c.path));
            } else {
                self.insert(c);
            }
        }
    }

    fn matching(&self, url: &Url, now: i64) -> Vec<&Cookie> {
        let mut v: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|c| c.expires.is_none_or(|e| e > now))
            .filter(|c| c.domain == url.host || (c.subdomains && url.host.ends_with(&format!(".{}", c.domain))))
            .filter(|c| path_matches(&url.path, &c.path))
            .filter(|c| url.https || !c.secure)
            .collect();
        // longer paths first, then older first (the order they were set)
        v.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
        v
    }

    /// The `Cookie` request header for `url` (empty when there is none).
    pub fn header_for(&self, url: &Url, now: i64) -> String {
        self.matching(url, now).iter().map(|c| format!("{}={}", c.name, c.value)).collect::<Vec<_>>().join("; ")
    }

    /// What `document.cookie` returns on a page at `url`: no `HttpOnly` cookies.
    pub fn script_view(&self, url: &Url, now: i64) -> String {
        self.matching(url, now).iter().filter(|c| !c.http_only).map(|c| format!("{}={}", c.name, c.value)).collect::<Vec<_>>().join("; ")
    }
}

fn is_ip(h: &str) -> bool {
    h.chars().all(|c| c.is_ascii_digit() || c == '.') || h.contains(':')
}

fn default_path(request_path: &str) -> String {
    match request_path.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => request_path[..i].to_string(),
    }
}

fn path_matches(request: &str, cookie: &str) -> bool {
    request == cookie || (request.starts_with(cookie) && (cookie.ends_with('/') || request[cookie.len()..].starts_with('/')))
}

/// `Wed, 21 Oct 2015 07:28:00 GMT` (and the older formats) → seconds since the epoch.
pub fn parse_http_date(s: &str) -> Option<i64> {
    let t: Vec<&str> = s.split(|c: char| c == ' ' || c == ',' || c == '-').filter(|x| !x.is_empty()).collect();
    let t = if t.first().is_some_and(|w| w.chars().all(|c| c.is_ascii_alphabetic()) && w.len() >= 3 && !t.get(1).is_some_and(|d| d.chars().all(|c| c.is_ascii_alphabetic()))) { &t[1..] } else { &t[..] };
    let day: i64 = t.first()?.parse().ok()?;
    let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| t.get(1).is_some_and(|x| x.to_ascii_lowercase().starts_with(m)))? as i64 + 1;
    let mut year: i64 = t.get(2)?.parse().ok()?;
    if year < 100 {
        year += if year < 70 { 2000 } else { 1900 };
    }
    let hms: Vec<i64> = t.get(3)?.split(':').filter_map(|x| x.parse().ok()).collect();
    if hms.len() != 3 {
        return None;
    }
    let (y, m) = if month <= 2 { (year - 1, month + 9) } else { (year, month - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146097 + doe - 719468) * 86400 + hms[0] * 3600 + hms[1] * 60 + hms[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn a_cookie_is_sent_only_where_its_host_path_and_security_match() {
        let mut j = CookieStore::new();
        j.store_from_header(&u("https://a.org/app/page"), "sid=1; Path=/app", 100);
        j.store_from_header(&u("https://a.org/"), "all=2", 100);
        j.store_from_header(&u("https://a.org/"), "sec=3; Secure", 100);
        j.store_from_header(&u("https://www.a.org/"), "wide=4; Domain=a.org", 100);
        assert_eq!(j.header_for(&u("https://a.org/app/x"), 100), "sid=1; all=2; sec=3; wide=4", "longer paths first");
        assert_eq!(j.header_for(&u("https://a.org/other"), 100), "all=2; sec=3; wide=4");
        assert_eq!(j.header_for(&u("https://a.org/application"), 100), "all=2; sec=3; wide=4", "/app does not match /application");
        assert_eq!(j.header_for(&u("http://a.org/"), 100), "all=2; wide=4", "Secure cookies stay on https");
        assert_eq!(j.header_for(&u("https://www.a.org/"), 100), "wide=4", "a host-only cookie stays on its host; Domain cookies go to subdomains");
        assert_eq!(j.header_for(&u("https://b.org/"), 100), "");
        assert_eq!(j.header_for(&u("https://evil-a.org/"), 100), "", "a look-alike host is a different host");
    }

    #[test]
    fn expiry_deletion_and_replacement() {
        let mut j = CookieStore::new();
        let a = u("https://a.org/");
        j.store_from_header(&a, "k=1; Max-Age=60", 1000);
        j.store_from_header(&a, "e=2; Expires=Thu, 01 Jan 1970 00:20:00 GMT", 1000);
        j.store_from_header(&a, "s=3", 1000);
        assert_eq!(j.header_for(&a, 1000), "k=1; e=2; s=3");
        assert_eq!(j.header_for(&a, 1100), "e=2; s=3", "Max-Age ran out");
        assert_eq!(j.header_for(&a, 1300), "s=3", "Expires passed");
        assert_eq!(j.persistent(1000).iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["k", "e"]);
        j.store_from_header(&a, "s=changed", 1000);
        assert_eq!(j.header_for(&a, 1000), "k=1; e=2; s=changed", "same name, host and path replaces");
        j.store_from_header(&a, "s=; Max-Age=0", 1000);
        assert_eq!(j.header_for(&a, 1000), "k=1; e=2", "Max-Age=0 deletes");
        j.store_from_header(&a, "k=1; Expires=Thu, 01 Jan 1970 00:00:01 GMT", 1000);
        assert_eq!(j.header_for(&a, 1000), "e=2", "a date in the past deletes");
    }

    #[test]
    fn domain_attributes_are_checked() {
        let mut j = CookieStore::new();
        j.store_from_header(&u("https://www.a.org/"), "good=1; Domain=a.org", 0);
        j.store_from_header(&u("https://www.a.org/"), "bare=1; Domain=org", 0);
        j.store_from_header(&u("https://www.a.org/"), "other=1; Domain=b.org", 0);
        j.store_from_header(&u("https://www.a.org/"), "sub=1; Domain=.www.a.org", 0);
        j.store_from_header(&u("https://a.org/"), "down=1; Domain=www.a.org", 0);
        let names: Vec<&str> = j.all().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["good", "sub"], "a bare suffix, another site and a child host are all refused");
        assert_eq!(j.header_for(&u("https://x.a.org/"), 0), "good=1");
    }

    #[test]
    fn scripts_see_no_http_only_cookies_and_cannot_set_them() {
        let mut j = CookieStore::new();
        let a = u("https://a.org/");
        j.store_from_header(&a, "session=secret; HttpOnly", 0);
        j.store_from_header(&a, "pref=dark", 0);
        assert_eq!(j.header_for(&a, 0), "session=secret; pref=dark", "requests carry everything");
        assert_eq!(j.script_view(&a, 0), "pref=dark", "scripts never see HttpOnly cookies");
        j.store_from_script(&a, "evil=1; HttpOnly", 0);
        j.store_from_script(&a, "pref=light", 0);
        j.store_from_script(&a, "on-http=1; Secure", 0);
        assert_eq!(j.header_for(&a, 0), "session=secret; pref=light; on-http=1", "a Secure cookie is fine from an https page");
        j.store_from_script(&u("http://a.org/"), "plain=1; Secure", 0);
        assert_eq!(j.len(), 3, "a Secure cookie cannot be set from plain http");
    }

    #[test]
    fn bad_cookies_and_limits() {
        let mut j = CookieStore::new();
        let a = u("https://a.org/dir/page");
        for bad in ["", "novalue", "=x", "bad name=1", &format!("big={}", "x".repeat(5000)), "ctl=a\u{1}b"] {
            j.store_from_header(&a, bad, 0);
        }
        assert!(j.is_empty());
        j.store_from_header(&a, "d=1", 0);
        assert_eq!(j.all()[0].path, "/dir", "the default path is the request's directory");
        for i in 0..100 {
            j.store_from_header(&a, &format!("c{i}=1"), 0);
        }
        assert_eq!(j.all().len(), 50, "one site cannot fill the jar");
        j.clear_host("a.org");
        assert!(j.is_empty());
    }

    #[test]
    fn saved_cookies_come_back_without_the_expired_ones() {
        let mut j = CookieStore::new();
        let a = u("https://a.org/");
        j.store_from_header(&a, "keep=1; Max-Age=1000", 0);
        j.store_from_header(&a, "gone=1; Max-Age=10", 0);
        j.store_from_header(&a, "session=1", 0);
        let saved = j.persistent(0);
        let mut k = CookieStore::new();
        k.load(saved, 500);
        assert_eq!(k.header_for(&a, 500), "keep=1", "session and expired cookies are not restored");
    }

    #[test]
    fn http_dates_parse() {
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT"), Some(1445412480));
        assert_eq!(parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"), Some(784111777));
        assert_eq!(parse_http_date("garbage"), None);
    }
}
