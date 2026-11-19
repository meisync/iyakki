//! The extension snapshot: what the shell hands the full web engine's web process so browser extensions can act on requests
//! (docs/development/BROWSER-EXTENSIONS-PLAN.md). One message type carries any number of extensions, each with an id, a version, the
//! capabilities it was granted and a list of hooks; a hook is a kind number and an opaque payload. The first and only hook kind
//! implemented is `request.headers` (custom request headers, F-BROW-79). Other kinds are named so they can be added without a new
//! message type or a protocol version bump: a kind this build does not know, or has no code for, is skipped and reported, never an
//! error.
//!
//! The C side is `dist/webview/wpe-helper/extsnap.h` (the same layout, plain libc, tested against this module by
//! `iyakki-agent::browserext` tests that compile and run `ext-check.c`).
//!
//! Wire layout (little-endian). A snapshot: `u32 version (1)`, `u32 n`, then n entries: `str id`, `str version`, `u32 ncaps`
//! `str cap`..., `u32 nhooks`, then per hook `u32 kind`, `u32 len`, `len` bytes. A `request.headers` payload: `u32 nrules`, per rule
//! `str header`, `str value`, `u8 replace`, `u32 nd` patterns (the domain filter), `u32 np` patterns (the URL prefix). A pattern:
//! `u8 scheme` (0 any, 1 http, 2 https), `u8 suffix`, `str host`, `u32 port` (0 any), `u32 nseg`, `str seg`... A `str` is `u32 len`
//! and bytes. The value of a header is a secret: no `Debug` here prints it, and a snapshot is never written to a file.

/// The snapshot layout version. A helper that does not know the number refuses the whole snapshot (and reports it).
pub const SNAPSHOT_VERSION: u32 = 1;
pub const MAX_SNAPSHOT: usize = 512 * 1024;
pub const MAX_ENTRIES: usize = 16;
pub const MAX_CAPS: usize = 16;
pub const MAX_HOOKS: usize = 16;
pub const MAX_RULES: usize = 50;
pub const MAX_PATTERNS: usize = 50;
pub const MAX_SEGMENTS: usize = 32;
pub const MAX_STR: usize = 4096;
/// The most headers added to one request.
pub const MAX_PER_REQUEST: usize = 16;

/// Hook kinds. Only `REQUEST_HEADERS` has code today; the rest are reserved numbers.
pub mod hook {
    pub const REQUEST_HEADERS: u32 = 1;
    pub const RESPONSE_HEADERS: u32 = 2;
    pub const REQUEST_BLOCK: u32 = 3;
    pub const CONTENT_SCRIPT: u32 = 4;
    pub const PAGE_ACTION: u32 = 5;

    pub fn name(kind: u32) -> Option<&'static str> {
        Some(match kind {
            REQUEST_HEADERS => "request.headers",
            RESPONSE_HEADERS => "response.headers",
            REQUEST_BLOCK => "request.block",
            CONTENT_SCRIPT => "content.script",
            PAGE_ACTION => "page.action",
            _ => return None,
        })
    }
    /// The capability an extension must hold for a hook kind (the catalogue name, or a proposed name for kinds without code yet).
    pub fn capability(kind: u32) -> Option<&'static str> {
        Some(match kind {
            REQUEST_HEADERS => "web.request.headers",
            RESPONSE_HEADERS => "web.response.headers",
            REQUEST_BLOCK => "web.request.block",
            CONTENT_SCRIPT => "web.content.script",
            PAGE_ACTION => "web.page.action",
            _ => return None,
        })
    }
    /// Whether this build can act on the kind.
    pub fn implemented(kind: u32) -> bool {
        kind == REQUEST_HEADERS
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ExtError {
    /// The snapshot layout number is not one this build reads.
    Version(u32),
    Truncated,
    TooLong,
    TooMany,
    BadUtf8,
}

impl std::fmt::Display for ExtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtError::Version(v) if *v > SNAPSHOT_VERSION => write!(f, "snapshot version {v} is newer than this build reads ({SNAPSHOT_VERSION}): update the program that reads it (the web engine helper, or IYAKKI)"),
            ExtError::Version(v) => write!(f, "snapshot version {v} is older than this build reads ({SNAPSHOT_VERSION}): update the program that wrote it"),
            ExtError::Truncated => write!(f, "snapshot is cut short"),
            ExtError::TooLong => write!(f, "snapshot has a string or a body that is too long"),
            ExtError::TooMany => write!(f, "snapshot has too many entries"),
            ExtError::BadUtf8 => write!(f, "snapshot has text that is not UTF-8"),
        }
    }
}

/// The bytes of a snapshot as they travel in `ToHelper::Extensions`. Opaque on purpose: the transport never looks inside, so a new hook
/// kind needs no change to it. `Debug` shows the size only (the bytes hold header values).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct ExtBlob(pub Vec<u8>);

impl std::fmt::Debug for ExtBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ExtBlob({} bytes, hidden)", self.0.len())
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hook {
    pub kind: u32,
    pub payload: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub id: String,
    pub version: String,
    /// The capabilities the broker granted it when the snapshot was built.
    pub caps: Vec<String>,
    pub hooks: Vec<Hook>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
}

/// A host pattern plus optional scheme, port and path segments (a `FilterSet` include entry reduced).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Pat {
    /// 0 any, 1 http, 2 https.
    pub scheme: u8,
    pub suffix: bool,
    pub host: String,
    pub port: u16,
    pub segs: Vec<String>,
}

/// One header rule of the `request.headers` hook. `Debug` never shows the value.
#[derive(Clone, PartialEq, Eq)]
pub struct HeaderRule {
    pub header: String,
    pub value: String,
    /// Replace a header the page already set (otherwise the page's wins).
    pub replace: bool,
    /// The domain filter: any of these (empty means any site).
    pub domains: Vec<Pat>,
    /// The URL prefix: any of these (empty means any address).
    pub prefix: Vec<Pat>,
}

impl std::fmt::Debug for HeaderRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HeaderRule({}: <hidden>)", self.header)
    }
}

// ---- writing ---------------------------------------------------------------------------------------------------------------

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        let mut end = s.len().min(MAX_STR);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.u32(end as u32);
        self.0.extend_from_slice(&s.as_bytes()[..end]);
    }
}

fn put_pats(w: &mut W, ps: &[Pat]) {
    w.u32(ps.len().min(MAX_PATTERNS) as u32);
    for p in ps.iter().take(MAX_PATTERNS) {
        w.u8(p.scheme);
        w.u8(p.suffix as u8);
        w.str(&p.host);
        w.u32(p.port as u32);
        w.u32(p.segs.len().min(MAX_SEGMENTS) as u32);
        for s in p.segs.iter().take(MAX_SEGMENTS) {
            w.str(s);
        }
    }
}

/// The payload of a `request.headers` hook.
pub fn encode_header_rules(rules: &[HeaderRule]) -> Vec<u8> {
    let mut w = W(Vec::new());
    w.u32(rules.len().min(MAX_RULES) as u32);
    for r in rules.iter().take(MAX_RULES) {
        w.str(&r.header);
        w.str(&r.value);
        w.u8(r.replace as u8);
        put_pats(&mut w, &r.domains);
        put_pats(&mut w, &r.prefix);
    }
    w.0
}

impl Snapshot {
    /// The bytes for `ToHelper::Extensions`. Cut to the documented caps, so the other side never refuses it for size; an empty
    /// snapshot (no extensions) is valid and clears what the helper held.
    pub fn encode(&self) -> ExtBlob {
        let mut w = W(Vec::new());
        w.u32(SNAPSHOT_VERSION);
        w.u32(self.entries.len().min(MAX_ENTRIES) as u32);
        for e in self.entries.iter().take(MAX_ENTRIES) {
            w.str(&e.id);
            w.str(&e.version);
            w.u32(e.caps.len().min(MAX_CAPS) as u32);
            for c in e.caps.iter().take(MAX_CAPS) {
                w.str(c);
            }
            w.u32(e.hooks.len().min(MAX_HOOKS) as u32);
            for h in e.hooks.iter().take(MAX_HOOKS) {
                w.u32(h.kind);
                w.u32(h.payload.len() as u32);
                w.0.extend_from_slice(&h.payload);
            }
        }
        if w.0.len() > MAX_SNAPSHOT {
            // too big to send: send an empty snapshot rather than a half one
            return Snapshot::default().encode();
        }
        ExtBlob(w.0)
    }

    pub fn decode(b: &[u8]) -> Result<Snapshot, ExtError> {
        if b.len() > MAX_SNAPSHOT {
            return Err(ExtError::TooLong);
        }
        let mut r = Rd(b);
        let v = r.u32()?;
        if v != SNAPSHOT_VERSION {
            return Err(ExtError::Version(v));
        }
        let n = r.count(MAX_ENTRIES)?;
        let mut entries = Vec::new();
        for _ in 0..n {
            let id = r.str()?;
            let version = r.str()?;
            let nc = r.count(MAX_CAPS)?;
            let mut caps = Vec::new();
            for _ in 0..nc {
                caps.push(r.str()?);
            }
            let nh = r.count(MAX_HOOKS)?;
            let mut hooks = Vec::new();
            for _ in 0..nh {
                let kind = r.u32()?;
                let len = r.u32()? as usize;
                hooks.push(Hook { kind, payload: r.take(len)?.to_vec() });
            }
            entries.push(Entry { id, version, caps, hooks });
        }
        Ok(Snapshot { entries })
    }
}

// ---- reading ---------------------------------------------------------------------------------------------------------------

struct Rd<'a>(&'a [u8]);

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ExtError> {
        if self.0.len() < n {
            return Err(ExtError::Truncated);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, ExtError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, ExtError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn count(&mut self, max: usize) -> Result<usize, ExtError> {
        let n = self.u32()? as usize;
        if n > max {
            return Err(ExtError::TooMany);
        }
        Ok(n)
    }
    fn str(&mut self) -> Result<String, ExtError> {
        let n = self.u32()? as usize;
        if n > MAX_STR {
            return Err(ExtError::TooLong);
        }
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| ExtError::BadUtf8)
    }
}

fn get_pats(r: &mut Rd) -> Result<Vec<Pat>, ExtError> {
    let n = r.count(MAX_PATTERNS)?;
    let mut out = Vec::new();
    for _ in 0..n {
        let scheme = r.u8()?;
        let suffix = r.u8()? != 0;
        let host = r.str()?;
        let port = r.u32()?;
        let ns = r.count(MAX_SEGMENTS)?;
        let mut segs = Vec::new();
        for _ in 0..ns {
            segs.push(r.str()?);
        }
        out.push(Pat { scheme, suffix, host, port: u16::try_from(port).map_err(|_| ExtError::TooLong)?, segs });
    }
    Ok(out)
}

pub fn decode_header_rules(b: &[u8]) -> Result<Vec<HeaderRule>, ExtError> {
    let mut r = Rd(b);
    let n = r.count(MAX_RULES)?;
    let mut out = Vec::new();
    for _ in 0..n {
        let header = r.str()?;
        let value = r.str()?;
        let replace = r.u8()? != 0;
        let domains = get_pats(&mut r)?;
        let prefix = get_pats(&mut r)?;
        out.push(HeaderRule { header, value, replace, domains, prefix });
    }
    Ok(out)
}

// ---- dispatch --------------------------------------------------------------------------------------------------------------

/// What the web process makes of a snapshot: the header rules it will apply and a report of what it ignored (shown in the helper log,
/// never with a value).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Dispatched {
    pub header_rules: Vec<HeaderRule>,
    pub report: Vec<String>,
}

impl Snapshot {
    /// Route every hook to its handler. A hook whose kind is unknown or has no code here, or whose extension does not list the
    /// capability the kind needs, or whose payload is broken, is skipped and reported; nothing else is affected.
    pub fn dispatch(&self) -> Dispatched {
        let mut d = Dispatched::default();
        for e in &self.entries {
            for h in &e.hooks {
                let Some(name) = hook::name(h.kind) else {
                    d.report.push(format!("{}: hook kind {} is unknown, ignored", e.id, h.kind));
                    continue;
                };
                if !hook::implemented(h.kind) {
                    d.report.push(format!("{}: hook {name} is not implemented in this build, ignored", e.id));
                    continue;
                }
                let need = hook::capability(h.kind).unwrap_or("");
                if !e.caps.iter().any(|c| c == need) {
                    d.report.push(format!("{}: hook {name} without the capability {need}, ignored", e.id));
                    continue;
                }
                match decode_header_rules(&h.payload) {
                    Ok(mut rs) => d.header_rules.append(&mut rs),
                    Err(err) => d.report.push(format!("{}: hook {name} is broken ({err}), ignored", e.id)),
                }
            }
        }
        d
    }
}

// ---- matching (the reference the C matcher is tested against) --------------------------------------------------------------

/// A request address reduced to what matching needs. `ambiguous` is set when the path has an escape, a backslash or a dot segment:
/// then only patterns without path segments can match.
#[derive(Clone, Debug)]
pub struct Target {
    pub https: bool,
    pub host: String,
    pub port: u16,
    pub segs: Vec<String>,
    pub ambiguous: bool,
}

fn pat_hits(p: &Pat, t: &Target) -> bool {
    let host_ok = if p.suffix { t.host.len() > p.host.len() + 1 && t.host.ends_with(&p.host) && t.host.as_bytes()[t.host.len() - p.host.len() - 1] == b'.' } else { t.host == p.host };
    if !host_ok {
        return false;
    }
    if (p.scheme == 1 && t.https) || (p.scheme == 2 && !t.https) {
        return false;
    }
    if p.port != 0 && p.port != t.port {
        return false;
    }
    if p.segs.is_empty() {
        return true;
    }
    !t.ambiguous && p.segs.len() <= t.segs.len() && p.segs.iter().zip(&t.segs).all(|(a, b)| a == b)
}

/// Plain http goes only to this computer by name (`localhost`); private address literals are not exported to this engine.
fn local_name(h: &str) -> bool {
    h == "localhost" || h.ends_with(".localhost")
}

impl HeaderRule {
    pub fn matches(&self, t: &Target) -> bool {
        if !t.https && !local_name(&t.host) {
            return false;
        }
        (self.domains.is_empty() || self.domains.iter().any(|p| pat_hits(p, t))) && (self.prefix.is_empty() || self.prefix.iter().any(|p| pat_hits(p, t)))
    }
}

/// The headers for one request: each matching rule once per header name (first rule wins), at most [`MAX_PER_REQUEST`].
/// Returns (name, value, replace). Re-run for every redirect hop by the caller.
pub fn headers_for<'a>(rules: &'a [HeaderRule], t: &Target) -> Vec<(&'a str, &'a str, bool)> {
    let mut out: Vec<(&str, &str, bool)> = Vec::new();
    for r in rules.iter().filter(|r| r.matches(t)) {
        if out.len() >= MAX_PER_REQUEST {
            break;
        }
        if !out.iter().any(|(n, _, _)| n.eq_ignore_ascii_case(&r.header)) {
            out.push((&r.header, &r.value, r.replace));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pat(host: &str, suffix: bool) -> Pat {
        Pat { scheme: 0, suffix, host: host.into(), port: 0, segs: vec![] }
    }
    fn rule(h: &str, v: &str, d: Vec<Pat>) -> HeaderRule {
        HeaderRule { header: h.into(), value: v.into(), replace: false, domains: d, prefix: vec![] }
    }
    fn snap(caps: &[&str], kind: u32, payload: Vec<u8>) -> Snapshot {
        Snapshot { entries: vec![Entry { id: "browser-headers".into(), version: "1".into(), caps: caps.iter().map(|s| s.to_string()).collect(), hooks: vec![Hook { kind, payload }] }] }
    }
    fn t(url_https: bool, host: &str, segs: &[&str]) -> Target {
        Target { https: url_https, host: host.into(), port: if url_https { 443 } else { 80 }, segs: segs.iter().map(|s| s.to_string()).collect(), ambiguous: false }
    }

    #[test]
    fn snapshot_round_trips_with_the_rules_inside() {
        let rs = vec![HeaderRule { replace: true, ..rule("X-Token", "s3cr3t", vec![pat("api.example.org", false), pat("corp.example", true)]) }];
        let s = snap(&["web.request.headers"], hook::REQUEST_HEADERS, encode_header_rules(&rs));
        let back = Snapshot::decode(&s.encode().0).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.dispatch().header_rules, rs);
        assert!(back.dispatch().report.is_empty());
    }

    #[test]
    fn debug_text_never_shows_a_value() {
        let rs = vec![rule("X-Token", "s3cr3t-value", vec![pat("a.example", false)])];
        let s = snap(&["web.request.headers"], hook::REQUEST_HEADERS, encode_header_rules(&rs));
        let blob = s.encode();
        assert!(!format!("{blob:?}").contains("s3cr3t"));
        assert!(!format!("{:?}", s.dispatch()).contains("s3cr3t"));
        assert!(!format!("{:?}", crate::proto::ToHelper::Extensions(blob)).contains("s3cr3t"));
        let all = format!("{:?}", s.dispatch().report);
        assert!(!all.contains("s3cr3t"));
    }

    #[test]
    fn unknown_hook_kind_is_ignored_and_reported_and_the_rest_still_works() {
        let rs = vec![rule("X-A", "v", vec![pat("a.example", false)])];
        let mut s = snap(&["web.request.headers"], hook::REQUEST_HEADERS, encode_header_rules(&rs));
        s.entries[0].hooks.push(Hook { kind: 999, payload: vec![1, 2, 3] });
        s.entries[0].hooks.push(Hook { kind: hook::CONTENT_SCRIPT, payload: vec![] });
        let d = Snapshot::decode(&s.encode().0).unwrap().dispatch();
        assert_eq!(d.header_rules.len(), 1);
        assert_eq!(d.report.len(), 2);
        assert!(d.report[0].contains("unknown") && d.report[1].contains("not implemented"));
    }

    #[test]
    fn a_version_mismatch_is_refused_cleanly() {
        let mut b = Snapshot::default().encode().0;
        b[0] = 2;
        assert_eq!(Snapshot::decode(&b), Err(ExtError::Version(2)));
        assert_eq!(Snapshot::decode(&[1, 0]), Err(ExtError::Truncated));
    }

    #[test]
    fn a_hook_without_its_capability_is_ignored() {
        let rs = vec![rule("X-A", "v", vec![pat("a.example", false)])];
        let s = snap(&["notify"], hook::REQUEST_HEADERS, encode_header_rules(&rs));
        let d = s.dispatch();
        assert!(d.header_rules.is_empty());
        assert!(d.report[0].contains("without the capability"));
    }

    #[test]
    fn a_broken_payload_is_skipped_not_fatal() {
        let s = snap(&["web.request.headers"], hook::REQUEST_HEADERS, vec![9, 9]);
        let d = s.dispatch();
        assert!(d.header_rules.is_empty() && d.report[0].contains("broken"));
    }

    #[test]
    fn size_caps_hold_on_both_sides() {
        let many: Vec<HeaderRule> = (0..80).map(|i| rule(&format!("X-{i}"), "v", vec![pat("a.example", false)])).collect();
        let enc = encode_header_rules(&many);
        assert_eq!(decode_header_rules(&enc).unwrap().len(), MAX_RULES);
        // a count above the cap in a received payload is refused
        let mut bad = enc.clone();
        bad[0..4].copy_from_slice(&1000u32.to_le_bytes());
        assert_eq!(decode_header_rules(&bad), Err(ExtError::TooMany));
        // a huge snapshot becomes an empty one instead of a refused one
        let big = snap(&["web.request.headers"], 1, vec![0; MAX_SNAPSHOT]);
        assert_eq!(Snapshot::decode(&big.encode().0).unwrap().entries.len(), 0);
        // a value longer than the string cap is cut, never overflows
        let long = rule("X-A", &"v".repeat(MAX_STR + 50), vec![]);
        assert_eq!(decode_header_rules(&encode_header_rules(&[long])).unwrap()[0].value.len(), MAX_STR);
    }

    #[test]
    fn matching_domain_suffix_scheme_and_prefix() {
        let mut with_prefix = rule("X-P", "v", vec![pat("example.org", false)]);
        with_prefix.prefix = vec![Pat { scheme: 2, suffix: false, host: "example.org".into(), port: 0, segs: vec!["api".into()] }];
        let rs = vec![rule("X-A", "v", vec![pat("api.example.org", false), pat("corp.example", true)]), with_prefix];
        let names = |t: &Target| headers_for(&rs, t).into_iter().map(|h| h.0.to_string()).collect::<Vec<_>>();
        assert_eq!(names(&t(true, "api.example.org", &[])), ["X-A"]);
        assert_eq!(names(&t(true, "x.corp.example", &[])), ["X-A"]);
        assert!(names(&t(true, "corp.example", &[])).is_empty());
        assert!(names(&t(true, "other.org", &[])).is_empty());
        assert_eq!(names(&t(true, "example.org", &["api", "v2"])), ["X-P"]);
        assert!(names(&t(true, "example.org", &["web"])).is_empty());
        let mut amb = t(true, "example.org", &["api"]);
        amb.ambiguous = true;
        assert!(names(&amb).is_empty());
    }

    #[test]
    fn plain_http_only_to_this_computer_and_a_redirect_is_judged_again() {
        let rs = vec![rule("X-A", "v", vec![pat("localhost", false), pat("api.example.org", false)])];
        assert_eq!(headers_for(&rs, &t(false, "localhost", &[])).len(), 1);
        assert!(headers_for(&rs, &t(false, "api.example.org", &[])).is_empty());
        // hop 1 matches, hop 2 (a redirect to another site) gets nothing: every hop is a new call
        assert_eq!(headers_for(&rs, &t(true, "api.example.org", &[])).len(), 1);
        assert!(headers_for(&rs, &t(true, "evil.example", &[])).is_empty());
    }

    #[test]
    fn at_most_sixteen_headers_and_one_per_name() {
        let mut rs: Vec<HeaderRule> = (0..30).map(|i| rule(&format!("X-{i}"), "v", vec![pat("a.example", false)])).collect();
        rs.insert(1, rule("x-0", "other", vec![pat("a.example", false)]));
        let h = headers_for(&rs, &t(true, "a.example", &[]));
        assert_eq!(h.len(), MAX_PER_REQUEST);
        assert_eq!(h.iter().filter(|x| x.0.eq_ignore_ascii_case("x-0")).count(), 1);
        assert_eq!(h[0].1, "v");
    }
}
