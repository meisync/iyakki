//! Types moved out of `iyakki_agent::devtools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Level {
    Error,
    Warn,
}

impl Level {
    pub fn word(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warning",
        }
    }
}

/// One thing the page's probe reported, already scrubbed.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DevEvent {
    /// A request that got no answer (`status` 0) or an error status.
    Failed { method: String, status: u16, ms: u32, url: String, error: String },
    /// A resource that took long to load.
    Slow { kind: String, bytes: u64, ms: u32, url: String },
    Console { level: Level, line: u32, col: u32, source: String, text: String },
    /// Times in ms after navigation start.
    Page { response_ms: u32, dcl_ms: u32, load_ms: u32, url: String },
}

/// An entry with the page (scrubbed address) that was open when it came in.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Tagged {
    pub page: String,
    pub ev: DevEvent,
    /// Counts up over all entries of all people (never reused), so a viewer can ask for what is newer than what it has seen.
    pub seq: u64,
    /// Seconds since the epoch when the shell received the line.
    pub at: i64,
}

/// What the browser's Developer tools panel shows: a copy of one person's buffers for the page they have open.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    /// [`DevTools::seq`] at the time of the copy.
    pub seq: u64,
    /// The page (scrubbed) the entries belong to.
    pub page: String,
    /// At least one line of the page probe was stored since collection began.
    pub probe_seen: bool,
    /// Oldest first.
    pub failed: Vec<Tagged>,
    pub slow: Vec<Tagged>,
    pub console: Vec<Tagged>,
    /// Page load timings: those of the current page, or when the probe named the page differently (a redirect) all kept ones.
    pub pages: Vec<Tagged>,
}

// ---- the buffers of the browser developer tools (moved from iyakki_agent::devtools; the chat tools stay there)

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use serde_json::{json, Value};

/// Entries kept per ring buffer and person.
pub const MAX_FAILED: usize = 200;
pub const MAX_RESOURCES: usize = 300;
pub const MAX_CONSOLE: usize = 200;
pub const MAX_PAGES: usize = 20;
/// People whose buffers are kept at the same time.
pub const MAX_USERS: usize = 16;
/// Longest address and message text stored.
pub const MAX_URL: usize = 200;
pub const MAX_TEXT: usize = 300;
/// Longest line accepted from the helper.
pub const MAX_LINE: usize = 1200;
/// A line of the helper: fields are separated by this character.
pub const SEP: char = '\u{1f}';
/// Words in a `key=value` pair whose value is never kept.
pub const SECRET_KEYS: [&str; 16] = ["password", "passwd", "pwd", "token", "secret", "key", "auth", "session", "sig", "signature", "code", "credential", "cookie", "bearer", "jwt", "sid"];

// ------------------------------------------------------------------------------------------------ scrubbing

pub fn secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    SECRET_KEYS.iter().any(|w| k.contains(w))
}

/// Looks like a token, key or other credential: a long run of letters and digits, hex, or a JWT.
pub fn looks_secret(t: &str) -> bool {
    let t = t.trim_matches(|c: char| matches!(c, '"' | '\'' | '(' | ')' | ',' | ';' | '[' | ']' | '<' | '>' | ':' | '{' | '}'));
    if t.len() < 16 || t.contains('/') {
        return false;
    }
    if !t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '=' | '.' | '~')) {
        return false;
    }
    if t.starts_with("eyJ") && t.contains('.') {
        return true;
    }
    if t.len() >= 32 && t.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    let (digits, letters) = (t.chars().filter(char::is_ascii_digit).count(), t.chars().filter(char::is_ascii_alphabetic).count());
    // a long word with digits in it (an id, a key); plain words and dotted names such as `Promise.resolve` stay
    t.len() >= 20 && digits >= 2 && letters >= 2 && !t.contains('.') || t.len() >= 40 && digits >= 1 && letters >= 1
}

pub fn cut_chars(s: &str, n: usize) -> String {
    let clean: String = s.chars().filter(|c| !c.is_control() || *c == ' ').collect();
    if clean.chars().count() <= n {
        clean
    } else {
        format!("{}…", clean.chars().take(n.saturating_sub(1)).collect::<String>())
    }
}

/// An address made safe to keep and show: scheme, host and port, and the path without anything that looks like a token. No user
/// info, no query string, no fragment, no `;parameters`. `data:` and `blob:` addresses become a plain word; other schemes keep
/// only the scheme. Cut to [`MAX_URL`] characters.
pub fn scrub_url(raw: &str) -> String {
    let raw = raw.trim();
    let lower = raw.to_ascii_lowercase();
    for (p, word) in [("data:", "data: URL"), ("blob:", "blob: URL"), ("javascript:", "javascript: URL"), ("about:", "about:")] {
        if lower.starts_with(p) {
            return word.to_string();
        }
    }
    let Some((scheme, rest)) = raw.split_once("://") else {
        return if raw.is_empty() { String::new() } else { "(not an address)".to_string() };
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "ws" | "wss" | "ftp") {
        return format!("{}:", scheme.chars().filter(|c| c.is_ascii_alphanumeric()).take(10).collect::<String>());
    }
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (auth, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let host = auth.rsplit('@').next().unwrap_or("");
    let mut out = format!("{}://{}", scheme.to_ascii_lowercase(), host.to_ascii_lowercase());
    for seg in path.split('/').skip(1) {
        let seg = seg.split(';').next().unwrap_or("");
        out.push('/');
        if looks_secret(seg) {
            out.push_str("<redacted>");
        } else {
            out.push_str(seg);
        }
    }
    cut_chars(&out, MAX_URL)
}

/// A message text made safe to keep: control characters out, addresses scrubbed, `key=value` pairs with a secret-sounding key and
/// token-like words replaced, cut to [`MAX_TEXT`] characters.
pub fn scrub_text(raw: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut after_scheme_word = false;
    for w in raw.split_whitespace() {
        let lower = w.to_ascii_lowercase();
        let piece = if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("ws://") || lower.starts_with("wss://") {
            let trimmed = w.trim_end_matches(['.', ',', ';', ')', '\'', '"', ':']);
            let tail = &w[trimmed.len()..];
            format!("{}{}", scrub_url(trimmed.trim_start_matches(['(', '\'', '"'])), tail)
        } else if after_scheme_word {
            "<redacted>".to_string()
        } else if let Some((k, v)) = w.split_once('=').or_else(|| w.split_once(':').filter(|(k, v)| !v.is_empty() && !v.starts_with("//") && k.len() >= 2 && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))) {
            if secret_key(k.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_')) && !v.is_empty() {
                format!("{k}={}", "<redacted>")
            } else if looks_secret(v) {
                format!("{k}=<redacted>")
            } else {
                w.to_string()
            }
        } else if looks_secret(w) {
            "<redacted>".to_string()
        } else {
            w.to_string()
        };
        after_scheme_word = matches!(lower.trim_end_matches(':'), "bearer" | "basic" | "authorization");
        out.push(piece);
    }
    cut_chars(&out.join(" "), MAX_TEXT)
}

// ------------------------------------------------------------------------------------------------ events




pub fn num(s: &str) -> u64 {
    s.trim().parse::<u64>().unwrap_or(0)
}

pub fn word(s: &str, n: usize) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' ')).take(n).collect::<String>().trim().to_string()
}

/// Parses one line of the helper's `DevEvent` message and scrubs it. `None` for anything that is not a well-formed line.
pub fn parse_line(line: &str) -> Option<DevEvent> {
    if line.len() > MAX_LINE {
        return None;
    }
    let f: Vec<&str> = line.split(SEP).collect();
    if f.len() != 6 {
        return None;
    }
    let (a, b, c, d, e) = (f[1], f[2], f[3], f[4], f[5]);
    match f[0] {
        "F" => {
            let url = scrub_url(d);
            if url.is_empty() {
                return None;
            }
            Some(DevEvent::Failed { method: word(a, 8).to_ascii_uppercase(), status: num(b).min(999) as u16, ms: num(c).min(u32::MAX as u64) as u32, url, error: scrub_text(e) })
        }
        "R" => Some(DevEvent::Slow { kind: word(a, 16), bytes: num(b), ms: num(c).min(u32::MAX as u64) as u32, url: scrub_url(d) }),
        "C" => {
            let level = match a.trim() {
                "error" => Level::Error,
                "warn" | "warning" => Level::Warn,
                _ => return None,
            };
            Some(DevEvent::Console { level, line: num(b).min(u32::MAX as u64) as u32, col: num(c).min(u32::MAX as u64) as u32, source: scrub_url(d), text: scrub_text(e) })
        }
        "P" => Some(DevEvent::Page { response_ms: num(a).min(u32::MAX as u64) as u32, dcl_ms: num(b).min(u32::MAX as u64) as u32, load_ms: num(c).min(u32::MAX as u64) as u32, url: scrub_url(d) }),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------------ the buffers

/// A bounded queue: the oldest entry goes when it is full.
#[derive(Debug)]
pub struct Ring<T> {
    cap: usize,
    items: VecDeque<T>,
}

impl<T> Ring<T> {
    pub fn new(cap: usize) -> Ring<T> {
        Ring { cap: cap.max(1), items: VecDeque::new() }
    }
    pub fn push(&mut self, v: T) {
        if self.items.len() >= self.cap {
            self.items.pop_front();
        }
        self.items.push_back(v);
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.items.iter()
    }
    pub fn clear(&mut self) {
        self.items.clear();
    }
}


#[derive(Debug)]
pub struct Buf {
    failed: Ring<Tagged>,
    slow: Ring<Tagged>,
    console: Ring<Tagged>,
    pages: Ring<Tagged>,
    /// The page the person has open now (scrubbed), the default scope of the tools.
    current: String,
    /// The `seq` of the last change (an entry, or a new current page); 0 when nothing happened yet.
    seq: u64,
    /// Lines stored since the buffers were created: a page probe that reports nothing at all is not there.
    stored: u64,
    /// The probe said hello (kind `H`, sent at document start) at least once: it is in the engine.
    hello: bool,
}

impl Buf {
    fn new() -> Buf {
        Buf { failed: Ring::new(MAX_FAILED), slow: Ring::new(MAX_RESOURCES), console: Ring::new(MAX_CONSOLE), pages: Ring::new(MAX_PAGES), current: String::new(), seq: 0, stored: 0, hello: false }
    }
}

/// The counts the BROWSER tile shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Summary {
    pub failed: usize,
    pub console_errors: usize,
}


pub struct DevTools {
    dir: PathBuf,
    on: Mutex<HashMap<String, bool>>,
    bufs: Mutex<HashMap<String, Buf>>,
    /// People whose Developer tools panel in the browser is open now. Memory only: it never touches the saved switch.
    panel: Mutex<HashSet<String>>,
    next_seq: AtomicU64,
}

pub fn who(user: &str) -> String {
    user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect()
}

impl DevTools {
    pub fn new(dir: &Path) -> DevTools {
        DevTools { dir: dir.to_path_buf(), on: Mutex::new(HashMap::new()), bufs: Mutex::new(HashMap::new()), panel: Mutex::new(HashSet::new()), next_seq: AtomicU64::new(1) }
    }

    fn path(&self, user: &str) -> PathBuf {
        self.dir.join(format!("devtools-{}.json", who(user)))
    }

    /// Whether the person turned developer tools for the chat agent on (off when nothing was ever chosen).
    pub fn enabled(&self, user: &str) -> bool {
        let mut m = self.on.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = m.get(user) {
            return *v;
        }
        let v = std::fs::read_to_string(self.path(user)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["on"].as_bool()).unwrap_or(false);
        m.insert(user.to_string(), v);
        v
    }

    /// Saves the choice. Turning it off forgets everything that was kept. The caller writes the audit line.
    pub fn set_enabled(&self, user: &str, on: bool) {
        let p = self.path(user);
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, json!({ "on": on }).to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
        self.on.lock().unwrap_or_else(|e| e.into_inner()).insert(user.to_string(), on);
        if !on {
            self.bufs.lock().unwrap_or_else(|e| e.into_inner()).remove(user);
        }
    }

    /// The person's page changed (the committed address, scrubbed here). Later events belong to it.
    pub fn set_page(&self, user: &str, url: &str) {
        if !self.collecting(user) {
            return;
        }
        let page = scrub_url(url);
        let mut m = self.bufs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(b) = Self::buf(&mut m, user) {
            if b.current != page {
                b.current = page;
                b.seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Something is collecting for this person: the chat-agent switch is on, or the browser's Developer tools panel is open.
    /// Only the switch lets the chat tools read what was collected.
    pub fn collecting(&self, user: &str) -> bool {
        self.enabled(user) || self.panel.lock().unwrap_or_else(|e| e.into_inner()).contains(user)
    }

    pub fn panel_open(&self, user: &str) -> bool {
        self.panel.lock().unwrap_or_else(|e| e.into_inner()).contains(user)
    }

    /// The person opened or closed the Developer tools panel. Opening starts collecting for this session (the saved switch is
    /// not touched); closing stops it and, unless the switch for the chat agent is on, forgets everything that was collected.
    pub fn set_panel(&self, user: &str, open: bool) {
        let mut p = self.panel.lock().unwrap_or_else(|e| e.into_inner());
        if open {
            p.insert(user.to_string());
            return;
        }
        p.remove(user);
        drop(p);
        if !self.enabled(user) {
            self.bufs.lock().unwrap_or_else(|e| e.into_inner()).remove(user);
        }
    }

    /// The `seq` of the newest change in this person's buffers (0: nothing kept). Cheap: a panel asks it every heartbeat and
    /// copies the buffers ([`DevTools::snapshot`]) only when it changed.
    pub fn seq(&self, user: &str) -> u64 {
        self.bufs.lock().unwrap_or_else(|e| e.into_inner()).get(user).map_or(0, |b| b.seq)
    }

    /// A copy of the buffers for the page that is open (see [`Snapshot`]).
    pub fn snapshot(&self, user: &str) -> Snapshot {
        let m = self.bufs.lock().unwrap_or_else(|e| e.into_inner());
        let Some(b) = m.get(user) else { return Snapshot::default() };
        let cur = |r: &Ring<Tagged>| r.iter().filter(|t| t.page == b.current).cloned().collect::<Vec<_>>();
        let mut pages = cur(&b.pages);
        if pages.is_empty() {
            pages = b.pages.iter().cloned().collect();
        }
        Snapshot { seq: b.seq, page: b.current.clone(), probe_seen: b.stored > 0 || b.hello, failed: cur(&b.failed), slow: cur(&b.slow), console: cur(&b.console), pages }
    }

    fn buf<'a>(m: &'a mut HashMap<String, Buf>, user: &str) -> Option<&'a mut Buf> {
        if !m.contains_key(user) && m.len() >= MAX_USERS {
            return None;
        }
        Some(m.entry(user.to_string()).or_insert_with(Buf::new))
    }

    /// One line from the helper. Nothing is kept while the switch is off or when the line is not well formed. True when stored.
    pub fn record_line(&self, user: &str, line: &str) -> bool {
        // the probe's hello (`H`) carries nothing: it only proves the probe is in the engine
        if line.len() <= MAX_LINE && line.starts_with('H') && line[1..].starts_with(SEP) {
            if !self.collecting(user) {
                return false;
            }
            let mut m = self.bufs.lock().unwrap_or_else(|e| e.into_inner());
            return match Self::buf(&mut m, user) {
                Some(b) => {
                    if !b.hello {
                        b.hello = true;
                        b.seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
                    }
                    true
                }
                None => false,
            };
        }
        match parse_line(line) {
            Some(ev) => self.record(user, ev),
            None => false,
        }
    }

    pub fn record(&self, user: &str, ev: DevEvent) -> bool {
        if !self.collecting(user) {
            return false;
        }
        let mut m = self.bufs.lock().unwrap_or_else(|e| e.into_inner());
        let Some(b) = Self::buf(&mut m, user) else { return false };
        // a page timing names its own page; other events belong to the page that is open
        let page = match &ev {
            DevEvent::Page { url, .. } => url.clone(),
            _ => b.current.clone(),
        };
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        b.seq = seq;
        b.stored += 1;
        let t = Tagged { page, ev, seq, at: iyakki_clock::now_unix() };
        match t.ev {
            DevEvent::Failed { .. } => b.failed.push(t),
            DevEvent::Slow { .. } => b.slow.push(t),
            DevEvent::Console { .. } => b.console.push(t),
            DevEvent::Page { .. } => b.pages.push(t),
        }
        true
    }

    /// The page probe has reported at least once (a hello or any event) since collection began. When it has not, the buffers are
    /// empty because nothing can be collected, which says nothing about the page.
    pub fn probe_alive(&self, user: &str) -> bool {
        self.bufs.lock().unwrap_or_else(|e| e.into_inner()).get(user).is_some_and(|b| b.hello || b.stored > 0)
    }

    /// Counts for the current page.
    pub fn summary(&self, user: &str) -> Summary {
        let m = self.bufs.lock().unwrap_or_else(|e| e.into_inner());
        let Some(b) = m.get(user) else { return Summary::default() };
        Summary {
            failed: b.failed.iter().filter(|t| t.page == b.current).count(),
            console_errors: b.console.iter().filter(|t| t.page == b.current && matches!(t.ev, DevEvent::Console { level: Level::Error, .. })).count(),
        }
    }

    fn scope_matches(b: &Buf, t: &Tagged, scope: &Scope) -> bool {
        match scope {
            Scope::Current => t.page == b.current,
            Scope::All => true,
            Scope::Page(p) => t.page.to_ascii_lowercase().contains(&p.to_ascii_lowercase()),
        }
    }

    fn with<R>(&self, user: &str, f: impl FnOnce(&Buf) -> R) -> R {
        let mut m = self.bufs.lock().unwrap_or_else(|e| e.into_inner());
        let b = m.entry(user.to_string()).or_insert_with(Buf::new);
        f(b)
    }

    /// Failed requests, the same one only once with a count.
    pub fn failed_report(&self, user: &str, scope: &Scope, limit: usize) -> String {
        self.with(user, |b| {
            let mut groups: Vec<(String, u16, String, String, usize, u32)> = Vec::new(); // method, status, url, error, count, slowest ms
            for t in b.failed.iter().rev().filter(|t| Self::scope_matches(b, t, scope)) {
                if let DevEvent::Failed { method, status, ms, url, error } = &t.ev {
                    match groups.iter_mut().find(|g| g.0 == *method && g.1 == *status && g.2 == *url && g.3 == *error) {
                        Some(g) => {
                            g.4 += 1;
                            g.5 = g.5.max(*ms);
                        }
                        None => groups.push((method.clone(), *status, url.clone(), error.clone(), 1, *ms)),
                    }
                }
            }
            let head = scope_line(b, scope);
            if groups.is_empty() {
                return format!("{head}\nNo failed requests were seen{}.", scope.suffix());
            }
            let mut out = format!("{head}\n{} failed request{} (newest first):\n", groups.len(), if groups.len() == 1 { "" } else { "s" });
            for (i, (m, s, u, e, n, ms)) in groups.iter().take(limit).enumerate() {
                let what = if *s == 0 { if e.is_empty() { "no answer".to_string() } else { e.clone() } } else if e.is_empty() { format!("status {s}") } else { format!("status {s} {e}") };
                out.push_str(&format!("{}. {} {} - {}{}{}\n", i + 1, if m.is_empty() { "GET" } else { m }, u, what, if *ms > 0 { format!(", {ms} ms") } else { String::new() }, if *n > 1 { format!(", seen {n} times") } else { String::new() }));
            }
            if groups.len() > limit {
                out.push_str(&format!("... and {} more.\n", groups.len() - limit));
            }
            out
        })
    }

    /// Page timings and the slowest resources.
    pub fn timings_report(&self, user: &str, scope: &Scope, limit: usize) -> String {
        self.with(user, |b| {
            let head = scope_line(b, scope);
            let mut out = format!("{head}\n");
            let pages: Vec<&Tagged> = b.pages.iter().rev().filter(|t| Self::scope_matches(b, t, scope)).take(limit.min(5)).collect();
            if pages.is_empty() {
                out.push_str("No page load timing was reported yet (it arrives a moment after a page finishes loading).\n");
            }
            for t in pages {
                if let DevEvent::Page { response_ms, dcl_ms, load_ms, url } = &t.ev {
                    out.push_str(&format!("Page {url}: first response {response_ms} ms, DOMContentLoaded {dcl_ms} ms, load {load_ms} ms (all counted from navigation start).\n"));
                }
            }
            let mut slow: Vec<(&str, &str, u32, u64)> = Vec::new();
            for t in b.slow.iter().filter(|t| Self::scope_matches(b, t, scope)) {
                if let DevEvent::Slow { kind, bytes, ms, url } = &t.ev {
                    slow.push((url, kind, *ms, *bytes));
                }
            }
            // failed requests also have a time
            for t in b.failed.iter().filter(|t| Self::scope_matches(b, t, scope)) {
                if let DevEvent::Failed { ms, url, .. } = &t.ev {
                    if *ms >= 300 {
                        slow.push((url, "failed", *ms, 0));
                    }
                }
            }
            slow.sort_by(|a, b| b.2.cmp(&a.2));
            slow.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
            if slow.is_empty() {
                out.push_str("No resource took 300 ms or more.\n");
            } else {
                out.push_str(&format!("Slowest resources ({} took 300 ms or more):\n", slow.len()));
                for (i, (u, k, ms, bytes)) in slow.iter().take(limit).enumerate() {
                    out.push_str(&format!("{}. {} ms  {}  ({}{})\n", i + 1, ms, u, if k.is_empty() { "resource" } else { k }, if *bytes > 0 { format!(", {} bytes", bytes) } else { String::new() }));
                }
            }
            out
        })
    }

    /// Console errors and warnings, newest first.
    pub fn console_report(&self, user: &str, scope: &Scope, level: &str, limit: usize) -> String {
        self.with(user, |b| {
            let head = scope_line(b, scope);
            let want_warn = !matches!(level, "error" | "errors");
            let mut rows: Vec<String> = Vec::new();
            for t in b.console.iter().rev().filter(|t| Self::scope_matches(b, t, scope)) {
                if let DevEvent::Console { level, line, col, source, text } = &t.ev {
                    if *level == Level::Warn && !want_warn {
                        continue;
                    }
                    let at = if source.is_empty() { String::new() } else if *line > 0 { format!(" ({source}:{line}:{col})") } else { format!(" ({source})") };
                    rows.push(format!("{}: {}{}", level.word(), text, at));
                }
            }
            if rows.is_empty() {
                return format!("{head}\nNo console {} were seen{}.", if want_warn { "errors or warnings" } else { "errors" }, scope.suffix());
            }
            let mut out = format!("{head}\n{} console message{} (newest first):\n", rows.len(), if rows.len() == 1 { "" } else { "s" });
            for (i, r) in rows.iter().take(limit).enumerate() {
                out.push_str(&format!("{}. {}\n", i + 1, r));
            }
            if rows.len() > limit {
                out.push_str(&format!("... and {} more.\n", rows.len() - limit));
            }
            out
        })
    }
}

pub fn scope_line(b: &Buf, scope: &Scope) -> String {
    match scope {
        Scope::Current if b.current.is_empty() => "Page: (no page is open in the BROWSER tile)".to_string(),
        Scope::Current => format!("Page: {}", b.current),
        Scope::All => "All pages seen in this session (newest first)".to_string(),
        Scope::Page(p) => format!("Pages matching “{}”", cut_chars(p, 60)),
    }
}

/// Which page the tools look at.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Scope {
    Current,
    All,
    /// Pages whose address contains this text.
    Page(String),
}

impl Scope {
    fn suffix(&self) -> &'static str {
        match self {
            Scope::Current => " for this page",
            _ => "",
        }
    }
    pub fn from_args(args: &Value) -> Scope {
        let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty());
        if args.get("all_pages").and_then(Value::as_bool).unwrap_or(false) {
            Scope::All
        } else if let Some(p) = s("page") {
            Scope::Page(p.chars().take(120).collect())
        } else {
            Scope::Current
        }
    }
}

