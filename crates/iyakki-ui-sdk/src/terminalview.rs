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
//! The Full button opens a whole session instead: a real program on a pseudo-terminal (`vi`, `top`, `ssh`), asked for once ("Open a full
//! terminal?" with Allow and Deny) and then drawn as the cell grid the core keeps. Every key goes to the program (Esc and Tab too; the Leave
//! button is the way out), text is pasted, the grid follows the size of the area and the wheel scrolls the history.
//!
//! A line that changes something does not run: the core asks, the view shows "Allow" and "Deny" (keys `y` and `n`; Esc denies), and the answer goes back to the core. The view never runs anything itself.

use crate::canvas::{fit, Canvas, TextStyle};
use crate::event::UiKey;
use crate::geom::{Color, Rect};
use crate::size::{Metrics, TypeRole};
use crate::terminal::{Frame, Question, ScreenColor, ScreenKey, TermError, TerminalPort};
use crate::theme::Theme;
use crate::tokens::{Radius, Space};
use crate::widgets;

/// The widest share of the terminal's input row that the Run button may take.
const RUN_SHARE: f32 = 0.35;

pub const ACTIONS: u32 = 8;
const A_FIELD: u32 = 0;
const A_RUN: u32 = 1;
const A_ALLOW: u32 = 2;
const A_DENY: u32 = 3;
const A_CLEAR: u32 = 4;
const A_FULL: u32 = 6;
const A_LEAVE: u32 = 7;
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
    screen: Screen,
}

/// Where the whole-session terminal is.
#[derive(Default)]
enum Screen {
    #[default]
    Off,
    /// Asked, the person has not answered.
    Asking,
    Live { frame: Frame, asked_size: (usize, usize) },
}

/// The size the first request of a whole session uses before the area is known.
const FIRST_SIZE: (usize, usize) = (80, 24);

impl TerminalView {
    pub fn new(port: Box<dyn TerminalPort>, action_base: u32) -> TerminalView {
        TerminalView { port, base: action_base, opened: false, lines: Vec::new(), prompt: "$".into(), input: String::new(), focused: true, question: None, history: Vec::new(), hist_at: None, scroll_back: 0, error: None, screen: Screen::Off }
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
        if let Screen::Live { .. } = self.screen {
            match self.port.screen_frame() {
                Ok(f) => {
                    if let Screen::Live { frame, .. } = &mut self.screen {
                        if f.seq != frame.seq || f.cols != frame.cols || f.rows != frame.rows {
                            *frame = f;
                        }
                    }
                }
                Err(e) => {
                    self.screen = Screen::Off;
                    self.fail(e);
                }
            }
            return;
        }
        if matches!(self.screen, Screen::Asking) {
            return;
        }
        self.ensure_open();
    }

    /// Whether a whole session is on screen (or asked for).
    pub fn in_screen(&self) -> bool {
        !matches!(self.screen, Screen::Off)
    }

    /// The whole session's screen as text (for tests and a screen reader).
    pub fn screen_text(&self) -> Option<String> {
        match &self.screen {
            Screen::Live { frame, .. } => Some(frame.text()),
            _ => None,
        }
    }

    /// Ask for a whole session. Nothing starts until the person answers the card.
    pub fn open_screen(&mut self) -> bool {
        if self.in_screen() || self.question.is_some() {
            return false;
        }
        match self.port.screen_open(FIRST_SIZE.0, FIRST_SIZE.1) {
            Ok(q) => {
                self.question = Some(q);
                self.screen = Screen::Asking;
                true
            }
            Err(e) => {
                self.fail(e);
                false
            }
        }
    }

    /// Ask for a whole session inside a pod (the K8S tile's "Open a terminal"): the same card as a full terminal, naming the pod. Nothing starts
    /// until the person answers it.
    pub fn open_pod(&mut self, pod: &crate::terminal::PodSession) -> bool {
        if self.in_screen() || self.question.is_some() {
            return false;
        }
        match self.port.screen_open_pod(FIRST_SIZE.0, FIRST_SIZE.1, pod) {
            Ok(q) => {
                self.question = Some(q);
                self.screen = Screen::Asking;
                true
            }
            Err(e) => {
                self.fail(e);
                false
            }
        }
    }

    pub fn leave_screen(&mut self) {
        if self.in_screen() {
            self.port.screen_close();
            self.screen = Screen::Off;
            if matches!(self.question, Some(_)) {
                self.question = None;
            }
            self.push(Kind::Note, "The full terminal was closed.");
        }
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
        if matches!(self.screen, Screen::Asking) {
            match self.port.screen_decide(&q.call, approve) {
                Ok(()) if approve => {
                    self.screen = Screen::Live { frame: Frame::default(), asked_size: FIRST_SIZE };
                    self.error = None;
                    self.tick();
                }
                Ok(()) => {
                    self.screen = Screen::Off;
                    self.push(Kind::Note, "Denied.");
                }
                Err(e) => {
                    self.screen = Screen::Off;
                    self.fail(e);
                }
            }
            return;
        }
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
            A_FULL => {
                self.open_screen();
            }
            A_LEAVE => self.leave_screen(),
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
        if let Screen::Live { .. } = self.screen {
            return self.screen_key(k);
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

    /// A key for the program of a whole session: all of them go (a control character is Ctrl and its letter). While the session is only
    /// asked for, keys are not taken (the card has them).
    fn screen_key(&mut self, k: UiKey) -> bool {
        let (key, ctrl) = match k {
            UiKey::Char(c) if (c as u32) > 0 && (c as u32) < 27 && !matches!(c, '\t' | '\n' | '\r' | '\u{8}') => (ScreenKey::Char((b'a' + (c as u32 as u8) - 1) as char), true),
            UiKey::Char(c) if c.is_control() => return true,
            UiKey::Char(c) => (ScreenKey::Char(c), false),
            UiKey::Enter => (ScreenKey::Enter, false),
            UiKey::Backspace => (ScreenKey::Backspace, false),
            UiKey::Delete => (ScreenKey::Delete, false),
            UiKey::Esc => (ScreenKey::Esc, false),
            UiKey::Tab => (ScreenKey::Tab, false),
            UiKey::Up => (ScreenKey::Up, false),
            UiKey::Down => (ScreenKey::Down, false),
            UiKey::Left => (ScreenKey::Left, false),
            UiKey::Right => (ScreenKey::Right, false),
        };
        if let Err(e) = self.port.screen_key(key, ctrl, false) {
            self.screen = Screen::Off;
            self.fail(e);
        } else {
            self.tick();
        }
        true
    }

    pub fn text(&mut self, t: &str) -> bool {
        if let Screen::Live { .. } = self.screen {
            if let Err(e) = self.port.screen_text(t) {
                self.screen = Screen::Off;
                self.fail(e);
            } else {
                self.tick();
            }
            return true;
        }
        if matches!(self.screen, Screen::Asking) {
            return true;
        }
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
        if let Screen::Live { .. } = self.screen {
            let _ = self.port.screen_scroll(if d > 0 { 3 } else { -3 });
            self.tick();
            return;
        }
        self.scroll_back = if d > 0 { self.scroll_back.saturating_add(3) } else { self.scroll_back.saturating_sub(3) };
    }

    pub fn close(&mut self) {
        if self.in_screen() {
            self.port.screen_close();
            self.screen = Screen::Off;
            self.question = None;
        }
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
        if let Screen::Live { .. } = self.screen {
            self.draw_screen(c, th, m, inner, row);
            return;
        }
        let run_w = m.em(4.0).min(row.w * RUN_SHARE);
        let full_w = m.em(4.5).min(row.w * RUN_SHARE);
        let (f_area, b_area) = row.split_left((row.w - run_w - full_w - 2.0 * sm).max(0.0));
        let label = format!("{} ", self.prompt);
        widgets::field(c, th, m, f_area, &format!("{label}type a command"), &self.input, self.focused, self.base + A_FIELD);
        widgets::button(c, th, m, Rect::new(b_area.x + sm, b_area.y, full_w, b_area.h), "Full", self.base + A_FULL, false);
        widgets::button(c, th, m, Rect::new(b_area.x + 2.0 * sm + full_w, b_area.y, (b_area.w - full_w - 2.0 * sm).max(0.0), b_area.h), "Run", self.base + A_RUN, true);
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

    /// The whole session: the cell grid, the cursor, and a row with the title and the Leave button. The grid takes the size of the area:
    /// when it changed the core is told (once per size).
    fn draw_screen(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, inner: Rect, row: Rect) {
        let fs = m.size(TypeRole::Small);
        let lh = m.leading(fs);
        let cw = c.measure(fs, "M", TextStyle::Mono).max(1.0);
        let sm = m.gap(Space::Sm);
        let grid = Rect::new(inner.x, inner.y, inner.w, (row.y - sm - inner.y).max(0.0));
        let want = (((grid.w / cw).floor() as usize).clamp(1, 300), ((grid.h / lh).floor() as usize).clamp(1, 100));
        let asked = match &self.screen {
            Screen::Live { asked_size, .. } => *asked_size,
            _ => want,
        };
        if want != asked && grid.w > 0.0 && grid.h > 0.0 {
            if let Screen::Live { asked_size, .. } = &mut self.screen {
                *asked_size = want;
            }
            if let Err(e) = self.port.screen_resize(want.0, want.1) {
                self.screen = Screen::Off;
                self.fail(e);
                return;
            }
            self.tick();
        }
        let Screen::Live { frame, .. } = &self.screen else { return };
        let (fg0, bg0) = (th.text, th.field);
        c.clipped(grid, &mut |c| {
            for (y, cells) in frame.cells.iter().enumerate().take(want.1.max(frame.rows.min(100))) {
                let py = grid.y + y as f32 * lh;
                if py + lh > grid.bottom() + 0.5 {
                    break;
                }
                // runs of cells with the same look are drawn as one text
                let mut x = 0;
                while x < cells.len() && (x as f32 + 1.0) * cw <= grid.w + 0.5 {
                    let first = cells[x];
                    let (fg, bg) = look(&first, fg0, bg0);
                    let mut end = x + 1;
                    let mut text = String::new();
                    text.push(if first.tail { ' ' } else { first.ch });
                    while end < cells.len() && (end as f32 + 1.0) * cw <= grid.w + 0.5 && look(&cells[end], fg0, bg0) == (fg, bg) && cells[end].attrs & 8 == first.attrs & 8 {
                        text.push(if cells[end].tail { ' ' } else { cells[end].ch });
                        end += 1;
                    }
                    let rect = Rect::new(grid.x + x as f32 * cw, py, (end - x) as f32 * cw, lh);
                    if bg != bg0 {
                        c.rect(rect, bg);
                    }
                    if first.attrs & 64 == 0 && !text.trim().is_empty() {
                        c.text(rect.x, py, fs, fg, &text, TextStyle::Mono);
                    }
                    if first.attrs & 8 != 0 {
                        c.line(rect.x, py + lh - 1.0, rect.right(), py + lh - 1.0, 1.0, fg);
                    }
                    x = end;
                }
            }
            if frame.cursor_visible && frame.exited.is_none() {
                let r = Rect::new(grid.x + frame.cursor.0 as f32 * cw, grid.y + frame.cursor.1 as f32 * lh, cw, lh);
                c.outline(r, 0.0, 1.5, th.accent);
            }
        });
        let status = match frame.exited {
            Some(code) => format!("The program ended (exit {code}). Leave to close."),
            None if frame.back > 0 => format!("Scrolled back {} of {} lines", frame.back, frame.history),
            None => "Full terminal. Every key goes to the program; Leave closes it.".to_string(),
        };
        let leave_w = m.em(5.0).min(row.w * RUN_SHARE);
        let shown = fit(c, fs, &status, TextStyle::Regular, (row.w - leave_w - sm).max(0.0));
        c.text(row.x, row.y + (row.h - lh) / 2.0, fs, th.muted, &shown, TextStyle::Regular);
        widgets::button(c, th, m, Rect::new(row.right() - leave_w, row.y, leave_w, row.h), "Leave", self.base + A_LEAVE, true);
    }
}

/// The colours a cell is drawn with: reverse swaps them, bold brightens the eight basic colours, dim fades the text.
fn look(cell: &crate::terminal::ScreenCell, fg0: Color, bg0: Color) -> (Color, Color) {
    let bold = cell.attrs & 1 != 0;
    let pick = |c: ScreenColor, default: Color, bright: bool| match c {
        ScreenColor::Default => default,
        ScreenColor::Indexed(i) => palette(if bright && i < 8 { i + 8 } else { i }),
        ScreenColor::Rgb(r, g, b) => [r, g, b, 255],
    };
    let (mut fg, mut bg) = (pick(cell.fg, fg0, bold), pick(cell.bg, bg0, false));
    if cell.attrs & 32 != 0 {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.attrs & 2 != 0 {
        fg = [fg[0] / 2 + bg[0] / 2, fg[1] / 2 + bg[1] / 2, fg[2] / 2 + bg[2] / 2, 255];
    }
    (fg, bg)
}

/// The xterm colour of an index: the sixteen, the 6x6x6 cube, the grey ramp.
pub fn palette(i: u8) -> Color {
    const BASE: [[u8; 3]; 16] = [[0, 0, 0], [205, 49, 49], [13, 188, 121], [229, 229, 16], [36, 114, 200], [188, 63, 188], [17, 168, 205], [229, 229, 229], [102, 102, 102], [241, 76, 76], [35, 209, 139], [245, 245, 67], [59, 142, 234], [214, 112, 214], [41, 184, 219], [255, 255, 255]];
    match i {
        0..=15 => {
            let b = BASE[i as usize];
            [b[0], b[1], b[2], 255]
        }
        16..=231 => {
            let n = i - 16;
            let lv = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            [lv(n / 36), lv((n / 6) % 6), lv(n % 6), 255]
        }
        _ => {
            let g = 8 + 10 * (i - 232);
            [g, g, g, 255]
        }
    }
}
