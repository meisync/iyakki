//! The inspector behind the browser's Developer tools panel (F-BROW-41): a console prompt that evaluates an expression in the built-in
//! reader's script engine, and an Elements page with the page's tree, the chosen node's attributes and its text.
//!
//! This file holds only data and rules (no engine, no drawing): the transcript of the prompt, the input line with its history, and the
//! tree as rows that can be opened and closed. The browser (`browser_inspecthost.rs`) owns the engine and fills these in; the panel
//! (`browser_devpanel.rs`) draws them. What an expression returns is shown on the screen of the person who typed it and is never
//! stored, never sent to the chat agent's buffers and never copied into the panel's text report.

use std::collections::HashSet;

use iyakki_web::html::{NodeKind, Tree};

/// Most lines the transcript keeps.
pub const MAX_LINES: usize = 400;
/// Most characters of one expression, and of one shown result line.
pub const MAX_INPUT: usize = 2000;
pub const MAX_SHOWN: usize = 600;
/// Most remembered expressions.
const MAX_HISTORY: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// What was typed.
    In,
    /// The value it gave.
    Out,
    /// An error, with where it happened.
    Err,
    /// A message the expression printed (`console.log`).
    Log,
    /// A note from the inspector itself.
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Out {
    pub kind: Kind,
    pub text: String,
}

/// One visible row of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub node: usize,
    pub depth: usize,
    pub label: String,
    pub has_kids: bool,
    pub open: bool,
}

#[derive(Default)]
pub struct Inspector {
    pub lines: Vec<Out>,
    pub input: String,
    pub editing: bool,
    history: Vec<String>,
    recall: Option<usize>,
    tree: Option<Tree>,
    /// Where the tree came from: the page as the script engine has it now, or as the server sent it.
    pub live: bool,
    open: HashSet<usize>,
    /// The chosen row of [`Inspector::rows`].
    pub sel: usize,
    pub scroll: usize,
    /// An expression is waiting for the engine to start.
    pub waiting: bool,
}

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut o: String = s.chars().take(n.saturating_sub(1)).collect();
        o.push('…');
        o
    }
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Inspector {
    // ------------------------------------------------------------------------------------------ the prompt

    pub fn push(&mut self, kind: Kind, text: impl Into<String>) {
        for l in text.into().lines().take(40) {
            self.lines.push(Out { kind, text: cut(l, MAX_SHOWN) });
        }
        if self.lines.len() > MAX_LINES {
            let extra = self.lines.len() - MAX_LINES;
            self.lines.drain(..extra);
        }
    }

    /// A key while the prompt has the keyboard. `Some(expression)` when Enter sent one.
    pub fn prompt_key(&mut self, k: &crate::ui::Key) -> Option<String> {
        use crate::ui::Key;
        match k {
            Key::Esc => self.editing = false,
            Key::Enter => {
                let e = self.input.trim().to_string();
                self.input.clear();
                self.recall = None;
                if e.is_empty() {
                    return None;
                }
                if self.history.last() != Some(&e) {
                    self.history.push(e.clone());
                    if self.history.len() > MAX_HISTORY {
                        self.history.remove(0);
                    }
                }
                return Some(e);
            }
            Key::Backspace => {
                self.input.pop();
            }
            Key::Up => self.recall_step(true),
            Key::Down => self.recall_step(false),
            Key::Char(c) if !c.is_control() && self.input.chars().count() < MAX_INPUT => self.input.push(*c),
            _ => {}
        }
        None
    }

    pub fn paste(&mut self, t: &str) {
        for c in t.chars().filter(|c| !c.is_control()) {
            if self.input.chars().count() < MAX_INPUT {
                self.input.push(c);
            }
        }
    }

    fn recall_step(&mut self, older: bool) {
        if self.history.is_empty() {
            return;
        }
        let n = self.history.len();
        let next = match (self.recall, older) {
            (None, true) => Some(n - 1),
            (Some(i), true) => Some(i.saturating_sub(1)),
            (Some(i), false) if i + 1 < n => Some(i + 1),
            _ => None,
        };
        self.recall = next;
        self.input = next.map(|i| self.history[i].clone()).unwrap_or_default();
    }

    /// The outcome of an expression: its value, or its error with the first places the engine named.
    pub fn show_result(&mut self, r: Result<String, String>) {
        match r {
            Ok(v) => self.push(Kind::Out, if v.is_empty() { "(no value)".to_string() } else { v }),
            Err(e) => {
                let mut it = e.lines();
                let head = it.next().unwrap_or("error").trim().to_string();
                self.push(Kind::Err, head);
                // "    at name (file:line:col)": where it happened, a few frames
                for f in it.map(str::trim).filter(|l| !l.is_empty()).take(4) {
                    self.push(Kind::Err, format!("   {f}"));
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------ the tree

    /// Show this tree (the page as the engine has it, or as the server sent it). The first levels are open; the choice is kept
    /// when the same node is still there.
    pub fn set_tree(&mut self, tree: Tree, live: bool) {
        let first = self.tree.is_none() || self.live != live;
        self.live = live;
        self.tree = Some(tree);
        if first || self.open.is_empty() {
            self.open_first_levels(3);
        }
        let n = self.rows().len();
        self.sel = self.sel.min(n.saturating_sub(1));
    }

    pub fn clear_tree(&mut self) {
        self.tree = None;
        self.open.clear();
        self.sel = 0;
        self.scroll = 0;
        self.live = false;
    }

    pub fn has_tree(&self) -> bool {
        self.tree.is_some()
    }

    fn open_first_levels(&mut self, levels: usize) {
        self.open.clear();
        let Some(t) = &self.tree else { return };
        let mut todo = vec![(0usize, 0usize)];
        while let Some((n, d)) = todo.pop() {
            if d < levels {
                self.open.insert(n);
                todo.extend(t.nodes[n].children.iter().map(|&c| (c, d + 1)));
            }
        }
    }

    fn label(t: &Tree, n: usize) -> String {
        match &t.nodes[n].kind {
            NodeKind::Text(s) => format!("\"{}\"", cut(&squash(s), 60)),
            NodeKind::Element { tag, attrs } => {
                let mut l = format!("<{tag}");
                if let Some((_, v)) = attrs.iter().find(|(k, _)| k == "id") {
                    l.push_str(&format!(" id=\"{}\"", cut(v, 30)));
                }
                if let Some((_, v)) = attrs.iter().find(|(k, _)| k == "class") {
                    l.push_str(&format!(" class=\"{}\"", cut(&squash(v), 30)));
                }
                l.push('>');
                l
            }
        }
    }

    fn shown_child(t: &Tree, c: usize) -> bool {
        match &t.nodes[c].kind {
            NodeKind::Text(s) => !s.trim().is_empty(),
            NodeKind::Element { .. } => true,
        }
    }

    /// The rows that are visible now, in document order.
    pub fn rows(&self) -> Vec<Row> {
        let Some(t) = &self.tree else { return Vec::new() };
        let mut out = Vec::new();
        let mut todo: Vec<(usize, usize)> = t.nodes[0].children.iter().rev().filter(|&&c| Self::shown_child(t, c)).map(|&c| (c, 0)).collect();
        while let Some((n, d)) = todo.pop() {
            let kids: Vec<usize> = t.nodes[n].children.iter().copied().filter(|&c| Self::shown_child(t, c)).collect();
            let open = self.open.contains(&n);
            out.push(Row { node: n, depth: d, label: Self::label(t, n), has_kids: !kids.is_empty(), open });
            if open {
                todo.extend(kids.iter().rev().map(|&c| (c, d + 1)));
            }
            if out.len() >= 5000 {
                break;
            }
        }
        out
    }

    /// Keys on the Elements page: Up and Down choose, Right opens (or goes in), Left closes (or goes out), Enter toggles.
    pub fn tree_key(&mut self, k: &crate::ui::Key) {
        use crate::ui::Key;
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let sel = self.sel.min(rows.len() - 1);
        match k {
            Key::Up => self.sel = sel.saturating_sub(1),
            Key::Down => self.sel = (sel + 1).min(rows.len() - 1),
            Key::PageUp => self.sel = sel.saturating_sub(8),
            Key::PageDown => self.sel = (sel + 8).min(rows.len() - 1),
            Key::Home => self.sel = 0,
            Key::End => self.sel = rows.len() - 1,
            Key::Right => {
                let r = &rows[sel];
                if r.has_kids && !r.open {
                    self.open.insert(r.node);
                } else if r.has_kids && sel + 1 < rows.len() {
                    self.sel = sel + 1;
                }
            }
            Key::Left => {
                let r = &rows[sel];
                if r.has_kids && r.open {
                    self.open.remove(&r.node);
                } else if let Some(p) = (0..sel).rev().find(|&i| rows[i].depth + 1 == r.depth) {
                    self.sel = p;
                }
            }
            Key::Enter => {
                let r = &rows[sel];
                if r.has_kids {
                    if r.open {
                        self.open.remove(&r.node);
                    } else {
                        self.open.insert(r.node);
                    }
                }
            }
            _ => {}
        }
    }

    /// A click on row `i` of the visible rows: chooses it, and a second click on a chosen row toggles it.
    pub fn pick(&mut self, i: usize) {
        let rows = self.rows();
        if i >= rows.len() {
            return;
        }
        if self.sel == i && rows[i].has_kids {
            self.tree_key(&crate::ui::Key::Enter);
        }
        self.sel = i;
    }

    /// What is known of the chosen node: its tag, attributes and text, as lines. Text is what a person would read there (the words
    /// under it with the spaces squeezed), cut to a screenful.
    pub fn detail(&self) -> Vec<String> {
        let (Some(t), rows) = (&self.tree, self.rows()) else { return Vec::new() };
        let Some(r) = rows.get(self.sel.min(rows.len().saturating_sub(1))) else { return Vec::new() };
        let mut out = Vec::new();
        match &t.nodes[r.node].kind {
            NodeKind::Text(s) => {
                out.push("text node".to_string());
                out.push(format!("text: {}", cut(&squash(s), 300)));
            }
            NodeKind::Element { tag, attrs } => {
                out.push(format!("<{tag}> with {} attribute{}", attrs.len(), if attrs.len() == 1 { "" } else { "s" }));
                for (k, v) in attrs.iter().take(12) {
                    out.push(format!("  {k}=\"{}\"", cut(v, 160)));
                }
                if attrs.len() > 12 {
                    out.push(format!("  ... and {} more", attrs.len() - 12));
                }
                let text = squash(&Self::words_under(t, r.node));
                out.push(if text.is_empty() { "text: (none)".to_string() } else { format!("text: {}", cut(&text, 300)) });
            }
        }
        out
    }

    /// The text under a node with a space where a block begins or ends, so that two paragraphs do not run together.
    fn words_under(t: &Tree, n: usize) -> String {
        const INLINE: [&str; 14] = ["a", "b", "i", "em", "strong", "span", "u", "s", "small", "code", "abbr", "mark", "sub", "sup"];
        let mut out = String::new();
        let mut todo = vec![(n, false)];
        while let Some((i, closing)) = todo.pop() {
            match &t.nodes[i].kind {
                NodeKind::Text(s) => out.push_str(s),
                NodeKind::Element { tag, .. } => {
                    let block = !INLINE.contains(&tag.as_str());
                    if closing || t.nodes[i].children.is_empty() {
                        if block {
                            out.push(' ');
                        }
                        continue;
                    }
                    if block {
                        out.push(' ');
                        todo.push((i, true));
                    }
                    todo.extend(t.nodes[i].children.iter().rev().map(|&c| (c, false)));
                }
            }
        }
        out
    }

    /// The visible rows as plain text lines (indented), for the panel's text report.
    pub fn tree_text(&self) -> String {
        self.rows().iter().map(|r| format!("{}{}", "  ".repeat(r.depth), r.label)).collect::<Vec<_>>().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Key;

    fn tree() -> Tree {
        iyakki_web::html::parse("<html><body><div id=main class=\"a  b\"><p>Hello <b>big</b> world</p><a href=\"/x\" title=t>link</a></div><ul><li>one<li>two</ul></body></html>")
    }

    fn typ(i: &mut Inspector, s: &str) {
        for c in s.chars() {
            i.prompt_key(&Key::Char(c));
        }
    }

    #[test]
    fn the_prompt_collects_text_sends_on_enter_and_recalls_older_lines() {
        let mut i = Inspector::default();
        typ(&mut i, "1+1");
        assert_eq!(i.prompt_key(&Key::Enter), Some("1+1".into()));
        assert!(i.input.is_empty());
        typ(&mut i, "document.title");
        i.prompt_key(&Key::Backspace);
        assert_eq!(i.input, "document.titl");
        assert_eq!(i.prompt_key(&Key::Enter), Some("document.titl".into()));
        assert_eq!(i.prompt_key(&Key::Enter), None, "an empty line sends nothing");
        i.prompt_key(&Key::Up);
        assert_eq!(i.input, "document.titl");
        i.prompt_key(&Key::Up);
        assert_eq!(i.input, "1+1");
        i.prompt_key(&Key::Down);
        assert_eq!(i.input, "document.titl");
        i.prompt_key(&Key::Down);
        assert!(i.input.is_empty(), "past the newest is a clean line");
        i.editing = true;
        i.prompt_key(&Key::Esc);
        assert!(!i.editing);
        i.paste("a\nb");
        assert_eq!(i.input, "ab", "control characters are not pasted");
    }

    #[test]
    fn results_errors_and_their_places_are_lines_and_the_transcript_is_bounded() {
        let mut i = Inspector::default();
        i.show_result(Ok("42".into()));
        i.show_result(Ok(String::new()));
        i.show_result(Err("ReferenceError: foo is not defined\n    at <eval> (console:1:1)\n    at run (app.js:10:5)".into()));
        let t: Vec<(Kind, &str)> = i.lines.iter().map(|l| (l.kind, l.text.as_str())).collect();
        assert_eq!(t[0], (Kind::Out, "42"));
        assert_eq!(t[1], (Kind::Out, "(no value)"));
        assert_eq!(t[2], (Kind::Err, "ReferenceError: foo is not defined"));
        assert_eq!(t[3], (Kind::Err, "   at <eval> (console:1:1)"));
        assert_eq!(t[4], (Kind::Err, "   at run (app.js:10:5)"));
        for n in 0..(MAX_LINES + 50) {
            i.push(Kind::Log, format!("line {n}"));
        }
        assert_eq!(i.lines.len(), MAX_LINES);
        assert_eq!(i.lines.last().unwrap().text, format!("line {}", MAX_LINES + 49));
        i.push(Kind::Out, "x".repeat(5000));
        assert!(i.lines.last().unwrap().text.chars().count() <= MAX_SHOWN);
    }

    #[test]
    fn the_tree_opens_closes_and_shows_attributes_and_text_of_the_chosen_node() {
        let mut i = Inspector::default();
        assert!(i.rows().is_empty() && i.detail().is_empty());
        i.set_tree(tree(), false);
        let rows = i.rows();
        assert!(rows[0].label.starts_with("<html"), "{:?}", rows[0]);
        let main = rows.iter().position(|r| r.label.contains("id=\"main\"")).expect("the div is shown");
        assert!(rows[main].label.contains("class=\"a b\""), "{}", rows[main].label);
        i.sel = main;
        let d = i.detail().join("\n");
        assert!(d.contains("<div> with 2 attributes") && d.contains("id=\"main\"") && d.contains("text: Hello big world link"), "{d}");
        // Right opens the div (the first three levels start open, not this one); Left closes it, so its children go
        i.tree_key(&Key::Right);
        let n = i.rows().len();
        i.tree_key(&Key::Left);
        assert!(i.rows().len() < n);
        i.tree_key(&Key::Right);
        assert_eq!(i.rows().len(), n);
        // Down goes to the first child; Left from a child goes up to the parent
        i.tree_key(&Key::Right);
        assert_eq!(i.sel, main + 1);
        i.tree_key(&Key::Left);
        assert_eq!(i.sel, main, "a closed child's Left goes out to the parent");
        // a link shows its attributes
        let a = i.rows().iter().position(|r| r.label == "<a>").unwrap();
        i.sel = a;
        let d = i.detail().join("\n");
        assert!(d.contains("href=\"/x\"") && d.contains("title=\"t\"") && d.contains("text: link"), "{d}");
        // a click chooses, a second click on it toggles
        i.pick(main);
        assert_eq!(i.sel, main);
        let n = i.rows().len();
        i.pick(main);
        assert!(i.rows().len() != n, "the second click toggled it");
        assert!(i.tree_text().contains("<html>"));
        i.clear_tree();
        assert!(!i.has_tree() && i.rows().is_empty());
    }

    #[test]
    fn setting_a_new_tree_keeps_the_choice_in_range() {
        let mut i = Inspector::default();
        i.set_tree(tree(), false);
        i.sel = 9;
        i.set_tree(iyakki_web::html::parse("<p>x</p>"), true);
        assert!(i.live && i.sel < i.rows().len().max(1));
        assert!(i.detail().join(" ").contains("<html>") || !i.detail().is_empty());
    }
}
