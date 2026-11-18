//! Links in the chat: finding them in text, deciding what each kind of address may do, and warning about the ones that look wrong.
//!
//! Nothing here opens anything. A link is opened only by the person's click (or Enter on a focused link), through the app that
//! owns the address (the browser, the editor, the clipboard), and some kinds are never opened at all (`javascript:`, `data:` …).

/// A link found in plain text: byte range and the address it stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub url: String,
}

/// An address longer than this is not made a link (nobody reads it, and it is how page-sized tricks hide).
pub const MAX_URL: usize = 2000;

const DOMAIN_ENDS: [&str; 14] = ["com", "org", "net", "io", "dev", "app", "ai", "in", "uk", "de", "edu", "gov", "co", "info"];

fn is_stop(c: char) -> bool {
    c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`' | '|' | '\u{201c}' | '\u{201d}')
}

/// The end of the address that starts at `start`, with trailing punctuation and unbalanced closing brackets taken off.
fn token_end(text: &str, start: usize) -> usize {
    let rest = &text[start..];
    let mut end = rest.find(is_stop).map_or(rest.len(), |i| i);
    loop {
        let t = &rest[..end];
        let Some(last) = t.chars().last() else { break };
        let trim = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '*' | '_' | '\u{2019}' => true,
            ')' => t.matches(')').count() > t.matches('(').count(),
            ']' => t.matches(']').count() > t.matches('[').count(),
            '}' => t.matches('}').count() > t.matches('{').count(),
            _ => false,
        };
        if trim {
            end -= last.len_utf8();
        } else {
            break;
        }
    }
    start + end
}

fn starts_with_ci(s: &str, p: &str) -> bool {
    s.len() >= p.len() && s.is_char_boundary(p.len()) && s[..p.len()].eq_ignore_ascii_case(p)
}

/// Links written as plain text: `http(s)://…`, `www.…`, `mailto:`, `tel:`, `sms:`, `geo:`, and a bare `domain.com/path`.
pub fn find_bare(text: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        let prev = text[..i].chars().last();
        let boundary = prev.is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '/' | '@' | ':' | '%')));
        let scheme = ["https://", "http://", "mailto:", "tel:", "sms:", "geo:"].into_iter().find(|p| starts_with_ci(rest, p));
        let www = starts_with_ci(rest, "www.") && rest.len() > 6;
        if boundary && (scheme.is_some() || www) {
            let end = token_end(text, i);
            let tok = &text[i..end];
            let min = scheme.map_or(7, str::len);
            if tok.len() > min && tok.len() <= MAX_URL {
                let url = if www { format!("https://{tok}") } else { tok.to_string() };
                out.push(Span { start: i, end, url });
                i = end;
                continue;
            }
        } else if boundary && rest.starts_with(|c: char| c.is_alphanumeric()) {
            // a bare domain: `maps.apple.com/place?q=x`, `example.com`
            let end = token_end(text, i);
            let tok = &text[i..end];
            let host_end = tok.find(['/', '?', '#']).unwrap_or(tok.len());
            let host = &tok[..host_end];
            let labels: Vec<&str> = host.split('.').collect();
            if labels.len() >= 2 && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_alphanumeric() || c == '-')) && DOMAIN_ENDS.contains(&labels[labels.len() - 1].to_ascii_lowercase().as_str()) && tok.len() <= MAX_URL && !host.contains('@') {
                out.push(Span { start: i, end, url: format!("https://{tok}") });
                i = end;
                continue;
            }
        }
        // on to the next word: just after the next space or opening bracket
        let first = rest.chars().next().map_or(1, char::len_utf8);
        let opener = |c: char| c.is_whitespace() || matches!(c, '(' | '[' | '<' | '"' | '*' | '_' | '\'' | '\u{201c}');
        if rest.starts_with(opener) {
            i += first;
        } else {
            match rest[first..].find(opener) {
                Some(j) => i += first + j,
                None => break,
            }
        }
    }
    out
}

/// A markdown destination starting right after `](`: the address (with `<…>` brackets and a `"title"` removed) and the index just after the closing `)`.
pub fn parse_destination(c: &[char], from: usize) -> Option<(String, usize)> {
    let mut i = from;
    while c.get(i) == Some(&' ') {
        i += 1;
    }
    let url: String;
    if c.get(i) == Some(&'<') {
        let close = (i + 1..c.len()).find(|&j| c[j] == '>')?;
        url = c[i + 1..close].iter().collect();
        i = close + 1;
    } else {
        let start = i;
        let mut depth = 0i32;
        while i < c.len() {
            match c[i] {
                '(' => depth += 1,
                ')' if depth == 0 => break,
                ')' => depth -= 1,
                ' ' | '\n' => break,
                _ => {}
            }
            i += 1;
        }
        url = c[start..i].iter().collect();
    }
    while c.get(i) == Some(&' ') {
        i += 1;
    }
    // an optional "title"
    if matches!(c.get(i), Some('"') | Some('\'')) {
        let q = c[i];
        let close = (i + 1..c.len()).find(|&j| c[j] == q)?;
        i = close + 1;
        while c.get(i) == Some(&' ') {
            i += 1;
        }
    }
    (c.get(i) == Some(&')') && !url.is_empty()).then_some((url.trim().to_string(), i + 1))
}

/// What an address is, for deciding what a click does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `http(s)`: the address, whether it is encrypted.
    Http { secure: bool },
    /// `about:blank`.
    Blank,
    Mail(String),
    Tel(String),
    /// `geo:lat,lon`.
    Geo(String),
    /// A file of the workspace or of the computer, with an optional line.
    File { path: String, line: Option<usize> },
    /// A protocol IYAKKI cannot open: the card says so and offers to copy.
    Confirm(String),
    /// Never opened (`javascript:`, `data:`, `vbscript:`, `blob:`, `about:` other than blank).
    Never(String),
    /// A scheme nobody here knows: the card, then copy only.
    Unknown(String),
}

fn scheme_of(url: &str) -> Option<&str> {
    let (s, rest) = url.split_once(':')?;
    let ok = !s.is_empty() && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
    // `src/main.rs:42` and `localhost:3000` are not schemes
    (ok && !rest.is_empty() && !(rest.chars().take_while(|c| c.is_ascii_digit()).count() == rest.len().min(6) && rest.chars().all(|c| c.is_ascii_digit() || c == '/'))).then_some(s)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && s.is_char_boundary(i + 1) && s.is_char_boundary(i + 3) {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `src/main.rs:42`, `./deploy/app.yaml`, `deploy/app.yaml#L7`: a path inside the workspace and the line, when it names a file of a known kind.
pub fn workspace_path(s: &str) -> Option<(String, Option<usize>)> {
    const EXTS: [&str; 24] = ["rs", "js", "ts", "tsx", "jsx", "py", "go", "java", "c", "h", "cpp", "hpp", "cs", "php", "rb", "sh", "md", "json", "yaml", "yml", "toml", "html", "css", "txt"];
    let t = s.trim().trim_start_matches("./");
    if t.is_empty() || t.starts_with('/') || t.contains("..") || t.contains("://") || t.contains(char::is_whitespace) {
        return None;
    }
    let (path, line) = match t.split_once('#') {
        Some((p, f)) => (p, f.trim_start_matches('L').split(['-', ':']).next().and_then(|n| n.parse::<usize>().ok())),
        None => match t.rsplit_once(':') {
            Some((p, l)) if l.chars().all(|c| c.is_ascii_digit()) && !l.is_empty() => (p, l.parse::<usize>().ok()),
            _ => (t, None),
        },
    };
    let name = path.rsplit('/').next().unwrap_or("");
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    (EXTS.contains(&ext.as_str()) && !name.starts_with('.') && path.chars().all(|c| c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))).then(|| (path.to_string(), line))
}

pub fn classify(url: &str) -> Kind {
    let u = url.trim();
    let Some(scheme) = scheme_of(u) else {
        // an absolute path: a file of this computer (the scopes decide whether it may be opened)
        if u.starts_with('/') && !u.starts_with("//") && !u.contains(char::is_whitespace) {
            let (path, line) = match u.rsplit_once(':') {
                Some((a, l)) if !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()) => (a.to_string(), l.parse().ok()),
                _ => (u.to_string(), None),
            };
            return Kind::File { path, line };
        }
        return match workspace_path(u) {
            Some((path, line)) => Kind::File { path, line },
            None => Kind::Unknown(String::new()),
        };
    };
    let rest = &u[scheme.len() + 1..];
    match scheme.to_ascii_lowercase().as_str() {
        "http" => Kind::Http { secure: false },
        "https" => Kind::Http { secure: true },
        "mailto" => Kind::Mail(rest.split('?').next().unwrap_or("").to_string()),
        "tel" | "sms" => Kind::Tel(rest.split(['?', ';']).next().unwrap_or("").to_string()),
        "geo" => Kind::Geo(rest.split(['?', ';']).next().unwrap_or("").to_string()),
        "file" => {
            let p = percent_decode(rest.trim_start_matches("//").trim_start_matches("localhost"));
            let (path, line) = match p.rsplit_once(':') {
                Some((a, l)) if !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()) => (a.to_string(), l.parse().ok()),
                _ => (p, None),
            };
            Kind::File { path, line }
        }
        "about" if rest == "blank" => Kind::Blank,
        s @ ("javascript" | "data" | "vbscript" | "blob" | "about") => Kind::Never(s.to_string()),
        s @ ("ssh" | "sftp" | "ftp" | "ftps" | "vnc" | "rdp" | "telnet" | "smb" | "nfs" | "ldap" | "ldaps" | "irc" | "git" | "svn" | "ws" | "wss" | "magnet" | "steam" | "slack" | "zoommtg" | "facetime" | "x-apple.systempreferences") => Kind::Confirm(s.to_string()),
        s => Kind::Unknown(s.to_string()),
    }
}

/// The host of an http(s) address, lower case.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.trim().split_once("://")?.1;
    let auth = rest.split(['/', '?', '#']).next()?;
    let auth = auth.rsplit('@').next()?;
    let host = if auth.starts_with('[') { auth.split(']').next()?.trim_start_matches('[').to_string() } else { auth.split(':').next()?.to_string() };
    (!host.is_empty()).then(|| host.to_lowercase())
}

/// An address of this computer or of a private network: plain http is expected there (a dev server, the cluster's own address).
pub fn is_local_host(host: &str) -> bool {
    let h = host.trim_end_matches('.');
    if h == "localhost" || h == "::1" || h.ends_with(".local") || h.ends_with(".lan") || h.ends_with(".svc") || h.ends_with(".cluster.local") || h.ends_with(".localhost") {
        return true;
    }
    let p: Vec<u32> = h.split('.').filter_map(|x| x.parse().ok()).collect();
    if p.len() != 4 || h.split('.').count() != 4 {
        return false;
    }
    p[0] == 127 || p[0] == 10 || (p[0] == 192 && p[1] == 168) || (p[0] == 172 && (16..=31).contains(&p[1])) || (p[0] == 169 && p[1] == 254) || (p[0] == 100 && (64..=127).contains(&p[1]))
}

/// Punycode (RFC 3492) of one label, decoded; None when it is not valid.
pub fn punycode_decode(input: &str) -> Option<String> {
    const BASE: u32 = 36;
    const TMIN: u32 = 1;
    const TMAX: u32 = 26;
    const SKEW: u32 = 38;
    const DAMP: u32 = 700;
    fn adapt(mut delta: u32, num: u32, first: bool) -> u32 {
        delta = if first { delta / DAMP } else { delta / 2 };
        delta += delta / num;
        let mut k = 0;
        while delta > ((BASE - TMIN) * TMAX) / 2 {
            delta /= BASE - TMIN;
            k += BASE;
        }
        k + (BASE * delta) / (delta + SKEW)
    }
    let (basic, ext) = match input.rfind('-') {
        Some(i) => (&input[..i], &input[i + 1..]),
        None => ("", input),
    };
    let mut out: Vec<char> = basic.chars().collect();
    let (mut n, mut i, mut bias) = (128u32, 0u32, 72u32);
    let mut it = ext.chars().peekable();
    while it.peek().is_some() {
        let oldi = i;
        let mut w = 1u32;
        let mut k = BASE;
        loop {
            let c = it.next()?;
            let digit = match c {
                'a'..='z' => c as u32 - 'a' as u32,
                'A'..='Z' => c as u32 - 'A' as u32,
                '0'..='9' => c as u32 - '0' as u32 + 26,
                _ => return None,
            };
            i = i.checked_add(digit.checked_mul(w)?)?;
            let t = if k <= bias { TMIN } else if k >= bias + TMAX { TMAX } else { k - bias };
            if digit < t {
                break;
            }
            w = w.checked_mul(BASE - t)?;
            k += BASE;
        }
        let len = out.len() as u32 + 1;
        bias = adapt(i - oldi, len, oldi == 0);
        n = n.checked_add(i / len)?;
        i %= len;
        out.insert(i as usize, char::from_u32(n)?);
        i += 1;
    }
    Some(out.into_iter().collect())
}

/// The host as the person should read it: `xn--` labels decoded. The flag says a look-alike (punycode) label was found.
pub fn display_host(url: &str) -> Option<(String, bool)> {
    let host = host_of(url)?;
    let mut puny = false;
    let shown: Vec<String> = host
        .split('.')
        .map(|l| match l.strip_prefix("xn--").and_then(punycode_decode) {
            Some(d) => {
                puny = true;
                d
            }
            None => l.to_string(),
        })
        .collect();
    Some((shown.join("."), puny))
}

/// What is wrong with a link, if anything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Warnings {
    /// The visible text reads like another address than the target.
    pub mismatch: bool,
    /// Plain http to an address that is not this computer or a private network.
    pub insecure: bool,
    /// The host has a punycode (look-alike) label.
    pub lookalike: bool,
    pub too_long: bool,
}

impl Warnings {
    pub fn any(&self) -> bool {
        self.mismatch || self.insecure || self.lookalike || self.too_long
    }

    /// Short words for the status line.
    pub fn text(&self, url: &str) -> String {
        let mut v = Vec::new();
        if self.mismatch {
            v.push("the text shows a different address than the link goes to".to_string());
        }
        if self.insecure {
            v.push("insecure: not encrypted (http)".to_string());
        }
        if self.lookalike {
            if let Some((h, _)) = display_host(url) {
                v.push(format!("look-alike address: the host reads {h}"));
            }
        }
        if self.too_long {
            v.push("very long address".to_string());
        }
        v.join(" · ")
    }
}

/// Does the visible text look like an address?
fn urlish(label: &str) -> Option<String> {
    let l = label.trim();
    let written = starts_with_ci(l, "http://") || starts_with_ci(l, "https://") || starts_with_ci(l, "www.");
    let t = l.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.");
    let host = t.split(['/', '?', '#']).next()?;
    let labels: Vec<&str> = host.split('.').collect();
    let ok = !t.contains(char::is_whitespace) && labels.len() >= 2 && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_alphanumeric() || c == '-')) && (written || DOMAIN_ENDS.contains(&labels[labels.len() - 1].to_ascii_lowercase().as_str()));
    ok.then(|| host.to_lowercase())
}

pub fn warnings(label: &str, url: &str) -> Warnings {
    let mut w = Warnings { too_long: url.len() > 1000, ..Default::default() };
    let target = host_of(url);
    if let (Some(shown), Some(t)) = (urlish(label), &target) {
        let t = t.strip_prefix("www.").unwrap_or(t);
        w.mismatch = shown.strip_prefix("www.").unwrap_or(&shown) != t;
    }
    if let Some(t) = &target {
        w.insecure = url.trim_start().to_ascii_lowercase().starts_with("http://") && !is_local_host(t);
        w.lookalike = display_host(url).is_some_and(|d| d.1);
    }
    w
}

/// A map link (or a `geo:` address): worth a "Copy coordinates / address" item.
pub fn is_map_link(url: &str) -> bool {
    if url.trim().to_ascii_lowercase().starts_with("geo:") {
        return true;
    }
    let Some(h) = host_of(url) else { return false };
    let h = h.strip_prefix("www.").unwrap_or(&h).to_string();
    h == "maps.apple.com" || h == "openstreetmap.org" || h.ends_with(".openstreetmap.org") || h == "maps.google.com" || ((h == "google.com" || h.ends_with(".google.com")) && url.contains("/maps")) || h == "goo.gl" && url.contains("/maps")
}

/// Coordinates or the address of a map link: `geo:12.9,80.2`, `@12.9,80.2` of Google Maps, `ll=` / `q=` of Apple Maps, `mlat`/`mlon` of OSM.
pub fn map_text(url: &str) -> Option<String> {
    let u = url.trim();
    if let Kind::Geo(c) = classify(u) {
        return (!c.is_empty()).then_some(c);
    }
    let after = |key: &str| -> Option<String> {
        let i = u.find(&format!("{key}="))? + key.len() + 1;
        let v = u[i..].split(['&', '#']).next()?;
        Some(percent_decode(&v.replace('+', " ")))
    };
    if let Some(i) = u.find('@') {
        let v: String = u[i + 1..].split(['/', '?', 'z']).next().unwrap_or("").split(',').take(2).collect::<Vec<_>>().join(",");
        if v.contains(',') && v.chars().all(|c| c.is_ascii_digit() || matches!(c, ',' | '.' | '-')) {
            return Some(v);
        }
    }
    if let (Some(a), Some(b)) = (after("mlat"), after("mlon")) {
        return Some(format!("{a},{b}"));
    }
    after("ll").or_else(|| after("coordinate")).or_else(|| after("address")).or_else(|| after("q")).or_else(|| after("name")).filter(|s| !s.is_empty())
}

/// A `geo:` address as an OpenStreetMap link.
pub fn geo_to_https(url: &str) -> Option<String> {
    let Kind::Geo(c) = classify(url) else { return None };
    let (lat, lon) = c.split_once(',')?;
    let (a, b): (f64, f64) = (lat.trim().parse().ok()?, lon.trim().parse().ok()?);
    Some(format!("https://www.openstreetmap.org/?mlat={a}&mlon={b}#map=16/{a}/{b}"))
}

/// The address cut for display (the full one stays in the link).
pub fn shown(url: &str, max: usize) -> String {
    if url.chars().count() <= max {
        return url.to_string();
    }
    let head: String = url.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(t: &str) -> Vec<String> {
        find_bare(t).into_iter().map(|s| s.url).collect()
    }

    #[test]
    fn bare_links_are_found_with_their_punctuation_and_brackets_handled() {
        assert_eq!(urls("see https://example.com/a. Then go on"), ["https://example.com/a"]);
        assert_eq!(urls("(https://example.com/a)"), ["https://example.com/a"]);
        assert_eq!(urls("wiki: https://en.wikipedia.org/wiki/Rust_(programming_language)."), ["https://en.wikipedia.org/wiki/Rust_(programming_language)"]);
        assert_eq!(urls("a, https://x.test/p?q=1&r=2; b"), ["https://x.test/p?q=1&r=2"]);
        assert_eq!(urls("<https://x.test/a>"), ["https://x.test/a"], "inside angle brackets");
        assert_eq!(urls("go to www.example.org/page now"), ["https://www.example.org/page"]);
        assert_eq!(urls("mail mailto:ann@example.com or tel:+4912345 or geo:12.9,80.2 or sms:+911"), ["mailto:ann@example.com", "tel:+4912345", "geo:12.9,80.2", "sms:+911"]);
        assert_eq!(urls("maps.apple.com/place?q=Cafe and example.com."), ["https://maps.apple.com/place?q=Cafe", "https://example.com"]);
        assert_eq!(urls("unicode https://例え.jp/パス?q=ü ok"), ["https://例え.jp/パス?q=ü"]);
        assert_eq!(urls("**https://x.test/a**"), ["https://x.test/a"], "emphasis marks are not part of it");
        // not links
        assert!(urls("main.rs and README.md and version 1.2.3 and e.g. this").is_empty());
        assert!(urls("https:// and http://").is_empty());
        assert!(urls("user@example.com is an address, not a web link").is_empty());
        let long = format!("https://x.test/{}", "a".repeat(2100));
        assert!(urls(&long).is_empty(), "a very long address is not made a link");
        let spans = find_bare("go https://x.test/a now");
        assert_eq!((spans[0].start, spans[0].end), (3, 19));
    }

    #[test]
    fn markdown_destinations_take_titles_brackets_and_nested_parentheses() {
        let p = |s: &str| {
            let c: Vec<char> = s.chars().collect();
            parse_destination(&c, 0)
        };
        assert_eq!(p("https://x.test/a)"), Some(("https://x.test/a".into(), 17)));
        assert_eq!(p("https://x.test/a \"A title\")"), Some(("https://x.test/a".into(), 27)));
        assert_eq!(p("<https://x.test/a b>)"), Some(("https://x.test/a b".into(), 21)));
        assert_eq!(p("https://en.wikipedia.org/wiki/Rust_(language))").map(|x| x.0), Some("https://en.wikipedia.org/wiki/Rust_(language)".into()));
        assert_eq!(p("https://x.test/a"), None, "never closed");
        assert_eq!(p(")"), None, "empty");
    }

    #[test]
    fn every_scheme_gets_its_kind() {
        assert_eq!(classify("https://x.test"), Kind::Http { secure: true });
        assert_eq!(classify("HTTP://x.test"), Kind::Http { secure: false });
        assert_eq!(classify("mailto:ann@example.com?subject=hi"), Kind::Mail("ann@example.com".into()));
        assert_eq!(classify("tel:+4912345"), Kind::Tel("+4912345".into()));
        assert_eq!(classify("sms:+911;x=1"), Kind::Tel("+911".into()));
        assert_eq!(classify("geo:12.9,80.2?z=5"), Kind::Geo("12.9,80.2".into()));
        assert_eq!(classify("file:///home/amy/a.txt:7"), Kind::File { path: "/home/amy/a.txt".into(), line: Some(7) });
        assert_eq!(classify("file:///home/amy/My%20Files/a.txt"), Kind::File { path: "/home/amy/My Files/a.txt".into(), line: None });
        assert_eq!(classify("src/main.rs:42"), Kind::File { path: "src/main.rs".into(), line: Some(42) });
        assert_eq!(classify("./deploy/app.yaml"), Kind::File { path: "deploy/app.yaml".into(), line: None });
        assert_eq!(classify("deploy/app.yaml#L7"), Kind::File { path: "deploy/app.yaml".into(), line: Some(7) });
        for never in ["javascript:alert(1)", "JavaScript:void(0)", "data:text/html;base64,AAAA", "vbscript:x", "blob:https://x.test/uuid", "about:config"] {
            assert!(matches!(classify(never), Kind::Never(_)), "{never}");
        }
        assert_eq!(classify("about:blank"), Kind::Blank);
        for confirm in ["ssh://host", "vnc://host:5900", "rdp://h", "ftp://h/x", "sftp://h", "smb://nas/share"] {
            assert!(matches!(classify(confirm), Kind::Confirm(_)), "{confirm}");
        }
        assert_eq!(classify("weirdproto://x"), Kind::Unknown("weirdproto".into()));
        assert_eq!(classify("localhost:3000"), Kind::Unknown(String::new()), "a host and port is not a scheme");
        assert!(workspace_path("../etc/passwd").is_none() && workspace_path("/etc/passwd").is_none() && workspace_path("note.xyz").is_none());
    }

    #[test]
    fn warnings_cover_a_text_that_shows_another_address_plain_http_and_lookalike_hosts() {
        assert!(warnings("https://bank.com", "https://evil.test/login").mismatch);
        assert!(warnings("bank.com", "https://www.bank.com/x").mismatch == false, "www is the same site");
        assert!(!warnings("Apple Maps", "https://maps.apple.com/p").mismatch, "a name is not an address");
        assert!(!warnings("https://x.test/a", "https://x.test/b").mismatch);
        assert!(warnings("see this", "http://example.com/x").insecure);
        for ok in ["http://localhost:3000/", "http://192.168.1.5:30080/", "http://10.43.219.73/", "http://web.shop-dev.svc/", "http://nas.local/"] {
            assert!(!warnings("x", ok).insecure, "{ok}");
        }
        assert!(!warnings("x", "https://example.com").insecure);
        // punycode is decoded and flagged
        assert_eq!(punycode_decode("bcher-kva"), Some("bücher".into()));
        assert_eq!(punycode_decode("80ak6aa92e"), Some("аррӏе".into()));
        assert_eq!(display_host("https://xn--80ak6aa92e.com/").unwrap(), ("аррӏе.com".to_string(), true));
        let w = warnings("Apple", "https://xn--80ak6aa92e.com/");
        assert!(w.lookalike && w.any() && w.text("https://xn--80ak6aa92e.com/").contains("аррӏе.com"));
        assert_eq!(display_host("https://example.com/").unwrap(), ("example.com".to_string(), false));
        assert!(warnings("x", &format!("https://x.test/{}", "a".repeat(1100))).too_long);
        assert!(!warnings("Apple Maps", "https://maps.apple.com/p").any());
        assert_eq!(shown("https://example.com/abcdef", 12), "https://exa…");
    }

    #[test]
    fn map_links_give_their_coordinates_or_address() {
        assert!(is_map_link("https://maps.apple.com/place?q=Cafe") && is_map_link("https://www.google.com/maps/place/x/@12.9,80.2,15z") && is_map_link("https://www.openstreetmap.org/?mlat=1&mlon=2") && is_map_link("geo:1,2"));
        assert!(!is_map_link("https://example.com/maps"));
        assert_eq!(map_text("geo:12.9,80.2"), Some("12.9,80.2".into()));
        assert_eq!(map_text("https://www.google.com/maps/place/Chennai/@13.0827,80.2707,12z"), Some("13.0827,80.2707".into()));
        assert_eq!(map_text("https://www.openstreetmap.org/?mlat=13.08&mlon=80.27#map=16/13.08/80.27"), Some("13.08,80.27".into()));
        assert_eq!(map_text("https://maps.apple.com/place?ll=13.08,80.27&q=Cafe"), Some("13.08,80.27".into()));
        assert_eq!(map_text("https://maps.apple.com/place?q=Cafe+de+Paris&address=1+Main+St"), Some("1 Main St".into()));
        assert_eq!(geo_to_https("geo:12.5,80.25").unwrap(), "https://www.openstreetmap.org/?mlat=12.5&mlon=80.25#map=16/12.5/80.25");
    }
}
