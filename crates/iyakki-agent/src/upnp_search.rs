//! The `Search` action of the ContentDirectory service (F-MEDIA-02): a player types a word and the server answers with the matching
//! entries. The criteria language of the UPnP ContentDirectory:1 specification is read here: relations (`dc:title contains "x"`,
//! `upnp:class derivedfrom "object.item.audioItem"`, `=`, `!=`, `<`, `<=`, `>`, `>=`, `doesNotContain`, `exists`), `and`, `or`,
//! brackets, and `*` for everything. Hostile criteria (very long, deeply nested, too many terms) are refused with the standard error
//! 708 and never followed.

use crate::upnp::{didl, soap_response, Action, Catalog, Entry, CD_TYPE, MAX_PAGE};

/// What  says.
pub const CAPABILITIES: &str = "dc:title,upnp:class,upnp:artist,upnp:album,@id,@parentID,res@size";

const MAX_CRITERIA: usize = 1024;
const MAX_TOKENS: usize = 96;
const MAX_NESTING: usize = 8;
/// Most entries looked at in one search, and the deepest folder.
const MAX_VISITED: usize = 5000;
const MAX_FOLDER_DEPTH: usize = 6;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Contains,
    NotContains,
    Derived,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    All,
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Rel(String, Op, String),
    Exists(String, bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Open,
    Close,
    Word(String),
    Quoted(String),
}

fn tokens(s: &str) -> Option<Vec<Tok>> {
    let mut out = Vec::new();
    let mut it = s.chars().peekable();
    while let Some(&c) = it.peek() {
        if out.len() > MAX_TOKENS {
            return None;
        }
        match c {
            c if c.is_whitespace() => {
                it.next();
            }
            '(' => {
                it.next();
                out.push(Tok::Open);
            }
            ')' => {
                it.next();
                out.push(Tok::Close);
            }
            '"' => {
                it.next();
                let mut v = String::new();
                loop {
                    match it.next()? {
                        '\\' => v.push(it.next()?),
                        '"' => break,
                        ch => v.push(ch),
                    }
                }
                out.push(Tok::Quoted(v));
            }
            _ => {
                let mut w = String::new();
                while let Some(&ch) = it.peek() {
                    if ch.is_whitespace() || ch == '(' || ch == ')' || ch == '"' {
                        break;
                    }
                    w.push(ch);
                    it.next();
                }
                out.push(Tok::Word(w));
            }
        }
    }
    Some(out)
}

struct P {
    t: Vec<Tok>,
    i: usize,
}

impl P {
    fn word(&mut self) -> Option<String> {
        match self.t.get(self.i)? {
            Tok::Word(w) => {
                self.i += 1;
                Some(w.clone())
            }
            _ => None,
        }
    }
    fn is_word(&self, w: &str) -> bool {
        matches!(self.t.get(self.i), Some(Tok::Word(x)) if x.eq_ignore_ascii_case(w))
    }
    fn or(&mut self, depth: usize) -> Option<Expr> {
        let mut left = self.and(depth)?;
        while self.is_word("or") {
            self.i += 1;
            let right = self.and(depth)?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Some(left)
    }
    fn and(&mut self, depth: usize) -> Option<Expr> {
        let mut left = self.primary(depth)?;
        while self.is_word("and") {
            self.i += 1;
            let right = self.primary(depth)?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Some(left)
    }
    fn primary(&mut self, depth: usize) -> Option<Expr> {
        if depth > MAX_NESTING {
            return None;
        }
        if self.t.get(self.i) == Some(&Tok::Open) {
            self.i += 1;
            let e = self.or(depth + 1)?;
            if self.t.get(self.i) != Some(&Tok::Close) {
                return None;
            }
            self.i += 1;
            return Some(e);
        }
        let prop = self.word()?;
        let op = self.word()?;
        if op.eq_ignore_ascii_case("exists") {
            let v = self.word()?;
            return match v.to_ascii_lowercase().as_str() {
                "true" => Some(Expr::Exists(prop, true)),
                "false" => Some(Expr::Exists(prop, false)),
                _ => None,
            };
        }
        let op = match op.to_ascii_lowercase().as_str() {
            "=" => Op::Eq,
            "!=" => Op::Ne,
            "<" => Op::Lt,
            "<=" => Op::Le,
            ">" => Op::Gt,
            ">=" => Op::Ge,
            "contains" => Op::Contains,
            "doesnotcontain" => Op::NotContains,
            "derivedfrom" => Op::Derived,
            _ => return None,
        };
        match self.t.get(self.i)? {
            Tok::Quoted(v) => {
                let v = v.clone();
                self.i += 1;
                Some(Expr::Rel(prop, op, v))
            }
            _ => None,
        }
    }
}

/// Read search criteria; `None` when they are not valid (error 708).
pub fn parse(criteria: &str) -> Option<Expr> {
    let c = criteria.trim();
    if c.len() > MAX_CRITERIA {
        return None;
    }
    if c == "*" {
        return Some(Expr::All);
    }
    let mut p = P { t: tokens(c)?, i: 0 };
    let e = p.or(0)?;
    (p.i == p.t.len()).then_some(e)
}

/// The value of a property of an entry, `None` when the entry has none.
fn value(e: &Entry, prop: &str) -> Option<String> {
    let nonempty = |s: &String| (!s.is_empty()).then(|| s.clone());
    match (prop, e) {
        ("@id", _) => Some(e.id().to_string()),
        ("@parentID", Entry::Container { parent, .. } | Entry::Item { parent, .. }) => Some(parent.clone()),
        ("dc:title", Entry::Container { title, .. } | Entry::Item { title, .. }) => Some(title.clone()),
        ("upnp:class", Entry::Container { .. }) => Some("object.container.storageFolder".into()),
        ("upnp:class", Entry::Item { class, .. }) => Some(class.clone()),
        ("upnp:artist", Entry::Item { artist, .. }) => nonempty(artist),
        ("upnp:album", Entry::Item { album, .. }) => nonempty(album),
        ("res@size", Entry::Item { size, .. }) => Some(size.to_string()),
        ("res@duration", Entry::Item { seconds, .. }) if *seconds > 0 => Some(format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60)),
        _ => None,
    }
}

fn rel(have: &str, op: &Op, want: &str) -> bool {
    let (h, w) = (have.to_lowercase(), want.to_lowercase());
    match op {
        Op::Eq => h == w,
        Op::Ne => h != w,
        Op::Contains => h.contains(&w),
        Op::NotContains => !h.contains(&w),
        Op::Derived => h == w || h.starts_with(&format!("{w}.")),
        Op::Lt | Op::Le | Op::Gt | Op::Ge => {
            let ord = match (have.parse::<f64>(), want.parse::<f64>()) {
                (Ok(a), Ok(b)) => a.partial_cmp(&b),
                _ => Some(h.cmp(&w)),
            };
            match (ord, op) {
                (Some(o), Op::Lt) => o.is_lt(),
                (Some(o), Op::Le) => o.is_le(),
                (Some(o), Op::Gt) => o.is_gt(),
                (Some(o), Op::Ge) => o.is_ge(),
                _ => false,
            }
        }
    }
}

pub fn matches(e: &Expr, entry: &Entry) -> bool {
    match e {
        Expr::All => true,
        Expr::And(a, b) => matches(a, entry) && matches(b, entry),
        Expr::Or(a, b) => matches(a, entry) || matches(b, entry),
        Expr::Exists(p, want) => value(entry, p).is_some() == *want,
        // a property the entry does not have never matches, not even `!=` or `doesNotContain`
        Expr::Rel(p, op, v) => value(entry, p).is_some_and(|h| rel(&h, op, v)),
    }
}

/// Everything below a container (not the container itself), folders before the files in them, as far as the limits allow.
fn below(cat: &dyn Catalog, id: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut queue = vec![(id.to_string(), 0usize)];
    while let Some((cid, depth)) = queue.pop() {
        let mut start = 0u32;
        loop {
            let Some((page, total)) = cat.children(&cid, start, MAX_PAGE) else { break };
            let got = page.len() as u32;
            for e in page {
                if let Entry::Container { id, .. } = &e {
                    if depth < MAX_FOLDER_DEPTH {
                        queue.push((id.clone(), depth + 1));
                    }
                }
                out.push(e);
                if out.len() >= MAX_VISITED {
                    return out;
                }
            }
            start += got;
            if got == 0 || start >= total {
                break;
            }
        }
    }
    out
}

/// Answer a `Search` action: `Err((code, text))` is a UPnP error for the caller to turn into a fault.
pub fn search(cat: &dyn Catalog, a: &Action) -> Result<String, (u16, &'static str)> {
    let id = a.args.get("ContainerID").map(String::as_str).unwrap_or("0");
    let crit = parse(a.args.get("SearchCriteria").map(String::as_str).unwrap_or("*")).ok_or((708, "Unsupported or invalid search criteria"))?;
    if !matches!(cat.get(id), Some(Entry::Container { .. })) {
        return Err((710, "No such container"));
    }
    let start = a.args.get("StartingIndex").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
    let want = a.args.get("RequestedCount").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
    let count = if want == 0 { MAX_PAGE as usize } else { want.min(MAX_PAGE as usize) };
    let found: Vec<Entry> = below(cat, id).into_iter().filter(|e| matches(&crit, e)).collect();
    let total = found.len();
    let page: Vec<Entry> = found.into_iter().skip(start).take(count).collect();
    Ok(soap_response(CD_TYPE, "Search", &[("Result", didl(&page)), ("NumberReturned", page.len().to_string()), ("TotalMatches", total.to_string()), ("UpdateID", cat.update_id().to_string())]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upnp::{ListCatalog, Media};
    use std::collections::BTreeMap;

    fn cat() -> ListCatalog {
        let m = |t: &str, ar: &str, v: bool, size: u64| Media { title: t.into(), artist: ar.into(), album: "Al".into(), video: v, mime: if v { "video/mp4" } else { "audio/mpeg" }.into(), size, seconds: 90 };
        ListCatalog { items: vec![m("Blue Moon", "Ada", false, 100), m("Film Night", "", true, 5000), m("Red Moon", "Bo", false, 300)], update: 3, url_for: Box::new(|id| format!("http://10.0.0.5/upnp/media/{id}")) }
    }

    fn act(container: &str, crit: &str, extra: &[(&str, &str)]) -> Action {
        let mut args: BTreeMap<String, String> = [("ContainerID", container), ("SearchCriteria", crit)].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        args.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Action { name: "Search".into(), args }
    }

    fn ids(r: &str) -> Vec<String> {
        r.split("id=&quot;").skip(1).map(|s| s.split("&quot;").next().unwrap().to_string()).collect()
    }

    #[test]
    fn titles_classes_and_artists_are_found_with_and_or_and_brackets() {
        let c = cat();
        let q = |crit: &str| ids(&search(&c, &act("0", crit, &[])).unwrap());
        assert_eq!(q("dc:title contains \"moon\""), vec!["a0", "a2"], "case does not matter");
        assert_eq!(q("dc:title contains \"moon\" and upnp:artist = \"Bo\""), vec!["a2"]);
        assert_eq!(q("upnp:class derivedfrom \"object.item.videoItem\""), vec!["v1"]);
        assert_eq!(q("upnp:class derivedfrom \"object.item.audioItem\" and (dc:title contains \"red\" or dc:title contains \"blue\")"), vec!["a0", "a2"]);
        assert_eq!(q("upnp:artist exists false and upnp:class derivedfrom \"object.item\""), vec!["v1"]);
        assert_eq!(q("res@size >= \"300\""), vec!["v1", "a2"]);
        assert_eq!(q("dc:title doesNotContain \"moon\" and upnp:class derivedfrom \"object.item\""), vec!["v1"]);
        assert_eq!(q("upnp:album = \"nothing\""), Vec::<String>::new());
        // a property the entry has not got never matches
        assert_eq!(q("upnp:artist != \"Ada\" and upnp:class derivedfrom \"object.item\""), vec!["a2"]);
    }

    #[test]
    fn everything_below_a_folder_and_pages_work() {
        let c = cat();
        let all = search(&c, &act("0", "*", &[])).unwrap();
        assert!(all.contains("TotalMatches"), "{all}");
        assert_eq!(ids(&all).len(), 5, "two folders and three files: {all}");
        let music = ids(&search(&c, &act("1", "*", &[])).unwrap());
        assert_eq!(music, vec!["a0", "a2"]);
        let page = search(&c, &act("0", "upnp:class derivedfrom \"object.item\"", &[("StartingIndex", "1"), ("RequestedCount", "1")])).unwrap();
        assert_eq!(ids(&page).len(), 1);
        assert!(page.contains("<TotalMatches>3</TotalMatches>"), "{page}");
    }

    #[test]
    fn bad_criteria_and_unknown_containers_give_the_standard_errors() {
        let c = cat();
        for bad in ["dc:title contains moon", "dc:title like \"x\"", "(dc:title contains \"x\"", "dc:title contains \"x\" and", "dc:title contains \"unterminated", "dc:title exists maybe", ")"] {
            assert_eq!(search(&c, &act("0", bad, &[])).unwrap_err().0, 708, "{bad}");
        }
        assert_eq!(search(&c, &act("9", "*", &[])).unwrap_err().0, 710);
        assert_eq!(search(&c, &act("a0", "*", &[])).unwrap_err().0, 710, "an item is not a container");
        let deep = format!("{}dc:title contains \"x\"{}", "(".repeat(40), ")".repeat(40));
        assert_eq!(search(&c, &act("0", &deep, &[])).unwrap_err().0, 708);
        let long = format!("dc:title contains \"{}\"", "x".repeat(2000));
        assert_eq!(search(&c, &act("0", &long, &[])).unwrap_err().0, 708);
        let many = vec!["dc:title contains \"x\""; 60].join(" or ");
        assert_eq!(search(&c, &act("0", &many, &[])).unwrap_err().0, 708);
    }

    #[test]
    fn hostile_text_never_panics() {
        let c = cat();
        let mut seed = 7u64;
        for _ in 0..400 {
            let mut s = String::new();
            for _ in 0..(seed % 40) {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let alphabet = b"()\"\\ aandor=<>!*.@:x";
                s.push(alphabet[((seed >> 33) as usize) % alphabet.len()] as char);
            }
            let _ = search(&c, &act("0", &s, &[]));
        }
    }
}
