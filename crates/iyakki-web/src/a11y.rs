//! The accessibility tree of a page (docs/ACCESSIBILITY.md, "Browser"): what a screen reader needs to say about a page, made from
//! the parsed HTML (after the page's scripts ran, when they did): roles (link, button, heading level, list, form field, image,
//! table, landmark), accessible names (`aria-labelledby`, `aria-label`, `label for`, a wrapping `label`, the text inside, `alt`,
//! `title`), states (checked, disabled, expanded, required, invalid, selected, pressed) and a flat list of the nodes in document
//! order. Content that is not shown (`display:none`, `hidden`, `aria-hidden`, `type=hidden`, a closed `details`) is left out; text
//! that is only visually hidden (the "screen reader only" pattern) stays, because that is what it is for.
//!
//! The words a reader says for a node are made by [`A11yTree::speak`]; keyboard navigation over the tree is in [`crate::a11y_nav`];
//! the order of a "read the page" command is [`A11yTree::script`].
//!
//! The model is the same as the toolkit's (`iyakki-uikit::access`: name, role, state), with the page-only roles added. This crate
//! has no dependencies, so the shell turns a node into speech through its own small speech trait.

use crate::doc::{Control, ControlKind};
use crate::html::{NodeKind, Tree};
use std::collections::HashMap;

const MAX_NODES: usize = 30_000;
const MAX_DEPTH: usize = 160;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Landmark {
    Navigation,
    Main,
    Banner,
    ContentInfo,
    Complementary,
    Search,
    Region,
    Form,
}

impl Landmark {
    /// The word a reader says: "navigation landmark".
    pub fn word(self) -> &'static str {
        match self {
            Landmark::Navigation => "navigation landmark",
            Landmark::Main => "main landmark",
            Landmark::Banner => "banner landmark",
            Landmark::ContentInfo => "footer landmark",
            Landmark::Complementary => "complementary landmark",
            Landmark::Search => "search landmark",
            Landmark::Region => "region landmark",
            Landmark::Form => "form",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Link,
    Button,
    Heading,
    List,
    ListItem,
    TextField,
    PasswordField,
    TextArea,
    Checkbox,
    Radio,
    Select,
    Slider,
    Image,
    Table,
    Row,
    ColumnHeader,
    RowHeader,
    Cell,
    Group,
    Form,
    Landmark(Landmark),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AState {
    /// `Some(true)` checked, `Some(false)` not checked; a half-checked box has `mixed`.
    pub checked: Option<bool>,
    pub mixed: bool,
    pub disabled: bool,
    pub expanded: Option<bool>,
    pub required: bool,
    pub invalid: bool,
    pub readonly: bool,
    pub selected: bool,
    pub pressed: Option<bool>,
    pub current: bool,
    pub multiline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct A11yNode {
    /// The element in the page's tree.
    pub dom: usize,
    pub role: Role,
    pub name: String,
    /// Extra words after the name: `aria-describedby`, and the error message of an invalid field.
    pub description: String,
    /// Heading level (1 to 6); 0 for everything else.
    pub level: u8,
    pub state: AState,
    /// What a field holds (never for a password).
    pub value: String,
    pub href: String,
    pub parent: Option<usize>,
    pub depth: u16,
    /// The `type` of an input (lower case), else empty.
    pub input_type: String,
    /// Radio group: the name, and the form it is in.
    pub group: String,
    pub form: Option<usize>,
    /// Position in a list or a radio group: (1-based position, size).
    pub pos: (usize, usize),
    /// Table size, for a table.
    pub rows: usize,
    pub cols: usize,
    /// The column header words of a cell.
    pub col_header: String,
    /// `tabindex` as written; `None` when absent.
    pub tab_index: Option<i32>,
    /// The element can take the keyboard focus (and is reached by Tab unless `tab_index` is negative).
    pub focusable: bool,
    /// The first words of the text under the node: how the browser finds it on the drawn page.
    pub anchor: String,
    pub id: String,
    /// Index into [`crate::Doc::controls`] / [`crate::Doc::links`] when the node is one of them.
    pub control: Option<usize>,
    pub link: Option<usize>,
    pub minlength: usize,
    pub pattern_hint: String,
    /// A form with `novalidate`: the browser does not check its fields before sending.
    pub novalidate: bool,
}

/// One thing the "read the page" command says, in document order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadItem {
    pub text: String,
    /// The tree node it belongs to, when it is one.
    pub node: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct A11yTree {
    pub nodes: Vec<A11yNode>,
    pub script: Vec<ReadItem>,
    /// The text of `role=alert` / `aria-live` regions that have words in them (what a reader says when a page loads).
    pub alerts: Vec<String>,
    /// The element id to the first node at or after it, for `#fragment` links and skip links.
    pub ids: HashMap<String, usize>,
}

/// Counts for the status line: "12 links, 3 headings, ...".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub links: usize,
    pub buttons: usize,
    pub headings: usize,
    pub landmarks: usize,
    pub fields: usize,
    pub tables: usize,
    pub images: usize,
    pub lists: usize,
    pub unlabelled_fields: usize,
    pub images_without_alt: usize,
    pub invalid_fields: usize,
}

const SKIP: [&str; 14] = ["script", "style", "noscript", "template", "head", "title", "meta", "link", "base", "canvas", "iframe", "object", "embed", "svg"];
const BLOCKISH: [&str; 38] = [
    "p", "div", "section", "article", "header", "footer", "nav", "aside", "main", "figure", "figcaption", "address", "details", "summary", "fieldset", "legend", "dl", "dd", "dt", "ul", "ol", "li", "blockquote", "form", "h1", "h2", "h3", "h4", "h5", "h6", "pre", "table", "tr", "td", "th", "hr", "br", "body",
];

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn short(s: &str) -> String {
    let c = collapse(s);
    if c.chars().count() <= 60 {
        return c;
    }
    let cut: String = c.chars().take(60).collect();
    // do not end in the middle of a word: the drawn page is searched for these words
    match cut.rfind(' ') {
        Some(i) if i > 10 => cut[..i].to_string(),
        _ => cut,
    }
}

struct Ctx {
    parent: Option<usize>,
    form: Option<usize>,
    label: Option<usize>,
    /// Inside a `fieldset` with `disabled`.
    disabled: bool,
    depth: u16,
    /// The nearest list: its node.
    list: Option<usize>,
    in_sectioning: bool,
}

struct Builder<'a> {
    tree: &'a Tree,
    hide: &'a dyn Fn(usize) -> bool,
    nodes: Vec<A11yNode>,
    script: Vec<ReadItem>,
    buf: String,
    /// While above zero the text of children is not added to the running paragraph (a node said it as a whole).
    mute: u32,
    labels: HashMap<String, usize>,
    ids: HashMap<String, usize>,
    id_dom: HashMap<String, usize>,
    alerts: Vec<String>,
}

impl<'a> Builder<'a> {
    fn attr(&self, n: usize, a: &str) -> Option<&'a str> {
        self.tree.attr(n, a)
    }

    fn has(&self, n: usize, a: &str) -> bool {
        self.attr(n, a).is_some()
    }

    fn is_true(&self, n: usize, a: &str) -> bool {
        self.attr(n, a).is_some_and(|v| v.eq_ignore_ascii_case("true"))
    }

    /// Not shown at all: left out of the tree together with everything under it.
    fn hidden(&self, n: usize) -> bool {
        let Some(tag) = self.tree.tag(n) else { return false };
        if SKIP.contains(&tag) || self.has(n, "hidden") || self.is_true(n, "aria-hidden") || (self.hide)(n) {
            return true;
        }
        if tag == "input" && self.attr(n, "type").is_some_and(|t| t.eq_ignore_ascii_case("hidden")) {
            return true;
        }
        if let Some(st) = self.attr(n, "style") {
            let s: String = st.to_ascii_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
            if s.contains("display:none") || s.contains("visibility:hidden") {
                return true;
            }
        }
        false
    }

    /// The words under an element for a name: text, the `alt` of pictures, nothing from hidden parts; blocks and line breaks
    /// separate words, inline elements do not.
    fn text(&self, n: usize) -> String {
        let mut out = String::new();
        self.text_into(n, &mut out, 0);
        collapse(&out)
    }

    fn text_into(&self, n: usize, out: &mut String, depth: usize) {
        if depth > MAX_DEPTH || out.len() > 4000 {
            return;
        }
        match &self.tree.nodes[n].kind {
            NodeKind::Text(t) => out.push_str(t),
            NodeKind::Element { tag, .. } => {
                if depth > 0 && self.hidden(n) {
                    return;
                }
                if tag == "img" {
                    if let Some(a) = self.attr(n, "alt") {
                        out.push(' ');
                        out.push_str(a);
                        out.push(' ');
                    }
                    return;
                }
                if matches!(tag.as_str(), "input" | "select" | "textarea") {
                    return;
                }
                let block = BLOCKISH.contains(&tag.as_str());
                if block {
                    out.push(' ');
                }
                for &c in &self.tree.nodes[n].children {
                    self.text_into(c, out, depth + 1);
                }
                if block {
                    out.push(' ');
                }
            }
        }
    }

    /// Text of the elements an `aria-labelledby` / `aria-describedby` names (hidden ones count: that is allowed on purpose).
    fn by_ids(&self, list: &str) -> String {
        let parts: Vec<String> = list.split_whitespace().filter_map(|id| self.id_dom.get(id)).map(|&n| self.text(n)).filter(|t| !t.is_empty()).collect();
        parts.join(" ")
    }

    fn first_token(&self, n: usize, a: &str) -> Option<String> {
        self.attr(n, a).and_then(|v| v.split_whitespace().next()).map(|s| s.to_ascii_lowercase())
    }

    fn name_from_attrs(&self, n: usize) -> Option<String> {
        if let Some(l) = self.attr(n, "aria-labelledby") {
            let t = self.by_ids(l);
            if !t.is_empty() {
                return Some(t);
            }
        }
        if let Some(l) = self.attr(n, "aria-label") {
            let t = collapse(l);
            if !t.is_empty() {
                return Some(t);
            }
        }
        None
    }

    fn field_name(&self, n: usize, ctx: &Ctx) -> String {
        if let Some(t) = self.name_from_attrs(n) {
            return t;
        }
        if let Some(id) = self.attr(n, "id") {
            if let Some(&l) = self.labels.get(id) {
                let t = self.text(l);
                if !t.is_empty() {
                    return t;
                }
            }
        }
        if let Some(l) = ctx.label {
            let t = self.text(l);
            if !t.is_empty() {
                return t;
            }
        }
        if let Some(t) = self.attr(n, "title") {
            let t = collapse(t);
            if !t.is_empty() {
                return t;
            }
        }
        collapse(self.attr(n, "placeholder").unwrap_or(""))
    }

    fn role_attr(&self, n: usize) -> Option<String> {
        self.first_token(n, "role")
    }

    fn add(&mut self, mut node: A11yNode) -> Option<usize> {
        if self.nodes.len() >= MAX_NODES {
            return None;
        }
        node.anchor = short(&node.anchor);
        if let Some(id) = self.attr(node.dom, "id") {
            node.id = id.to_string();
            let at = self.nodes.len();
            self.ids.entry(id.to_string()).or_insert(at);
        }
        node.tab_index = self.attr(node.dom, "tabindex").and_then(|t| t.trim().parse().ok());
        self.nodes.push(node);
        Some(self.nodes.len() - 1)
    }

    fn blank(&self, dom: usize, role: Role, ctx: &Ctx) -> A11yNode {
        A11yNode {
            dom,
            role,
            name: String::new(),
            description: String::new(),
            level: 0,
            state: AState::default(),
            value: String::new(),
            href: String::new(),
            parent: ctx.parent,
            depth: ctx.depth,
            input_type: String::new(),
            group: String::new(),
            form: ctx.form,
            pos: (0, 0),
            rows: 0,
            cols: 0,
            col_header: String::new(),
            tab_index: None,
            focusable: false,
            anchor: String::new(),
            id: String::new(),
            control: None,
            link: None,
            minlength: 0,
            pattern_hint: String::new(),
            novalidate: false,
        }
    }

    fn flush(&mut self, node: Option<usize>) {
        let t = collapse(&self.buf);
        self.buf.clear();
        if !t.is_empty() {
            self.script.push(ReadItem { text: t, node });
        }
    }

    fn say(&mut self, text: String, node: Option<usize>) {
        self.flush(None);
        if !text.trim().is_empty() {
            self.script.push(ReadItem { text, node });
        }
    }

    fn walk_children(&mut self, n: usize, ctx: &Ctx, depth: usize) {
        let kids = self.tree.nodes[n].children.clone();
        for c in kids {
            self.visit(c, ctx, depth + 1);
        }
    }

    fn base_ctx(&self, ctx: &Ctx, parent: Option<usize>) -> Ctx {
        Ctx { parent, form: ctx.form, label: ctx.label, disabled: ctx.disabled, depth: ctx.depth + 1, list: ctx.list, in_sectioning: ctx.in_sectioning }
    }

    fn fill_state(&self, n: usize, st: &mut AState, ctx: &Ctx) {
        st.disabled = self.has(n, "disabled") || self.is_true(n, "aria-disabled") || ctx.disabled;
        st.required = self.has(n, "required") || self.is_true(n, "aria-required");
        st.readonly = self.has(n, "readonly") || self.is_true(n, "aria-readonly");
        st.invalid = self.attr(n, "aria-invalid").is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "grammar" | "spelling"));
        if let Some(v) = self.attr(n, "aria-expanded") {
            st.expanded = Some(v.eq_ignore_ascii_case("true"));
        }
        if let Some(v) = self.attr(n, "aria-pressed") {
            if v.eq_ignore_ascii_case("mixed") {
                st.mixed = true;
            } else {
                st.pressed = Some(v.eq_ignore_ascii_case("true"));
            }
        }
        st.selected = self.is_true(n, "aria-selected");
        st.current = self.attr(n, "aria-current").is_some_and(|v| !v.eq_ignore_ascii_case("false"));
    }

    fn description_of(&self, n: usize) -> String {
        let mut parts = Vec::new();
        if let Some(d) = self.attr(n, "aria-describedby") {
            let t = self.by_ids(d);
            if !t.is_empty() {
                parts.push(t);
            }
        }
        if let Some(e) = self.attr(n, "aria-errormessage") {
            let t = self.by_ids(e);
            if !t.is_empty() && !parts.contains(&t) {
                parts.push(t);
            }
        }
        parts.join(". ")
    }

    fn visit(&mut self, n: usize, ctx: &Ctx, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let tag = match &self.tree.nodes[n].kind {
            NodeKind::Text(t) => {
                if self.mute == 0 {
                    self.buf.push_str(t);
                }
                return;
            }
            NodeKind::Element { tag, .. } => tag.clone(),
        };
        if self.hidden(n) {
            return;
        }
        // a closed disclosure shows only its summary
        if tag == "details" && !self.has(n, "open") {
            let sum = self.tree.nodes[n].children.iter().copied().find(|&c| self.tree.tag(c) == Some("summary"));
            self.flush(None);
            if let Some(s) = sum {
                self.visit(s, ctx, depth + 1);
            }
            return;
        }
        let role_attr = self.role_attr(n);
        let role = role_attr.as_deref();
        let block = BLOCKISH.contains(&tag.as_str());
        if matches!(role, Some("presentation" | "none")) && tag == "img" {
            return;
        }
        // live regions: what they hold is said when the page appears
        if matches!(role, Some("alert" | "status" | "alertdialog")) || self.attr(n, "aria-live").is_some_and(|v| matches!(v, "assertive" | "polite")) {
            let t = self.text(n);
            if !t.is_empty() && self.alerts.len() < 8 {
                self.alerts.push(t);
            }
        }
        match tag.as_str() {
            "a" | "area" if self.has(n, "href") && !matches!(role, Some("button" | "presentation" | "none")) => self.link(n, ctx, depth),
            "button" => self.button(n, ctx, depth),
            "summary" => self.summary(n, ctx, depth),
            "input" => self.input(n, ctx, role),
            "textarea" => self.textarea(n, ctx),
            "select" => self.select(n, ctx),
            "img" => self.image(n, ctx),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let lvl = tag.as_bytes()[1] - b'0';
                self.heading(n, ctx, depth, lvl);
            }
            "ul" | "ol" | "menu" if !matches!(role, Some("presentation" | "none")) => self.list(n, ctx, depth),
            "li" if ctx.list.is_some() => self.list_item(n, ctx, depth),
            "table" if !matches!(role, Some("presentation" | "none")) => self.table(n, ctx, depth),
            "form" => self.form(n, ctx, depth),
            "fieldset" => self.fieldset(n, ctx, depth),
            "pre" => {
                self.flush(None);
                let t = self.tree.text_of(n);
                let t = t.trim_end().to_string();
                if !t.is_empty() {
                    self.script.push(ReadItem { text: collapse(&t), node: None });
                }
                // links and fields inside still count as nodes
                self.mute += 1;
                self.walk_children(n, ctx, depth);
                self.mute -= 1;
            }
            _ => {
                if let Some(r) = role {
                    if self.by_role(n, r, ctx, depth) {
                        return;
                    }
                }
                if let Some(l) = landmark_of(&tag, role, ctx, self.name_from_attrs(n).is_some()) {
                    self.landmark(n, ctx, depth, l);
                    return;
                }
                if tag == "label" {
                    let c = Ctx { label: Some(n), ..self.base_ctx(ctx, ctx.parent) };
                    self.walk_children(n, &c, depth);
                    return;
                }
                if tag == "br" {
                    self.flush(None);
                    return;
                }
                if block {
                    self.flush(None);
                }
                let c = self.base_ctx(ctx, ctx.parent);
                self.walk_children(n, &c, depth);
                if block {
                    self.flush(None);
                }
            }
        }
    }

    /// Elements that get their meaning from `role` (custom widgets). Returns whether it was one.
    fn by_role(&mut self, n: usize, role: &str, ctx: &Ctx, depth: usize) -> bool {
        match role {
            "button" => self.button(n, ctx, depth),
            "link" => self.link(n, ctx, depth),
            "checkbox" | "switch" | "menuitemcheckbox" => self.aria_toggle(n, ctx, Role::Checkbox),
            "radio" | "menuitemradio" => self.aria_toggle(n, ctx, Role::Radio),
            "textbox" | "searchbox" => self.aria_textbox(n, ctx),
            "heading" => {
                let lvl = self.attr(n, "aria-level").and_then(|v| v.trim().parse::<u8>().ok()).filter(|l| (1..=6).contains(l)).unwrap_or(2);
                self.heading(n, ctx, depth, lvl);
            }
            "img" => self.image(n, ctx),
            "list" => self.list(n, ctx, depth),
            "listitem" if ctx.list.is_some() => self.list_item(n, ctx, depth),
            "tab" | "menuitem" | "option" | "treeitem" => self.button_like(n, ctx, depth, role),
            _ => return false,
        }
        true
    }

    fn link(&mut self, n: usize, ctx: &Ctx, depth: usize) {
        let mut nd = self.blank(n, Role::Link, ctx);
        nd.name = self.name_from_attrs(n).unwrap_or_else(|| {
            let t = self.text(n);
            if t.is_empty() { collapse(self.attr(n, "title").unwrap_or("")) } else { t }
        });
        nd.anchor = self.text(n);
        nd.href = self.attr(n, "href").unwrap_or("").to_string();
        nd.focusable = true;
        nd.description = self.description_of(n);
        let mut st = AState::default();
        self.fill_state(n, &mut st, ctx);
        nd.state = st;
        let idx = self.add(nd);
        // the words of a link are part of the paragraph around it
        let c = self.base_ctx(ctx, idx);
        self.walk_children(n, &c, depth);
    }

    fn button(&mut self, n: usize, ctx: &Ctx, depth: usize) {
        let mut nd = self.blank(n, Role::Button, ctx);
        nd.name = self.name_from_attrs(n).unwrap_or_else(|| {
            let t = self.text(n);
            if t.is_empty() { collapse(self.attr(n, "title").unwrap_or("")) } else { t }
        });
        nd.anchor = self.text(n);
        nd.focusable = true;
        nd.description = self.description_of(n);
        let mut st = AState::default();
        self.fill_state(n, &mut st, ctx);
        nd.state = st;
        let disabled = nd.state.disabled;
        nd.focusable = !disabled;
        let name = nd.name.clone();
        let idx = self.add(nd);
        self.flush(None);
        if !name.is_empty() {
            self.script.push(ReadItem { text: format!("{name}, button"), node: idx });
        }
        self.mute += 1;
        let c = self.base_ctx(ctx, idx);
        self.walk_children(n, &c, depth);
        self.mute -= 1;
    }

    fn button_like(&mut self, n: usize, ctx: &Ctx, depth: usize, role: &str) {
        self.button(n, ctx, depth);
        if let Some(i) = self.nodes.iter().rposition(|x| x.dom == n) {
            if role == "tab" || role == "option" {
                // a tab or an option says whether it is the chosen one
                self.nodes[i].state.selected = self.is_true(n, "aria-selected");
            }
        }
    }

    fn summary(&mut self, n: usize, ctx: &Ctx, depth: usize) {
        let parent_open = self.tree.nodes[n].parent != crate::html::NONE && self.has(self.tree.nodes[n].parent, "open");
        let mut nd = self.blank(n, Role::Button, ctx);
        nd.name = self.name_from_attrs(n).unwrap_or_else(|| self.text(n));
        nd.anchor = nd.name.clone();
        nd.focusable = true;
        nd.state.expanded = Some(parent_open);
        let idx = self.add(nd);
        let nm = idx.map(|i| self.nodes[i].name.clone()).unwrap_or_default();
        self.say(format!("{nm}, button, {}", if parent_open { "expanded" } else { "collapsed" }), idx);
        self.mute += 1;
        let c = self.base_ctx(ctx, idx);
        self.walk_children(n, &c, depth);
        self.mute -= 1;
    }

    fn input(&mut self, n: usize, ctx: &Ctx, role: Option<&str>) {
        let ty = self.attr(n, "type").unwrap_or("text").to_ascii_lowercase();
        let r = match ty.as_str() {
            "hidden" => return,
            "checkbox" => Role::Checkbox,
            "radio" => Role::Radio,
            "password" => Role::PasswordField,
            "submit" | "reset" | "button" | "image" | "file" => Role::Button,
            "range" => Role::Slider,
            _ => Role::TextField,
        };
        let r = match role {
            Some("combobox" | "searchbox" | "textbox") if r == Role::TextField => Role::TextField,
            _ => r,
        };
        let mut nd = self.blank(n, r, ctx);
        nd.input_type = ty.clone();
        let mut st = AState::default();
        self.fill_state(n, &mut st, ctx);
        if matches!(r, Role::Checkbox | Role::Radio) {
            st.checked = Some(self.has(n, "checked") || self.is_true(n, "aria-checked"));
        }
        if r == Role::Button {
            let default = match ty.as_str() {
                "submit" => "Submit",
                "reset" => "Reset",
                "file" => "Choose file",
                _ => "",
            };
            nd.name = self.name_from_attrs(n).or_else(|| self.attr(n, "value").map(collapse).filter(|v| !v.is_empty())).or_else(|| self.attr(n, "alt").map(collapse).filter(|v| !v.is_empty())).unwrap_or_else(|| default.to_string());
        } else {
            nd.name = self.field_name(n, ctx);
        }
        if r != Role::PasswordField && r != Role::Button {
            nd.value = collapse(self.attr(n, "value").unwrap_or(""));
        }
        nd.group = self.attr(n, "name").unwrap_or("").to_string();
        nd.minlength = self.attr(n, "minlength").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        nd.pattern_hint = self.attr(n, "title").unwrap_or("").to_string();
        nd.description = self.description_of(n);
        nd.state = st;
        nd.focusable = !nd.state.disabled;
        nd.anchor = nd.name.clone();
        let idx = self.add(nd);
        self.field_line(idx);
    }

    fn field_line(&mut self, idx: Option<usize>) {
        if let Some(i) = idx {
            let t = speak_node(&self.nodes[i]);
            self.say(t, Some(i));
        }
    }

    fn textarea(&mut self, n: usize, ctx: &Ctx) {
        let mut nd = self.blank(n, Role::TextArea, ctx);
        nd.name = self.field_name(n, ctx);
        nd.value = collapse(&self.tree.text_of(n));
        let mut st = AState::default();
        self.fill_state(n, &mut st, ctx);
        st.multiline = true;
        nd.state = st;
        nd.focusable = !nd.state.disabled;
        nd.description = self.description_of(n);
        nd.anchor = nd.name.clone();
        let idx = self.add(nd);
        self.field_line(idx);
    }

    fn select(&mut self, n: usize, ctx: &Ctx) {
        let mut nd = self.blank(n, Role::Select, ctx);
        nd.name = self.field_name(n, ctx);
        let opts: Vec<usize> = self.tree.nodes[n].children.iter().flat_map(|&c| if self.tree.tag(c) == Some("optgroup") { self.tree.nodes[c].children.clone() } else { vec![c] }).filter(|&c| self.tree.tag(c) == Some("option")).collect();
        let chosen = opts.iter().find(|&&o| self.has(o, "selected")).or(opts.first()).map(|&o| collapse(&self.tree.text_of(o))).unwrap_or_default();
        nd.value = chosen;
        nd.rows = opts.len();
        let mut st = AState::default();
        self.fill_state(n, &mut st, ctx);
        nd.state = st;
        nd.focusable = !nd.state.disabled;
        nd.description = self.description_of(n);
        nd.anchor = nd.name.clone();
        let idx = self.add(nd);
        self.field_line(idx);
    }

    fn aria_toggle(&mut self, n: usize, ctx: &Ctx, role: Role) {
        let mut nd = self.blank(n, role, ctx);
        nd.name = self.name_from_attrs(n).unwrap_or_else(|| self.text(n));
        let mut st = AState::default();
        self.fill_state(n, &mut st, ctx);
        match self.attr(n, "aria-checked") {
            Some(v) if v.eq_ignore_ascii_case("mixed") => {
                st.checked = Some(false);
                st.mixed = true;
            }
            Some(v) => st.checked = Some(v.eq_ignore_ascii_case("true")),
            None => st.checked = Some(false),
        }
        nd.group = self.attr(n, "name").unwrap_or("").to_string();
        nd.state = st;
        nd.focusable = !nd.state.disabled;
        nd.anchor = nd.name.clone();
        let idx = self.add(nd);
        self.field_line(idx);
    }

    fn aria_textbox(&mut self, n: usize, ctx: &Ctx) {
        let mut nd = self.blank(n, Role::TextField, ctx);
        nd.name = self.field_name(n, ctx);
        nd.value = self.text(n);
        let mut st = AState::default();
        self.fill_state(n, &mut st, ctx);
        st.multiline = self.is_true(n, "aria-multiline");
        nd.state = st;
        nd.focusable = !nd.state.disabled;
        nd.description = self.description_of(n);
        nd.anchor = nd.name.clone();
        let idx = self.add(nd);
        self.field_line(idx);
    }

    fn image(&mut self, n: usize, ctx: &Ctx) {
        let alt = self.attr(n, "alt");
        let labelled = self.name_from_attrs(n);
        // an empty alt marks a decorative picture: it is not in the tree and nothing is said
        if alt == Some("") && labelled.is_none() && !self.has(n, "title") {
            return;
        }
        let name = labelled.or_else(|| alt.map(collapse).filter(|a| !a.is_empty())).or_else(|| self.attr(n, "title").map(collapse)).unwrap_or_default();
        let mut nd = self.blank(n, Role::Image, ctx);
        nd.name = name;
        // the picture inside a link or a button is part of that name, not a stop of its own
        let inside = ctx.parent.is_some_and(|p| matches!(self.nodes[p].role, Role::Link | Role::Button));
        if inside {
            if self.mute == 0 {
                self.buf.push(' ');
                self.buf.push_str(&nd.name);
                self.buf.push(' ');
            }
            return;
        }
        let idx = self.add(nd);
        if let Some(i) = idx {
            let nm = self.nodes[i].name.clone();
            self.flush(None);
            if self.mute == 0 {
                self.script.push(ReadItem { text: if nm.is_empty() { "Image, no description".into() } else { format!("Image, {nm}") }, node: Some(i) });
            }
        }
    }

    fn heading(&mut self, n: usize, ctx: &Ctx, depth: usize, level: u8) {
        let mut nd = self.blank(n, Role::Heading, ctx);
        nd.level = level;
        nd.name = self.name_from_attrs(n).unwrap_or_else(|| self.text(n));
        nd.anchor = self.text(n);
        let idx = self.add(nd);
        self.flush(None);
        if let Some(i) = idx {
            let t = speak_node(&self.nodes[i]);
            self.script.push(ReadItem { text: t, node: Some(i) });
        }
        self.mute += 1;
        let c = self.base_ctx(ctx, idx);
        self.walk_children(n, &c, depth);
        self.mute -= 1;
        self.flush(None);
    }

    fn list(&mut self, n: usize, ctx: &Ctx, depth: usize) {
        let items = self.tree.nodes[n].children.iter().filter(|&&c| self.tree.tag(c) == Some("li") && !self.hidden(c)).count();
        let mut nd = self.blank(n, Role::List, ctx);
        nd.name = self.name_from_attrs(n).unwrap_or_default();
        nd.rows = items;
        nd.anchor = self.text(n);
        let idx = self.add(nd);
        self.flush(None);
        if let Some(i) = idx {
            self.script.push(ReadItem { text: speak_node(&self.nodes[i]), node: Some(i) });
        }
        let c = Ctx { list: idx, ..self.base_ctx(ctx, idx) };
        self.walk_children(n, &c, depth);
        self.flush(None);
    }

    fn list_item(&mut self, n: usize, ctx: &Ctx, depth: usize) {
        let list = ctx.list;
        let mut nd = self.blank(n, Role::ListItem, ctx);
        nd.anchor = self.text(n);
        nd.name = nd.anchor.clone();
        // position among the shown items of the list
        if let Some(l) = list {
            let ldom = self.nodes[l].dom;
            let sibs: Vec<usize> = self.tree.nodes[ldom].children.iter().copied().filter(|&c| self.tree.tag(c) == Some("li") && !self.hidden(c)).collect();
            nd.pos = (sibs.iter().position(|&s| s == n).map_or(0, |p| p + 1), sibs.len());
        }
        let idx = self.add(nd);
        self.flush(None);
        let c = Ctx { list: None, ..self.base_ctx(ctx, idx) };
        self.walk_children(n, &c, depth);
        self.flush(idx);
    }

    fn form(&mut self, n: usize, ctx: &Ctx, depth: usize) {
        let name = self.name_from_attrs(n).unwrap_or_default();
        let role = if matches!(self.role_attr(n).as_deref(), Some("search")) { Role::Landmark(Landmark::Search) } else if name.is_empty() { Role::Form } else { Role::Landmark(Landmark::Form) };
        let mut nd = self.blank(n, role, ctx);
        nd.name = name;
        nd.novalidate = self.has(n, "novalidate");
        nd.anchor = self.text(n);
        let idx = self.add(nd);
        self.flush(None);
        if let Some(i) = idx {
            if matches!(self.nodes[i].role, Role::Landmark(_)) {
                self.script.push(ReadItem { text: speak_node(&self.nodes[i]), node: Some(i) });
            }
        }
        let c = Ctx { form: idx, ..self.base_ctx(ctx, idx) };
        self.walk_children(n, &c, depth);
        self.flush(None);
    }

    fn fieldset(&mut self, n: usize, ctx: &Ctx, depth: usize) {
        let legend = self.tree.nodes[n].children.iter().copied().find(|&c| self.tree.tag(c) == Some("legend"));
        let mut nd = self.blank(n, Role::Group, ctx);
        nd.name = self.name_from_attrs(n).or_else(|| legend.map(|l| self.text(l))).unwrap_or_default();
        nd.anchor = nd.name.clone();
        let disabled = self.has(n, "disabled") || ctx.disabled;
        nd.state.disabled = disabled;
        let idx = self.add(nd);
        self.flush(None);
        if let Some(i) = idx {
            if !self.nodes[i].name.is_empty() {
                self.script.push(ReadItem { text: speak_node(&self.nodes[i]), node: Some(i) });
            }
        }
        let c = Ctx { disabled, ..self.base_ctx(ctx, idx) };
        // the legend's words were just said as the group's name
        for ch in self.tree.nodes[n].children.clone() {
            if Some(ch) == legend {
                continue;
            }
            self.visit(ch, &c, depth + 1);
        }
        self.flush(None);
    }

    fn landmark(&mut self, n: usize, ctx: &Ctx, depth: usize, l: Landmark) {
        let mut nd = self.blank(n, Role::Landmark(l), ctx);
        nd.name = self.name_from_attrs(n).unwrap_or_default();
        nd.anchor = self.text(n);
        let idx = self.add(nd);
        self.flush(None);
        if let Some(i) = idx {
            self.script.push(ReadItem { text: speak_node(&self.nodes[i]), node: Some(i) });
        }
        let c = Ctx { in_sectioning: ctx.in_sectioning || matches!(l, Landmark::Main | Landmark::Region) || matches!(self.tree.tag(n), Some("article" | "section" | "aside" | "nav")), ..self.base_ctx(ctx, idx) };
        self.walk_children(n, &c, depth);
        self.flush(None);
    }

    fn table(&mut self, n: usize, ctx: &Ctx, depth: usize) {
        // rows may sit in thead / tbody / tfoot
        let mut rows: Vec<usize> = Vec::new();
        let mut todo: Vec<usize> = self.tree.nodes[n].children.iter().rev().copied().collect();
        while let Some(c) = todo.pop() {
            match self.tree.tag(c) {
                Some("tr") if !self.hidden(c) => rows.push(c),
                Some("thead" | "tbody" | "tfoot") if !self.hidden(c) => todo.extend(self.tree.nodes[c].children.iter().rev().copied()),
                _ => {}
            }
        }
        let cells_of = |s: &Self, r: usize| -> Vec<usize> { s.tree.nodes[r].children.iter().copied().filter(|&c| matches!(s.tree.tag(c), Some("td" | "th")) && !s.hidden(c)).collect() };
        let cols = rows.iter().map(|&r| cells_of(self, r).len()).max().unwrap_or(0);
        // the column headers: the cells of `th` in the first row that has any
        let header_row = rows.iter().copied().find(|&r| cells_of(self, r).iter().any(|&c| self.tree.tag(c) == Some("th")));
        let headers: Vec<String> = header_row.map(|r| cells_of(self, r).iter().map(|&c| self.text(c)).collect()).unwrap_or_default();
        let caption = self.tree.nodes[n].children.iter().copied().find(|&c| self.tree.tag(c) == Some("caption")).map(|c| self.text(c));
        let mut nd = self.blank(n, Role::Table, ctx);
        nd.name = self.name_from_attrs(n).or(caption).unwrap_or_default();
        nd.rows = rows.len();
        nd.cols = cols;
        nd.anchor = self.text(n);
        let idx = self.add(nd);
        self.flush(None);
        if let Some(i) = idx {
            self.script.push(ReadItem { text: speak_node(&self.nodes[i]), node: Some(i) });
        }
        let mut c = self.base_ctx(ctx, idx);
        c.list = None;
        for (ri, &r) in rows.iter().enumerate() {
            let mut rn = self.blank(r, Role::Row, &c);
            rn.pos = (ri + 1, rows.len());
            rn.anchor = self.text(r);
            let rid = self.add(rn);
            let cells = cells_of(self, r);
            let mut said: Vec<String> = Vec::new();
            for (ci, &cell) in cells.iter().enumerate() {
                let is_th = self.tree.tag(cell) == Some("th");
                let row_header = is_th && (self.attr(cell, "scope") == Some("row") || (Some(r) != header_row && ci == 0));
                let role = if is_th && !row_header { Role::ColumnHeader } else if row_header { Role::RowHeader } else { Role::Cell };
                let cc = Ctx { parent: rid, depth: c.depth + 1, ..self.base_ctx(&c, rid) };
                let mut cn = self.blank(cell, role, &cc);
                cn.name = self.text(cell);
                cn.anchor = cn.name.clone();
                cn.pos = (ci + 1, cells.len());
                if role == Role::Cell || role == Role::RowHeader {
                    cn.col_header = headers.get(ci).cloned().unwrap_or_default();
                }
                let hdr = cn.col_header.clone();
                let text = cn.name.clone();
                let cid = self.add(cn);
                let _ = cid;
                // links and fields inside a cell are nodes of their own
                self.mute += 1;
                self.walk_children(cell, &cc, depth + 2);
                self.mute -= 1;
                if !text.is_empty() {
                    said.push(if role == Role::Cell && !hdr.is_empty() { format!("{hdr}: {text}") } else { text });
                }
            }
            self.flush(None);
            if !said.is_empty() {
                self.script.push(ReadItem { text: format!("Row {} of {}. {}", ri + 1, rows.len(), said.join(", ")), node: rid });
            }
        }
    }

    fn finish(mut self) -> A11yTree {
        self.flush(None);
        let mut nodes = std::mem::take(&mut self.nodes);
        // radio positions: by group name inside one form
        let mut groups: HashMap<(Option<usize>, String), Vec<usize>> = HashMap::new();
        for (i, nd) in nodes.iter().enumerate() {
            if nd.role == Role::Radio && !nd.group.is_empty() {
                groups.entry((nd.form, nd.group.clone())).or_default().push(i);
            }
        }
        for v in groups.values() {
            for (k, &i) in v.iter().enumerate() {
                nodes[i].pos = (k + 1, v.len());
            }
        }
        // an element id points at the first node at or after that element
        let mut ids = HashMap::new();
        for (id, &dom) in &self.id_dom {
            if let Some(at) = nodes.iter().position(|nd| nd.dom >= dom) {
                ids.insert(id.clone(), at);
            }
        }
        for (id, at) in self.ids.drain() {
            ids.insert(id, at);
        }
        A11yTree { nodes, script: self.script, alerts: self.alerts, ids }
    }
}

fn landmark_of(tag: &str, role: Option<&str>, ctx: &Ctx, named: bool) -> Option<Landmark> {
    match role {
        Some("navigation") => return Some(Landmark::Navigation),
        Some("main") => return Some(Landmark::Main),
        Some("banner") => return Some(Landmark::Banner),
        Some("contentinfo") => return Some(Landmark::ContentInfo),
        Some("complementary") => return Some(Landmark::Complementary),
        Some("search") => return Some(Landmark::Search),
        Some("region") => return Some(Landmark::Region),
        _ => {}
    }
    match tag {
        "nav" => Some(Landmark::Navigation),
        "main" => Some(Landmark::Main),
        "aside" => Some(Landmark::Complementary),
        // a page-level header and footer are landmarks; inside an article or section they only head or end it
        "header" if !ctx.in_sectioning => Some(Landmark::Banner),
        "footer" if !ctx.in_sectioning => Some(Landmark::ContentInfo),
        "section" if named => Some(Landmark::Region),
        _ => None,
    }
}

/// Build the tree of a page. `hide` says which elements the style sheets removed (`display:none`); pass `&|_| false` without styles.
pub fn build(tree: &Tree, hide: &dyn Fn(usize) -> bool) -> A11yTree {
    let mut labels = HashMap::new();
    let mut id_dom = HashMap::new();
    for (i, nd) in tree.nodes.iter().enumerate() {
        if let NodeKind::Element { tag, attrs } = &nd.kind {
            if let Some((_, id)) = attrs.iter().find(|(k, _)| k == "id") {
                id_dom.entry(id.clone()).or_insert(i);
            }
            if tag == "label" {
                if let Some((_, f)) = attrs.iter().find(|(k, _)| k == "for") {
                    labels.entry(f.clone()).or_insert(i);
                }
            }
        }
    }
    let mut b = Builder { tree, hide, nodes: Vec::new(), script: Vec::new(), buf: String::new(), mute: 0, labels, ids: HashMap::new(), id_dom, alerts: Vec::new() };
    let ctx = Ctx { parent: None, form: None, label: None, disabled: false, depth: 0, list: None, in_sectioning: false };
    b.visit(0, &ctx, 0);
    b.finish()
}

/// Build without style sheets.
pub fn from_tree(tree: &Tree) -> A11yTree {
    build(tree, &|_| false)
}

/// Parse and build in one step (tests and tools).
pub fn from_html(src: &str) -> A11yTree {
    from_tree(&crate::html::parse(src))
}

/// What a reader says for a node: "Sign in, button", "Heading level 2, Pricing", "Email, edit text, required".
pub fn speak_node(n: &A11yNode) -> String {
    let mut p: Vec<String> = Vec::new();
    let name = n.name.trim();
    let st = &n.state;
    match n.role {
        Role::Heading => {
            p.push(if name.is_empty() { format!("Heading level {}", n.level) } else { format!("Heading level {}, {name}", n.level) });
        }
        Role::Image => {
            p.push(if name.is_empty() { "Image, no description".into() } else { format!("Image, {name}") });
        }
        Role::Landmark(l) => {
            p.push(if name.is_empty() { let w = l.word(); format!("{}{}", w[..1].to_uppercase(), &w[1..]) } else { format!("{name}, {}", l.word()) });
        }
        Role::List => {
            p.push(if n.rows == 1 { "List, 1 item".to_string() } else { format!("List, {} items", n.rows) });
            if !name.is_empty() {
                p.insert(0, name.to_string());
            }
        }
        Role::ListItem => {
            let pos = if n.pos.1 > 0 { format!("Item {} of {}", n.pos.0, n.pos.1) } else { "Item".to_string() };
            p.push(if name.is_empty() { pos } else { format!("{pos}, {name}") });
        }
        Role::Table => {
            let size = format!("Table, {} row{}, {} column{}", n.rows, if n.rows == 1 { "" } else { "s" }, n.cols, if n.cols == 1 { "" } else { "s" });
            p.push(if name.is_empty() { size } else { format!("{name}, {size}") });
        }
        Role::Row => p.push(format!("Row {} of {}", n.pos.0, n.pos.1)),
        Role::ColumnHeader => p.push(format!("{name}, column header")),
        Role::RowHeader => p.push(format!("{name}, row header")),
        Role::Cell => {
            let who = if n.col_header.is_empty() { String::new() } else { format!(", column {}", n.col_header) };
            p.push(format!("{name}{who}"));
        }
        Role::Group => p.push(if name.is_empty() { "Group".into() } else { format!("{name}, group") }),
        Role::Form => p.push("Form".into()),
        other => {
            p.push(if name.is_empty() { "unlabelled".to_string() } else { name.to_string() });
            p.push(
                match other {
                    Role::Link => "link",
                    Role::Button => "button",
                    Role::TextField => "edit text",
                    Role::PasswordField => "password edit text",
                    Role::TextArea => "multi-line edit text",
                    Role::Checkbox => "checkbox",
                    Role::Radio => "radio button",
                    Role::Select => "combo box",
                    Role::Slider => "slider",
                    _ => "",
                }
                .to_string(),
            );
            if let Some(c) = st.checked {
                if other == Role::Radio {
                    p.push(if c { "selected".into() } else { "not selected".into() });
                    if n.pos.1 > 1 {
                        p.push(format!("{} of {}", n.pos.0, n.pos.1));
                    }
                } else {
                    p.push(if st.mixed { "partially checked".into() } else if c { "checked".into() } else { "not checked".into() });
                }
            }
            if other == Role::Button {
                if st.mixed {
                    p.push("mixed".into());
                } else if let Some(pr) = st.pressed {
                    p.push(if pr { "pressed".into() } else { "not pressed".into() });
                }
            }
        }
    }
    if st.selected && n.role != Role::Radio {
        p.push("selected".into());
    }
    if let Some(e) = st.expanded {
        // a summary already said it
        if n.role != Role::Button || !p.iter().any(|x| x == "expanded" || x == "collapsed") {
            p.push(if e { "expanded".into() } else { "collapsed".into() });
        }
    }
    if matches!(n.role, Role::TextField | Role::PasswordField | Role::TextArea | Role::Select | Role::Checkbox | Role::Radio) && st.required {
        p.push("required".into());
    }
    if st.invalid {
        p.push("invalid".into());
    }
    if st.readonly {
        p.push("read only".into());
    }
    if matches!(n.role, Role::TextField | Role::TextArea | Role::Select | Role::Slider) && !n.value.is_empty() {
        p.push(n.value.clone());
    }
    if st.current {
        p.push("current".into());
    }
    if st.disabled {
        p.push("unavailable".into());
    }
    if !n.description.is_empty() {
        p.push(n.description.clone());
    }
    p.retain(|x| !x.is_empty());
    p.join(", ")
}

impl A11yTree {
    pub fn speak(&self, i: usize) -> String {
        self.nodes.get(i).map(speak_node).unwrap_or_default()
    }

    /// Match the nodes with the page model: which node is control `i` of the page and which is link `j`.
    pub fn attach(&mut self, links: &[String], controls: &[Control]) {
        let by_dom: HashMap<usize, usize> = self.nodes.iter().enumerate().map(|(i, n)| (n.dom, i)).collect();
        for (ci, c) in controls.iter().enumerate() {
            if c.kind == ControlKind::Hidden {
                continue;
            }
            if let Some(&i) = by_dom.get(&c.node) {
                self.nodes[i].control = Some(ci);
            }
        }
        // links are numbered in document order; the page may have dropped some, so match by address in order
        let mut j = 0;
        for n in self.nodes.iter_mut().filter(|n| n.role == Role::Link) {
            if let Some(k) = (j..links.len()).find(|&k| links[k] == n.href) {
                n.link = Some(k);
                j = k + 1;
            }
        }
    }

    pub fn stats(&self) -> Stats {
        let mut s = Stats::default();
        for n in &self.nodes {
            match n.role {
                Role::Link => s.links += 1,
                Role::Button => s.buttons += 1,
                Role::Heading => s.headings += 1,
                Role::Landmark(_) => s.landmarks += 1,
                Role::Table => s.tables += 1,
                Role::List => s.lists += 1,
                Role::Image => {
                    s.images += 1;
                    if n.name.is_empty() {
                        s.images_without_alt += 1;
                    }
                }
                Role::TextField | Role::PasswordField | Role::TextArea | Role::Checkbox | Role::Radio | Role::Select | Role::Slider => {
                    s.fields += 1;
                    if n.name.is_empty() {
                        s.unlabelled_fields += 1;
                    }
                    if n.state.invalid {
                        s.invalid_fields += 1;
                    }
                }
                _ => {}
            }
        }
        s
    }

    /// One line about the page for a status message or the assistant.
    pub fn summary(&self) -> String {
        let s = self.stats();
        let mut parts = vec![format!("{} links", s.links), format!("{} headings", s.headings), format!("{} landmarks", s.landmarks), format!("{} form fields", s.fields), format!("{} buttons", s.buttons)];
        if s.tables > 0 {
            parts.push(format!("{} tables", s.tables));
        }
        if s.unlabelled_fields > 0 {
            parts.push(format!("{} fields without a label", s.unlabelled_fields));
        }
        if s.images_without_alt > 0 {
            parts.push(format!("{} images without a description", s.images_without_alt));
        }
        if s.invalid_fields > 0 {
            parts.push(format!("{} fields marked invalid", s.invalid_fields));
        }
        parts.join(", ")
    }

    pub fn is_landmark(&self, i: usize) -> bool {
        match self.nodes.get(i).map(|n| n.role) {
            Some(Role::Landmark(_)) => true,
            _ => false,
        }
    }

    /// The nodes the keyboard focus can reach (Tab order is made by [`crate::a11y_nav::tab_order`]).
    pub fn is_field(&self, i: usize) -> bool {
        matches!(self.nodes.get(i).map(|n| n.role), Some(Role::TextField | Role::PasswordField | Role::TextArea | Role::Checkbox | Role::Radio | Role::Select | Role::Slider))
    }

    /// Why a field's current text would not pass the page's own rules (`required`, `type=email`, `minlength`), in words; `None` if it passes.
    /// This is what a browser checks before it sends a form.
    pub fn check_value(&self, i: usize, value: &str, checked: bool) -> Option<String> {
        let n = self.nodes.get(i)?;
        if n.state.disabled {
            return None;
        }
        let v = value.trim();
        match n.role {
            Role::Checkbox if n.state.required && !checked => return Some("must be checked".into()),
            Role::Radio => return None,
            _ => {}
        }
        if !matches!(n.role, Role::TextField | Role::PasswordField | Role::TextArea | Role::Select) {
            return None;
        }
        if n.state.required && v.is_empty() {
            return Some("is required".into());
        }
        if v.is_empty() {
            return None;
        }
        match n.input_type.as_str() {
            "email" if !(v.contains('@') && !v.starts_with('@') && !v.ends_with('@')) => return Some("needs an email address with an at sign".into()),
            "url" if !(v.starts_with("http://") || v.starts_with("https://")) => return Some("needs a web address starting with http".into()),
            "number" if v.parse::<f64>().is_err() => return Some("needs a number".into()),
            _ => {}
        }
        if n.minlength > 0 && v.chars().count() < n.minlength {
            return Some(format!("needs at least {} characters", n.minlength));
        }
        None
    }
}

#[cfg(test)]
#[path = "a11y_tests.rs"]
mod tests;
