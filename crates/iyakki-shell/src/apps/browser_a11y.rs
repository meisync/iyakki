//! Accessibility of the built-in Browser tile (docs/ACCESSIBILITY.md, "Browser"). A child module of `browser`, so it can use the
//! tab's fields.
//!
//! * The page's accessibility tree (`iyakki_web::a11y`, built with the page) is walked with the keyboard: Tab and Shift+Tab over
//!   links and controls in page order, the quick keys of **browse mode** (V turns it on and off; H and 1 to 6 headings, L links,
//!   F form fields, B buttons, T tables, R landmarks, I lists, G pictures; Shift goes back), M skips to the main part, K / J / E / Z
//!   list the links, headings, form fields and landmarks (type to search, Enter goes there), A reads the article aloud, W the
//!   whole page in order, A again pauses, Esc stops. Arrow keys move inside a group of radio buttons.
//! * Speech goes through the small [`PageSpeech`] trait; the real one hands sentences to the operating system's screen reader
//!   (`iyakki_uikit::access`), tests pass a fake that records. Nothing here speaks unless the OS screen reader is on.
//! * Looks: the high-contrast palette of the OS theme replaces the page's colours when asked (or when the computer is in High
//!   contrast), a minimum font size, moving pictures stand still with Reduce motion, a thicker focus ring with the big ring option.
//!   The text size of Settings -> Display already scales the whole tile, the page included (`s`), the per-site zoom comes on top.
//! * Forms: a failed send (a `required` field empty, an email without an at sign) stops, puts the focus on the field and says why; a
//!   page that came back from a send with `aria-invalid` fields gets the focus on the first one and says how many there are.
//!
//! Not covered: pages shown by the full web engine (F), canvas, pictures of text, content a script builds after the page loaded
//! (the tree is made once per page), custom widgets without a role.

use super::*;
use iyakki_agent::browsera11y as cfg;
use iyakki_web::a11y::{speak_node, A11yTree, ReadItem, Role as WRole};
use iyakki_web::a11y_nav::{self as nav, Entry, Quick, Rotor};
use iyakki_web::a11y_style::{Adapt, Palette};
use std::time::SystemTime;

/// What the browser says out loud. The operating system's screen reader is the real listener ([`OsSpeech`]); a test passes a fake.
pub trait PageSpeech {
    /// The element the person moved to: replaces whatever is being said.
    fn focus(&mut self, text: &str);
    /// Something to say after the current sentence (a page loaded, a message, the next paragraph of "read page").
    fn notice(&mut self, text: &str);
}

/// The real thing: sentences go to the screen reader of the shell, which speaks them only when it is on.
pub struct OsSpeech;

impl PageSpeech for OsSpeech {
    fn focus(&mut self, text: &str) {
        iyakki_uikit::access::report_focus(iyakki_uikit::access::Node::new(text, iyakki_uikit::access::Role::Spoken, iyakki_uikit::access::State::default()));
    }
    fn notice(&mut self, text: &str) {
        iyakki_uikit::access::announce(text);
    }
}

enum Say {
    Focus(String),
    Notice(String),
}

pub(super) struct RotorUi {
    pub(super) kind: Rotor,
    pub(super) query: String,
    pub(super) all: Vec<Entry>,
    pub(super) hits: Vec<usize>,
    pub(super) sel: usize,
}

pub(super) struct Reading {
    pub(super) items: Vec<ReadItem>,
    pub(super) next: usize,
    pub(super) paused: bool,
    due: u64,
}

/// First row of the list's clickable rows (`Event::Tab`).
const ROW_TAB: usize = 8000;
const ROWS_MAX: usize = 300;

pub(super) struct A11y {
    pub(super) eff: cfg::Effective,
    /// Browse mode switched by V in this session (`None`: as the choices say).
    browse_user: Option<bool>,
    /// The element the virtual cursor is on (a node of the page's tree).
    pub(super) cursor: Option<usize>,
    /// The visit the cursor and the announcement belong to.
    serial: u64,
    pub(super) rotor: Option<RotorUi>,
    pub(super) reading: Option<Reading>,
    queue: Vec<Say>,
    pub(super) speech: Box<dyn PageSpeech>,
    stamp: Option<[Option<SystemTime>; 4]>,
    /// The choices were set by a test: do not read the files.
    pinned: bool,
    /// A form was just sent: the next page shows whether it was accepted.
    pub(super) submit_pending: bool,
    last_status: String,
    ticks: u32,
    summary_for: u64,
}

impl A11y {
    pub(super) fn new() -> A11y {
        A11y {
            eff: cfg::resolve(&cfg::BrowserA11y::default(), &iyakki_agent::accessibility::Access::default(), false),
            browse_user: None,
            cursor: None,
            serial: 0,
            rotor: None,
            reading: None,
            queue: Vec::new(),
            speech: Box::new(OsSpeech),
            stamp: None,
            pinned: false,
            submit_pending: false,
            last_status: String::new(),
            ticks: 0,
            summary_for: u64::MAX,
        }
    }
}

/// The palette of forced colours: the OS High contrast theme.
pub(super) fn os_palette() -> Palette {
    let t = &iyakki_uikit::theme::HIGH_CONTRAST;
    Palette { bg: t.bg_top, fg: t.text, link: t.accent, visited: t.cyan, focus: t.accent, border: t.border }
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// The `k`-th place where `anchor` (lower case words) occurs in the drawn page, as the rectangle round it (page coordinates).
fn find_anchor(prims: &[Prim], anchor: &str, k: usize) -> Option<R> {
    if anchor.is_empty() {
        return None;
    }
    let (chars, map) = text_stream(prims);
    // letters in lower case, runs of white space as one space, remembering the place in the stream
    let mut flat: Vec<char> = Vec::with_capacity(chars.len());
    let mut at: Vec<usize> = Vec::with_capacity(chars.len());
    let mut space = true;
    for (i, c) in chars.iter().enumerate() {
        if c.is_whitespace() {
            if !space {
                flat.push(' ');
                at.push(i);
            }
            space = true;
        } else {
            flat.push(c.to_lowercase().next().unwrap_or(*c));
            at.push(i);
            space = false;
        }
    }
    let a: Vec<char> = anchor.chars().collect();
    let mut found: Vec<usize> = Vec::new();
    let mut i = 0;
    while i + a.len() <= flat.len() {
        if flat[i..i + a.len()] == a[..] {
            found.push(i);
            i += a.len();
        } else {
            i += 1;
        }
    }
    let start = *found.get(k).or(found.last())?;
    let first = map.get(at[start]).copied().flatten()?.0;
    let last_char = at[(start + a.len() - 1).min(at.len() - 1)];
    let last = map.get(last_char).copied().flatten().map_or(first, |m| m.0).max(first).min(first + 12);
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in prims[first..=last].iter().filter(|p| matches!(p.kind, Kind::Text { .. })) {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x + p.w);
        y1 = y1.max(p.y + p.h);
    }
    (x0 < x1).then(|| R::new(x0, y0, x1 - x0, y1 - y0))
}

/// Which occurrence of its words in the page a node is: nodes before it whose words are the same, not counting a node that only
/// repeats its parent (the link inside a list item says the same words once).
fn occurrence(tree: &A11yTree, node: usize) -> usize {
    let same_as_parent = |i: usize| tree.nodes[i].parent.is_some_and(|p| tree.nodes[p].anchor == tree.nodes[i].anchor);
    let mut top = node;
    while same_as_parent(top) {
        top = tree.nodes[top].parent.unwrap_or(top);
    }
    let a = &tree.nodes[top].anchor;
    (0..top).filter(|&j| &tree.nodes[j].anchor == a && !same_as_parent(j)).count()
}

/// The sentences of a plain (reader view) page in order.
fn doc_script(doc: &Doc) -> Vec<ReadItem> {
    fn walk(blocks: &[iyakki_web::doc::Block], doc: &Doc, out: &mut Vec<ReadItem>) {
        use iyakki_web::doc::Block;
        for b in blocks {
            match b {
                Block::Text(t) => {
                    let text: String = t.spans.iter().map(|s| s.text.as_str()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
                    for img in t.spans.iter().filter_map(|s| s.image).filter_map(|i| doc.images.get(i)).filter(|i| !i.alt.trim().is_empty()) {
                        out.push(ReadItem { text: format!("Image, {}", img.alt.trim()), node: None });
                    }
                    if text.is_empty() {
                        continue;
                    }
                    let line = if t.level > 0 { format!("Heading level {}, {text}", t.level) } else if !t.marker.is_empty() { format!("{} {text}", t.marker) } else { text };
                    out.push(ReadItem { text: line, node: None });
                }
                Block::Pre(p) => {
                    let t = p.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !t.is_empty() {
                        out.push(ReadItem { text: t, node: None });
                    }
                }
                Block::Box(x) => walk(&x.children, doc, out),
                Block::Row(r) => r.cols.iter().for_each(|c| walk(&c.blocks, doc, out)),
                Block::Table(t) => t.rows.iter().for_each(|r| {
                    let cells: Vec<String> = r.iter().map(|c| c.text()).filter(|c| !c.trim().is_empty()).collect();
                    if !cells.is_empty() {
                        out.push(ReadItem { text: cells.join(", "), node: None });
                    }
                }),
                Block::Rule => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&doc.blocks, doc, &mut out);
    out
}

impl BrowserApp {
    // ------------------------------------------------------------------------------------------------------- the choices

    /// Read the choices again when a file changed (the browser's own, Settings -> Accessibility, the theme).
    pub(super) fn a11y_refresh(&mut self, cx: &AppCx) {
        if self.a11y.pinned {
            return;
        }
        self.a11y.ticks = self.a11y.ticks.wrapping_add(1);
        if self.a11y.stamp.is_some() && self.a11y.ticks % 15 != 0 {
            return;
        }
        let (dir, user) = (cx.services.dir(), cx.session.user.as_str());
        let stamp = cfg::stamp(dir, user);
        if self.a11y.stamp == Some(stamp) {
            return;
        }
        self.a11y.stamp = Some(stamp);
        let eff = cfg::effective(dir, user);
        self.a11y_set_effective(eff);
    }

    pub(super) fn a11y_set_effective(&mut self, eff: cfg::Effective) {
        if self.a11y.eff != eff {
            // the colours, the font floor and the animation change how the page is laid out and drawn
            if self.a11y.eff.forced_colors != eff.forced_colors || self.a11y.eff.min_font_px != eff.min_font_px {
                self.layout = None;
            }
            self.a11y.eff = eff;
        }
    }

    /// For tests: use these choices and do not read the files.
    #[cfg(test)]
    pub(super) fn a11y_pin(&mut self, eff: cfg::Effective, speech: Box<dyn PageSpeech>) {
        self.a11y.pinned = true;
        self.a11y_set_effective(eff);
        self.a11y.speech = speech;
    }

    pub(super) fn a11y_browse(&self) -> bool {
        self.a11y.browse_user.unwrap_or(self.a11y.eff.browse_mode)
    }

    /// The tree of the page in the tab (not for pages the reader view or the browser itself made).
    pub(super) fn a11y_tree(&self) -> Option<Arc<A11yTree>> {
        self.page().and_then(|p| p.doc.access.clone())
    }

    /// The forced colours and the font floor for a page, or `None` when neither is asked for.
    pub(super) fn a11y_adapt(&self, doc: &Doc) -> Option<Doc> {
        let a = Adapt { palette: self.a11y.eff.forced_colors.then(os_palette), min_font_px: self.a11y.eff.min_font_px };
        if a.is_off() {
            return None;
        }
        let seen: std::collections::HashSet<&str> = self.data.history.iter().map(|v| v.url.as_str()).collect();
        let base = self.page().map(|p| p.url.clone());
        iyakki_web::a11y_style::adapt(doc, &a, &|href| base.as_ref().and_then(|b| b.join(href).ok()).is_some_and(|u| seen.contains(u.to_string().as_str())))
    }

    /// The colour and the thickness factor of the focus ring.
    pub(super) fn a11y_ring(&self) -> ([u8; 4], f32) {
        let color = if self.a11y.eff.forced_colors { os_palette().focus } else { ACCENT };
        (color, if self.a11y.eff.big_ring || self.a11y.eff.forced_colors { 2.0 } else { 1.0 })
    }

    // ---------------------------------------------------------------------------------------------------------- speaking

    fn a11y_focus_say(&mut self, text: String) {
        self.a11y.queue.push(Say::Focus(text));
    }

    fn a11y_notice(&mut self, text: impl Into<String>) {
        self.a11y.queue.push(Say::Notice(text.into()));
    }

    /// What a reader says for a node, with what the person typed or chose (the tree only knows the page as it was sent).
    fn a11y_speak(&self, tree: &A11yTree, node: usize) -> String {
        let mut n = tree.nodes[node].clone();
        if let (Some(ci), Some((c, st))) = (n.control, n.control.and_then(|i| self.control(i))) {
            let _ = ci;
            match n.role {
                WRole::Checkbox | WRole::Radio => n.state.checked = Some(st.checked),
                WRole::TextField | WRole::TextArea => n.value = st.value.split_whitespace().collect::<Vec<_>>().join(" "),
                WRole::Select => n.value = c.options.get(st.sel).map(|o| o.label.trim().to_string()).unwrap_or_default(),
                _ => {}
            }
        }
        speak_node(&n)
    }

    /// Hand what was queued to the speech, announce a page that just appeared, and pace "read page". Called every frame.
    pub(super) fn a11y_flush(&mut self, tick: u64) {
        self.a11y_page_changed();
        // the status line is spoken when it changes
        let status = self.status.as_ref().map(|s| s.0.clone()).unwrap_or_default();
        if status != self.a11y.last_status {
            if !status.is_empty() {
                self.a11y.queue.push(Say::Notice(status.clone()));
            }
            self.a11y.last_status = status;
        }
        if let Some(r) = &mut self.a11y.reading {
            if !r.paused && tick >= r.due {
                if let Some(item) = r.items.get(r.next).cloned() {
                    r.next += 1;
                    r.due = tick + (item.text.chars().count() as u64 * 2).max(12);
                    self.a11y.queue.push(Say::Notice(item.text));
                    if let Some(n) = item.node {
                        self.a11y.cursor = Some(n);
                    }
                } else {
                    self.a11y.reading = None;
                    self.a11y.queue.push(Say::Notice("End of page".into()));
                }
            }
        }
        let queue = std::mem::take(&mut self.a11y.queue);
        if !self.a11y.eff.screen_reader {
            return;
        }
        let last_focus = queue.iter().rposition(|s| matches!(s, Say::Focus(_)));
        for (i, s) in queue.into_iter().enumerate() {
            match s {
                Say::Focus(t) if Some(i) == last_focus => self.a11y.speech.focus(&t),
                Say::Focus(_) => {}
                Say::Notice(t) => self.a11y.speech.notice(&t),
            }
        }
    }

    /// A different page is showing: forget the cursor, say what it is, write the small summary for the assistant, and after a failed
    /// send put the focus on the first field the page marks invalid.
    fn a11y_page_changed(&mut self) {
        let serial = self.history.get(self.pos).map_or(0, |e| e.serial);
        if serial == self.a11y.serial {
            return;
        }
        self.a11y.serial = serial;
        self.a11y.cursor = None;
        self.a11y.reading = None;
        self.a11y.rotor = None;
        let sent = std::mem::take(&mut self.a11y.submit_pending);
        let (Some(page), Some(tree)) = (self.page().cloned(), self.a11y_tree()) else { return };
        let st = tree.stats();
        let title = if page.doc.title.trim().is_empty() { page.url.host.clone() } else { page.doc.title.trim().to_string() };
        let mut line = format!("{title}. {} links, {} headings", st.links, st.headings);
        if st.fields > 0 {
            line.push_str(&format!(", {} form fields", st.fields));
        }
        if self.a11y_browse() {
            line.push_str(". Browse mode is on");
        }
        self.a11y_notice(line);
        for a in tree.alerts.iter() {
            self.a11y_notice(format!("Alert. {a}"));
        }
        if sent {
            if let Some(i) = nav::first_invalid(&tree) {
                let n = st.invalid_fields;
                self.a11y_notice(format!("The form has {n} field{} with errors", if n == 1 { "" } else { "s" }));
                self.a11y_goto(i, None);
            }
        }
    }

    /// The facts about the page in the tab for the assistant's status tool: the site, the title and the counts. Never the address or
    /// the text.
    pub(super) fn a11y_summary(&self) -> Option<String> {
        let (page, tree) = (self.page()?, self.a11y_tree()?);
        let title = if page.doc.title.trim().is_empty() { "(no title)".to_string() } else { page.doc.title.trim().to_string() };
        Some(format!("{} \"{}\": {}", page.url.host, title, tree.summary()))
    }

    /// Write the small summary file once per page shown.
    pub(super) fn a11y_publish(&mut self, cx: &AppCx) {
        let serial = self.history.get(self.pos).map_or(0, |e| e.serial);
        if serial == self.a11y.summary_for {
            return;
        }
        if let Some(s) = self.a11y_summary() {
            self.a11y.summary_for = serial;
            cfg::write_page_summary(cx.services.dir(), &cx.session.user, &s);
        }
    }

    // ------------------------------------------------------------------------------------------------------------ moving

    /// The node the person is on: the focused link or control, else the virtual cursor.
    pub(super) fn a11y_here(&self, tree: &A11yTree) -> Option<usize> {
        if let Some(c) = self.focus_ctl {
            if let Some(i) = tree.nodes.iter().position(|n| n.control == Some(c)) {
                return Some(i);
            }
        }
        if let Some(l) = self.focus_link {
            if let Some(i) = tree.nodes.iter().position(|n| n.link == Some(l)) {
                return Some(i);
            }
        }
        self.a11y.cursor.filter(|&i| i < tree.nodes.len())
    }

    /// Where a node is on the drawn page.
    pub(super) fn a11y_locate(&self, tree: &A11yTree, node: usize) -> Option<R> {
        let l = self.layout.as_ref()?;
        let n = tree.nodes.get(node)?;
        if let Some(r) = n.link.and_then(|i| l.links.get(i).copied().flatten()) {
            return Some(r);
        }
        if let Some(r) = n.control.and_then(|i| l.controls.get(i).copied().flatten()) {
            return Some(r);
        }
        find_anchor(&l.prims, &norm(&n.anchor), occurrence(tree, node))
    }

    fn a11y_reveal(&mut self, r: R) {
        if r.y < self.scroll || r.y + r.h > self.scroll + self.view_h {
            self.scroll = (r.y - self.view_h * 0.3).max(0.0).min(self.max_scroll());
        }
    }

    /// Put the keyboard on a node: a link or a control gets the real focus (so Enter and Space work), anything else the cursor and
    /// its ring. The node is scrolled into view and said.
    pub(super) fn a11y_goto(&mut self, node: usize, prefix: Option<&str>) {
        let Some(tree) = self.a11y_tree() else { return };
        let Some(n) = tree.nodes.get(node) else { return };
        self.a11y.cursor = Some(node);
        self.addr_focus = false;
        match (n.link, n.control) {
            (Some(l), _) => {
                self.focus_link = Some(l);
                self.focus_ctl = None;
            }
            (None, Some(c)) => {
                self.focus_link = None;
                self.focus_control(c);
            }
            _ => {
                self.focus_link = None;
                self.focus_ctl = None;
            }
        }
        if let Some(r) = self.a11y_locate(&tree, node) {
            self.a11y_reveal(r);
        }
        let text = self.a11y_speak(&tree, node);
        self.a11y_focus_say(match prefix {
            Some(p) => format!("{p} {text}"),
            None => text,
        });
    }

    /// Tab and Shift+Tab by the page's own tab order. `false`: no tree, the old walk does it.
    pub(super) fn a11y_tab(&mut self, forward: bool) -> bool {
        let Some(tree) = self.a11y_tree() else { return false };
        if self.layout.is_none() {
            return false;
        }
        self.a11y.reading = None;
        let here = self.a11y_here(&tree);
        match nav::tab_step(&tree, here, !forward) {
            Some(n) => {
                self.a11y_goto(n, None);
                true
            }
            None => {
                self.a11y_notice("Nothing on this page can take the keyboard");
                true
            }
        }
    }

    fn a11y_quick(&mut self, q: Quick, back: bool) {
        let Some(tree) = self.a11y_tree() else { return };
        self.a11y.reading = None;
        let here = self.a11y_here(&tree);
        match nav::quick_wrap(&tree, here, q, back) {
            None => self.a11y_notice(format!("No {} on this page", q.plural())),
            Some((i, wrapped)) => self.a11y_goto(i, wrapped.then_some(if back { "Wrapped to the end." } else { "Wrapped to the start." })),
        }
    }

    /// A link to a place on the same page (`#story`): go there.
    pub(super) fn a11y_follow_fragment(&mut self, fragment: &str) {
        let Some(tree) = self.a11y_tree() else { return };
        match nav::fragment_target(&tree, &format!("#{fragment}")) {
            Some(n) => self.a11y_goto(n, None),
            None => {
                if fragment.is_empty() {
                    self.scroll = 0.0;
                }
            }
        }
    }

    fn a11y_skip_to_main(&mut self) {
        let Some(tree) = self.a11y_tree() else { return };
        let target = nav::skip_link(&tree).and_then(|s| nav::fragment_target(&tree, &tree.nodes[s].href)).or_else(|| nav::main_landmark(&tree));
        match target {
            Some(n) => self.a11y_goto(n, None),
            None => self.a11y_notice("This page has no main part to skip to"),
        }
    }

    // --------------------------------------------------------------------------------------------------------- the lists

    fn a11y_open_rotor(&mut self, kind: Rotor) {
        let Some(tree) = self.a11y_tree() else { return };
        let all = nav::entries(&tree, kind);
        if all.is_empty() {
            self.a11y_notice(format!("No {} on this page", kind.title().to_lowercase()));
            return;
        }
        let hits = nav::filter(&all, "");
        let first = all[0].label.clone();
        let n = all.len();
        // start on the entry after the cursor
        let here = self.a11y_here(&tree).unwrap_or(0);
        let sel = all.iter().position(|e| e.node >= here).unwrap_or(0);
        let label = all[sel].label.clone();
        let _ = first;
        self.a11y.rotor = Some(RotorUi { kind, query: String::new(), all, hits, sel });
        self.a11y_focus_say(format!("{} list, {n} item{}. Type to search, Up and Down choose, Enter goes there, Escape closes. {label}", kind.title(), if n == 1 { "" } else { "s" }));
    }

    fn a11y_rotor_say(&mut self) {
        let Some(r) = &self.a11y.rotor else { return };
        let text = match r.hits.get(r.sel).and_then(|&i| r.all.get(i)) {
            Some(e) => format!("{}, {} of {}", e.label, r.sel + 1, r.hits.len()),
            None => "No matches".to_string(),
        };
        self.a11y_focus_say(text);
    }

    pub(super) fn a11y_rotor_key(&mut self, k: &Key) {
        let Some(r) = &mut self.a11y.rotor else { return };
        let n = r.hits.len();
        match k {
            Key::Esc => {
                self.a11y.rotor = None;
                self.a11y_notice("List closed");
                return;
            }
            Key::Enter => {
                let node = r.hits.get(r.sel).and_then(|&i| r.all.get(i)).map(|e| e.node);
                self.a11y.rotor = None;
                if let Some(node) = node {
                    self.a11y_goto(node, None);
                }
                return;
            }
            Key::Down | Key::Tab if n > 0 => r.sel = (r.sel + 1).min(n - 1),
            Key::Up | Key::BackTab if n > 0 => r.sel = r.sel.saturating_sub(1),
            Key::Home => r.sel = 0,
            Key::End => r.sel = n.saturating_sub(1),
            Key::PageDown => r.sel = (r.sel + 8).min(n.saturating_sub(1)),
            Key::PageUp => r.sel = r.sel.saturating_sub(8),
            Key::Backspace => {
                r.query.pop();
                r.hits = nav::filter(&r.all, &r.query);
                r.sel = 0;
            }
            Key::Char(c) if !c.is_control() => {
                r.query.push(*c);
                r.hits = nav::filter(&r.all, &r.query);
                r.sel = 0;
            }
            _ => {}
        }
        self.a11y_rotor_say();
    }

    fn a11y_rotor_pick(&mut self, row: usize) {
        let Some(r) = &mut self.a11y.rotor else { return };
        let node = r.hits.get(row).and_then(|&i| r.all.get(i)).map(|e| e.node);
        self.a11y.rotor = None;
        if let Some(node) = node {
            self.a11y_goto(node, None);
        }
    }

    // ------------------------------------------------------------------------------------------------------------ reading

    /// A starts reading the article (the reader view's text) from the cursor or the top, A again pauses and then continues, Esc
    /// stops. Shift+A reads the whole page in order, with its menus and landmarks.
    fn a11y_read(&mut self, whole: bool) {
        if let Some(r) = &mut self.a11y.reading {
            r.paused = !r.paused;
            let word = if r.paused { "Paused. A goes on, Escape stops." } else { "Reading on." };
            self.a11y_focus_say(word.into());
            return;
        }
        let Some(page) = self.page().cloned() else {
            self.a11y_notice("Open a page first");
            return;
        };
        let items = match (&page.doc.access, whole) {
            (Some(t), true) => t.script.clone(),
            _ => match iyakki_web::reader::simplify(&page.doc) {
                Some(d) => doc_script(&d),
                None => match &page.doc.access {
                    Some(t) => t.script.clone(),
                    None => doc_script(&page.doc),
                },
            },
        };
        if items.is_empty() {
            self.a11y_notice("There is nothing to read on this page");
            return;
        }
        let from = self.a11y.cursor.and_then(|c| items.iter().position(|i| i.node == Some(c))).unwrap_or(0);
        self.a11y_focus_say(format!("Reading. A pauses, Escape stops. {}", if whole { "The whole page." } else { "The article." }));
        self.a11y.reading = Some(Reading { items, next: from, paused: false, due: 0 });
    }

    fn a11y_stop_reading(&mut self) -> bool {
        if self.a11y.reading.take().is_some() {
            self.a11y_focus_say("Stopped reading".into());
            return true;
        }
        false
    }

    // -------------------------------------------------------------------------------------------------------------- keys

    /// Keys of the page when the keyboard is on the page: lists, browse mode, reading, skip to main. `None`: not mine.
    pub(super) fn a11y_key(&mut self, k: &Key, _cx: &AppCx) -> Option<Flow> {
        if self.a11y.rotor.is_some() {
            self.a11y_rotor_key(k);
            return Some(Flow::Stay);
        }
        if self.page().is_none() || self.full.is_some() || self.pdf.is_some() {
            return None;
        }
        if matches!(k, Key::Esc) && self.a11y_stop_reading() {
            return Some(Flow::Stay);
        }
        let Key::Char(c) = k else { return None };
        match c {
            'v' => {
                let on = !self.a11y_browse();
                self.a11y.browse_user = Some(on);
                self.a11y_focus_say(if on { "Browse mode on. H and 1 to 6 headings, L links, F form fields, B buttons, T tables, R landmarks, I lists, G pictures. Shift goes back. V turns it off.".into() } else { "Browse mode off".into() });
                return Some(Flow::Stay);
            }
            'K' => self.a11y_open_rotor(Rotor::Links),
            'J' => self.a11y_open_rotor(Rotor::Headings),
            'E' => self.a11y_open_rotor(Rotor::Fields),
            'Z' => self.a11y_open_rotor(Rotor::Landmarks),
            'A' => self.a11y_read(false),
            'W' => self.a11y_read_whole(),
            'm' => self.a11y_skip_to_main(),
            '?' => {
                let text = self.a11y_summary().map_or("This page has no accessibility information".to_string(), |s| s.split_once(": ").map_or(s.clone(), |(_, rest)| rest.to_string()));
                self.a11y_notice(text);
            }
            '/' if self.a11y_browse() => self.open_find(),
            c if self.a11y_browse() => {
                if *c == 'a' {
                    return None;
                }
                let (q, back) = nav::quick_key(*c)?;
                self.a11y_quick(q, back);
            }
            _ => return None,
        }
        Some(Flow::Stay)
    }

    /// W: read the whole page in order, with its menus and landmarks (A reads the article).
    pub(super) fn a11y_read_whole(&mut self) {
        self.a11y.reading = None;
        self.a11y_read(true);
    }

    /// Keys while a form control has the keyboard: arrows inside a group of radio buttons. `None`: not mine.
    pub(super) fn a11y_control_key(&mut self, c: usize, k: &Key, cx: &AppCx) -> Option<Flow> {
        let tree = self.a11y_tree()?;
        let me = tree.nodes.iter().position(|n| n.control == Some(c))?;
        match (tree.nodes[me].role, k) {
            (WRole::Radio, Key::Up | Key::Left | Key::Down | Key::Right) => {
                let back = matches!(k, Key::Up | Key::Left);
                let target = nav::radio_step(&tree, me, back)?;
                let to = tree.nodes[target].control?;
                self.click_control(to, 0.0, cx);
                self.a11y.cursor = Some(target);
                let text = self.a11y_speak(&tree, target);
                self.a11y_focus_say(text);
                Some(Flow::Stay)
            }
            _ => None,
        }
    }

    /// After a press on a checkbox, radio button or drop-down that changed it: say the new state.
    pub(super) fn a11y_say_control(&mut self, c: usize) {
        let Some(tree) = self.a11y_tree() else { return };
        if let Some(i) = tree.nodes.iter().position(|n| n.control == Some(c)) {
            let text = self.a11y_speak(&tree, i);
            self.a11y_focus_say(text);
        }
    }

    /// Before a form is sent: the first field that does not pass the page's own rules (`required`, an email without an at sign, too
    /// short) stops the send, takes the focus and says why. Returns true when it stopped.
    pub(super) fn a11y_validate(&mut self, form: usize) -> bool {
        let (Some(tree), Some(page)) = (self.a11y_tree(), self.page().cloned()) else { return false };
        let ctls: Vec<usize> = page.doc.controls.iter().enumerate().filter(|(_, c)| c.form == Some(form)).map(|(i, _)| i).collect();
        let node_of = |ci: usize| tree.nodes.iter().position(|n| n.control == Some(ci));
        if let Some(f) = ctls.first().and_then(|&ci| node_of(ci)).and_then(|n| tree.nodes[n].form) {
            if tree.nodes[f].novalidate {
                return false;
            }
        }
        for ci in ctls {
            let Some(n) = node_of(ci) else { continue };
            let Some(st) = self.history.get(self.pos).and_then(|e| e.ctl.get(ci)) else { continue };
            let value = match tree.nodes[n].role {
                WRole::Select => page.doc.controls[ci].options.get(st.sel).map(|o| o.value.clone()).unwrap_or_default(),
                _ => st.value.clone(),
            };
            if let Some(why) = tree.check_value(n, &value, st.checked) {
                let name = if tree.nodes[n].name.is_empty() { "This field".to_string() } else { tree.nodes[n].name.clone() };
                self.focus_link = None;
                self.focus_control(ci);
                self.a11y.cursor = Some(n);
                if let Some(r) = self.layout.as_ref().and_then(|l| l.controls.get(ci).copied().flatten()) {
                    self.a11y_reveal(r);
                }
                self.status = Some((format!("{name} {why}. The form was not sent."), false));
                let text = self.a11y_speak(&tree, n);
                self.a11y_focus_say(format!("{name} {why}. {text}"));
                return true;
            }
        }
        self.a11y.submit_pending = true;
        false
    }

    // ------------------------------------------------------------------------------------------------------------ drawing

    /// The ring round a node that is neither a link nor a control (a heading, a landmark, a table), and the list over the page.
    pub(super) fn a11y_draw(&mut self, g: &mut Gfx, view: R, col_x: f32, top: f32, s: f32) {
        let (color, factor) = self.a11y_ring();
        if let (Some(tree), Some(n)) = (self.a11y_tree(), self.a11y.cursor) {
            if tree.nodes.get(n).is_some_and(|x| x.link.is_none() && x.control.is_none()) {
                if let Some(r) = self.a11y_locate(&tree, n) {
                    g.rrect_outline(col_x + r.x - 4.0 * s, top + r.y - self.scroll - 12.0 * s - 2.0, r.w + 8.0 * s, r.h + 4.0, 4.0 * s, 2.0 * s * factor, color);
                }
            }
        }
        // a thicker ring on the control that has the keyboard when the person asked for one
        if factor > 1.0 {
            if let Some(r) = self.focus_ctl.and_then(|c| self.layout.as_ref().and_then(|l| l.controls.get(c).copied().flatten())) {
                g.rrect_outline(col_x + r.x - 3.0 * s, top + r.y - self.scroll - 12.0 * s - 2.0, r.w + 6.0 * s, r.h + 4.0, 6.0 * s, 2.0 * s * factor, color);
            }
        }
        let Some(r) = &self.a11y.rotor else { return };
        let w = (view.w - 40.0 * s).min(560.0 * s);
        let rows = 9usize;
        let rh = 28.0 * s;
        let h = 56.0 * s + rows as f32 * rh;
        let (x, y) = (view.x + (view.w - w) / 2.0, view.y + 16.0 * s);
        let h = h.min(view.h - 24.0 * s);
        g.rrect(x, y, w, h, 10.0 * s, [8, 11, 24, 246]);
        g.rrect_outline(x, y, w, h, 10.0 * s, 1.5 * s * factor, color);
        g.text(x + 14.0 * s, y + 8.0 * s, 15.0 * s, color, &format!("{} · {} of {}", r.kind.title(), r.hits.len(), r.all.len()), Face::Bold);
        let q = if r.query.is_empty() { "Type to search".to_string() } else { r.query.clone() };
        g.text(x + 14.0 * s, y + 8.0 * s + 22.0 * s, 14.0 * s, if r.query.is_empty() { MUTED } else { TEXT }, &q, Face::Sans);
        let first = r.sel.saturating_sub(rows / 2).min(r.hits.len().saturating_sub(rows));
        for (i, &hit) in r.hits.iter().enumerate().skip(first).take(rows) {
            let Some(e) = r.all.get(hit) else { continue };
            let row = R::new(x + 8.0 * s, y + 50.0 * s + (i - first) as f32 * rh, w - 16.0 * s, rh - 2.0 * s);
            if row.y + row.h > y + h {
                break;
            }
            let hot = i == r.sel;
            crate::fx::row_at(g, row.x, row.y, row.w, row.h, 6.0 * s, hot, g.hovered(row), color);
            let text = g.fit(14.0 * s, &e.label, Face::Sans, row.w - 16.0 * s - e.indent as f32 * 14.0 * s);
            g.text(row.x + 8.0 * s + e.indent as f32 * 14.0 * s, row.y + 5.0 * s, 14.0 * s, TEXT, &text, Face::Sans);
            if i < ROWS_MAX {
                g.hit(row, Event::Tab(ROW_TAB + i), &e.label);
            }
        }
    }

    /// A press on a row of the list.
    pub(super) fn a11y_event(&mut self, n: usize) -> bool {
        if (ROW_TAB..ROW_TAB + ROWS_MAX).contains(&n) && self.a11y.rotor.is_some() {
            self.a11y_rotor_pick(n - ROW_TAB);
            return true;
        }
        false
    }
}
