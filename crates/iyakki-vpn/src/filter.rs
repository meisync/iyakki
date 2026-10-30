//! Hybrid mode filters: which destinations go through the VPN. A [`FilterSet`] is parsed from lines the person typed,
//! validated hard (counts, lengths, characters, catch-all patterns) and answers "tunnel or direct, and why" for a domain
//! name, a URL or an address. Pure: no sockets, no resolver.
//!
//! # Syntax (one entry per line, `#` starts a comment, a leading `!` makes an entry an exclusion)
//!
//! | Entry | Meaning |
//! |---|---|
//! | `example.com` | exactly this name (not its subdomains) |
//! | `*.example.com` | every subdomain at any depth, **not** `example.com` itself (list both to cover both) |
//! | `https://example.com/app` | a URL rule: scheme (optional), host (exact or `*.` form), port (optional), path prefix by whole segments |
//! | `example.com/app` | the same without a scheme (any of http, https, ws, wss) |
//! | `203.0.113.0/24`, `2001:db8::/32`, `203.0.113.7` | an address range or one address |
//!
//! # Precedence
//!
//! Every rule that matches gets a specificity: an exact host beats any wildcard, a longer wildcard suffix beats a shorter one,
//! a longer prefix beats a shorter one, and for URL rules more path segments, a scheme and a port each add to it. The **most
//! specific matching rule decides**. At equal specificity an **exclusion wins** over an inclusion. Nothing matching means
//! direct. (So `*.example.com` plus `!mail.example.com` sends everything but `mail.example.com` through the VPN; and an
//! inclusion of `login.example.com` stays included under `!*.example.com`, because the exact name is more specific.)
//!
//! # What can and cannot be seen (honest limits)
//!
//! * A **domain** rule is applied by the local DNS forwarder (see [`crate::dnsroute`]) for every program on the machine, and
//!   by the app layer ([`FilterSet::decide_url`]) where IYAKKI sees the URL.
//! * A **URL path or scheme** can only be matched where IYAKKI sees the whole URL: its own browser, its in-house HTTP client,
//!   its proxy. For any other program only the destination name (DNS) or address is visible, and TLS is never intercepted.
//!   [`FilterSet::decide_domain`] therefore **ignores URL rules**; [`FilterSet::dns_ignored_url_rules`] counts them so the
//!   page can say so.
//! * A program that does its own DNS (DNS over HTTPS inside a browser, a hard-coded resolver address over TLS) bypasses the
//!   forwarder; its traffic matches only by address rules.
//! * A name that is a CNAME for a matched name is judged by the name that was asked for, not by the alias target.
//! * A catch-all is refused: `*`, a wildcard under a single label (`*.com`) or under a common two-label public suffix
//!   (`*.co.uk`; the list in [`PUBLIC_SUFFIXES`] is short, not complete), and an address range wider than /8 (IPv4) or /16
//!   (IPv6). To send everything through the VPN, choose FULL mode.

use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Serialize};

use crate::error::{invalid, rejected, VpnError};
use crate::net::{check_route, parse_ip, Cidr, Context, RouteVerdict};

pub const MAX_RULES: usize = 500;
pub const MAX_ENTRY: usize = 300;
pub const MAX_TEXT: usize = 64 * 1024;
pub const MAX_URL: usize = 2048;
pub const MAX_PATH_SEGMENTS: usize = 16;
pub const MIN_PREFIX_V4: u8 = 8;
pub const MIN_PREFIX_V6: u8 = 16;

/// Two-label suffixes under which a wildcard would cover unrelated organisations. Short on purpose; not the Public Suffix List.
pub const PUBLIC_SUFFIXES: &[&str] = &[
    "co.uk", "org.uk", "ac.uk", "gov.uk", "com.au", "net.au", "org.au", "co.nz", "co.jp", "ne.jp", "or.jp", "co.in", "co.za", "com.br", "com.cn", "com.mx", "com.tr", "com.sg", "com.hk", "com.tw", "co.kr",
    "co.id", "co.il", "com.ar", "com.my", "com.ph", "com.pk", "com.ua", "github.io", "blogspot.com", "herokuapp.com", "appspot.com", "cloudfront.net", "amazonaws.com", "azurewebsites.net",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    Tunnel,
    Direct,
}

// ---- names and URLs ---------------------------------------------------------------------------------------------------

/// A DNS name as asked: lower case, no trailing dot, labels of 1 to 63 of letters, digits, `-` and `_`, at most 253 bytes.
/// Anything else is `None` (such a name matches no rule).
pub fn normalize_name(s: &str) -> Option<String> {
    let s = s.strip_suffix('.').unwrap_or(s);
    if s.is_empty() || s.len() > 253 {
        return None;
    }
    let l = s.to_ascii_lowercase();
    l.split('.').all(|x| !x.is_empty() && x.len() <= 63 && x.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')).then_some(l)
}

fn labels(s: &str) -> usize {
    s.split('.').count()
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Host {
    Name(String),
    Ip(IpAddr),
}

/// A URL reduced to what matching needs. Query, fragment and user info are not kept (user info is refused).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub scheme: String,
    host: Host,
    pub port: u16,
    /// Normalised path segments (dot segments resolved, harmless escapes decoded).
    pub segments: Vec<String>,
}

fn default_port(scheme: &str) -> u16 {
    match scheme {
        "http" | "ws" => 80,
        "https" | "wss" => 443,
        "ftp" => 21,
        _ => 0,
    }
}

fn hexv(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// Decode `%XX` of unreserved characters, upper-case the other escapes, refuse an encoded slash or backslash, treat `\` as `/`,
/// resolve `.` and `..`. A trailing empty segment (a final slash) is dropped.
fn normalize_path(raw: &str) -> Option<Vec<String>> {
    let b = raw.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let (h, l) = (hexv(*b.get(i + 1)?)?, hexv(*b.get(i + 2)?)?);
                let v = h * 16 + l;
                if v == b'/' || v == b'\\' || v == 0 {
                    return None;
                }
                if v.is_ascii_alphanumeric() || b"-._~".contains(&v) {
                    out.push(v as char);
                } else {
                    out.push_str(&format!("%{v:02X}"));
                }
                i += 3;
            }
            b'\\' => {
                out.push('/');
                i += 1;
            }
            c if c.is_ascii_graphic() => {
                out.push(c as char);
                i += 1;
            }
            _ => return None,
        }
    }
    let mut segs: Vec<String> = Vec::new();
    let parts: Vec<&str> = out.strip_prefix('/').unwrap_or(&out).split('/').collect();
    for (n, p) in parts.iter().enumerate() {
        let last = n + 1 == parts.len();
        match *p {
            "." => {
                if last {
                    segs.push(String::new());
                }
            }
            ".." => {
                segs.pop();
                if last {
                    segs.push(String::new());
                }
            }
            s => segs.push(s.to_string()),
        }
    }
    if segs.last().is_some_and(|s| s.is_empty()) {
        segs.pop();
    }
    Some(segs)
}

/// Parse a host, either a DNS name, a dotted IPv4 address or `[IPv6]`.
fn parse_host(h: &str) -> Option<Host> {
    if let Some(v6) = h.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        return v6.parse::<std::net::Ipv6Addr>().ok().map(|a| Host::Ip(IpAddr::V6(a)));
    }
    let n = normalize_name(h)?;
    if !crate::text::valid_hostname(&n) {
        return None;
    }
    let last = n.rsplit('.').next().unwrap_or("");
    if last.bytes().all(|b| b.is_ascii_digit()) || last.starts_with("0x") {
        // looks like an address: only the canonical dotted form is accepted
        return n.parse::<Ipv4Addr>().ok().filter(|a| a.to_string() == n).map(|a| Host::Ip(IpAddr::V4(a)));
    }
    Some(Host::Name(n))
}

/// Split `scheme://authority/path?query#frag`. Returns the pieces; refuses user info, whitespace, control and non-ASCII.
fn split_url(s: &str) -> Option<(&str, &str, &str)> {
    if s.is_empty() || s.len() > MAX_URL || !s.bytes().all(|b| b.is_ascii_graphic()) {
        return None;
    }
    let (scheme, rest) = s.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (auth, tail) = rest.split_at(end);
    if auth.is_empty() || auth.contains('@') || auth.contains('\\') {
        return None;
    }
    let path_end = tail.find(['?', '#']).unwrap_or(tail.len());
    Some((scheme, auth, &tail[..path_end]))
}

fn split_port(auth: &str) -> Option<(&str, Option<u16>)> {
    let (h, p) = if auth.starts_with('[') {
        let close = auth.find(']')?;
        let rest = &auth[close + 1..];
        match rest {
            "" => (&auth[..=close], None),
            r => (&auth[..=close], Some(r.strip_prefix(':')?)),
        }
    } else {
        match auth.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (auth, None),
        }
    };
    match p {
        None => Some((h, None)),
        Some("") => Some((h, None)),
        Some(p) if p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) => Some((h, Some(p.parse::<u16>().ok().filter(|&p| p != 0)?))),
        _ => None,
    }
}

impl Url {
    /// Parse a URL for a decision. `None` for anything ambiguous: callers must then **not connect**.
    pub fn parse(s: &str) -> Option<Url> {
        let (scheme, auth, path) = split_url(s)?;
        let scheme = scheme.to_ascii_lowercase();
        if !["http", "https", "ws", "wss", "ftp"].contains(&scheme.as_str()) {
            return None;
        }
        let (h, port) = split_port(auth)?;
        Some(Url { host: parse_host(h)?, port: port.unwrap_or_else(|| default_port(&scheme)), segments: normalize_path(path)?, scheme })
    }
    pub fn host_text(&self) -> String {
        match &self.host {
            Host::Name(n) => n.clone(),
            Host::Ip(a) => a.to_string(),
        }
    }
}

// ---- rules -------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum HostPat {
    Name(String),
    Suffix(String),
    Ip(IpAddr),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Pattern {
    Domain(String),
    Suffix(String),
    Url { scheme: Option<String>, host: HostPat, port: Option<u16>, path: Vec<String> },
    Net(Cidr),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub include: bool,
    /// As typed (normalised), for display and for the explanation.
    pub text: String,
    pattern: Pattern,
}

/// (host or prefix part, extra part), compared in that order.
type Score = (u32, u32);

fn host_score(h: &HostPat) -> u32 {
    match h {
        HostPat::Name(_) => 10_000,
        HostPat::Suffix(s) => 100 + 10 * labels(s) as u32,
        HostPat::Ip(IpAddr::V4(_)) => 32,
        HostPat::Ip(IpAddr::V6(_)) => 128,
    }
}

impl Rule {
    pub fn is_url_rule(&self) -> bool {
        matches!(self.pattern, Pattern::Url { .. })
    }

    fn name_matches(&self, name: &str) -> Option<Score> {
        match &self.pattern {
            Pattern::Domain(d) if d == name => Some((10_000, 0)),
            Pattern::Suffix(s) if name.len() > s.len() + 1 && name.ends_with(s.as_str()) && name.as_bytes()[name.len() - s.len() - 1] == b'.' => Some((100 + 10 * labels(s) as u32, 0)),
            _ => None,
        }
    }

    fn ip_matches(&self, ip: IpAddr) -> Option<Score> {
        match &self.pattern {
            Pattern::Net(c) if c.contains_ip(ip) => Some((c.prefix as u32, 0)),
            _ => None,
        }
    }

    fn url_matches(&self, u: &Url) -> Option<Score> {
        let Pattern::Url { scheme, host, port, path } = &self.pattern else { return None };
        let host_ok = match (host, &u.host) {
            (HostPat::Name(n), Host::Name(h)) => n == h,
            (HostPat::Suffix(s), Host::Name(h)) => h.len() > s.len() + 1 && h.ends_with(s.as_str()) && h.as_bytes()[h.len() - s.len() - 1] == b'.',
            (HostPat::Ip(a), Host::Ip(b)) => a == b,
            _ => false,
        };
        if !host_ok || scheme.as_ref().is_some_and(|s| *s != u.scheme) || port.is_some_and(|p| p != u.port) {
            return None;
        }
        if path.len() > u.segments.len() || path.iter().zip(&u.segments).any(|(a, b)| a != b) {
            return None;
        }
        Some((host_score(host), 1 + 2 * path.len() as u32 + scheme.is_some() as u32 + port.is_some() as u32))
    }
}

// ---- decisions ---------------------------------------------------------------------------------------------------------

/// Why a destination was routed as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The most specific matching rule; `overruled` is the least specific other rule that also matched and pointed the other way.
    Rule { text: String, include: bool, overruled: Option<String> },
    /// No rule matches.
    NoRule,
    /// FULL mode: everything goes through the VPN.
    Full,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub route: Route,
    pub reason: Reason,
    /// What was asked about (a name, a URL, an address), for the sentence.
    pub subject: String,
}

impl Decision {
    /// The sentence for the page, in plain words.
    pub fn explain(&self) -> String {
        let s = &self.subject;
        match (&self.reason, self.route) {
            (Reason::Full, _) => format!("{s} goes through the VPN: this profile sends all traffic through it."),
            (Reason::NoRule, _) => format!("{s} goes direct: no rule of this profile matches it."),
            (Reason::Rule { text, include: true, overruled }, _) => {
                let mut m = format!("{s} goes through the VPN because the rule \"{text}\" matches it.");
                if let Some(o) = overruled {
                    m.push_str(&format!(" The exclusion \"{o}\" also matches but is less specific."));
                }
                m
            }
            (Reason::Rule { text, include: false, overruled }, _) => {
                let mut m = format!("{s} goes direct because the exclusion \"{text}\" matches it.");
                if let Some(o) = overruled {
                    m.push_str(&format!(" The rule \"{o}\" also matches but is less specific or equally specific (exclusions win ties)."));
                }
                m
            }
        }
    }
}

// ---- the set -----------------------------------------------------------------------------------------------------------

/// The rules of one hybrid profile. Serialised as the list of its entries.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct FilterSet {
    rules: Vec<Rule>,
}

impl From<FilterSet> for Vec<String> {
    fn from(f: FilterSet) -> Vec<String> {
        f.entries()
    }
}

impl TryFrom<Vec<String>> for FilterSet {
    type Error = String;
    fn try_from(v: Vec<String>) -> Result<FilterSet, String> {
        FilterSet::from_entries(&v).map_err(|e| e.to_string())
    }
}

fn parse_entry(line_no: usize, raw: &str) -> Result<Rule, VpnError> {
    let (include, body) = match raw.strip_prefix('!') {
        Some(r) => (false, r.trim_start()),
        None => (true, raw),
    };
    let bad = |why: &str| invalid(line_no, "filter", why);
    if body.is_empty() {
        return Err(bad("empty entry"));
    }
    if body.chars().any(|c| c.is_whitespace()) {
        return Err(bad("no spaces inside an entry"));
    }
    // an address or range
    if let Some(c) = Cidr::parse(body) {
        let min = if c.is_v4() { MIN_PREFIX_V4 } else { MIN_PREFIX_V6 };
        if c.prefix < min {
            return Err(rejected(line_no, "filter", &format!("a range wider than /{min} is a catch-all; choose FULL mode to send everything through the VPN")));
        }
        let c = c.network();
        return Ok(Rule { include, text: format!("{}{c}", if include { "" } else { "!" }), pattern: Pattern::Net(c) });
    }
    if body.contains('/') || body.contains("://") {
        return parse_url_rule(line_no, include, body);
    }
    if let Some(sfx) = body.strip_prefix("*.") {
        let s = suffix(line_no, sfx)?;
        return Ok(Rule { include, text: format!("{}*.{s}", if include { "" } else { "!" }), pattern: Pattern::Suffix(s) });
    }
    if body.contains('*') {
        return Err(rejected(line_no, "filter", "a wildcard is allowed only as the first label, as in *.example.com"));
    }
    let n = domain_name(line_no, body)?;
    Ok(Rule { include, text: format!("{}{n}", if include { "" } else { "!" }), pattern: Pattern::Domain(n) })
}

fn domain_name(line_no: usize, s: &str) -> Result<String, VpnError> {
    let n = normalize_name(s).filter(|n| crate::text::valid_hostname(n)).ok_or_else(|| invalid(line_no, "filter", "not a valid host name (use punycode for non-ASCII names)"))?;
    if n.rsplit('.').next().is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit())) {
        return Err(invalid(line_no, "filter", "not a valid host name or address"));
    }
    if n == "localhost" || n.ends_with(".localhost") {
        return Err(rejected(line_no, "filter", "localhost always stays on this computer"));
    }
    Ok(n)
}

fn suffix(line_no: usize, s: &str) -> Result<String, VpnError> {
    let n = domain_name(line_no, s)?;
    if labels(&n) < 2 || PUBLIC_SUFFIXES.contains(&n.as_str()) {
        return Err(rejected(line_no, "filter", "this wildcard would cover whole top-level or shared domains; name the organisation's own domain, or choose FULL mode"));
    }
    Ok(n)
}

fn parse_url_rule(line_no: usize, include: bool, body: &str) -> Result<Rule, VpnError> {
    let bad = |why: &str| invalid(line_no, "filter", why);
    if body.len() > MAX_URL || !body.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(bad("not a plain ASCII URL"));
    }
    if body.contains(['?', '#', '@', '\\']) {
        return Err(bad("a URL rule has no query, fragment, user info or backslash"));
    }
    let (scheme, rest) = match body.split_once("://") {
        Some((s, r)) => {
            let s = s.to_ascii_lowercase();
            if !["http", "https", "ws", "wss"].contains(&s.as_str()) {
                return Err(bad("the scheme must be http, https, ws or wss"));
            }
            (Some(s), r)
        }
        None => (None, body),
    };
    let (auth, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let (h, port) = split_port(auth).ok_or_else(|| bad("bad host or port"))?;
    let host = if let Some(sfx) = h.strip_prefix("*.") {
        HostPat::Suffix(suffix(line_no, sfx)?)
    } else if h.contains('*') {
        return Err(rejected(line_no, "filter", "a wildcard is allowed only as the first label"));
    } else {
        match parse_host(h) {
            Some(Host::Ip(_)) if scheme.is_none() => return Err(bad("write an address range as 203.0.113.0/24, or give a URL rule on an address a scheme")),
            Some(Host::Ip(a)) => HostPat::Ip(a),
            Some(Host::Name(_)) => HostPat::Name(domain_name(line_no, h)?),
            None => return Err(bad("bad host")),
        }
    };
    if path.contains('%') {
        return Err(bad("write the path without % escapes"));
    }
    let segs = normalize_path(path).ok_or_else(|| bad("bad path"))?;
    if path.split('/').any(|s| s == "." || s == "..") {
        return Err(bad("no . or .. in a path"));
    }
    if segs.len() > MAX_PATH_SEGMENTS {
        return Err(bad("the path is too deep"));
    }
    if segs.iter().any(|s| s.contains('*')) {
        return Err(rejected(line_no, "filter", "a path is a prefix; wildcards are not supported in it"));
    }
    let shown_host = match &host {
        HostPat::Name(n) => n.clone(),
        HostPat::Suffix(s) => format!("*.{s}"),
        HostPat::Ip(IpAddr::V6(a)) => format!("[{a}]"),
        HostPat::Ip(a) => a.to_string(),
    };
    let text = format!(
        "{}{}{}{}{}",
        if include { "" } else { "!" },
        scheme.as_ref().map(|s| format!("{s}://")).unwrap_or_default(),
        shown_host,
        port.map(|p| format!(":{p}")).unwrap_or_default(),
        if segs.is_empty() { if path.is_empty() { String::new() } else { "/".to_string() } } else { format!("/{}", segs.join("/")) }
    );
    Ok(Rule { include, text, pattern: Pattern::Url { scheme, host, port, path: segs } })
}

impl FilterSet {
    pub fn new() -> FilterSet {
        FilterSet::default()
    }

    /// Parse the text of the filter box.
    pub fn parse(text: &str) -> Result<FilterSet, VpnError> {
        if text.len() > MAX_TEXT {
            return Err(VpnError::TooLarge(format!("the filter list is larger than {} KB", MAX_TEXT / 1024)));
        }
        if let Some(c) = text.chars().find(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r')) {
            return Err(VpnError::BadText(format!("the filter list contains a control character (U+{:04X})", c as u32)));
        }
        let lines: Vec<&str> = text.lines().collect();
        FilterSet::build(lines.iter().enumerate().map(|(i, l)| (i + 1, *l)))
    }

    pub fn from_entries(entries: &[String]) -> Result<FilterSet, VpnError> {
        if entries.iter().map(|e| e.len()).sum::<usize>() > MAX_TEXT {
            return Err(VpnError::TooLarge("the filter list is too large".into()));
        }
        FilterSet::build(entries.iter().enumerate().map(|(i, l)| (i + 1, l.as_str())))
    }

    fn build<'a>(lines: impl Iterator<Item = (usize, &'a str)>) -> Result<FilterSet, VpnError> {
        let mut rules: Vec<Rule> = Vec::new();
        for (n, raw) in lines {
            let line = raw.split_once(" #").map(|(a, _)| a).unwrap_or(raw).trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if raw.chars().any(|c| c.is_control() && c != '\t' && c != '\r') {
                return Err(VpnError::BadText(format!("line {n}: control character")));
            }
            if line.len() > MAX_ENTRY {
                return Err(VpnError::TooLarge(format!("line {n}: an entry is longer than {MAX_ENTRY} characters")));
            }
            let r = parse_entry(n, line)?;
            if rules.iter().any(|x| x.text == r.text) {
                continue;
            }
            if rules.len() >= MAX_RULES {
                return Err(VpnError::TooLarge(format!("more than {MAX_RULES} entries")));
            }
            rules.push(r);
        }
        Ok(FilterSet { rules })
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
    pub fn len(&self) -> usize {
        self.rules.len()
    }
    /// The entries as they are stored and shown.
    pub fn entries(&self) -> Vec<String> {
        self.rules.iter().map(|r| r.text.clone()).collect()
    }
    /// How many URL rules the DNS layer cannot apply (they work only where IYAKKI sees the URL).
    pub fn dns_ignored_url_rules(&self) -> usize {
        self.rules.iter().filter(|r| r.is_url_rule()).count()
    }
    /// The address rules, for the route plan and the fail-closed guard: `(range, include)`.
    pub fn nets(&self) -> Vec<(Cidr, bool)> {
        self.rules.iter().filter_map(|r| if let Pattern::Net(c) = &r.pattern { Some((*c, r.include)) } else { None }).collect()
    }
    /// Are there rules that can send something through the tunnel at all?
    pub fn has_includes(&self) -> bool {
        self.rules.iter().any(|r| r.include)
    }

    /// Check the address rules against the real network (cluster, LAN, loopback, link-local): an inclusion that overlaps them is
    /// an error; a private range that is not the tunnel's own is a warning. Exclusions are always fine.
    pub fn check_routes(&self, ctx: &Context) -> Result<Vec<String>, VpnError> {
        let mut warnings = Vec::new();
        for r in self.rules.iter().filter(|r| r.include) {
            if let Pattern::Net(c) = &r.pattern {
                match check_route(c, ctx) {
                    RouteVerdict::Ok => {}
                    RouteVerdict::Warn(w) => warnings.push(w),
                    RouteVerdict::Refuse(w) => return Err(VpnError::Unsafe(w)),
                }
            }
        }
        if self.dns_ignored_url_rules() > 0 {
            warnings.push(format!("{} URL rule(s) work only in IYAKKI's own browser and HTTP client; other programs are routed by name and address only", self.dns_ignored_url_rules()));
        }
        Ok(warnings)
    }

    fn pick<'a>(&'a self, hits: Vec<(Score, &'a Rule)>, subject: String) -> Decision {
        let best = hits.iter().max_by_key(|(s, r)| (*s, !r.include));
        match best {
            None => Decision { route: Route::Direct, reason: Reason::NoRule, subject },
            Some((_, br)) => {
                // the strongest rule that points the other way, to say what was overruled
                let overruled = hits.iter().filter(|(_, r)| r.include != br.include).max_by_key(|(s, r)| (*s, !r.include)).map(|(_, r)| r.text.clone());
                Decision { route: if br.include { Route::Tunnel } else { Route::Direct }, reason: Reason::Rule { text: br.text.clone(), include: br.include, overruled }, subject }
            }
        }
    }

    /// For the DNS layer: a name that was asked for. URL rules and address rules are not used here.
    pub fn decide_domain(&self, name: &str) -> Decision {
        let Some(n) = normalize_name(name) else {
            return Decision { route: Route::Direct, reason: Reason::NoRule, subject: name.chars().take(80).collect() };
        };
        let hits = self.rules.iter().filter_map(|r| r.name_matches(&n).map(|s| (s, r))).collect();
        self.pick(hits, n)
    }

    /// For the connection layer: an address (a learned or typed one).
    pub fn decide_ip(&self, ip: IpAddr) -> Decision {
        let ip = ip.to_canonical();
        let hits = self.rules.iter().filter_map(|r| r.ip_matches(ip).map(|s| (s, r))).collect();
        self.pick(hits, ip.to_string())
    }

    /// For the app layer (browser, HTTP client, proxy): a URL. All rule kinds that can apply to it are used. `Err` means the URL is
    /// ambiguous or unsupported (user info, odd host spelling, encoded slash, unknown scheme): the caller must **not connect**.
    pub fn decide_url(&self, url: &str) -> Result<Decision, VpnError> {
        let u = Url::parse(url).ok_or_else(|| VpnError::Invalid { line: 0, directive: "url".into(), reason: "not an http, https, ws, wss or ftp URL that can be judged unambiguously".into() })?;
        let mut hits: Vec<(Score, &Rule)> = Vec::new();
        for r in &self.rules {
            let s = match &u.host {
                Host::Name(h) => r.name_matches(h).or_else(|| r.url_matches(&u)),
                Host::Ip(a) => r.ip_matches(*a).or_else(|| r.url_matches(&u)),
            };
            if let Some(s) = s {
                hits.push((s, r));
            }
        }
        let subject = format!("{}://{}{}", u.scheme, u.host_text(), if u.segments.is_empty() { String::new() } else { format!("/{}", u.segments.join("/")) });
        Ok(self.pick(hits, subject))
    }
}

/// FULL mode answers "tunnel" for everything; HYBRID uses the filters. One call for the browser and the HTTP client.
pub fn decide_url_for(full: bool, filters: &FilterSet, url: &str) -> Result<Decision, VpnError> {
    if full {
        let u = Url::parse(url).ok_or_else(|| VpnError::Invalid { line: 0, directive: "url".into(), reason: "not a URL that can be judged".into() })?;
        return Ok(Decision { route: Route::Tunnel, reason: Reason::Full, subject: format!("{}://{}", u.scheme, u.host_text()) });
    }
    filters.decide_url(url)
}

/// A bare address typed by the person, for the "Test a destination" box: name, URL or address, whichever it is.
pub fn decide_any(filters: &FilterSet, text: &str) -> Result<Decision, VpnError> {
    let t = text.trim();
    if t.contains("://") {
        return filters.decide_url(t);
    }
    if let Some(ip) = parse_ip(t) {
        return Ok(filters.decide_ip(ip));
    }
    if normalize_name(t).is_some() {
        return Ok(filters.decide_domain(t));
    }
    Err(VpnError::Invalid { line: 0, directive: "destination".into(), reason: "type a name, an address or a URL".into() })
}
