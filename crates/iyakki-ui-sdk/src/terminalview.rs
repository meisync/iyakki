//! `TerminalView`: the IYAKKI command line as a component an SDK app places in its layout (docs/development/UI-SDK.md, "Embedding the
//! terminal"). The app keeps a `TerminalView` in its state, gives it a [`TerminalPort`] (the core's) and a base action number, and a rectangle
//! to draw in. The view owns what is on screen: the lines typed and printed, the field, the person's yes/no card. It draws through [`Canvas`]
//! in the monospace style, so it works in the shell, in a browser or client and as a WASM tile.
//!
//! What the app does:
//! ```text
//! event(Tick):   term.tick()                                      (opens the session once)
//! event(Action): if term.owns(a) { term.action(a) }   event(Key): if term.key(k) { return }   event(Text): term.text(t)
//! view:          term.draw(c, th, &metrics, rect)     leaving: term.close()
//! ```
//! A line that changes something does not run: the core asks, the view shows "Allow" and "Deny" (keys `y` and `n`; Esc denies), and the answer goes back to the core. The view never runs anything itself.

use crate::canvas::{fit, Canvas, TextStyle};
use crate::event::UiKey;
use crate::geom::Rect;
use crate::size::{Metrics, TypeRole};
use crate::terminal::{Question, TermError, TerminalPort};
use crate::theme::Theme;
use crate::tokens::{Radius, Space};
use crate::widgets;

/// The widest share of the terminal's input row that the Run button may take.
const RUN_SHARE: f32 = 0.35;

pub const ACTIONS: u32 = 6;
const A_FIELD: u32 = 0;
const A_RUN: u32 = 1;
const A_ALLOW: u32 = 2;
const A_DENY: u32 = 3;
const A_CLEAR: u32 = 4;
/// The longest line the field takes and how many typed lines are remembered.
pub const MAX_INPUT: usize = 1000;
const MAX_HISTORY: usize = 50;
/// Lines kept on screen (older ones are dropped from the view; the core keeps nothing but the session).
const MAX_LINES: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Typed,
    Output,
    Note,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: Kind,
    pub text: String,
}

pub struct TerminalView {
    port: Box<dyn TerminalPort>,
    base: u32,
    opened: bool,
    pub lines: Vec<Line>,
    prompt: String,
    pub input: String,
    pub focused: bool,
    question: Option<Question>,
    history: Vec<String>,
    hist_at: Option<usize>,
    scroll_back: usize,
    error: Option<String>,
}

impl TerminalView {
    pub fn new(port: Box<dyn TerminalPort>, action_base: u32) -> TerminalView {
        TerminalView { port, base: action_base, opened: false, lines: Vec::new(), prompt: "$".into(), input: String::new(), focused: true, question: None, history: Vec::new(), hist_at: None, scroll_back: 0, error: None }
    }

    pub fn owns(&self, a: u32) -> bool {
        (self.base..self.base + ACTIONS).contains(&a)
    }

    pub fn wants_keyboard(&self) -> bool {
        self.focused
    }

    pub fn pending_question(&self) -> Option<&Question> {
        self.question.as_ref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn push(&mut self, kind: Kind, text: &str) {
        for l in text.split('\n') {
            self.lines.push(Line { kind, text: l.to_string() });
        }
        if self.lines.len() > MAX_LINES {
            let n = self.lines.len() - MAX_LINES;
            self.lines.drain(..n);
        }
        self.scroll_back = 0;
    }

    fn fail(&mut self, e: TermError) {
        let s = e.sentence();
        self.error = Some(s.clone());
        self.push(Kind::Error, &s);
    }

    /// Open the session once. A refusal is shown and tried again at the next line.
    pub fn tick(&mut self) {
        self.ensure_open();
    }

    fn ensure_open(&mut self) -> bool {
        if self.opened {
            return true;
        }
        match self.port.open() {
            Ok(o) => {
                self.opened = true;
                self.error = None;
                self.prompt = o.prompt;
                if !o.banner.is_empty() {
                    self.push(Kind::Note, &o.banner);
                }
                true
            }
            Err(e) => {
                if self.error.is_none() {
                    self.fail(e);
                }
                false
            }
        }
    }

    fn take(&mut self, r: Result<crate::terminal::Reply, TermError>) {
        match r {
            Ok(rep) => {
                if rep.clear {
                    self.lines.clear();
                }
                if !rep.text.is_empty() {
                    self.push(Kind::Output, &rep.text);
                }
                if rep.cut {
                    self.push(Kind::Note, "(the output was cut)");
                }
                if !rep.prompt.is_empty() {
                    self.prompt = rep.prompt;
                }
                if let Some(q) = rep.question {
                    self.question = Some(q);
                }
                self.error = None;
            }
            Err(e) => self.fail(e),
        }
    }

    /// Run a line as the person typed it. Returns whether it was sent.
    pub fn submit(&mut self, line: &str) -> bool {
        let l = line.trim();
        if l.is_empty() || self.question.is_some() || !self.ensure_open() {
            return false;
        }
        self.push(Kind::Typed, &format!("{} {}", self.prompt, l));
        if self.history.last().map(String::as_str) != Some(l) {
            self.history.push(l.to_string());
            if self.history.len() > MAX_HISTORY {
                self.history.remove(0);
            }
        }
        self.hist_at = None;
        let r = self.port.run(l);
        self.take(r);
        true
    }

    /// The person's answer to the waiting line.
    pub fn decide(&mut self, approve: bool) {
        let Some(q) = self.question.take() else { return };
        self.push(Kind::Note, if approve { "Allowed." } else { "Denied." });
        let r = self.port.decide(&q.call, approve);
        self.take(r);
    }

    pub fn action(&mut self, a: u32) -> bool {
        if !self.owns(a) {
            return false;
        }
        match a - self.base {
            A_FIELD => self.focused = true,
            A_RUN => {
                let t = std::mem::take(&mut self.input);
                if !self.submit(&t) {
                    self.input = t;
                }
            }
            A_ALLOW => self.decide(true),
            A_DENY => self.decide(false),
            A_CLEAR => self.lines.clear(),
            _ => {}
        }
        true
    }

    /// Keys: in the field typing, Enter runs, Up and Down walk the lines typed before, Esc leaves the field; while a question waits `y` allows
    /// and `n` or Esc denies and nothing else is taken. Returns whether the key was used.
    pub fn key(&mut self, k: UiKey) -> bool {
        if self.question.is_some() {
            match k {
                UiKey::Char('y') | UiKey::Char('Y') => self.decide(true),
                UiKey::Char('n') | UiKey::Char('N') | UiKey::Esc => self.decide(false),
                _ => return false,
            }
            return true;
        }
        if !self.focused {
            return false;
        }
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
                if !self.submit(&t) {
                    self.input = t;
                }
            }
            UiKey::Up if !self.history.is_empty() => {
                let i = self.hist_at.map_or(self.history.len() - 1, |i| i.saturating_sub(1));
                self.hist_at = Some(i);
                self.input = self.history[i].clone();
            }
            UiKey::Down => {
                self.hist_at = match self.hist_at {
                    Some(i) if i + 1 < self.history.len() => {
                        self.input = self.history[i + 1].clone();
                        Some(i + 1)
                    }
                    _ => {
                        self.input.clear();
                        None
                    }
                }
            }
            UiKey::Esc | UiKey::Tab => self.focused = false,
            _ => return false,
        }
        true
    }

    pub fn text(&mut self, t: &str) -> bool {
        if !self.focused {
            return false;
        }
        for c in t.chars().filter(|c| !c.is_control()) {
            if self.input.chars().count() < MAX_INPUT {
                self.input.push(c);
            }
        }
        true
    }

    pub fn wheel(&mut self, d: i32) {
        self.scroll_back = if d > 0 { self.scroll_back.saturating_add(3) } else { self.scroll_back.saturating_sub(3) };
    }

    pub fn close(&mut self) {
        if self.opened {
            self.port.close();
            self.opened = false;
            self.question = None;
        }
    }

    /// The screen as plain lines, for tests and a screen reader summary.
    pub fn transcript(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.text.clone()).collect()
    }

    pub fn draw(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, area: Rect) {
        let t = m.min_target();
        widgets::surface(c, th, m, area, th.field, Radius::Lg, true);
        let (sm, md) = (m.gap(Space::Sm), m.gap(Space::Md));
        let inner = area.inset(md);
        // the input row at the bottom: the prompt, the field, Run
        let row = Rect::new(inner.x, (inner.bottom() - t).max(inner.y), inner.w, t);
        let run_w = m.em(4.0).min(row.w * RUN_SHARE);
        let (f_area, b_area) = row.split_left((row.w - run_w - sm).max(0.0));
        let label = format!("{} ", self.prompt);
        widgets::field(c, th, m, f_area, &format!("{label}type a command"), &self.input, self.focused, self.base + A_FIELD);
        widgets::button(c, th, m, Rect::new(b_area.x + sm, b_area.y, (b_area.w - sm).max(0.0), b_area.h), "Run", self.base + A_RUN, true);
        let mut bottom = row.y - sm;
        // the question card
        if let Some(q) = self.question.clone() {
            let size = m.size(TypeRole::Body);
            let shown = fit(c, size, &q.summary, TextStyle::Bold, inner.w - 2.0 * md);
            let h = m.leading(size) + t + 2.0 * md + m.gap(Space::Xs);
            if bottom - h > inner.y {
                let card = Rect::new(inner.x, bottom - h, inner.w, h);
                let rad = widgets::surface(c, th, m, card, th.card_sel, Radius::Md, true);
                c.outline(card, rad, m.tokens.focus_width(), th.amber);
                c.text(card.x + md, card.y + sm, size, th.text, &shown, TextStyle::Bold);
                let by = card.bottom() - t - sm;
                let bw = ((card.w - md - 2.0 * md) / 2.0).max(0.0);
                widgets::button(c, th, m, Rect::new(card.x + md, by, bw, t), "Allow", self.base + A_ALLOW, true);
                widgets::button(c, th, m, Rect::new(card.x + 2.0 * md + bw, by, bw, t), "Deny", self.base + A_DENY, false);
                bottom -= h + sm;
            }
        }
        // the lines, newest at the bottom, each cut to the width (a long line is wrapped by characters)
        let fs = m.size(TypeRole::Small);
        let lh = m.leading(fs);
        let cw = c.measure(fs, "M", TextStyle::Mono).max(1.0);
        let per_line = ((inner.w / cw).floor() as usize).max(8);
        let rows_fit = (((bottom - inner.y) / lh).floor() as usize).max(1);
        let mut visual: Vec<(Kind, String)> = Vec::new();
        for l in &self.lines {
            let chars: Vec<char> = l.text.chars().collect();
            if chars.is_empty() {
                visual.push((l.kind, String::new()));
            }
            for chunk in chars.chunks(per_line) {
                visual.push((l.kind, chunk.iter().collect()));
            }
        }
        let max_back = visual.len().saturating_sub(rows_fit);
        self.scroll_back = self.scroll_back.min(max_back);
        let end = visual.len() - self.scroll_back;
        let start = end.saturating_sub(rows_fit);
        let area_text = Rect::new(inner.x, inner.y, inner.w, (bottom - inner.y).max(0.0));
        c.clipped(area_text, &mut |c| {
            for (i, (kind, s)) in visual[start..end].iter().enumerate() {
                let y = area_text.bottom() - (end - start - i) as f32 * lh;
                let col = match kind {
                    Kind::Typed => th.accent,
                    Kind::Output => th.text,
                    Kind::Note => th.muted,
                    Kind::Error => th.err,
                };
                c.text(area_text.x, y, fs, col, s, TextStyle::Mono);
            }
        });
        if visual.is_empty() {
            c.text(inner.x, inner.y, fs, th.dim, "Nothing here yet.", TextStyle::Regular);
        }
    }
}
