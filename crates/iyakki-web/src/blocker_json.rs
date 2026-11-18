//! Turns compiled filter lists into WebKit's content-blocker rule list (the JSON format `WebKitUserContentFilterStore` loads),
//! so the full web engine blocks the same requests inside its own network stack. WebKit refuses a whole list when one rule is
//! invalid or when it has more than 150 000 rules, so this module only writes the regular-expression forms WebKit is known to
//! accept, drops what it cannot express (and counts it) and stays under a rule limit.
//!
//! Order matters in this format: later rules override earlier ones, so the output is: blocking rules, exceptions
//! (`ignore-previous-rules`), then `$important` blocking rules, then element hiding.

use crate::blocker::{Blocker, NetRule};

/// WebKit's own limit on rules in one list.
pub const WEBKIT_MAX_RULES: usize = 150_000;
/// What this program writes by default: below WebKit's limit so a Mac's network process is not asked to compile the most it
/// accepts (compile time and memory in WebKit's rule compiler were not measured on the Macs).
pub const DEFAULT_LIMIT: usize = 100_000;

/// The result of an export.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JsonList {
    pub json: String,
    pub rules: usize,
    /// Network rules this format cannot say (both include and exclude domains, non-ASCII, ...).
    pub unsupported: usize,
    /// Rules left out because of the limit.
    pub over_limit: usize,
}

/// Escape for a JSON string.
pub fn json_escape(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn regex_escape(c: char, out: &mut String) {
    if matches!(c, '\\' | '.' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' | '*') {
        out.push('\\');
    }
    out.push(c);
}

/// The `url-filter` regular expression for a rule's pattern; `None` for text the format cannot hold.
pub fn url_filter(r: &NetRule) -> Option<String> {
    if !r.pattern.is_ascii() {
        return None;
    }
    let mut s = String::with_capacity(r.pattern.len() + 40);
    if r.left_domain {
        s.push_str("^[^:]+://+([^:/]+\\.)?");
    } else if r.left {
        s.push('^');
    }
    for c in r.pattern.chars() {
        match c {
            '*' => s.push_str(".*"),
            '^' => s.push_str("[^a-z0-9_.%-]"),
            c => regex_escape(c, &mut s),
        }
    }
    if r.right {
        s.push('$');
    }
    if s.len() > 1500 {
        return None;
    }
    Some(s)
}

/// Which part of a rule's types a JSON rule carries: the ordinary types, or frame loads (which need a load-context).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Rest,
    Frame,
}

/// The types a rule means: with none written, every type but the page and frames (frames are only blocked by rules that say
/// `$subdocument`: WebKit cannot say "frame loads and scripts" in one rule, and doubling every rule would not fit its limit).
fn rule_mask(r: &NetRule) -> u16 {
    use crate::blocker::ResType as T;
    if r.types == 0 {
        T::Stylesheet.bit() | T::Script.bit() | T::Image.bit() | T::Font.bit() | T::Media.bit() | T::Xhr.bit() | T::Ping.bit() | T::Websocket.bit() | T::Other.bit()
    } else {
        r.types
    }
}

/// `"resource-type"`, `"load-type"` and domain conditions for a rule as the inside of a trigger object (after `url-filter`).
fn trigger_rest(r: &NetRule, part: Part, out: &mut String) -> bool {
    use crate::blocker::ResType as T;
    let mask = rule_mask(r);
    let mut types: Vec<&str> = Vec::new();
    if part == Part::Frame {
        types.push("document");
    } else {
        for (t, names) in [
            (T::Document, &["document"][..]),
            (T::Stylesheet, &["style-sheet"][..]),
            (T::Script, &["script"][..]),
            (T::Image, &["image", "svg-document"][..]),
            (T::Font, &["font"][..]),
            (T::Media, &["media"][..]),
            (T::Xhr, &["raw", "fetch"][..]),
            (T::Ping, &["ping"][..]),
            (T::Websocket, &["websocket"][..]),
            (T::Other, &["other"][..]),
        ] {
            if mask & t.bit() != 0 {
                types.extend(names.iter().copied());
            }
        }
    }
    out.push_str(",\"resource-type\":[");
    for (i, t) in types.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(t);
        out.push('"');
    }
    out.push(']');
    if part == Part::Frame {
        out.push_str(",\"load-context\":[\"child-frame\"]");
    }
    match r.party {
        1 => out.push_str(",\"load-type\":[\"third-party\"]"),
        2 => out.push_str(",\"load-type\":[\"first-party\"]"),
        _ => {}
    }
    if let Some(d) = &r.domains {
        // WebKit allows one kind of domain condition per rule
        if !d.include.is_empty() && !d.exclude.is_empty() {
            return false;
        }
        let (key, list) = if d.include.is_empty() { ("unless-domain", &d.exclude) } else { ("if-domain", &d.include) };
        if list.iter().any(|x| !x.is_ascii() || x.contains(['"', '\\'])) {
            return false;
        }
        out.push_str(&format!(",\"{key}\":["));
        for (i, x) in list.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            json_escape(&format!("*{x}"), out);
        }
        out.push(']');
    }
    true
}

/// The JSON rules for one network rule (one, or two when it names both frame loads and other types). False: not expressible.
fn rule_json(r: &NetRule, action: &str, out: &mut String) -> bool {
    use crate::blocker::ResType as T;
    let Some(f) = url_filter(r) else { return false };
    let mask = rule_mask(r);
    let mut parts = Vec::new();
    if mask & !T::Subdocument.bit() != 0 {
        parts.push(Part::Rest);
    }
    if mask & T::Subdocument.bit() != 0 {
        parts.push(Part::Frame);
    }
    let mark = out.len();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"trigger\":{\"url-filter\":");
        json_escape(&f, out);
        if !trigger_rest(r, *part, out) {
            out.truncate(mark);
            return false;
        }
        out.push_str("},\"action\":");
        out.push_str(action);
        out.push('}');
    }
    !parts.is_empty()
}

/// A rule-list for `blocker`. `limit` is the most rules written (at most [`WEBKIT_MAX_RULES`]); `cosmetic` also writes the
/// element-hiding rules.
pub fn export(b: &Blocker, limit: usize, cosmetic: bool) -> JsonList {
    export_allowing(b, limit, cosmetic, &[])
}

/// Like [`export`], with a last rule per allowed site that cancels everything before it for pages on that site (the person's
/// "allow on this site" list; WebKit cannot be told about a change without loading the list again).
pub fn export_allowing(b: &Blocker, limit: usize, cosmetic: bool, allow: &[String]) -> JsonList {
    let limit = limit.clamp(1, WEBKIT_MAX_RULES);
    let mut res = JsonList::default();
    let mut body = String::with_capacity(1 << 20);
    let mut count = 0usize;
    let mut first = true;
    let rules = b.net_rules();
    let n_exc = rules.iter().filter(|r| r.exception).count();
    let n_imp = rules.iter().filter(|r| r.important && !r.exception).count();
    // room for blocking rules: what is left after exceptions, important rules and a share for hiding rules
    let cos_budget = if cosmetic { (limit / 10).min(b.generic_selectors().len() / 20 + b.domain_cosmetics().len() + 1) } else { 0 };
    let block_room = limit.saturating_sub(n_exc.min(limit / 4) + n_imp.min(limit / 8) + cos_budget);
    let emit = |s: &str, body: &mut String, first: &mut bool| {
        if !*first {
            body.push(',');
        }
        *first = false;
        body.push_str(s);
    };
    let mut tmp = String::new();
    let mut blocks = 0usize;
    // when the limit bites, whole-domain rules (`||host^`, the ones that remove the most) are kept before path and word rules
    let ordered = rules.iter().filter(|r| !r.exception && !r.important && r.left_domain).chain(rules.iter().filter(|r| !r.exception && !r.important && !r.left_domain));
    for r in ordered {
        if blocks >= block_room {
            res.over_limit += 1;
            continue;
        }
        tmp.clear();
        if rule_json(r, "{\"type\":\"block\"}", &mut tmp) {
            emit(&tmp, &mut body, &mut first);
            blocks += 1;
            count += 1;
        } else {
            res.unsupported += 1;
        }
    }
    let mut excs = 0usize;
    for r in rules.iter().filter(|r| r.exception) {
        if excs >= limit / 4 {
            res.over_limit += 1;
            continue;
        }
        tmp.clear();
        if rule_json(r, "{\"type\":\"ignore-previous-rules\"}", &mut tmp) {
            emit(&tmp, &mut body, &mut first);
            excs += 1;
            count += 1;
        } else {
            res.unsupported += 1;
        }
    }
    let mut imps = 0usize;
    for r in rules.iter().filter(|r| r.important && !r.exception) {
        if imps >= limit / 8 {
            res.over_limit += 1;
            continue;
        }
        tmp.clear();
        if rule_json(r, "{\"type\":\"block\"}", &mut tmp) {
            emit(&tmp, &mut body, &mut first);
            imps += 1;
            count += 1;
        } else {
            res.unsupported += 1;
        }
    }
    if cosmetic {
        // generic selectors in groups (the engine takes a selector list), then rules for named sites one by one
        let mut left = cos_budget;
        for chunk in b.generic_selectors().chunks(20) {
            if left == 0 {
                break;
            }
            tmp.clear();
            tmp.push_str("{\"trigger\":{\"url-filter\":\".*\"},\"action\":{\"type\":\"css-display-none\",\"selector\":");
            json_escape(&chunk.join(","), &mut tmp);
            tmp.push_str("}}");
            emit(&tmp, &mut body, &mut first);
            count += 1;
            left -= 1;
        }
        for c in b.domain_cosmetics() {
            if left == 0 {
                break;
            }
            let Some(d) = &c.domains else { continue };
            if d.include.is_empty() || !d.exclude.is_empty() || d.include.iter().any(|x| !x.is_ascii()) {
                continue;
            }
            tmp.clear();
            tmp.push_str("{\"trigger\":{\"url-filter\":\".*\",\"if-domain\":[");
            for (i, x) in d.include.iter().enumerate() {
                if i > 0 {
                    tmp.push(',');
                }
                json_escape(&format!("*{x}"), &mut tmp);
            }
            tmp.push_str("]},\"action\":{\"type\":\"css-display-none\",\"selector\":");
            json_escape(&c.selector, &mut tmp);
            tmp.push_str("}}");
            emit(&tmp, &mut body, &mut first);
            count += 1;
            left -= 1;
        }
    }
    for host in allow.iter().take(500) {
        if host.is_empty() || !host.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_')) {
            continue;
        }
        tmp.clear();
        tmp.push_str("{\"trigger\":{\"url-filter\":\".*\",\"if-domain\":[");
        json_escape(&format!("*{}", host.to_ascii_lowercase()), &mut tmp);
        tmp.push_str("]},\"action\":{\"type\":\"ignore-previous-rules\"}}");
        emit(&tmp, &mut body, &mut first);
        count += 1;
    }
    res.rules = count;
    res.json = format!("[{body}]");
    res
}

/// Does `s` look like a rule list this module wrote (a JSON array of objects)? A cheap check before handing a file to WebKit.
pub fn looks_valid(s: &str) -> bool {
    let t = s.trim();
    t.starts_with('[') && t.ends_with(']')
}
