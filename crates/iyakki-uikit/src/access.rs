//! Accessibility basics of the toolkit (docs/ACCESSIBILITY.md).
//!
//! The toolkit draws immediately (no tree of widgets), so the model is built from what a frame registers anyway: every clickable region
//! is a [`Hit`] (rectangle, event, visible label). From a hit this module makes a [`Node`]: an accessible name (the visible label, so most
//! tiles need no change), a [`Role`] (button, field, list item, toggle, tab, heading, slider, link) and a [`State`] (selected, checked,
//! disabled, expanded, a value). A widget that knows more than the label says (a switch knows it is on, a slider its percent, a field its
//! text) calls [`describe`]; the widgets of this crate already do. A field that shows dots never gives its text to a node: [`Node::field`]
//! keeps only the number of characters typed.
//!
//! What a frame says about focus travels as an event: a widget that draws itself focused calls [`report_focus`], the shell takes it once
//! per frame with [`take_focus`] and the [`Reader`] decides what to say. The reader (screen reader mode) is a plain speech queue that
//! interrupts at the next focus change; the speaker behind it is a [`SpeechSink`] (the shell passes the system voice, tests a fake).
//!
//! Also here, all pure and testable without a screen: the order and the cursor of the keyboard walk over a frame's hits ([`entries`],
//! [`Cursor`]), sticky keys and slow keys ([`StickyKeys`], [`SlowKeys`]), the display options ([`Prefs`]) and the contrast arithmetic
//! of WCAG 2 ([`contrast_ratio`]) the theme tests use.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::ui::{Event, Hit, Key, R};

// ------------------------------------------------------------------------------------------------------------ the model

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Button,
    Field,
    ListItem,
    Toggle,
    Tab,
    Heading,
    Slider,
    Link,
    /// A sentence a tile built itself (the Browser tile: "Heading level 2, Pricing"): the name is said as it is, no role word follows.
    Spoken,
}

impl Role {
    /// The word a screen reader says after the name.
    pub fn word(self, secret: bool) -> &'static str {
        match self {
            Role::Button => "button",
            Role::Field if secret => "password field",
            Role::Field => "edit",
            Role::ListItem => "item",
            Role::Toggle => "switch",
            Role::Tab => "tab",
            Role::Heading => "heading",
            Role::Slider => "slider",
            Role::Link => "link",
            Role::Spoken => "",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub selected: bool,
    pub checked: Option<bool>,
    pub disabled: bool,
    pub expanded: Option<bool>,
    value: Option<String>,
    secret_len: Option<usize>,
}

impl State {
    pub fn selected(mut self, on: bool) -> State {
        self.selected = on;
        self
    }
    pub fn checked(mut self, on: bool) -> State {
        self.checked = Some(on);
        self
    }
    pub fn disabled(mut self, on: bool) -> State {
        self.disabled = on;
        self
    }
    pub fn expanded(mut self, on: bool) -> State {
        self.expanded = Some(on);
        self
    }
    /// A value to read after the role ("40 percent").
    pub fn value(mut self, v: impl Into<String>) -> State {
        self.value = Some(v.into());
        self
    }
    pub fn value_text(&self) -> Option<&str> {
        self.value.as_deref()
    }
    /// How many characters were typed into a field that hides them (never the characters).
    pub fn secret_len(&self) -> Option<usize> {
        self.secret_len
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub name: String,
    pub role: Role,
    pub state: State,
}

impl Node {
    pub fn new(name: &str, role: Role, state: State) -> Node {
        Node { name: clean_name(name), role, state }
    }

    /// A text field. With `secret` the text is not kept: only its length is.
    pub fn field(name: &str, text: &str, secret: bool) -> Node {
        let state = if secret { State { secret_len: Some(text.chars().count()), ..State::default() } } else { State::default().value(text) };
        Node { name: clean_name(name), role: Role::Field, state }
    }

    pub fn is_secret(&self) -> bool {
        self.state.secret_len.is_some()
    }

    /// What a screen reader says: "Save, button", "Volume, slider, 40 percent", "Password, password field, 5 characters typed".
    pub fn speak(&self) -> String {
        let mut parts: Vec<String> = vec![if self.name.is_empty() { "unnamed".to_string() } else { self.name.clone() }, self.role.word(self.is_secret()).to_string()];
        if let Some(n) = self.state.secret_len {
            parts.push(match n {
                0 => "empty".to_string(),
                1 => "1 character typed".to_string(),
                n => format!("{n} characters typed"),
            });
        } else if let Some(v) = self.state.value.as_deref() {
            if self.role == Role::Field && v.is_empty() {
                parts.push("empty".into());
            } else {
                parts.push(v.to_string());
            }
        }
        parts.retain(|p| !p.is_empty());
        if let Some(c) = self.state.checked {
            parts.push(match (self.role, c) {
                (Role::Toggle, true) => "on".into(),
                (Role::Toggle, false) => "off".into(),
                (_, true) => "checked".into(),
                (_, false) => "not checked".into(),
            });
        }
        if self.state.selected {
            parts.push("selected".into());
        }
        if let Some(e) = self.state.expanded {
            parts.push(if e { "expanded" } else { "collapsed" }.into());
        }
        if self.state.disabled {
            parts.push("unavailable".into());
        }
        parts.join(", ")
    }
}

/// The accessible name made from a visible label: symbols that a voice would skip or spell are put into words, a prefix the toolkit adds
/// ("group Build") is dropped.
pub fn clean_name(label: &str) -> String {
    let t = label.trim();
    let t = t.strip_prefix("group ").unwrap_or(t);
    let spoken = match t {
        "+" => "plus",
        "−" | "-" => "minus",
        "×" | "✕" | "x" => "close",
        "←" => "back",
        "→" => "forward",
        "↑" => "up",
        "↓" => "down",
        "…" | "⋯" => "more",
        "☰" => "menu",
        "⚙" => "settings",
        other => other,
    };
    spoken.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ------------------------------------------------------------------------------------- what widgets know beyond the label

/// What a widget adds to the hit it registered.
#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub name: Option<String>,
    pub role: Option<Role>,
    pub state: State,
    /// Not a control for a person (a decoration that is clickable only for convenience): left out of the keyboard walk.
    pub hidden: bool,
}

impl Meta {
    pub fn new(role: Role, state: State) -> Meta {
        Meta { name: None, role: Some(role), state, hidden: false }
    }
    pub fn named(mut self, name: &str) -> Meta {
        self.name = Some(name.to_string());
        self
    }
}

thread_local! {
    static META: RefCell<Vec<(String, Event, Meta)>> = const { RefCell::new(Vec::new()) };
    static SELECTED: RefCell<Vec<R>> = const { RefCell::new(Vec::new()) };
    static FOCUS: RefCell<Option<Node>> = const { RefCell::new(None) };
    static NOTICES: RefCell<VecDeque<String>> = const { RefCell::new(VecDeque::new()) };
    static PREFS: Cell<Prefs> = const { Cell::new(Prefs::OFF) };
}

/// A frame starts: what widgets said about the last one is forgotten.
pub fn start_frame() {
    META.with(|m| m.borrow_mut().clear());
    FOCUS.with(|f| *f.borrow_mut() = None);
    SELECTED.with(|s| s.borrow_mut().clear());
}

/// Say more about the hit registered with this label and event (see [`Meta`]). The widgets of the toolkit call it; a tile may too.
pub fn describe(label: &str, ev: &Event, meta: Meta) {
    META.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() < 4096 {
            m.push((label.to_string(), ev.clone(), meta));
        }
    });
}

/// A list row was drawn chosen (the arrow keys moved it): the shell finds the control under it and reports it as the focus.
pub fn note_selected(r: R) {
    SELECTED.with(|s| {
        let mut s = s.borrow_mut();
        if s.len() < 16 {
            s.push(r);
        }
    });
}

/// The chosen rows of the frame, in drawing order.
pub fn take_selected() -> Vec<R> {
    SELECTED.with(|s| std::mem::take(&mut *s.borrow_mut()))
}

/// A widget drew itself focused (a text field with the caret, a list's chosen row): the shell speaks the change once.
pub fn report_focus(n: Node) {
    FOCUS.with(|f| *f.borrow_mut() = Some(n));
}

/// The focus a frame reported, once.
pub fn take_focus() -> Option<Node> {
    FOCUS.with(|f| f.borrow_mut().take())
}

/// A message to read out at once, not tied to a control (a result, an error from a tile). Capped, oldest first.
pub fn announce(text: &str) {
    NOTICES.with(|n| {
        let mut n = n.borrow_mut();
        if n.len() >= 16 {
            n.pop_front();
        }
        n.push_back(text.to_string());
    });
}

pub fn take_announcements() -> Vec<String> {
    NOTICES.with(|n| n.borrow_mut().drain(..).collect())
}

fn meta_for(label: &str, ev: &Event) -> Option<Meta> {
    META.with(|m| m.borrow().iter().rev().find(|(l, e, _)| l == label && e == ev).map(|(_, _, mt)| mt.clone()))
}

/// The node of a hit: the registered description when there is one, else the label and a role from the kind of event. `None` for a hit
/// that is no control (a pointer movement, a scroll area, an unnamed region).
pub fn node_of(h: &Hit) -> Option<Node> {
    let meta = meta_for(&h.label, &h.ev);
    if meta.as_ref().is_some_and(|m| m.hidden) {
        return None;
    }
    let default_role = match &h.ev {
        Event::Focus(_) | Event::FocusAt(..) => Role::Field,
        Event::Area(_) | Event::AreaAt(..) if h.label == "field" => Role::Field,
        Event::Row(_) => Role::ListItem,
        // a region that is clicked where it is (a chip, a tile) registers the click itself as its event
        Event::Tab(_) | Event::Key(_) | Event::Sub(..) | Event::Fn(..) | Event::Layout(_) | Event::Area(_) | Event::AreaAt(..) | Event::MouseDown(..) => Role::Button,
        _ => return None,
    };
    let name = match meta.as_ref().and_then(|m| m.name.clone()) {
        Some(n) => clean_name(&n),
        None => {
            let n = clean_name(&h.label);
            if n.is_empty() || n == "backdrop" {
                match &h.ev {
                    Event::Key(Key::Esc) => "Close".to_string(),
                    Event::Key(Key::Enter) => "Confirm".to_string(),
                    _ => return None,
                }
            } else if n == "field" {
                "Text field".to_string()
            } else {
                n
            }
        }
    };
    let (role, state) = match meta {
        Some(m) => (m.role.unwrap_or(default_role), m.state),
        None => (default_role, State::default()),
    };
    Some(Node { name, role, state })
}

// ------------------------------------------------------------------------------------------------- the keyboard walk

/// A control the keyboard walk can stop on.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub hit: usize,
    pub node: Node,
    pub rect: R,
}

/// The controls of one frame in reading order (top to bottom, left to right within a row), for a screen of `screen` pixels. Hits that are
/// off the screen, tiny, a whole-screen backdrop, or the same control registered twice are left out.
pub fn entries(hits: &[Hit], screen: (f32, f32)) -> Vec<Entry> {
    let area = screen.0 * screen.1;
    let mut out: Vec<Entry> = Vec::new();
    for (i, h) in hits.iter().enumerate() {
        let r = h.rect;
        if r.w < 2.0 || r.h < 2.0 || r.w * r.h >= area * 0.8 {
            continue;
        }
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        if cx < 0.0 || cy < 0.0 || cx >= screen.0 || cy >= screen.1 {
            continue;
        }
        let Some(node) = node_of(h) else { continue };
        if out.iter().any(|e| hits[e.hit].label == h.label && hits[e.hit].ev == h.ev && same_rect(e.rect, r)) {
            continue;
        }
        out.push(Entry { hit: i, node, rect: r });
    }
    out.sort_by(|a, b| (a.rect.y + a.rect.h / 2.0).total_cmp(&(b.rect.y + b.rect.h / 2.0)));
    // rows: an entry joins the row of the first entry whose middle is within half of that entry's height
    let mut rows: Vec<Vec<Entry>> = Vec::new();
    for e in out {
        let mid = e.rect.y + e.rect.h / 2.0;
        match rows.last_mut() {
            Some(row) if (mid - (row[0].rect.y + row[0].rect.h / 2.0)).abs() <= row[0].rect.h.min(e.rect.h) / 2.0 => row.push(e),
            _ => rows.push(vec![e]),
        }
    }
    rows.into_iter()
        .flat_map(|mut r| {
            r.sort_by(|a, b| a.rect.x.total_cmp(&b.rect.x));
            r
        })
        .collect()
}

fn dist(a: R, b: R) -> f32 {
    (a.x - b.x).abs() + (a.y - b.y).abs()
}

fn same_rect(a: R, b: R) -> bool {
    (a.x - b.x).abs() < 1.0 && (a.y - b.y).abs() < 1.0 && (a.w - b.w).abs() < 1.0 && (a.h - b.h).abs() < 1.0
}

/// Where the keyboard walk stands. It remembers the control by label and event, so a redraw (and a list that moved) keeps it.
#[derive(Clone, Debug, Default)]
pub struct Cursor {
    at: Option<(String, Event, R)>,
}

impl Cursor {
    pub fn clear(&mut self) {
        self.at = None;
    }
    pub fn is_set(&self) -> bool {
        self.at.is_some()
    }
    /// The index in `list` of the control the cursor stands on, if it is still there.
    pub fn find(&self, list: &[Entry], hits: &[Hit]) -> Option<usize> {
        let (l, e, r) = self.at.as_ref()?;
        // two regions may share a label and an event (a button and the switch with the same key): the one nearest to where it was
        list.iter().enumerate().filter(|(_, x)| &hits[x.hit].label == l && &hits[x.hit].ev == e).min_by(|(_, a), (_, b)| dist(a.rect, *r).total_cmp(&dist(b.rect, *r))).map(|(i, _)| i)
    }
    /// Move to the next (or previous) control, wrapping round; the first (last) one when the cursor is not on screen. `None` when there
    /// is nothing to stand on.
    pub fn step(&mut self, list: &[Entry], hits: &[Hit], back: bool) -> Option<usize> {
        if list.is_empty() {
            self.at = None;
            return None;
        }
        let n = list.len();
        let i = match (self.find(list, hits), back) {
            (Some(i), false) => (i + 1) % n,
            (Some(i), true) => (i + n - 1) % n,
            (None, false) => 0,
            (None, true) => n - 1,
        };
        let h = &hits[list[i].hit];
        self.at = Some((h.label.clone(), h.ev.clone(), list[i].rect));
        Some(i)
    }
}

// ------------------------------------------------------------------------------------------------------ screen reader

/// What speaks. The shell passes the system voice; tests pass a fake that records.
pub trait SpeechSink {
    fn say(&mut self, text: &str) -> Result<(), String>;
    fn busy(&mut self) -> bool;
    fn stop(&mut self);
}

struct Utterance {
    text: String,
}

/// The speech queue. A focus change drops everything waiting and interrupts what is being said; a notice waits its turn behind the
/// one being said (and is dropped if the person moves on before it starts).
#[derive(Default)]
pub struct Reader {
    pending: VecDeque<Utterance>,
    interrupt: bool,
    last_focus: Option<Node>,
    last_text: String,
    /// The last problem the voice reported (shown to the person, once per distinct message).
    pub problem: Option<String>,
}

impl Reader {
    pub fn new() -> Reader {
        Reader::default()
    }

    /// The person moved to `n`. Said at once; the same node twice in a row is not said again unless `force`.
    pub fn focus(&mut self, n: &Node, force: bool) -> bool {
        if !force && self.last_focus.as_ref() == Some(n) {
            return false;
        }
        self.last_focus = Some(n.clone());
        self.pending.clear();
        self.interrupt = true;
        let text = n.speak();
        self.last_text = text.clone();
        self.pending.push_back(Utterance { text });
        true
    }

    /// The screen changed: `prefix` (its title) and, when a control has the focus, that control, in one breath.
    pub fn focus_after(&mut self, prefix: &str, n: &Node) {
        self.last_focus = Some(n.clone());
        self.pending.clear();
        self.interrupt = true;
        let text = format!("{prefix}. {}", n.speak());
        self.last_text = text.clone();
        self.pending.push_back(Utterance { text });
    }

    /// Say `text` at once, interrupting (a new screen with nothing focused yet).
    pub fn say_now(&mut self, text: &str) {
        self.pending.clear();
        self.interrupt = true;
        self.last_text = text.to_string();
        self.pending.push_back(Utterance { text: text.to_string() });
    }

    /// A toast, an error, a dialog title: said after what is being said now, unless the person moves first.
    pub fn notice(&mut self, text: &str) {
        let t = text.trim();
        if t.is_empty() || self.pending.back().is_some_and(|u| u.text == t) {
            return;
        }
        if self.pending.len() >= 8 {
            self.pending.pop_front();
        }
        self.last_text = t.to_string();
        self.pending.push_back(Utterance { text: t.to_string() });
    }

    /// Say the last thing again (Super+Alt+R).
    pub fn repeat(&mut self) {
        if self.last_text.is_empty() {
            return;
        }
        self.pending.clear();
        self.interrupt = true;
        self.pending.push_back(Utterance { text: self.last_text.clone() });
    }

    /// Forget the focus (a new screen): the first control reported is said even if it equals the old one.
    pub fn forget_focus(&mut self) {
        self.last_focus = None;
    }

    /// The reader was switched off: drop everything and stop the voice.
    pub fn silence(&mut self, sink: &mut dyn SpeechSink) {
        self.pending.clear();
        self.interrupt = false;
        self.last_focus = None;
        sink.stop();
    }

    pub fn waiting(&self) -> usize {
        self.pending.len()
    }

    /// Give the next utterance to the voice when it is free (at once after an interrupt). Returns a problem the voice reported that the
    /// person has not been told about yet.
    pub fn pump(&mut self, sink: &mut dyn SpeechSink) -> Option<String> {
        if self.interrupt {
            sink.stop();
        }
        let free = self.interrupt || !sink.busy();
        self.interrupt = false;
        if !free {
            return None;
        }
        let u = self.pending.pop_front()?;
        match sink.say(&u.text) {
            Ok(()) => {
                self.problem = None;
                None
            }
            Err(e) => {
                self.pending.clear();
                if self.problem.as_deref() == Some(e.as_str()) {
                    None
                } else {
                    self.problem = Some(e.clone());
                    Some(e)
                }
            }
        }
    }
}

// --------------------------------------------------------------------------------------------------- display options

/// The options that change how the toolkit draws (the rest of the accessibility choices live in the agent's `accessibility` module).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub screen_reader: bool,
    pub large_pointer: bool,
    pub big_ring: bool,
    pub reduce_motion: bool,
}

impl Prefs {
    pub const OFF: Prefs = Prefs { screen_reader: false, large_pointer: false, big_ring: false, reduce_motion: false };
}

static POINTER_BOOST: AtomicUsize = AtomicUsize::new(0);
static STICKY_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SLOW_MS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Sticky keys and slow keys are read by the input thread, which is not the thread that draws: the choices travel in atomics.
pub fn set_key_options(sticky: bool, slow_ms: u32) {
    STICKY_ON.store(sticky, Ordering::SeqCst);
    SLOW_MS.store(slow_ms, Ordering::SeqCst);
}

pub fn sticky_enabled() -> bool {
    STICKY_ON.load(Ordering::SeqCst)
}

pub fn slow_ms() -> u32 {
    SLOW_MS.load(Ordering::SeqCst)
}

/// Apply the options to the thread that draws (reduced motion goes to `fx`) and to the pointer (an atomic: another thread draws it).
pub fn set_prefs(p: Prefs) {
    PREFS.with(|c| c.set(p));
    crate::fx::set_reduced_motion(p.reduce_motion);
    POINTER_BOOST.store(if p.large_pointer { 2 } else { 0 }, Ordering::SeqCst);
}

pub fn prefs() -> Prefs {
    PREFS.with(|c| c.get())
}

/// How many whole pixels per sprite pixel the pointer gains with the large pointer option.
pub fn pointer_boost() -> usize {
    POINTER_BOOST.load(Ordering::SeqCst)
}

/// Thickness factor of focus rings with the big ring option.
pub fn focus_ring_factor() -> f32 {
    if prefs().big_ring { 2.0 } else { 1.0 }
}

// -------------------------------------------------------------------------------------------------------------- keys

/// The four modifier keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Shift,
    Ctrl,
    Alt,
    Meta,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Latch {
    #[default]
    Off,
    /// Held down by the finger now.
    Down,
    /// Let go: stays on for the next key.
    Latched,
    /// Pressed twice in a row: stays on until pressed again.
    Locked,
    /// Locked and held for the second press: the release keeps it locked.
    LockedDown,
    /// Held down and an ordinary key was used with it: the release does not latch it.
    DownUsed,
    /// Locked and pressed the third time: the release turns it off.
    Unlock,
}

/// Sticky keys: a modifier pressed and released stays on for the next ordinary key, so Super+Alt+S can be typed one key at a time.
/// Pressed twice it stays on until pressed a third time. While the option is off the machine stands aside.
#[derive(Clone, Debug, Default)]
pub struct StickyKeys {
    on: bool,
    shift: Latch,
    ctrl: Latch,
    alt: Latch,
    meta: Latch,
}

impl StickyKeys {
    pub fn set_enabled(&mut self, on: bool) {
        if self.on != on {
            *self = StickyKeys { on, ..StickyKeys::default() };
        }
    }
    pub fn enabled(&self) -> bool {
        self.on
    }
    fn slot(&mut self, m: Modifier) -> &mut Latch {
        match m {
            Modifier::Shift => &mut self.shift,
            Modifier::Ctrl => &mut self.ctrl,
            Modifier::Alt => &mut self.alt,
            Modifier::Meta => &mut self.meta,
        }
    }
    /// A modifier key went down or up.
    pub fn modifier(&mut self, m: Modifier, down: bool) {
        let s = self.slot(m);
        *s = match (*s, down) {
            (Latch::Off, true) => Latch::Down,
            (Latch::Down, false) => Latch::Latched,
            (Latch::Latched, true) => Latch::LockedDown,
            (Latch::DownUsed, false) => Latch::Off,
            (Latch::Locked, true) => Latch::Unlock,
            (Latch::Unlock, false) => Latch::Off,
            (Latch::LockedDown, false) => Latch::Locked,
            (other, _) => other,
        };
    }
    /// The modifiers in force now (held, latched or locked).
    pub fn mods(&self) -> Mods {
        let on = |l: Latch| !matches!(l, Latch::Off);
        Mods { shift: on(self.shift), ctrl: on(self.ctrl), alt: on(self.alt), meta: on(self.meta) }
    }
    /// An ordinary key (or a mouse button) was used: latched modifiers are spent, locked and held ones stay.
    pub fn key_used(&mut self) {
        for l in [&mut self.shift, &mut self.ctrl, &mut self.alt, &mut self.meta] {
            match *l {
                Latch::Latched => *l = Latch::Off,
                Latch::Down => *l = Latch::DownUsed,
                _ => {}
            }
        }
    }
    /// Something is latched or locked (the shell can show a small mark).
    pub fn any_latched(&self) -> bool {
        [self.shift, self.ctrl, self.alt, self.meta].iter().any(|l| matches!(l, Latch::Latched | Latch::Locked | Latch::LockedDown))
    }
}

/// Slow keys: a key counts only after it was held `delay_ms`. A tremor or a brush against a key is ignored. Time is passed in, so a test
/// runs it without waiting.
#[derive(Clone, Debug, Default)]
pub struct SlowKeys {
    delay_ms: u64,
    down: Vec<(u16, u64, bool)>,
}

impl SlowKeys {
    pub fn set_delay(&mut self, ms: u32) {
        if self.delay_ms != ms as u64 {
            self.delay_ms = ms as u64;
            self.down.clear();
        }
    }
    pub fn delay(&self) -> u32 {
        self.delay_ms as u32
    }
    /// A key event (`value` 1 down, 0 up, 2 repeat) at `now_ms`: the values to pass on instead (none = swallowed). A key that was held long
    /// enough is passed as a press when the repeat or the release shows it, then as a release.
    pub fn filter(&mut self, code: u16, value: i32, now_ms: u64) -> Vec<i32> {
        if self.delay_ms == 0 {
            return vec![value];
        }
        let i = self.down.iter().position(|d| d.0 == code);
        match (value, i) {
            (1, Some(i)) => {
                self.down[i] = (code, now_ms, false);
                vec![]
            }
            (1, None) => {
                if self.down.len() >= 32 {
                    self.down.remove(0);
                }
                self.down.push((code, now_ms, false));
                vec![]
            }
            (2, Some(i)) => {
                let (_, since, accepted) = self.down[i];
                if accepted {
                    vec![2]
                } else if now_ms.saturating_sub(since) >= self.delay_ms {
                    self.down[i].2 = true;
                    vec![1]
                } else {
                    vec![]
                }
            }
            (0, Some(i)) => {
                let (_, since, accepted) = self.down.remove(i);
                if accepted {
                    vec![0]
                } else if now_ms.saturating_sub(since) >= self.delay_ms {
                    vec![1, 0]
                } else {
                    vec![]
                }
            }
            (2, None) | (0, None) => vec![],
            _ => vec![value],
        }
    }
}

// --------------------------------------------------------------------------------------------------------------- WCAG

/// Relative luminance of an sRGB colour (WCAG 2.x).
pub fn luminance(c: [u8; 4]) -> f64 {
    let lin = |v: u8| {
        let s = v as f64 / 255.0;
        if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// Contrast ratio of two colours, 1.0 to 21.0 (AA needs 4.5 for text, 3.0 for large text and for lines; AAA needs 7.0).
pub fn contrast_ratio(a: [u8; 4], b: [u8; 4]) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
#[path = "access_tests.rs"]
mod tests;
