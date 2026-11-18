//! `ChatView`: the IYAKKI assistant as a component an SDK app places in its layout (docs/development/UI-SDK.md, "Embedding the assistant").
//!
//! The app keeps a `ChatView` in its state, gives it an [`AssistantPort`] (the core's), a [`SessionConfig`] (a conversation name, the tools
//! the app offers, the context) and a rectangle to draw in. The view owns the conversation on screen: the transcript, the field, the
//! approval card, the model and role choice. It draws through [`Canvas`], so it works in the shell, in a browser or client (display
//! list) and as a WASM tile, with the colours and size class of the host.
//!
//! What the app does, in four lines:
//! ```text
//! event(Tick):   for inv in chat.tick() { let r = self.run_tool(&inv); chat.answer(&inv, r); }
//! event(Action): if chat.owns(a) { chat.action(a) }      event(Key/Text): chat.key(k) / chat.text(t) first
//! view:          chat.draw(c, th, &metrics, rect)         context changed: chat.set_context(items)
//! ```
//! An app tool runs in the app, never in the view: the view hands out a [`ToolInvocation`] and the app answers it. A tool that asks has
//! already been allowed by the person (through the core) when the invocation arrives.

use crate::assistant::{AssistError, AssistantPort, Choice, ChatEvent, ContextItem, SessionConfig, Speaker};
use crate::canvas::{Canvas, TextStyle};
use crate::event::UiKey;
use crate::geom::Rect;
use crate::size::{Metrics, TypeRole};
use crate::tokens::{Radius, Space};
use crate::theme::Theme;
use crate::widgets;

/// How many action numbers a chat view uses, from the base the app gave it.
pub const ACTIONS: u32 = 8;
/// The width of the Send and Stop button in text sizes.
const SEND_EM: f32 = 5.0;
const A_FIELD: u32 = 0;
const A_SEND: u32 = 1;
const A_STOP: u32 = 2;
const A_ALLOW: u32 = 3;
const A_DENY: u32 = 4;
const A_MODEL: u32 = 5;
const A_ROLE: u32 = 6;
/// The longest message the field takes.
pub const MAX_INPUT: usize = 2000;
/// The transcript keeps this many entries (older ones are dropped from the screen; the core keeps the history).
const MAX_ENTRIES: usize = 200;

/// An app tool the model called and the person allowed: run it and answer with [`ChatView::answer`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolInvocation {
    pub call: String,
    /// The app's plain tool name.
    pub tool: String,
    /// The arguments as JSON text, checked against the schema the app declared.
    pub args: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Approval {
    pub call: String,
    pub tool: String,
    pub summary: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolState {
    Asked,
    Allowed,
    Denied,
    Done,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    User(String),
    Assistant(String),
    Tool { call: String, tool: String, state: ToolState, text: String },
    Note(String),
    Error(String),
}

pub struct ChatView {
    port: Box<dyn AssistantPort>,
    cfg: SessionConfig,
    base: u32,
    opened: bool,
    pub entries: Vec<Entry>,
    partial: String,
    busy: bool,
    approval: Option<Approval>,
    pub input: String,
    pub focused: bool,
    error: Option<String>,
    models: Vec<Choice>,
    roles: Vec<Choice>,
    model_at: Option<usize>,
    role_at: Option<usize>,
    scroll_back: usize,
    ticks: u32,
    /// App tool calls that were read and not yet handed to the app.
    ready: Vec<ToolInvocation>,
}

impl ChatView {
    pub fn new(port: Box<dyn AssistantPort>, cfg: SessionConfig, action_base: u32) -> ChatView {
        ChatView { port, cfg, base: action_base, opened: false, entries: Vec::new(), partial: String::new(), busy: false, approval: None, input: String::new(), focused: false, error: None, models: Vec::new(), roles: Vec::new(), model_at: None, role_at: None, scroll_back: 0, ticks: 0, ready: Vec::new() }
    }

    /// Whether an action number belongs to this view.
    pub fn owns(&self, action: u32) -> bool {
        (self.base..self.base + ACTIONS).contains(&action)
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn pending_approval(&self) -> Option<&Approval> {
        self.approval.as_ref()
    }

    /// The last refusal or failure as a sentence (also in the transcript).
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// True while the field has focus: the host shows the on-screen keyboard.
    pub fn wants_keyboard(&self) -> bool {
        self.focused
    }

    fn fail(&mut self, e: AssistError) {
        let s = e.sentence();
        self.error = Some(s.clone());
        self.busy = false;
        self.entries.push(Entry::Error(s));
        self.trim();
    }

    fn trim(&mut self) {
        if self.entries.len() > MAX_ENTRIES {
            let n = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..n);
        }
    }

    /// Open the session once (the first tick, send or context change). A refusal is shown and tried again at the next message.
    fn ensure_open(&mut self) -> bool {
        if self.opened {
            return true;
        }
        match self.port.open(&self.cfg) {
            Ok(resumed) => {
                self.opened = true;
                self.error = None;
                self.models = self.port.models();
                self.roles = self.port.roles();
                if resumed {
                    for h in self.port.history() {
                        self.entries.push(match h.who {
                            Speaker::User => Entry::User(h.text),
                            Speaker::Assistant => Entry::Assistant(h.text),
                            Speaker::Tool => Entry::Note(h.text),
                        });
                    }
                    self.trim();
                }
                true
            }
            Err(e) => {
                self.fail(e);
                false
            }
        }
    }

    /// Attach what the person is looking at now (replaces the earlier context).
    pub fn set_context(&mut self, items: Vec<ContextItem>) {
        if self.cfg.context == items {
            return;
        }
        self.cfg.context = items;
        if self.opened {
            if let Err(e) = self.port.set_context(&self.cfg.context) {
                self.fail(e);
            }
        }
    }

    /// Send a message as the person. A refusal (the assistant is busy, the core says no) is shown and the text stays in the field.
    pub fn send(&mut self, text: &str) -> bool {
        let t = text.trim();
        if t.is_empty() || self.busy || !self.ensure_open() {
            return false;
        }
        match self.port.send(t) {
            Ok(()) => {
                self.entries.push(Entry::User(t.to_string()));
                self.trim();
                self.busy = true;
                self.error = None;
                self.partial.clear();
                self.scroll_back = 0;
                self.read_events();
                true
            }
            Err(e) => {
                self.fail(e);
                false
            }
        }
    }

    pub fn cancel(&mut self) {
        if self.busy {
            if let Err(e) = self.port.cancel() {
                self.fail(e);
            }
            self.approval = None;
            self.read_events();
        }
    }

    /// Answer an app tool call (what the app did, or why it could not).
    pub fn answer(&mut self, inv: &ToolInvocation, result: Result<String, String>) {
        let (ok, text) = match result {
            Ok(t) => (true, t),
            Err(t) => (false, t),
        };
        if let Err(e) = self.port.answer(&inv.call, ok, &text) {
            self.fail(e);
        }
        self.read_events();
    }

    /// Read what the session says now (every tick while an answer is coming, every tenth tick otherwise) and return the app tool calls.
    pub fn tick(&mut self) -> Vec<ToolInvocation> {
        self.ticks = self.ticks.wrapping_add(1);
        let every = if self.busy { 3 } else { 30 };
        if self.opened && self.ticks.is_multiple_of(every) {
            self.read_events();
        }
        std::mem::take(&mut self.ready)
    }

    /// Read the events now and return the app tool calls that are ready.
    pub fn pump(&mut self) -> Vec<ToolInvocation> {
        self.read_events();
        std::mem::take(&mut self.ready)
    }

    fn read_events(&mut self) {
        if !self.opened {
            return;
        }
        for ev in self.port.poll() {
            let ends = ev.ends_turn();
            match ev {
                ChatEvent::Partial(t) => self.partial = t,
                ChatEvent::Note(t) => self.entries.push(Entry::Note(t)),
                ChatEvent::ToolRequest { call, tool, args } => self.entries.push(Entry::Tool { call, tool, state: ToolState::Asked, text: args }),
                ChatEvent::Approval { call, tool, summary, detail } => {
                    // the field gives up the focus, so `y` and `n` answer the card at once (tap the field to type again)
                    self.focused = false;
                    self.approval = Some(Approval { call, tool, summary, detail });
                }
                ChatEvent::Decision { call, approved, .. } => {
                    if self.approval.as_ref().is_some_and(|a| a.call == call) {
                        self.approval = None;
                    }
                    self.set_state(&call, if approved { ToolState::Allowed } else { ToolState::Denied }, None);
                }
                ChatEvent::ToolCall { call, tool, args } => self.ready.push(ToolInvocation { call, tool, args }),
                ChatEvent::ToolResult { call, ok, text, .. } => self.set_state(&call, if ok { ToolState::Done } else { ToolState::Failed }, Some(text)),
                ChatEvent::Final(t) => {
                    self.partial.clear();
                    self.entries.push(Entry::Assistant(t));
                }
                ChatEvent::Failed(e) => {
                    self.partial.clear();
                    self.error = Some(e.clone());
                    self.entries.push(Entry::Error(e));
                }
                ChatEvent::Stopped(r) => {
                    self.partial.clear();
                    self.entries.push(Entry::Note(r));
                }
            }
            if ends {
                self.busy = false;
                self.approval = None;
            }
        }
        self.trim();
    }

    fn set_state(&mut self, call: &str, state: ToolState, text: Option<String>) {
        if let Some(Entry::Tool { state: s, text: t, .. }) = self.entries.iter_mut().rev().find(|e| matches!(e, Entry::Tool { call: c, .. } if c == call)) {
            *s = state;
            if let Some(x) = text {
                *t = x;
            }
        }
    }

    fn decide(&mut self, approve: bool) {
        if let Some(a) = self.approval.take() {
            if let Err(e) = self.port.decide(&a.call, approve) {
                self.fail(e);
            }
            self.read_events();
        }
    }

    fn cycle(&mut self, models: bool) {
        if !self.ensure_open() {
            return;
        }
        let (list, at) = if models { (&self.models, &mut self.model_at) } else { (&self.roles, &mut self.role_at) };
        if list.is_empty() {
            return;
        }
        let next = at.map_or(0, |i| (i + 1) % list.len());
        let id = list[next].id.clone();
        *at = Some(next);
        let r = if models { self.port.choose(Some(&id), None) } else { self.port.choose(None, Some(&id)) };
        match r {
            Ok(()) => {
                if models {
                    self.cfg.model = Some(id);
                } else {
                    self.cfg.role = Some(id);
                }
            }
            Err(e) => self.fail(e),
        }
    }

    /// A hit region of this view was used. Returns whether the action was ours.
    pub fn action(&mut self, a: u32) -> bool {
        if !self.owns(a) {
            return false;
        }
        match a - self.base {
            A_FIELD => self.focused = true,
            A_SEND => {
                let t = std::mem::take(&mut self.input);
                if !self.send(&t) {
                    self.input = t;
                }
            }
            A_STOP => self.cancel(),
            A_ALLOW => self.decide(true),
            A_DENY => self.decide(false),
            A_MODEL => self.cycle(true),
            A_ROLE => self.cycle(false),
            _ => {}
        }
        true
    }

    /// A key. Returns whether the view used it (the app then does not). With the field focused every key is the field's; without it only
    /// `y` and `n` while an approval card is showing.
    pub fn key(&mut self, k: UiKey) -> bool {
        if self.focused {
            match k {
                UiKey::Char(c) if !c.is_control() => {
                    if self.input.chars().count() < MAX_INPUT {
                        self.input.push(c);
                    }
                }
                UiKey::Backspace => {
                    self.input.pop();
                }
                UiKey::Enter => {
                    let t = std::mem::take(&mut self.input);
                    if !self.send(&t) {
                        self.input = t;
                    }
                }
                UiKey::Esc | UiKey::Tab => self.focused = false,
                UiKey::Up => self.scroll_back = self.scroll_back.saturating_add(1),
                UiKey::Down => self.scroll_back = self.scroll_back.saturating_sub(1),
                _ => {}
            }
            return true;
        }
        if self.approval.is_some() {
            match k {
                UiKey::Char('y') | UiKey::Char('Y') => {
                    self.decide(true);
                    return true;
                }
                UiKey::Char('n') | UiKey::Char('N') => {
                    self.decide(false);
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    /// Typed, pasted or composed text for the focused field.
    pub fn text(&mut self, t: &str) -> bool {
        if !self.focused {
            return false;
        }
        for c in t.chars().filter(|c| !c.is_control() || *c == '\n') {
            if c == '\n' {
                continue;
            }
            if self.input.chars().count() < MAX_INPUT {
                self.input.push(c);
            }
        }
        true
    }

    /// The wheel over the transcript (positive: towards older lines).
    pub fn wheel(&mut self, d: i32) {
        self.scroll_back = if d > 0 { self.scroll_back.saturating_add(3) } else { self.scroll_back.saturating_sub(3) };
    }

    /// The transcript as plain lines (for tests and a screen reader summary).
    pub fn transcript(&self) -> Vec<String> {
        let mut v: Vec<String> = self.entries.iter().map(entry_text).collect();
        if !self.partial.is_empty() {
            v.push(format!("Assistant: {}", self.partial));
        }
        v
    }

    /// Draw the view inside `area` (never outside it). Radii, gaps, text sizes and shadows are the skin's (`m.tokens`).
    pub fn draw(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, area: Rect) {
        let t = m.min_target();
        let (sm, md) = (m.gap(Space::Sm), m.gap(Space::Md));
        widgets::panel(c, th, m, area);
        let inner = area.inset(md);
        let mut top = inner.y;
        // model and role choice
        if !self.models.is_empty() || !self.roles.is_empty() {
            let row = Rect::new(inner.x, top, inner.w, t);
            let half = if !self.models.is_empty() && !self.roles.is_empty() { row.w / 2.0 } else { row.w };
            let mut x = row.x;
            if !self.models.is_empty() {
                let label = format!("Model: {}", self.model_at.map_or("default".to_string(), |i| self.models[i].label.clone()));
                widgets::button(c, th, m, Rect::new(x, row.y, (half - m.gap(Space::Xs)).max(0.0), t), &label, self.base + A_MODEL, false);
                x += half;
            }
            if !self.roles.is_empty() {
                let label = format!("Role: {}", self.role_at.map_or("default".to_string(), |i| self.roles[i].label.clone()));
                widgets::button(c, th, m, Rect::new(x, row.y, (half - m.gap(Space::Xs)).max(0.0), t), &label, self.base + A_ROLE, false);
            }
            top += t + sm;
        }
        // the input row at the bottom
        let input_row = Rect::new(inner.x, (inner.bottom() - t).max(top), inner.w, t);
        let send_w = m.em(SEND_EM).min(inner.w * 0.4);
        let (f_area, b_area) = input_row.split_left((input_row.w - send_w - sm).max(0.0));
        widgets::field(c, th, m, f_area, "Ask the assistant", &self.input, self.focused, self.base + A_FIELD);
        let b = Rect::new(b_area.x + sm, b_area.y, (b_area.w - sm).max(0.0), b_area.h);
        if self.busy {
            widgets::button(c, th, m, b, "Stop", self.base + A_STOP, false);
        } else {
            widgets::button(c, th, m, b, "Send", self.base + A_SEND, true);
        }
        let mut bottom = input_row.y - sm;
        // a refusal
        let small = m.size(TypeRole::Caption);
        if let Some(e) = self.error.clone() {
            let lines = wrap(c, small, TextStyle::Regular, &e, inner.w);
            let lead = m.leading(small);
            let h = lead * lines.len().min(2) as f32;
            if bottom - h > top {
                for (i, l) in lines.iter().take(2).enumerate() {
                    c.text(inner.x, bottom - h + i as f32 * lead, small, th.err, l, TextStyle::Regular);
                }
                bottom -= h + m.gap(Space::Xs);
            }
        }
        // the approval card
        if let Some(a) = self.approval.clone() {
            let size = m.size(TypeRole::Body);
            let lead = m.leading(size);
            let lines = wrap(c, size, TextStyle::Bold, &format!("Allow: {}", a.summary), inner.w - 2.0 * md);
            let shown = lines.len().min(3);
            let h = shown as f32 * lead + t + 2.0 * md + sm;
            if bottom - h > top {
                let card = Rect::new(inner.x, bottom - h, inner.w, h);
                let rad = widgets::surface(c, th, m, card, th.card_sel, Radius::Md, true);
                c.outline(card, rad, m.tokens.focus_width(), th.amber);
                for (i, l) in lines.iter().take(shown).enumerate() {
                    c.text(card.x + md, card.y + sm + i as f32 * lead, size, th.text, l, TextStyle::Bold);
                }
                let by = card.bottom() - t - sm;
                let bw = ((card.w - md - 2.0 * md) / 2.0).max(0.0);
                widgets::button(c, th, m, Rect::new(card.x + md, by, bw, t), "Allow", self.base + A_ALLOW, true);
                widgets::button(c, th, m, Rect::new(card.x + 2.0 * md + bw, by, bw, t), "Deny", self.base + A_DENY, false);
                bottom -= h + sm;
            }
        }
        // the transcript
        let log = Rect::new(inner.x, top, inner.w, (bottom - top).max(0.0));
        let size = m.size(TypeRole::Body);
        let lh = m.leading(size);
        let pad = m.gap(Space::Xxs);
        let mut lines: Vec<(String, [u8; 4], TextStyle)> = Vec::new();
        for e in &self.entries {
            push_entry(c, th, e, size, log.w - 2.0 * pad, &mut lines);
        }
        if !self.partial.is_empty() {
            for l in wrap(c, size, TextStyle::Regular, &format!("Assistant: {}", self.partial), log.w - 2.0 * pad) {
                lines.push((l, th.text, TextStyle::Regular));
            }
        } else if self.busy && self.approval.is_none() {
            lines.push(("Assistant is working...".into(), th.muted, TextStyle::Regular));
        }
        if lines.is_empty() && self.error.is_none() {
            lines.push(("Ask something, or give the assistant a task in this app.".into(), th.dim, TextStyle::Regular));
        }
        let fit_n = ((log.h / lh).floor() as usize).max(0);
        let max_back = lines.len().saturating_sub(fit_n);
        self.scroll_back = self.scroll_back.min(max_back);
        let start = lines.len().saturating_sub(fit_n + self.scroll_back);
        let end = (start + fit_n).min(lines.len());
        c.clipped(log, &mut |cc| {
            for (i, (l, col, st)) in lines[start..end].iter().enumerate() {
                cc.text(log.x + pad, log.y + i as f32 * lh, size, *col, l, *st);
            }
        });
    }
}

fn entry_text(e: &Entry) -> String {
    match e {
        Entry::User(t) => format!("You: {t}"),
        Entry::Assistant(t) => format!("Assistant: {t}"),
        Entry::Tool { tool, state, text, .. } => {
            let s = match state {
                ToolState::Asked => "asked",
                ToolState::Allowed => "allowed",
                ToolState::Denied => "not allowed",
                ToolState::Done => "done",
                ToolState::Failed => "failed",
            };
            if matches!(state, ToolState::Done | ToolState::Failed) { format!("Tool {tool} {s}: {text}") } else { format!("Tool {tool} {s}") }
        }
        Entry::Note(t) => format!("Note: {t}"),
        Entry::Error(t) => format!("Problem: {t}"),
    }
}

fn push_entry(c: &mut dyn Canvas, th: &Theme, e: &Entry, size: f32, w: f32, out: &mut Vec<(String, [u8; 4], TextStyle)>) {
    let (col, st) = match e {
        Entry::User(_) => (th.accent, TextStyle::Bold),
        Entry::Assistant(_) => (th.text, TextStyle::Regular),
        Entry::Tool { state: ToolState::Failed | ToolState::Denied, .. } => (th.err, TextStyle::Regular),
        Entry::Tool { .. } | Entry::Note(_) => (th.muted, TextStyle::Regular),
        Entry::Error(_) => (th.err, TextStyle::Regular),
    };
    for l in wrap(c, size, st, &entry_text(e), w) {
        out.push((l, col, st));
    }
}

/// Break `s` into lines that each measure at most `max_w` (on words; a word longer than a line is cut). Empty lines of the text stay.
pub fn wrap(c: &mut dyn Canvas, size: f32, style: TextStyle, s: &str, max_w: f32) -> Vec<String> {
    let max_w = max_w.max(size);
    let mut out = Vec::new();
    for para in s.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let cand = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if c.measure(size, &cand, style) <= max_w {
                line = cand;
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            // a word longer than the line is cut by characters
            let mut piece = String::new();
            for ch in word.chars() {
                piece.push(ch);
                if c.measure(size, &piece, style) > max_w && piece.chars().count() > 1 {
                    piece.pop();
                    out.push(std::mem::take(&mut piece));
                    piece.push(ch);
                }
            }
            line = piece;
        }
        out.push(line);
    }
    out
}
