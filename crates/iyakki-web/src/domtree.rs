//! Editing and searching a parsed page: what a script's `document` needs. Nodes are numbered (their index in the tree);
//! a node that has been removed keeps its number and can be put back.

use crate::html::{self, Node, NodeKind, Tree, NONE};

const MAX_NODES: usize = 400_000;
const VOID: [&str; 15] = ["br", "hr", "img", "input", "meta", "link", "area", "base", "col", "embed", "source", "track", "wbr", "param", "basefont"];

impl Tree {
    pub fn parent_of(&self, n: usize) -> Option<usize> {
        let p = self.nodes.get(n)?.parent;
        (p != NONE).then_some(p)
    }

    pub fn is_element(&self, n: usize) -> bool {
        matches!(self.nodes.get(n).map(|x| &x.kind), Some(NodeKind::Element { .. }))
    }

    pub fn new_element(&mut self, tag: &str) -> Result<usize, String> {
        if self.nodes.len() >= MAX_NODES {
            return Err("the page has too many elements".into());
        }
        if tag.is_empty() || !tag.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '#')) {
            return Err(format!("InvalidCharacterError: {tag:?} is not a valid element name"));
        }
        self.nodes.push(Node { kind: NodeKind::Element { tag: tag.to_ascii_lowercase(), attrs: vec![] }, children: vec![], parent: NONE });
        Ok(self.nodes.len() - 1)
    }

    pub fn new_text(&mut self, text: &str) -> Result<usize, String> {
        if self.nodes.len() >= MAX_NODES {
            return Err("the page has too many elements".into());
        }
        self.nodes.push(Node { kind: NodeKind::Text(text.to_string()), children: vec![], parent: NONE });
        Ok(self.nodes.len() - 1)
    }

    /// Take a node out of its parent (it keeps its own children).
    pub fn detach(&mut self, n: usize) {
        let Some(p) = self.parent_of(n) else { return };
        self.nodes[p].children.retain(|&c| c != n);
        self.nodes[n].parent = NONE;
    }

    fn is_ancestor(&self, a: usize, of: usize) -> bool {
        let mut cur = Some(of);
        while let Some(c) = cur {
            if c == a {
                return true;
            }
            cur = self.parent_of(c);
        }
        false
    }

    /// Put `child` under `parent`, before `before` (at the end when `None`). Moves it if it is already somewhere.
    pub fn insert(&mut self, parent: usize, child: usize, before: Option<usize>) -> Result<(), String> {
        if !self.is_element(parent) {
            return Err("HierarchyRequestError: only elements can have children".into());
        }
        if self.is_ancestor(child, parent) {
            return Err("HierarchyRequestError: a node cannot be put inside itself".into());
        }
        if let Some(b) = before {
            if self.parent_of(b) != Some(parent) {
                return Err("NotFoundError: the reference node is not a child of this node".into());
            }
        }
        self.detach(child);
        let at = before.and_then(|b| self.nodes[parent].children.iter().position(|&c| c == b)).unwrap_or(self.nodes[parent].children.len());
        self.nodes[parent].children.insert(at, child);
        self.nodes[child].parent = parent;
        Ok(())
    }

    pub fn append(&mut self, parent: usize, child: usize) -> Result<(), String> {
        self.insert(parent, child, None)
    }

    pub fn set_attr(&mut self, n: usize, name: &str, value: &str) {
        if let NodeKind::Element { attrs, .. } = &mut self.nodes[n].kind {
            let name = name.to_ascii_lowercase();
            if let Some(a) = attrs.iter_mut().find(|(k, _)| *k == name) {
                a.1 = value.to_string();
            } else if attrs.len() < 128 {
                attrs.push((name, value.to_string()));
            }
        }
    }

    pub fn remove_attr(&mut self, n: usize, name: &str) {
        if let NodeKind::Element { attrs, .. } = &mut self.nodes[n].kind {
            let name = name.to_ascii_lowercase();
            attrs.retain(|(k, _)| *k != name);
        }
    }

    pub fn attr_list(&self, n: usize) -> Vec<(String, String)> {
        match &self.nodes[n].kind {
            NodeKind::Element { attrs, .. } => attrs.clone(),
            NodeKind::Text(_) => vec![],
        }
    }

    /// Replace everything under an element with one piece of text (or change a text node's text).
    pub fn set_text_content(&mut self, n: usize, text: &str) -> Result<(), String> {
        match &mut self.nodes[n].kind {
            NodeKind::Text(t) => {
                *t = text.to_string();
                Ok(())
            }
            NodeKind::Element { .. } => {
                for c in std::mem::take(&mut self.nodes[n].children) {
                    self.nodes[c].parent = NONE;
                }
                if !text.is_empty() {
                    let t = self.new_text(text)?;
                    self.append(n, t)?;
                }
                Ok(())
            }
        }
    }

    pub fn clone_node(&mut self, n: usize, deep: bool) -> Result<usize, String> {
        match self.nodes[n].kind.clone() {
            NodeKind::Text(t) => self.new_text(&t),
            NodeKind::Element { tag, attrs } => {
                let e = self.new_element(&tag)?;
                self.nodes[e].kind = NodeKind::Element { tag, attrs };
                if deep {
                    for c in self.nodes[n].children.clone() {
                        let cc = self.clone_node(c, true)?;
                        self.nodes[cc].parent = e;
                        self.nodes[e].children.push(cc);
                    }
                }
                Ok(e)
            }
        }
    }

    /// Copy `node` (and its descendants) out of `from` into this tree, detached. The new node's number.
    fn import(&mut self, from: &Tree, node: usize) -> Result<usize, String> {
        let copy = match &from.nodes[node].kind {
            NodeKind::Text(t) => self.new_text(t)?,
            NodeKind::Element { tag, attrs } => {
                let e = self.new_element(tag)?;
                self.nodes[e].kind = NodeKind::Element { tag: tag.clone(), attrs: attrs.clone() };
                for &c in &from.nodes[node].children {
                    let cc = self.import(from, c)?;
                    self.nodes[cc].parent = e;
                    self.nodes[e].children.push(cc);
                }
                e
            }
        };
        Ok(copy)
    }

    /// `element.innerHTML = html`: the markup replaces the children.
    pub fn set_inner_html(&mut self, n: usize, markup: &str) -> Result<(), String> {
        if !self.is_element(n) {
            return Err("HierarchyRequestError".into());
        }
        let fragment = html::parse(markup);
        for c in std::mem::take(&mut self.nodes[n].children) {
            self.nodes[c].parent = NONE;
        }
        for &c in &fragment.nodes[0].children {
            let copy = self.import(&fragment, c)?;
            self.append(n, copy)?;
        }
        Ok(())
    }

    pub fn inner_html(&self, n: usize) -> String {
        let mut out = String::new();
        for &c in &self.nodes[n].children {
            self.write_html(c, &mut out, 0);
        }
        out
    }

    pub fn outer_html(&self, n: usize) -> String {
        let mut out = String::new();
        self.write_html(n, &mut out, 0);
        out
    }

    fn write_html(&self, n: usize, out: &mut String, depth: usize) {
        if depth > 300 || out.len() > 8 << 20 {
            return;
        }
        match &self.nodes[n].kind {
            NodeKind::Text(t) => {
                let raw = self.parent_of(n).and_then(|p| self.tag(p)).is_some_and(|p| matches!(p, "script" | "style"));
                out.push_str(&if raw { t.clone() } else { escape_text(t) });
            }
            NodeKind::Element { tag, attrs } => {
                if tag.starts_with('#') {
                    for &c in &self.nodes[n].children {
                        self.write_html(c, out, depth + 1);
                    }
                    return;
                }
                out.push('<');
                out.push_str(tag);
                for (k, v) in attrs {
                    out.push(' ');
                    out.push_str(k);
                    out.push_str("=\"");
                    out.push_str(&v.replace('&', "&amp;").replace('"', "&quot;"));
                    out.push('"');
                }
                out.push('>');
                if !VOID.contains(&tag.as_str()) {
                    for &c in &self.nodes[n].children {
                        self.write_html(c, out, depth + 1);
                    }
                    out.push_str("</");
                    out.push_str(tag);
                    out.push('>');
                }
            }
        }
    }

    pub fn get_element_by_id(&self, root: usize, id: &str) -> Option<usize> {
        let mut todo: Vec<usize> = self.nodes[root].children.iter().rev().copied().collect();
        while let Some(n) = todo.pop() {
            if let NodeKind::Element { attrs, .. } = &self.nodes[n].kind {
                if attrs.iter().any(|(k, v)| k == "id" && v == id) {
                    return Some(n);
                }
                todo.extend(self.nodes[n].children.iter().rev());
            }
        }
        None
    }

    /// Is `n` in the tree under node 0 (not detached)?
    pub fn is_connected(&self, n: usize) -> bool {
        let mut cur = n;
        for _ in 0..100_000 {
            if cur == 0 {
                return true;
            }
            match self.parent_of(cur) {
                Some(p) => cur = p,
                None => return false,
            }
        }
        false
    }

    /// Elements under `root` in document order (not including `root`).
    pub fn descendants(&self, root: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut todo: Vec<usize> = self.nodes[root].children.iter().rev().copied().collect();
        while let Some(n) = todo.pop() {
            if self.is_element(n) {
                out.push(n);
                todo.extend(self.nodes[n].children.iter().rev());
            }
        }
        out
    }

    /// Elements under `root` that match a CSS selector (comma lists, tag, `*`, `#id`, `.class`, `[attr]`, `[attr=v]`,
    /// `^=`, `$=`, `*=`, `~=`, the structural pseudo-classes (`:first-child`, `:nth-child(2n+1)`, `:not(...)`, `:root`...),
    /// and the combinators ` `, `>`, `+`, `~`). Pseudo-classes that depend on the user's pointer (`:hover`) and
    /// pseudo-elements (`::before`) are valid and never match.
    pub fn query_selector_all(&self, root: usize, selector: &str) -> Result<Vec<usize>, String> {
        let list = parse_selectors(selector)?;
        Ok(self.query_selectors(root, &list, false))
    }

    /// Like [`Tree::query_selector_all`] with the selector already parsed; `first` stops at the first match.
    pub fn query_selectors(&self, root: usize, list: &[Selector], first: bool) -> Vec<usize> {
        let mut out = Vec::new();
        let mut todo: Vec<usize> = self.nodes[root].children.iter().rev().copied().collect();
        while let Some(n) = todo.pop() {
            if self.is_element(n) {
                if list.iter().any(|s| self.selector_matches(n, s)) {
                    out.push(n);
                    if first {
                        break;
                    }
                }
                todo.extend(self.nodes[n].children.iter().rev());
            }
        }
        out
    }

    /// The nearest element, `n` itself first and then its ancestors, that matches one of the selectors.
    pub fn closest(&self, n: usize, list: &[Selector]) -> Option<usize> {
        let mut cur = Some(n);
        while let Some(c) = cur {
            if c == 0 {
                return None;
            }
            if self.is_element(c) && list.iter().any(|s| self.selector_matches(c, s)) {
                return Some(c);
            }
            cur = self.parent_of(c);
        }
        None
    }

    /// Is `b` the same node as `a` or inside it?
    pub fn contains_node(&self, a: usize, b: usize) -> bool {
        self.is_ancestor(a, b)
    }

    /// Elements under `root` whose name has a dash (the names custom elements must have), in document order.
    pub fn dashed_descendants(&self, root: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut todo: Vec<usize> = self.nodes[root].children.iter().rev().copied().collect();
        while let Some(n) = todo.pop() {
            if let NodeKind::Element { tag, .. } = &self.nodes[n].kind {
                if tag.contains('-') {
                    out.push(n);
                }
                todo.extend(self.nodes[n].children.iter().rev());
            }
        }
        out
    }

    /// `<script>` elements under `root` (including `root`), in document order.
    pub fn scripts_under(&self, root: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut todo = vec![root];
        while let Some(n) = todo.pop() {
            if let NodeKind::Element { tag, .. } = &self.nodes[n].kind {
                if tag == "script" {
                    out.push(n);
                }
                todo.extend(self.nodes[n].children.iter().rev());
            }
        }
        out
    }

    /// The previous (`-1`) or next (`1`) sibling, whatever it is.
    pub fn node_sibling(&self, n: usize, dir: i32) -> Option<usize> {
        let p = self.parent_of(n)?;
        let kids = &self.nodes[p].children;
        let i = kids.iter().position(|&c| c == n)?;
        if dir < 0 { i.checked_sub(1).map(|j| kids[j]) } else { kids.get(i + 1).copied() }
    }

    pub fn matches_selector(&self, n: usize, selector: &str) -> Result<bool, String> {
        let list = parse_selectors(selector)?;
        Ok(list.iter().any(|s| self.selector_matches(n, s)))
    }

    /// Does this parsed selector match element `n`?
    pub fn selector_matches(&self, n: usize, sel: &Selector) -> bool {
        !sel.never && sel.pseudo_el.is_none() && self.is_element(n) && self.matches_complex(n, &sel.complex, NONE)
    }

    /// Does this selector, ending in `::before`/`::after` of the given kind, style the generated box of element `n`?
    pub fn pseudo_matches(&self, n: usize, sel: &Selector, which: PseudoEl) -> bool {
        !sel.never && sel.pseudo_el == Some(which) && self.is_element(n) && self.matches_complex(n, &sel.complex, NONE)
    }

    fn matches_complex(&self, n: usize, sel: &[(Combinator, Compound)], scope: usize) -> bool {
        // match right to left
        let Some(((_, last), rest)) = sel.split_last() else { return false };
        if !self.matches_compound(n, last) {
            return false;
        }
        self.match_rest(n, last_combinator(sel), rest, scope)
    }

    fn match_rest(&self, n: usize, comb: Combinator, rest: &[(Combinator, Compound)], scope: usize) -> bool {
        let Some(((prev_comb, c), before)) = rest.split_last() else { return true };
        let candidates: Vec<usize> = match comb {
            Combinator::Child => self.parent_of(n).into_iter().collect(),
            Combinator::Descendant => {
                let mut v = vec![];
                let mut cur = self.parent_of(n);
                while let Some(p) = cur {
                    v.push(p);
                    cur = self.parent_of(p);
                }
                v
            }
            Combinator::Adjacent => self.element_sibling(n, -1).into_iter().collect(),
            Combinator::Sibling => {
                let mut v = vec![];
                let mut cur = self.element_sibling(n, -1);
                while let Some(p) = cur {
                    v.push(p);
                    cur = self.element_sibling(p, -1);
                }
                v
            }
            Combinator::None => vec![],
        };
        let _ = scope;
        candidates.into_iter().filter(|&p| self.is_element(p) && self.tag(p) != Some("#root")).any(|p| self.matches_compound(p, c) && self.match_rest(p, *prev_comb, before, scope))
    }

    /// 1-based position of element `n` among its siblings (or among those with the same tag), counted from the start
    /// (`from_start`) or the end.
    fn position_among(&self, n: usize, same_tag: bool, from_start: bool) -> Option<usize> {
        let p = self.parent_of(n)?;
        let tag = self.tag(n)?;
        let kids: Vec<usize> = self.nodes[p].children.iter().copied().filter(|&c| self.is_element(c) && (!same_tag || self.tag(c) == Some(tag))).collect();
        let i = kids.iter().position(|&c| c == n)?;
        Some(if from_start { i + 1 } else { kids.len() - i })
    }

    /// The previous (`-1`) or next (`1`) sibling that is an element.
    pub fn element_sibling(&self, n: usize, dir: i32) -> Option<usize> {
        let p = self.parent_of(n)?;
        let kids = &self.nodes[p].children;
        let i = kids.iter().position(|&c| c == n)?;
        let iter: Box<dyn Iterator<Item = &usize>> = if dir < 0 { Box::new(kids[..i].iter().rev()) } else { Box::new(kids[i + 1..].iter()) };
        iter.copied().find(|&c| self.is_element(c))
    }

    fn matches_compound(&self, n: usize, c: &Compound) -> bool {
        let NodeKind::Element { tag, attrs } = &self.nodes[n].kind else { return false };
        if c.tag.as_deref().is_some_and(|t| t != "*" && t != tag) {
            return false;
        }
        if c.id.as_deref().is_some_and(|i| attrs.iter().find(|(k, _)| k == "id").map(|a| a.1.as_str()) != Some(i)) {
            return false;
        }
        let classes = attrs.iter().find(|(k, _)| k == "class").map_or("", |a| a.1.as_str());
        if !c.classes.iter().all(|cl| classes.split_whitespace().any(|x| x == cl)) {
            return false;
        }
        for (name, op, val) in &c.attrs {
            let Some(actual) = attrs.iter().find(|(k, _)| k == name).map(|a| a.1.as_str()) else { return false };
            let ok = match op {
                AttrOp::Exists => true,
                AttrOp::Equals => actual == val,
                AttrOp::Prefix => actual.starts_with(val.as_str()) && !val.is_empty(),
                AttrOp::Suffix => actual.ends_with(val.as_str()) && !val.is_empty(),
                AttrOp::Contains => actual.contains(val.as_str()) && !val.is_empty(),
                AttrOp::Word => actual.split_whitespace().any(|w| w == val),
            };
            if !ok {
                return false;
            }
        }
        for p in &c.pseudos {
            let ok = match p {
                Pseudo::FirstChild => self.element_sibling(n, -1).is_none(),
                Pseudo::LastChild => self.element_sibling(n, 1).is_none(),
                Pseudo::OnlyChild => self.element_sibling(n, -1).is_none() && self.element_sibling(n, 1).is_none(),
                Pseudo::FirstOfType => self.position_among(n, true, true) == Some(1),
                Pseudo::LastOfType => self.position_among(n, true, true).is_some() && self.position_among(n, true, false) == Some(1),
                Pseudo::Nth { a, b, last, of_type } => self.position_among(n, *of_type, !*last).is_some_and(|pos| {
                    let pos = pos as i32;
                    // pos = a*k + b for some k >= 0
                    if *a == 0 { pos == *b } else { (pos - *b) % *a == 0 && (pos - *b) / *a >= 0 }
                }),
                Pseudo::Empty => self.nodes[n].children.iter().all(|&ch| matches!(&self.nodes[ch].kind, NodeKind::Text(t) if t.is_empty())),
                Pseudo::Root => self.parent_of(n).is_some_and(|p| self.tag(p) == Some("#root")),
                Pseudo::Link => tag == "a" && attrs.iter().any(|(k, _)| k == "href") || tag == "area" && attrs.iter().any(|(k, _)| k == "href"),
                Pseudo::HasAttr(name) => attrs.iter().any(|(k, _)| k == name),
                Pseudo::LacksAttr(name) => !attrs.iter().any(|(k, _)| k == name),
                Pseudo::Not(list) => !list.iter().any(|x| self.matches_compound(n, x)),
                Pseudo::Is(list) => list.iter().any(|x| self.matches_compound(n, x)),
                Pseudo::Never => false,
            };
            if !ok {
                return false;
            }
        }
        true
    }
}

fn escape_text(t: &str) -> String {
    t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Combinator {
    None,
    Descendant,
    Child,
    Adjacent,
    Sibling,
}

fn last_combinator(sel: &[(Combinator, Compound)]) -> Combinator {
    sel.last().map_or(Combinator::None, |l| l.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttrOp {
    Exists,
    Equals,
    Prefix,
    Suffix,
    Contains,
    Word,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Pseudo {
    FirstChild,
    LastChild,
    OnlyChild,
    FirstOfType,
    LastOfType,
    /// `:nth-child(an+b)` and its relatives: position counted from the start or the end, among all siblings or only
    /// those of the same tag.
    Nth { a: i32, b: i32, last: bool, of_type: bool },
    Empty,
    Root,
    Link,
    /// An element with this attribute (`:checked`, `:disabled`, `:required`...).
    HasAttr(&'static str),
    /// An element without this attribute (`:enabled`).
    LacksAttr(&'static str),
    Not(Vec<Compound>),
    Is(Vec<Compound>),
    /// A state we never have (`:hover`, `:focus`, `:visited`, a pseudo-element): nothing matches.
    Never,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<(String, AttrOp, String)>,
    pseudos: Vec<Pseudo>,
    /// `::before` or `::after` at the end of the compound (also the one-colon legacy forms).
    pseudo_el: Option<PseudoEl>,
}

/// A generated-content pseudo-element.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PseudoEl {
    Before,
    After,
}

/// Each entry carries the combinator that connects it to the entry on its left.
type Complex = Vec<(Combinator, Compound)>;

/// A parsed selector, for matching many elements against one rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    complex: Complex,
    /// (ids, classes/attributes/pseudo-classes, tags): compared in that order, as in CSS.
    pub specificity: (u32, u32, u32),
    /// Contains something that can never match here (`:hover`, `::before`).
    pub never: bool,
    /// The selector styles the generated box of the matched element (`a::before`), not the element.
    pub pseudo_el: Option<PseudoEl>,
}

impl Selector {
    /// `#name` and nothing else: found through an id index instead of a scan.
    pub fn only_id(&self) -> Option<&str> {
        match self.complex.as_slice() {
            [(_, c)] if !self.never && self.pseudo_el.is_none() && c.tag.is_none() && c.classes.is_empty() && c.attrs.is_empty() && c.pseudos.is_empty() && c.pseudo_el.is_none() => c.id.as_deref(),
            _ => None,
        }
    }

    /// The rightmost compound's id, first class and tag, to find candidate rules quickly.
    pub fn key(&self) -> (Option<&str>, Option<&str>, Option<&str>) {
        match self.complex.last() {
            Some((_, c)) => (c.id.as_deref(), c.classes.first().map(String::as_str), c.tag.as_deref().filter(|t| *t != "*")),
            None => (None, None, None),
        }
    }
}

fn compound_specificity(c: &Compound) -> (u32, u32, u32) {
    let mut sp = (c.id.is_some() as u32, c.classes.len() as u32 + c.attrs.len() as u32, c.tag.as_deref().is_some_and(|t| t != "*") as u32);
    sp.2 += c.pseudo_el.is_some() as u32;
    for p in &c.pseudos {
        match p {
            Pseudo::Not(list) | Pseudo::Is(list) => {
                // the most specific argument counts
                if let Some(m) = list.iter().map(compound_specificity).max() {
                    sp = (sp.0 + m.0, sp.1 + m.1, sp.2 + m.2);
                }
            }
            Pseudo::Never => sp.1 += 1,
            _ => sp.1 += 1,
        }
    }
    sp
}

fn has_never(c: &Compound) -> bool {
    c.pseudos.iter().any(|p| match p {
        Pseudo::Never => true,
        Pseudo::Is(list) => !list.is_empty() && list.iter().all(has_never),
        _ => false,
    })
}

/// Parse a selector list ("a, b > c:first-child"). Syntax errors are errors; unsupported states are not.
pub fn parse_selectors(s: &str) -> Result<Vec<Selector>, String> {
    if s.len() > 200_000 {
        return Err("SyntaxError: the selector is too long".into());
    }
    let mut out = Vec::new();
    for part in split_top_level(s, ',') {
        let complex = parse_complex(part.trim())?;
        let mut sp = (0, 0, 0);
        for (_, c) in &complex {
            let x = compound_specificity(c);
            sp = (sp.0 + x.0, sp.1 + x.1, sp.2 + x.2);
        }
        let mut never = complex.iter().any(|(_, c)| has_never(c));
        // a pseudo-element can only end the selector
        never |= complex.iter().rev().skip(1).any(|(_, c)| c.pseudo_el.is_some());
        let pseudo_el = complex.last().and_then(|(_, c)| c.pseudo_el);
        out.push(Selector { complex, specificity: sp, never, pseudo_el });
    }
    if out.is_empty() {
        return Err(format!("SyntaxError: {s:?} is not a valid selector"));
    }
    Ok(out)
}

fn parse_compound_list(s: &str) -> Option<Vec<Compound>> {
    let mut out = Vec::new();
    for part in split_top_level(s, ',') {
        let chars: Vec<char> = part.trim().chars().collect();
        let (c, end) = parse_compound(&chars, 0)?;
        if end != chars.len() {
            return None;
        }
        out.push(c);
    }
    (!out.is_empty()).then_some(out)
}

/// `an+b`, `odd`, `even`, `3`, `-n+4`.
fn parse_nth(arg: &str, last: bool, of_type: bool) -> Option<Pseudo> {
    let t: String = arg.split_whitespace().collect::<String>().to_ascii_lowercase();
    let (a, b) = match t.as_str() {
        "odd" => (2, 1),
        "even" => (2, 0),
        _ => match t.split_once('n') {
            Some((a, rest)) => {
                let a = match a {
                    "" | "+" => 1,
                    "-" => -1,
                    n => n.parse().ok()?,
                };
                let b = if rest.is_empty() { 0 } else { rest.trim_start_matches('+').parse().ok()? };
                (a, b)
            }
            None => (0, t.parse().ok()?),
        },
    };
    Some(Pseudo::Nth { a, b, last, of_type })
}

fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let (mut depth, mut start, mut out) = (0i32, 0, Vec::new());
    let mut in_str: Option<char> = None;
    for (i, c) in s.char_indices() {
        match (c, in_str) {
            ('"' | '\'', None) => in_str = Some(c),
            (q, Some(open)) if q == open => in_str = None,
            ('(' | '[', None) => depth += 1,
            (')' | ']', None) => depth -= 1,
            (c, None) if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn parse_complex(s: &str) -> Result<Complex, String> {
    let bad = || format!("SyntaxError: {s:?} is not a valid selector");
    if s.is_empty() {
        return Err(bad());
    }
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out: Complex = Vec::new();
    let mut pending = Combinator::None;
    while i < chars.len() {
        // combinators and white space
        let mut saw_space = false;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
            saw_space = true;
        }
        if i >= chars.len() {
            break;
        }
        if matches!(chars[i], '>' | '+' | '~') {
            pending = match chars[i] {
                '>' => Combinator::Child,
                '+' => Combinator::Adjacent,
                _ => Combinator::Sibling,
            };
            i += 1;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
        } else if saw_space && !out.is_empty() {
            pending = Combinator::Descendant;
        }
        let (c, next) = parse_compound(&chars, i).ok_or_else(bad)?;
        i = next;
        out.push((if out.is_empty() { Combinator::None } else { pending }, c));
        pending = Combinator::None;
    }
    if out.is_empty() {
        return Err(bad());
    }
    Ok(out)
}

fn ident(chars: &[char], mut i: usize) -> (String, usize) {
    let start = i;
    while i < chars.len() && (chars[i].is_alphanumeric() || matches!(chars[i], '-' | '_') || (chars[i] == '\\' && i + 1 < chars.len())) {
        if chars[i] == '\\' {
            i += 1;
        }
        i += 1;
    }
    (chars[start..i].iter().filter(|c| **c != '\\').collect(), i)
}

fn parse_compound(chars: &[char], mut i: usize) -> Option<(Compound, usize)> {
    let mut c = Compound::default();
    let start = i;
    if i < chars.len() && chars[i] == '*' {
        c.tag = Some("*".into());
        i += 1;
    } else if i < chars.len() && (chars[i].is_alphabetic() || chars[i] == '_') {
        let (t, n) = ident(chars, i);
        c.tag = Some(t.to_ascii_lowercase());
        i = n;
    }
    while i < chars.len() {
        match chars[i] {
            '#' => {
                let (v, n) = ident(chars, i + 1);
                if v.is_empty() {
                    return None;
                }
                c.id = Some(v);
                i = n;
            }
            '.' => {
                let (v, n) = ident(chars, i + 1);
                if v.is_empty() {
                    return None;
                }
                c.classes.push(v);
                i = n;
            }
            '[' => {
                let close = chars[i..].iter().position(|&x| x == ']')? + i;
                let body: String = chars[i + 1..close].iter().collect();
                let (name, op, val) = parse_attr(&body)?;
                c.attrs.push((name, op, val));
                i = close + 1;
            }
            ':' => {
                // `::before` and friends are pseudo-elements: valid, never matched here
                let double = chars.get(i + 1) == Some(&':');
                let (name, n) = ident(chars, i + if double { 2 } else { 1 });
                i = n;
                if chars.get(i) != Some(&'(') && (name.eq_ignore_ascii_case("before") || name.eq_ignore_ascii_case("after")) {
                    c.pseudo_el = Some(if name.eq_ignore_ascii_case("before") { PseudoEl::Before } else { PseudoEl::After });
                    continue;
                }
                if double || name.is_empty() {
                    c.pseudos.push(Pseudo::Never);
                    continue;
                }
                let name = name.to_ascii_lowercase();
                // functional pseudo-classes take their argument up to the matching ')'
                let arg = if chars.get(i) == Some(&'(') {
                    let mut depth = 0;
                    let mut end = None;
                    for (k, ch) in chars[i..].iter().enumerate() {
                        match ch {
                            '(' => depth += 1,
                            ')' => {
                                depth -= 1;
                                if depth == 0 {
                                    end = Some(i + k);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    let e = end?;
                    let a: String = chars[i + 1..e].iter().collect();
                    i = e + 1;
                    Some(a)
                } else {
                    None
                };
                let p = match (name.as_str(), arg.as_deref()) {
                    ("first-child", None) => Pseudo::FirstChild,
                    ("last-child", None) => Pseudo::LastChild,
                    ("only-child", None) => Pseudo::OnlyChild,
                    ("first-of-type", None) => Pseudo::FirstOfType,
                    ("last-of-type", None) => Pseudo::LastOfType,
                    ("empty", None) => Pseudo::Empty,
                    ("root", None) => Pseudo::Root,
                    ("link" | "any-link", None) => Pseudo::Link,
                    ("checked", None) => Pseudo::HasAttr("checked"),
                    ("disabled", None) => Pseudo::HasAttr("disabled"),
                    ("required", None) => Pseudo::HasAttr("required"),
                    ("read-only", None) => Pseudo::HasAttr("readonly"),
                    ("enabled", None) => Pseudo::LacksAttr("disabled"),
                    ("nth-child", Some(a)) => parse_nth(a, false, false)?,
                    ("nth-last-child", Some(a)) => parse_nth(a, true, false)?,
                    ("nth-of-type", Some(a)) => parse_nth(a, false, true)?,
                    ("nth-last-of-type", Some(a)) => parse_nth(a, true, true)?,
                    ("not", Some(a)) => Pseudo::Not(parse_compound_list(a)?),
                    ("is" | "where" | "matches" | "-webkit-any" | "-moz-any", Some(a)) => Pseudo::Is(parse_compound_list(a)?),
                    _ => Pseudo::Never,
                };
                c.pseudos.push(p);
            }
            _ => break,
        }
    }
    (i > start).then_some((c, i))
}

fn parse_attr(body: &str) -> Option<(String, AttrOp, String)> {
    let body = body.trim();
    for (tok, op) in [("^=", AttrOp::Prefix), ("$=", AttrOp::Suffix), ("*=", AttrOp::Contains), ("~=", AttrOp::Word), ("=", AttrOp::Equals)] {
        if let Some((n, v)) = body.split_once(tok) {
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
            return (!n.trim().is_empty()).then(|| (n.trim().to_ascii_lowercase(), op, v.to_string()));
        }
    }
    (!body.is_empty() && body.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ':'))).then(|| (body.to_ascii_lowercase(), AttrOp::Exists, String::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(s: &str) -> Tree {
        html::parse(s)
    }

    fn ids(t: &Tree, sel: &str) -> Vec<String> {
        t.query_selector_all(0, sel).unwrap().into_iter().map(|n| t.attr(n, "id").unwrap_or("?").to_string()).collect()
    }

    const PAGE: &str = r#"<div id="a" class="box big"><p id="p1" class="x">one</p><p id="p2">two</p><span id="s1"><b id="b1" data-k="v-1">bold</b></span></div><div id="b"><p id="p3" class="x y">three</p><a id="l1" href="https://e.org/x.pdf" target=_blank>link</a></div>"#;

    #[test]
    fn selectors_find_what_a_script_asks_for() {
        let t = tree(PAGE);
        assert_eq!(ids(&t, "p"), ["p1", "p2", "p3"]);
        assert_eq!(ids(&t, "#b"), ["b"]);
        assert_eq!(ids(&t, ".x"), ["p1", "p3"]);
        assert_eq!(ids(&t, "p.x.y"), ["p3"]);
        assert_eq!(ids(&t, "div p"), ["p1", "p2", "p3"]);
        assert_eq!(ids(&t, "div > p"), ["p1", "p2", "p3"]);
        assert_eq!(ids(&t, "div > b"), Vec::<String>::new(), "b is a grandchild");
        assert_eq!(ids(&t, "div b"), ["b1"]);
        assert_eq!(ids(&t, "#a .x, #b .y"), ["p1", "p3"]);
        assert_eq!(ids(&t, "p + p"), ["p2"]);
        assert_eq!(ids(&t, "p ~ span"), ["s1"]);
        assert_eq!(ids(&t, "[data-k]"), ["b1"]);
        assert_eq!(ids(&t, "[data-k=v-1]"), ["b1"]);
        assert_eq!(ids(&t, "[href$='.pdf']"), ["l1"]);
        assert_eq!(ids(&t, "a[href^=https][target]"), ["l1"]);
        assert_eq!(ids(&t, "[class~=big]"), ["a"]);
        assert_eq!(ids(&t, "p:first-child"), ["p1", "p3"]);
        assert_eq!(ids(&t, "p:last-child"), Vec::<String>::new());
        assert_eq!(ids(&t, "span:last-child, a:last-child"), ["s1", "l1"]);
        assert_eq!(ids(&t, "p:not(.x)"), ["p2"]);
        assert_eq!(ids(&t, "*").len(), 8);
        assert_eq!(ids(&t, "nothing"), Vec::<String>::new());
    }

    #[test]
    fn structural_pseudo_classes_and_states_we_never_have() {
        let t = tree(PAGE);
        assert_eq!(ids(&t, "p:hover"), Vec::<String>::new(), "valid, never matches");
        assert_eq!(ids(&t, "p::before"), Vec::<String>::new());
        let sel = parse_selectors("div#a.b > p:not(.x), a").unwrap();
        assert_eq!(sel[0].specificity, (1, 2, 2));
        assert_eq!(sel[1].specificity, (0, 0, 1));
        assert!(parse_selectors("p:nth-child(2n+1)").is_ok() && parse_selectors("li:nth-child(odd)").is_ok());
        assert!(parse_selectors("p:hover").unwrap()[0].never);
    }

    #[test]
    fn bad_selectors_are_errors_not_crashes() {
        let t = tree(PAGE);
        for bad in ["", "   ", "[", "#", ".", "p >", "a,,b", "p[=x]"] {
            assert!(t.query_selector_all(0, bad).is_err() || bad == "p >" || bad == "a,,b", "{bad:?}");
        }
        assert!(t.query_selector_all(0, &"a ".repeat(110_000)).is_err(), "very long selectors are refused");
    }

    #[test]
    fn nodes_can_be_created_moved_and_removed() {
        let mut t = tree("<ul id=l><li id=a>a<li id=b>b</ul><p id=p>x</p>");
        let l = t.get_element_by_id(0, "l").unwrap();
        let (a, b, p) = (t.get_element_by_id(0, "a").unwrap(), t.get_element_by_id(0, "b").unwrap(), t.get_element_by_id(0, "p").unwrap());
        let c = t.new_element("li").unwrap();
        t.set_attr(c, "id", "c");
        t.insert(l, c, Some(b)).unwrap();
        assert_eq!(ids(&t, "li"), ["a", "c", "b"]);
        t.append(l, a).unwrap();
        assert_eq!(ids(&t, "li"), ["c", "b", "a"], "an attached node moves");
        t.detach(b);
        assert_eq!(ids(&t, "li"), ["c", "a"]);
        assert_eq!(t.parent_of(b), None);
        t.append(p, b).unwrap();
        assert_eq!(t.outer_html(p), "<p id=\"p\">x<li id=\"b\">b</li></p>");
        assert!(t.append(a, l).is_err(), "a node cannot go inside its own descendant");
        let text = t.new_text("hi").unwrap();
        assert!(t.append(text, a).is_err(), "text nodes have no children");
        assert!(t.insert(l, a, Some(p)).is_err(), "the reference must be a child");
        assert!(t.new_element("a b").is_err() && t.new_element("").is_err());
    }

    #[test]
    fn markup_goes_in_and_out() {
        let mut t = tree("<div id=d>old <b>stuff</b></div>");
        let d = t.get_element_by_id(0, "d").unwrap();
        assert_eq!(t.inner_html(d), "old <b>stuff</b>");
        t.set_inner_html(d, "<p class='n'>new &amp; <i>fresh</i></p><br><img src=x alt='a \"q\"'>").unwrap();
        assert_eq!(t.inner_html(d), "<p class=\"n\">new &amp; <i>fresh</i></p><br><img src=\"x\" alt=\"a &quot;q&quot;\">");
        assert_eq!(ids(&t, "p.n"), ["?"]);
        t.set_text_content(d, "<script>alert(1)</script>").unwrap();
        assert_eq!(t.inner_html(d), "&lt;script&gt;alert(1)&lt;/script&gt;", "text is escaped, never parsed as markup");
        t.set_text_content(d, "").unwrap();
        assert_eq!(t.inner_html(d), "");
        let x = tree("<script>if (a < b) {}</script>");
        let s = x.find("script").unwrap();
        assert_eq!(x.inner_html(s), "if (a < b) {}", "script text is not escaped");
    }

    #[test]
    fn cloning_copies_a_subtree_without_attaching_it() {
        let mut t = tree("<div id=d><p id=p>hi <b>there</b></p></div>");
        let p = t.get_element_by_id(0, "p").unwrap();
        let deep = t.clone_node(p, true).unwrap();
        let shallow = t.clone_node(p, false).unwrap();
        assert_eq!(t.outer_html(deep), "<p id=\"p\">hi <b>there</b></p>");
        assert_eq!(t.outer_html(shallow), "<p id=\"p\"></p>");
        assert_eq!(t.parent_of(deep), None);
        assert_eq!(ids(&t, "p"), ["p"], "the copy is not in the page until it is attached");
    }

    #[test]
    fn siblings_attributes_and_matching_a_single_node() {
        let mut t = tree("<ul><li id=a>1<li id=b>2<li id=c>3</ul>");
        let (a, b, c) = (t.get_element_by_id(0, "a").unwrap(), t.get_element_by_id(0, "b").unwrap(), t.get_element_by_id(0, "c").unwrap());
        assert_eq!((t.element_sibling(b, -1), t.element_sibling(b, 1), t.element_sibling(a, -1), t.element_sibling(c, 1)), (Some(a), Some(c), None, None));
        t.set_attr(b, "Data-X", "1");
        assert_eq!(t.attr(b, "data-x"), Some("1"));
        t.remove_attr(b, "DATA-X");
        assert_eq!(t.attr(b, "data-x"), None);
        assert!(t.matches_selector(b, "li:not(:first-child)").unwrap() || t.matches_selector(b, "ul li").unwrap());
        assert!(!t.matches_selector(b, "ol li").unwrap());
        assert_eq!(t.attr_list(a), vec![("id".to_string(), "a".to_string())]);
    }

    #[test]
    fn pseudo_elements_are_parsed_but_do_not_match_the_element() {
        let t = tree("<p class=a>x</p>");
        let p = t.query_selector_all(0, "p").unwrap()[0];
        for sel in ["p::before", "p:before", ".a::AFTER", "p:after"] {
            let l = parse_selectors(sel).unwrap();
            assert!(l[0].pseudo_el.is_some() && !l[0].never, "{sel}");
            assert!(!t.selector_matches(p, &l[0]), "{sel}");
        }
        let l = parse_selectors(".a::before").unwrap();
        assert!(t.pseudo_matches(p, &l[0], PseudoEl::Before));
        assert!(!t.pseudo_matches(p, &l[0], PseudoEl::After));
        assert_eq!(l[0].specificity, (0, 1, 1));
        assert!(parse_selectors("p::before span").unwrap()[0].never);
        assert!(parse_selectors("p::first-line").unwrap()[0].never);
    }
}
