//! Keyboard navigation over the accessibility tree ([`crate::a11y`]): the Tab order, the quick keys of "browse mode" (H next heading,
//! 1 to 6 by level, L link, F form field, B button, T table, R landmark; Shift goes back), arrow keys inside a radio group, the lists
//! a reader calls a rotor (links, headings, landmarks, form fields) with a search by typing, skip links and `#fragment` targets.
//! All pure functions: the browser tile keeps the position (a node index) and asks what comes next.

use crate::a11y::{A11yTree, Role};
use std::collections::HashSet;

/// What a quick key looks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quick {
    Heading,
    HeadingLevel(u8),
    Link,
    Field,
    Button,
    Table,
    Landmark,
    List,
    Image,
}

impl Quick {
    /// "heading", "link", ... for messages like "No more headings".
    pub fn plural(self) -> &'static str {
        match self {
            Quick::Heading => "headings",
            Quick::HeadingLevel(_) => "headings of that level",
            Quick::Link => "links",
            Quick::Field => "form fields",
            Quick::Button => "buttons",
            Quick::Table => "tables",
            Quick::Landmark => "landmarks",
            Quick::List => "lists",
            Quick::Image => "images",
        }
    }
}

/// The key to the search and the direction (`true`: backwards). Letters go forward in lower case and back in upper case; the digits
/// 1 to 6 go forward and the shifted digits back (`!` `@` `#` `$` `%` `^`).
pub fn quick_key(c: char) -> Option<(Quick, bool)> {
    let level = |d: char| d.to_digit(10).map(|v| Quick::HeadingLevel(v as u8));
    Some(match c {
        'h' => (Quick::Heading, false),
        'H' => (Quick::Heading, true),
        '1'..='6' => (level(c)?, false),
        '!' => (Quick::HeadingLevel(1), true),
        '@' => (Quick::HeadingLevel(2), true),
        '#' => (Quick::HeadingLevel(3), true),
        '$' => (Quick::HeadingLevel(4), true),
        '%' => (Quick::HeadingLevel(5), true),
        '^' => (Quick::HeadingLevel(6), true),
        'l' => (Quick::Link, false),
        'L' => (Quick::Link, true),
        'f' => (Quick::Field, false),
        'F' => (Quick::Field, true),
        'b' => (Quick::Button, false),
        'B' => (Quick::Button, true),
        't' => (Quick::Table, false),
        'T' => (Quick::Table, true),
        'r' => (Quick::Landmark, false),
        'R' => (Quick::Landmark, true),
        'i' => (Quick::List, false),
        'I' => (Quick::List, true),
        'g' => (Quick::Image, false),
        'G' => (Quick::Image, true),
        _ => return None,
    })
}

fn is_a(t: &A11yTree, i: usize, q: Quick) -> bool {
    let n = &t.nodes[i];
    match q {
        Quick::Heading => n.role == Role::Heading,
        Quick::HeadingLevel(l) => n.role == Role::Heading && n.level == l,
        Quick::Link => n.role == Role::Link,
        Quick::Field => t.is_field(i),
        Quick::Button => n.role == Role::Button,
        Quick::Table => n.role == Role::Table,
        Quick::Landmark => t.is_landmark(i),
        Quick::List => n.role == Role::List,
        Quick::Image => n.role == Role::Image,
    }
}

/// The next (or previous) node of that kind after `from` in document order, or `None` at the end.
pub fn quick(t: &A11yTree, from: Option<usize>, q: Quick, back: bool) -> Option<usize> {
    let n = t.nodes.len();
    if back {
        let end = from.unwrap_or(n).min(n);
        (0..end).rev().find(|&i| is_a(t, i, q))
    } else {
        let start = from.map_or(0, |f| f + 1);
        (start..n).find(|&i| is_a(t, i, q))
    }
}

/// Like [`quick`], but past the end it starts again from the other end. The flag says it wrapped.
pub fn quick_wrap(t: &A11yTree, from: Option<usize>, q: Quick, back: bool) -> Option<(usize, bool)> {
    if let Some(i) = quick(t, from, q, back) {
        return Some((i, false));
    }
    let again = quick(t, None, q, back)?;
    Some((again, true))
}

/// The nodes Tab visits: links with an address, enabled buttons and fields, summaries, custom widgets with a role; `tabindex` above 0
/// first, in its own order, then the rest in document order; `tabindex="-1"` is left out; of a group of radio buttons only the chosen
/// one (else the first) is a stop, the arrow keys move inside the group.
pub fn tab_order(t: &A11yTree) -> Vec<usize> {
    let mut seen_group: HashSet<(Option<usize>, String)> = HashSet::new();
    // a checked radio of each group is the stop
    let mut chosen: std::collections::HashMap<(Option<usize>, String), usize> = std::collections::HashMap::new();
    for (i, n) in t.nodes.iter().enumerate() {
        if n.role == Role::Radio && !n.group.is_empty() && !n.state.disabled {
            let k = (n.form, n.group.clone());
            if n.state.checked == Some(true) {
                chosen.insert(k, i);
            } else {
                chosen.entry(k).or_insert(i);
            }
        }
    }
    let mut positive: Vec<(i32, usize)> = Vec::new();
    let mut rest: Vec<usize> = Vec::new();
    for (i, n) in t.nodes.iter().enumerate() {
        if !n.focusable || n.tab_index.is_some_and(|x| x < 0) {
            continue;
        }
        if n.role == Role::Radio && !n.group.is_empty() {
            let k = (n.form, n.group.clone());
            if chosen.get(&k) != Some(&i) || !seen_group.insert(k) {
                continue;
            }
        }
        match n.tab_index {
            Some(x) if x > 0 => positive.push((x, i)),
            _ => rest.push(i),
        }
    }
    positive.sort_by_key(|(x, i)| (*x, *i));
    positive.into_iter().map(|(_, i)| i).chain(rest).collect()
}

/// Tab from `from` (a node, or nothing yet): the next stop, wrapping round the page. Shift+Tab with `back`.
pub fn tab_step(t: &A11yTree, from: Option<usize>, back: bool) -> Option<usize> {
    let order = tab_order(t);
    if order.is_empty() {
        return None;
    }
    // the position of the node in the order, or the stop just before/after it in document order
    let here = from.and_then(|f| order.iter().position(|&o| o == f));
    let n = order.len();
    Some(match (here, from, back) {
        (Some(h), _, false) => order[(h + 1) % n],
        (Some(h), _, true) => order[(h + n - 1) % n],
        (None, None, false) => order[0],
        (None, None, true) => order[n - 1],
        (None, Some(f), false) => order.iter().copied().find(|&o| o > f).unwrap_or(order[0]),
        (None, Some(f), true) => order.iter().copied().rev().find(|&o| o < f).unwrap_or(order[n - 1]),
    })
}

/// An arrow key on a radio button: the previous or next enabled button of its group, wrapping.
pub fn radio_step(t: &A11yTree, i: usize, back: bool) -> Option<usize> {
    let me = t.nodes.get(i)?;
    if me.role != Role::Radio {
        return None;
    }
    let group: Vec<usize> = (0..t.nodes.len()).filter(|&k| t.nodes[k].role == Role::Radio && t.nodes[k].form == me.form && t.nodes[k].group == me.group && !t.nodes[k].state.disabled).collect();
    let at = group.iter().position(|&g| g == i)?;
    let n = group.len();
    (n > 1).then(|| group[if back { (at + n - 1) % n } else { (at + 1) % n }])
}

/// The first field the page marks invalid (`aria-invalid`), where the focus goes after a form came back with errors.
pub fn first_invalid(t: &A11yTree) -> Option<usize> {
    (0..t.nodes.len()).find(|&i| t.is_field(i) && t.nodes[i].state.invalid)
}

/// A skip link: one of the first links of the page that goes to a place on the same page and says "skip" or names the main content.
pub fn skip_link(t: &A11yTree) -> Option<usize> {
    tab_order(t).into_iter().take(6).find(|&i| {
        let n = &t.nodes[i];
        if n.role != Role::Link || !n.href.starts_with('#') || n.href.len() < 2 {
            return false;
        }
        let w = n.name.to_lowercase();
        (w.contains("skip") || w.contains("jump to") || w.contains("main content")) && fragment_target(t, &n.href).is_some()
    })
}

/// Where a `#fragment` address points: the first node at or after the element with that id (the id may also be a node's own).
pub fn fragment_target(t: &A11yTree, href: &str) -> Option<usize> {
    let id = href.strip_prefix('#')?;
    t.ids.get(id).copied()
}

/// The main landmark, where "skip to main content" lands when the page has no skip link of its own.
pub fn main_landmark(t: &A11yTree) -> Option<usize> {
    (0..t.nodes.len()).find(|&i| matches!(t.nodes[i].role, Role::Landmark(crate::a11y::Landmark::Main)))
}

// ------------------------------------------------------------------------------------------------------------------ the rotor

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotor {
    Links,
    Headings,
    Landmarks,
    Fields,
}

impl Rotor {
    pub fn title(self) -> &'static str {
        match self {
            Rotor::Links => "Links",
            Rotor::Headings => "Headings",
            Rotor::Landmarks => "Landmarks",
            Rotor::Fields => "Form fields",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub node: usize,
    pub label: String,
    /// Indent steps (heading level 1 is 0).
    pub indent: u8,
}

pub fn entries(t: &A11yTree, r: Rotor) -> Vec<Entry> {
    let mut out = Vec::new();
    for (i, n) in t.nodes.iter().enumerate() {
        let (label, indent) = match r {
            Rotor::Links if n.role == Role::Link => (if n.name.is_empty() { n.href.clone() } else { n.name.clone() }, 0),
            Rotor::Headings if n.role == Role::Heading => (n.name.clone(), n.level.saturating_sub(1)),
            Rotor::Landmarks if t.is_landmark(i) => (t.speak(i), 0),
            Rotor::Fields if t.is_field(i) => (t.speak(i), 0),
            _ => continue,
        };
        out.push(Entry { node: i, label, indent });
    }
    out
}

/// The entries that match what was typed (letters in either case): those that start with it first, then those that contain it, in page
/// order within each. An empty query keeps everything. Returns indexes into `all`.
pub fn filter(all: &[Entry], query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return (0..all.len()).collect();
    }
    let lower: Vec<String> = all.iter().map(|e| e.label.to_lowercase()).collect();
    let starts = (0..all.len()).filter(|&i| lower[i].starts_with(&q));
    let has = (0..all.len()).filter(|&i| !lower[i].starts_with(&q) && lower[i].contains(&q));
    starts.chain(has).collect()
}
