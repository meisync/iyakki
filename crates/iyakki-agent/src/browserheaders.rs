//! Custom request headers for the Browser (Settings -> Browser -> Custom headers). A rule is a header name, a value, a domain filter
//! and an optional URL prefix, and can be enabled, disabled and deleted. Matching uses the same hybrid filter as the VPN and the SSH
//! tunnels (`iyakki_vpn::filter::FilterSet`).
//!
//! A header value is secret-like (a token, a cookie): it lives only in the person's vault (through [`Values`]), never in the rule
//! file, never in an audit line or a log, and the lists show it masked. The header is sent only to requests that match, over https
//! (plain http only to this computer or a private address), and the in-house engine asks again for every redirect hop, so a header
//! never follows a redirect to a site it was not meant for. Names the browser owns are refused.
//!
//! Where it is applied: the in-house engine (`iyakki_web::headerfetch`, `iyakki_proxy::https::BrowserFetch`). The full web engine
//! (WPE) is NOT wired: it needs a web process extension hooking `send-request`, which the helper does not have yet.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use iyakki_vpn::filter::{FilterSet, Route};
use serde::{Deserialize, Serialize};

pub const MAX_RULES: usize = 50;
pub const MAX_VALUE: usize = 4096;
pub const MAX_NAME: usize = 60;
pub const MAX_FILTER: usize = 600;
/// The most headers added to one request.
pub const MAX_PER_REQUEST: usize = 16;
/// What the lists show instead of a value.
pub const MASK: &str = "********";

/// Headers the browser or the transport owns: refused.
const FORBIDDEN: &[&str] = &[
    "host", "content-length", "transfer-encoding", "connection", "keep-alive", "upgrade", "te", "trailer", "expect", "accept-encoding", "content-encoding", "content-type", "set-cookie", "range", "if-none-match", "if-modified-since",
    "http2-settings", "via", "forwarded",
];
/// Allowed only with the person's explicit "I know" tick.
const NEEDS_ACK: &[&str] = &["cookie", "authorization"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HeaderRule {
    pub id: String,
    /// A label for the list ("Staging token").
    pub name: String,
    /// The domain filter: entries separated by commas (`api.example.org`, `*.corp.example`); empty when only a prefix is given.
    pub domains: String,
    /// An optional URL prefix (`https://example.org/api`); the rule applies only below it.
    pub prefix: String,
    /// An optional regular expression the request's path (and query, `/api/v2/items?id=7`) must match, on top of the domain filter and
    /// the prefix: `^/api/v[0-9]+/`, `\\.json$`. Narrows a rule; never sends a header to a site by itself. Matched by the small in-house
    /// engine (a step limit stops a pattern that would run forever).
    #[serde(default)]
    pub path_regex: String,
    pub header: String,
    pub enabled: bool,
    /// The person ticked "I know" for Cookie or Authorization.
    pub ack: bool,
}

/// Where values are kept: the person's vault in the shell, memory in tests.
pub trait Values: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<String>, String>;
    fn set(&self, id: &str, value: &str) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<(), String>;
}

/// A header name check: refuses what the browser owns. `Err` says why, in words.
pub fn check_name(name: &str, ack: bool) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() || n.len() > MAX_NAME {
        return Err(format!("A header name is 1 to {MAX_NAME} characters."));
    }
    if !n.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)) {
        return Err("A header name has letters, digits and - _ . only (no spaces, no colon).".into());
    }
    let l = n.to_ascii_lowercase();
    if FORBIDDEN.contains(&l.as_str()) || l.starts_with("proxy-") || l.starts_with("sec-") {
        return Err(format!("{n} is set by the browser itself and cannot be changed here."));
    }
    if NEEDS_ACK.contains(&l.as_str()) && !ack {
        return Err(format!("{n} carries a sign-in. Tick \"I know\" to confirm it is meant for the sites in the filter."));
    }
    Ok(())
}

/// A header value check: printable ASCII and tab only (no line breaks, no control characters), at most [`MAX_VALUE`].
pub fn check_value(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("The header value is empty.".into());
    }
    if value.len() > MAX_VALUE {
        return Err(format!("The header value is longer than {MAX_VALUE} characters."));
    }
    if !value.bytes().all(|b| b == b'\t' || (0x20..0x7f).contains(&b)) {
        return Err("The header value may hold printable ASCII only (no line breaks or control characters).".into());
    }
    Ok(())
}

fn entries(text: &str) -> Vec<String> {
    text.split([',', '\n', ' ']).map(str::trim).filter(|e| !e.is_empty()).map(str::to_string).collect()
}

fn filter_of(text: &str, what: &str) -> Result<Option<FilterSet>, String> {
    let es = entries(text);
    if es.is_empty() {
        return Ok(None);
    }
    if es.iter().any(|e| e.starts_with('!')) {
        return Err(format!("The {what} filter lists the sites the header goes to; an exclusion (!) is not used here."));
    }
    FilterSet::from_entries(&es).map(Some).map_err(|e| format!("The {what} filter: {e}"))
}

/// Checks a rule without its value (the value is checked on its own).
pub fn check_rule(r: &HeaderRule) -> Result<(), String> {
    let name = r.name.trim();
    if name.is_empty() || name.len() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(format!("Give the rule a name of 1 to {MAX_NAME} characters."));
    }
    check_name(&r.header, r.ack)?;
    if r.domains.len() > MAX_FILTER || r.prefix.len() > MAX_FILTER {
        return Err("The filter is too long.".into());
    }
    let d = filter_of(&r.domains, "domain")?;
    let p = filter_of(&r.prefix, "URL prefix")?;
    if r.path_regex.len() > MAX_FILTER || r.path_regex.chars().any(char::is_control) {
        return Err("The path pattern is too long or has a control character.".into());
    }
    if !r.path_regex.is_empty() {
        crate::regex::Regex::new(&r.path_regex, true, false).map_err(|e| format!("The path pattern is not a valid regular expression: {e}"))?;
    }
    if d.is_none() && p.is_none() {
        return Err("Say which sites get the header: a domain (api.example.org, *.corp.example) or a URL prefix. It is never sent to every site.".into());
    }
    if let Some(p) = &p {
        if p.len() != 1 || !p.rules()[0].is_url_rule() {
            return Err("The URL prefix is one address with a path, such as https://example.org/api.".into());
        }
    }
    Ok(())
}

/// A value shown in a list: never any part of it.
pub fn masked(_value: &str) -> &'static str {
    MASK
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    rules: Vec<HeaderRule>,
}

/// The rules of one person: a small file of everything but the values, and the vault for the values.
pub struct Store {
    path: PathBuf,
    values: Arc<dyn Values>,
}

fn new_id() -> String {
    let mut b = [0u8; 8];
    let _ = iyakki_sec::random_fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Store {
    pub fn new(path: &Path, values: Arc<dyn Values>) -> Store {
        Store { path: path.to_path_buf(), values }
    }

    pub fn list(&self) -> Vec<HeaderRule> {
        std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str::<File>(&t).ok()).map(|f| f.rules).unwrap_or_default().into_iter().take(MAX_RULES).collect()
    }

    fn save(&self, rules: &[HeaderRule]) -> Result<(), String> {
        let tmp = self.path.with_extension("tmp");
        let text = serde_json::to_string(&File { rules: rules.to_vec() }).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &self.path)).map_err(|e| format!("The rules could not be saved: {e}"))
    }

    /// Add a rule (its id is made here). The value goes to the vault; if that fails nothing is kept.
    pub fn add(&self, mut r: HeaderRule, value: &str) -> Result<HeaderRule, String> {
        check_rule(&r)?;
        check_value(value)?;
        let mut rules = self.list();
        if rules.len() >= MAX_RULES {
            return Err(format!("At most {MAX_RULES} rules."));
        }
        r.id = new_id();
        r.name = r.name.trim().to_string();
        r.header = r.header.trim().to_string();
        self.values.set(&r.id, value)?;
        rules.push(r.clone());
        if let Err(e) = self.save(&rules) {
            let _ = self.values.delete(&r.id);
            return Err(e);
        }
        Ok(r)
    }

    /// Change a rule; a new value only when `value` is given (the old one is kept otherwise).
    pub fn update(&self, id: &str, mut r: HeaderRule, value: Option<&str>) -> Result<HeaderRule, String> {
        check_rule(&r)?;
        if let Some(v) = value {
            check_value(v)?;
        }
        let mut rules = self.list();
        let at = rules.iter().position(|x| x.id == id).ok_or("That rule is gone.")?;
        r.id = id.to_string();
        r.name = r.name.trim().to_string();
        r.header = r.header.trim().to_string();
        if let Some(v) = value {
            self.values.set(id, v)?;
        }
        rules[at] = r.clone();
        self.save(&rules)?;
        Ok(r)
    }

    pub fn set_enabled(&self, id: &str, on: bool) -> Result<HeaderRule, String> {
        let mut rules = self.list();
        let r = rules.iter_mut().find(|x| x.id == id).ok_or("That rule is gone.")?;
        r.enabled = on;
        let out = r.clone();
        self.save(&rules)?;
        Ok(out)
    }

    pub fn delete(&self, id: &str) -> Result<HeaderRule, String> {
        let mut rules = self.list();
        let at = rules.iter().position(|x| x.id == id).ok_or("That rule is gone.")?;
        let gone = rules.remove(at);
        self.save(&rules)?;
        let _ = self.values.delete(id);
        Ok(gone)
    }

    /// Enabled rules with their values read from the vault; the second number counts enabled rules whose value could not be read
    /// (a locked vault).
    pub fn compile(&self) -> (Vec<Compiled>, usize) {
        let (mut out, mut skipped) = (Vec::new(), 0);
        for r in self.list().into_iter().filter(|r| r.enabled) {
            let Ok(Some(value)) = self.values.get(&r.id) else {
                skipped += 1;
                continue;
            };
            match Compiled::new(&r, value) {
                Some(c) => out.push(c),
                None => skipped += 1,
            }
        }
        (out, skipped)
    }
}

/// A rule ready to match requests.
pub struct Compiled {
    pub name: String,
    header: String,
    value: String,
    domains: Option<FilterSet>,
    prefix: Option<FilterSet>,
    path: Option<crate::regex::Regex>,
}

impl std::fmt::Debug for Compiled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Compiled({}: {} = <hidden>)", self.name, self.header)
    }
}

/// The address as the filter judges it: scheme, host, port, path (no query, no fragment).
fn judged(u: &iyakki_web::Url) -> String {
    let default = if u.https { 443 } else { 80 };
    let port = u.port.filter(|p| *p != default).map(|p| format!(":{p}")).unwrap_or_default();
    format!("{}://{}{}{}", if u.https { "https" } else { "http" }, u.host, port, u.path)
}

/// Plain http is allowed only to this computer or a private address.
fn local_host(h: &str) -> bool {
    if h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    match h.trim_matches(|c| c == '[' || c == ']').parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(a)) => a.is_loopback() || a.is_private(),
        Ok(std::net::IpAddr::V6(a)) => a.is_loopback(),
        Err(_) => false,
    }
}

impl Compiled {
    pub fn new(r: &HeaderRule, value: String) -> Option<Compiled> {
        check_rule(r).ok()?;
        check_value(&value).ok()?;
        Some(Compiled { name: r.name.clone(), header: r.header.clone(), value, domains: filter_of(&r.domains, "domain").ok()?, prefix: filter_of(&r.prefix, "URL prefix").ok()?, path: (!r.path_regex.is_empty()).then(|| crate::regex::Regex::new(&r.path_regex, true, false).ok()).flatten() })
    }

    pub fn matches(&self, url: &iyakki_web::Url) -> bool {
        if !url.https && !local_host(&url.host) {
            return false;
        }
        let text = judged(url);
        let hit = |f: &FilterSet| f.decide_url(&text).is_ok_and(|d| d.route == Route::Tunnel);
        let path_ok = self.path.as_ref().is_none_or(|re| match &url.query {
            Some(q) => re.is_match(&format!("{}?{q}", url.path)),
            None => re.is_match(&url.path),
        });
        self.domains.as_ref().is_none_or(hit) && self.prefix.as_ref().is_none_or(hit) && path_ok
    }
}

/// The headers for one request: every matching rule once per header name (the first rule in list order wins).
pub fn headers_for(rules: &[Compiled], url: &iyakki_web::Url) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for r in rules.iter().filter(|r| r.matches(url)) {
        if out.len() >= MAX_PER_REQUEST {
            break;
        }
        if !out.iter().any(|(n, _)| n.eq_ignore_ascii_case(&r.header)) {
            out.push((r.header.clone(), r.value.clone()));
        }
    }
    out
}

/// How many rules apply to this address (for "custom headers active here (n)"), and their names (never values).
pub fn active_names(rules: &[Compiled], url: &iyakki_web::Url) -> Vec<String> {
    rules.iter().filter(|r| r.matches(url)).map(|r| r.name.clone()).collect()
}

// ---- the set the browser's transport reads -------------------------------------------------------------------------------------

static ACTIVE: RwLock<Vec<Compiled>> = RwLock::new(Vec::new());

/// Make these the rules the transport applies (called when the person signs in and after every change in Settings).
pub fn activate(rules: Vec<Compiled>) {
    if let Ok(mut a) = ACTIVE.write() {
        *a = rules;
    }
}

/// Active names for an address, for the page-info line.
pub fn active_here(url: &iyakki_web::Url) -> Vec<String> {
    ACTIVE.read().map(|a| active_names(&a, url)).unwrap_or_default()
}

/// The function the transport wrapper asks for each request.
pub fn provider() -> iyakki_web::headerfetch::Provider {
    Arc::new(|u: &iyakki_web::Url| ACTIVE.read().map(|a| headers_for(&a, u)).unwrap_or_default())
}

#[cfg(test)]
#[path = "browserheaders_tests.rs"]
mod tests;
