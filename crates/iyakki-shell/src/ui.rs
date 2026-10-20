//! Shared UI pieces: input events, colour theme, hit-testing and a single-line text field.

use crate::gfx::{rgb, Face, Gfx};

/// Navigation keys that can carry modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    /// An arrow / Home / End / Page key with Shift (extend selection) and/or Ctrl (by word, or to the
    /// start/end of the document) held. Plain presses stay `Up`, `Left`, …
    Nav { nav: Nav, shift: bool, ctrl: bool },
    Char(char),
    /// Ctrl+letter. An uppercase letter means Shift was held too (Ctrl+Shift+V = `Ctrl('V')`).
    Ctrl(char),
    Enter,
    Backspace,
    Delete,
    Tab,
    BackTab,
    Esc,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// A function key, F1 to F12.
    F(u8),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Key(Key),
    MouseMove(i32, i32),
    MouseDown(i32, i32),
    MouseRight(i32, i32),
    /// The left button (or the touch) was released.
    MouseUp(i32, i32),
    /// Positive = scroll up (towards older content).
    Scroll(i32),
    /// ~30 Hz heartbeat for animation and background results.
    Tick,
    /// Text to insert into the focused field (from the host clipboard or the clipboard picker).
    Paste(String),
    /// Select tab `n` (from a click on a tab).
    Tab(usize),
    /// Select row `n` of the current list (from a click).
    Row(usize),
    /// Click landed on field `id`; `x` is the distance from the field's left edge (pixels).
    FocusAt(usize, f32),
    /// Registered on hit regions; the shell turns it into [`Event::FocusAt`] with the click position.
    Focus(usize),
    /// Click landed in area `id` at (`x`, `y`) pixels from its top-left corner (text editors).
    AreaAt(usize, f32, f32),
    /// Registered on hit regions; the shell turns it into [`Event::AreaAt`] with the click position.
    Area(usize),
    /// A key meant for an embedded component (the editor inside the chat) rather than its host.
    Sub(u8, Key),
}

/// A clickable region registered while drawing. Later registrations sit on top of earlier ones.
/// The shape the mouse pointer should have where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pointer {
    #[default]
    Default,
    /// Over a link or button.
    Hand,
    /// Over text that can be edited or selected.
    Text,
    Wait,
    Crosshair,
    Move,
    NotAllowed,
    ResizeH,
    ResizeV,
    Grab,
    Help,
    Hidden,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub rect: R,
    pub ev: Event,
    /// Human-readable name (button text); used by tests to find controls.
    pub label: String,
}

/// How much bigger or smaller than usual the person wants text and buttons (1.0 is the screen's own best guess). Set at sign-in
/// from the person's saved choice and put back to 1.0 at the sign-in screen. It belongs to the thread that runs the shell, so
/// each person signed in through the browser has their own.
pub fn ui_size() -> f32 {
    UI_SIZE.with(|v| v.get())
}

pub fn set_ui_size(v: f32) {
    UI_SIZE.with(|c| c.set(v.clamp(0.5, 3.0)));
}

thread_local! {
    static UI_SIZE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
    static DRAGGING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static CLICKS: std::cell::Cell<u8> = const { std::cell::Cell::new(1) };
    static SHIFT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// True while the shell is repeating a press as the pointer moves with the button held: a text field or an editor that
/// gets `FocusAt`/`AreaAt` in that state extends its selection instead of starting a new one.
pub fn dragging() -> bool {
    DRAGGING.with(|d| d.get())
}

pub fn set_dragging(on: bool) {
    DRAGGING.with(|d| d.set(on));
}

/// 1 for a single click, 2 for a double click (select a word), 3 for a triple click (select everything); set by the shell
/// around the press event.
pub fn click_count() -> u8 {
    CLICKS.with(|c| c.get())
}

/// Whether Shift is held while the pointer event being handled happened (a shift+click extends a selection). The front
/// ends set it around pointer events; it is false otherwise.
pub fn shift_held() -> bool {
    SHIFT.with(|c| c.get())
}

pub fn set_shift_held(on: bool) {
    SHIFT.with(|c| c.set(on));
}

pub fn set_click_count(n: u8) {
    CLICKS.with(|c| c.set(n.clamp(1, 3)));
}

pub const BG_TOP: [u8; 4] = rgb(20, 25, 44);
pub const BG_BOTTOM: [u8; 4] = rgb(9, 11, 20);
pub const CARD: [u8; 4] = rgb(30, 37, 62);
pub const CARD_HOVER: [u8; 4] = rgb(40, 49, 80);
pub const CARD_SEL: [u8; 4] = rgb(48, 58, 96);
pub const TEXT: [u8; 4] = rgb(231, 235, 247);
pub const MUTED: [u8; 4] = rgb(165, 175, 202);
pub const DIM: [u8; 4] = rgb(132, 142, 174);
pub const ERR: [u8; 4] = rgb(255, 120, 120);
pub const OK: [u8; 4] = rgb(110, 224, 160);
pub const FIELD: [u8; 4] = rgb(16, 20, 36);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> R {
        R { x, y, w, h }
    }
    pub fn contains(&self, px: i32, py: i32) -> bool {
        let (px, py) = (px as f32, py as f32);
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    pub fn inset(&self, d: f32) -> R {
        R::new(self.x + d, self.y + d, (self.w - 2.0 * d).max(0.0), (self.h - 2.0 * d).max(0.0))
    }
}

/// Process-wide (per UI thread) clipboard with history, plus a notice channel for toasts.
/// The desktop window mirrors copies to the system clipboard; the bare-metal shell has only this.
pub mod clipboard {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// One copy in the queue: some text, or a picture (a screenshot).
    #[derive(Clone, Debug)]
    pub enum Entry {
        Text(String),
        Picture { width: u32, height: u32, rgba: Rc<Vec<u8>> },
    }

    /// How many copies are kept unless the person chooses otherwise, and the most they can choose.
    pub const DEFAULT_LIMIT: usize = 10;
    pub const MAX_LIMIT: usize = 50;
    pub const MAX_BYTES: usize = 64 * 1024;

    #[derive(Default)]
    struct State {
        items: VecDeque<Entry>,
        host_out: Option<String>,
        toast: Option<String>,
        /// The latest picture copied (width, height, RGBA), and whether the host still has to be given it.
        image: Option<(u32, u32, Vec<u8>)>,
        host_image: bool,
        /// How many copies to keep (0: the default).
        limit: usize,
    }

    thread_local! {
        static S: RefCell<State> = RefCell::new(State::default());
    }

    fn push(text: &str) {
        let t: String = text.chars().take(MAX_BYTES).collect();
        if t.is_empty() {
            return;
        }
        S.with(|s| {
            let mut s = s.borrow_mut();
            s.items.retain(|i| !matches!(i, Entry::Text(x) if *x == t));
            s.items.push_front(Entry::Text(t));
            let keep = if s.limit == 0 { DEFAULT_LIMIT } else { s.limit };
            s.items.truncate(keep);
        });
    }

    /// How many copies the history keeps.
    pub fn limit() -> usize {
        S.with(|s| match s.borrow().limit {
            0 => DEFAULT_LIMIT,
            n => n,
        })
    }

    /// Keep this many copies (1 to `MAX_LIMIT`); the oldest are dropped at once.
    pub fn set_limit(n: usize) {
        let n = n.clamp(1, MAX_LIMIT);
        S.with(|s| {
            let mut s = s.borrow_mut();
            s.limit = n;
            s.items.truncate(n);
        });
    }

    /// Copy inside IYAKKI (also queued for the host clipboard).
    pub fn copy(text: &str) {
        push(text);
        S.with(|s| {
            let mut s = s.borrow_mut();
            s.host_out = Some(text.chars().take(MAX_BYTES).collect());
            s.toast = Some("Copied to clipboard".into());
        });
    }

    /// Take `text` out of the copy history again (a secret that was copied once to be pasted).
    pub fn forget(text: &str) {
        S.with(|s| s.borrow_mut().items.retain(|i| !matches!(i, Entry::Text(x) if x == text)));
    }

    /// Record text that arrived *from* the host clipboard (not sent back out).
    pub fn remember(text: &str) {
        push(text);
    }

    /// The newest copy that is text: what a plain paste uses.
    pub fn latest() -> Option<String> {
        S.with(|s| {
            s.borrow().items.iter().find_map(|e| match e {
                Entry::Text(t) => Some(t.clone()),
                Entry::Picture { .. } => None,
            })
        })
    }

    /// The text copies, newest first.
    pub fn history() -> Vec<String> {
        S.with(|s| {
            s.borrow()
                .items
                .iter()
                .filter_map(|e| match e {
                    Entry::Text(t) => Some(t.clone()),
                    Entry::Picture { .. } => None,
                })
                .collect()
        })
    }

    /// Everything in the queue, text and pictures, newest first.
    pub fn entries() -> Vec<Entry> {
        S.with(|s| s.borrow().items.iter().cloned().collect())
    }

    /// Make the picture at `index` of [`entries`] the current one again (it moves to the front and goes to the system
    /// clipboard). False when that entry is not a picture.
    pub fn promote_picture(index: usize) -> bool {
        S.with(|s| {
            let mut s = s.borrow_mut();
            let Some(Entry::Picture { width, height, rgba }) = s.items.get(index).cloned() else { return false };
            let e = s.items.remove(index);
            if let Some(e) = e {
                s.items.push_front(e);
            }
            s.image = Some((width, height, (*rgba).clone()));
            s.host_image = true;
            true
        })
    }

    pub fn take_host_copy() -> Option<String> {
        S.with(|s| s.borrow_mut().host_out.take())
    }

    pub fn toast(msg: &str) {
        S.with(|s| s.borrow_mut().toast = Some(msg.into()));
    }

    /// Copy a picture (a screenshot): kept inside IYAKKI and queued for the host clipboard.
    pub fn copy_image(width: u32, height: u32, rgba: Vec<u8>) {
        S.with(|s| {
            let mut s = s.borrow_mut();
            s.items.push_front(Entry::Picture { width, height, rgba: Rc::new(rgba.clone()) });
            let keep = if s.limit == 0 { DEFAULT_LIMIT } else { s.limit };
            s.items.truncate(keep);
            s.image = Some((width, height, rgba));
            s.host_image = true;
        });
    }

    /// The picture last copied, if any.
    pub fn latest_image() -> Option<(u32, u32, Vec<u8>)> {
        S.with(|s| s.borrow().image.clone())
    }

    /// The picture for the host (desktop window) to put on the system clipboard, once.
    pub fn take_host_image() -> Option<(u32, u32, Vec<u8>)> {
        S.with(|s| {
            let mut s = s.borrow_mut();
            if std::mem::take(&mut s.host_image) { s.image.clone() } else { None }
        })
    }

    pub fn take_toast() -> Option<String> {
        S.with(|s| s.borrow_mut().toast.take())
    }

    pub fn clear() {
        S.with(|s| *s.borrow_mut() = State::default());
    }

    /// One-line description for the picker. Token-like strings (long, no spaces) are shortened so a
    /// pasted API key is never shown in full on screen.
    pub fn preview(item: &str) -> String {
        let n = item.chars().count();
        let one_line: String = item.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        if n >= 16 && !item.chars().any(char::is_whitespace) {
            let head: String = item.chars().take(4).collect();
            format!("{head}…  ({n} characters)")
        } else if n > 70 {
            format!("{}…  ({n} characters)", one_line.chars().take(60).collect::<String>())
        } else {
            one_line
        }
    }
}

thread_local! {
    static CARETS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RINGS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static WINDOW_FOCUSED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    static PANE_INPUT_OK: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    #[cfg(test)]
    static LEGACY_RINGS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}


/// The host tells whether the window has the operating system's keyboard focus. Without it no input shows a caret or a focus ring.
pub fn set_window_focused(on: bool) {
    WINDOW_FOCUSED.with(|c| c.set(on));
}

pub fn window_focused() -> bool {
    WINDOW_FOCUSED.with(|c| c.get())
}

/// May an input being drawn now look focused (accent frame, caret)? Only while the window has the focus and the pane or dialog
/// being drawn owns the keyboard (see [`with_input_focus`]).
pub fn input_focus_allowed() -> bool {
    window_focused() && PANE_INPUT_OK.with(|c| c.get())
}

/// Draw `f` with focused-looking inputs allowed only when `on` (and they already were). The AI IDE and the chat draw several
/// inputs side by side; only the one that owns the keyboard may show a caret and the accent frame.
pub fn with_input_focus<T>(on: bool, f: impl FnOnce() -> T) -> T {
    let before = PANE_INPUT_OK.with(|c| c.replace(c.get() && on));
    let out = f();
    PANE_INPUT_OK.with(|c| c.set(before));
    out
}

/// A focused-looking input frame was drawn (tests count them, like carets).
pub fn note_focus_ring() {
    RINGS.with(|c| c.set(c.get() + 1));
}

/// How many focus frames were drawn since the last call.
pub fn take_focus_rings() -> usize {
    RINGS.with(|c| c.replace(0))
}

/// Test only: are the old always-focused looks on (see [`legacy_rings`])?
#[cfg(test)]
pub fn legacy_on() -> bool {
    LEGACY_RINGS.with(|c| c.get())
}

/// Test only: draw inputs the way they were before the one-owner rule (frame and clipboard button on a constantly "focused" box).
#[cfg(test)]
pub fn legacy_rings(on: bool) {
    LEGACY_RINGS.with(|c| c.set(on));
}

/// A caret was drawn (tests count them to prove that only the input that owns the keyboard shows one).
pub fn note_caret() {
    CARETS.with(|c| c.set(c.get() + 1));
}

/// How many carets were drawn since the last call; starts counting again from zero.
pub fn take_carets() -> usize {
    CARETS.with(|c| c.replace(0))
}

/// Single-line text input with a caret, a selection (keys, mouse drag, double click for a word), and clipboard support.
#[derive(Clone, Debug, Default)]
pub struct TextField {
    pub value: String,
    pub mask: bool,
    pub max: usize,
    pub placeholder: String,
    cursor: usize,
    /// The other end of the selection (the caret is one end).
    anchor: Option<usize>,
    /// Last drawn layout, so clicks can be mapped back to a character position.
    layout: std::cell::RefCell<FieldLayout>,
}

#[derive(Clone, Debug, Default)]
struct FieldLayout {
    start: usize,
    text_x: f32,
    advances: Vec<f32>, // cumulative widths of the visible characters (len = visible + 1)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl TextField {
    pub fn new(placeholder: &str, mask: bool) -> TextField {
        TextField { value: String::new(), mask, max: 512, placeholder: placeholder.into(), ..Default::default() }
    }

    fn len(&self) -> usize {
        self.value.chars().count()
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.value.char_indices().nth(char_idx).map_or(self.value.len(), |(b, _)| b)
    }

    pub fn cursor(&self) -> usize {
        self.cursor.min(self.len())
    }

    /// The selected range in characters (start < end), if any.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?.min(self.len());
        let c = self.cursor();
        (a != c).then(|| (a.min(c), a.max(c)))
    }

    pub fn selected_text(&self) -> Option<String> {
        let (s, e) = self.selection()?;
        Some(self.value.chars().skip(s).take(e - s).collect())
    }

    pub fn is_all_selected(&self) -> bool {
        self.selection() == Some((0, self.len())) && !self.value.is_empty()
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.len();
    }

    /// Replace the text and put the caret at the end.
    pub fn set(&mut self, text: &str) {
        self.value = text.into();
        self.cursor = self.len();
        self.anchor = None;
    }

    /// Remove the selected text; true if there was a selection.
    fn delete_selection(&mut self) -> bool {
        let Some((s, e)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let (bs, be) = (self.byte_at(s), self.byte_at(e));
        self.value.replace_range(bs..be, "");
        self.cursor = s;
        self.anchor = None;
        true
    }

    fn move_to(&mut self, pos: usize, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor());
            }
        } else {
            self.anchor = None;
        }
        self.cursor = pos.min(self.len());
    }

    /// The start of the word before `from`, or the end of the word after it.
    fn word_edge(&self, from: usize, forward: bool) -> usize {
        let chars: Vec<char> = self.value.chars().collect();
        let mut i = from.min(chars.len());
        if forward {
            while i < chars.len() && !is_word(chars[i]) {
                i += 1;
            }
            while i < chars.len() && is_word(chars[i]) {
                i += 1;
            }
        } else {
            while i > 0 && !is_word(chars[i - 1]) {
                i -= 1;
            }
            while i > 0 && is_word(chars[i - 1]) {
                i -= 1;
            }
        }
        i
    }

    /// Select the word (or the run of spaces or symbols) around character position `pos`.
    pub fn select_word_at(&mut self, pos: usize) {
        let chars: Vec<char> = self.value.chars().collect();
        if chars.is_empty() {
            return;
        }
        let i = pos.min(chars.len() - 1);
        let class = |c: char| if is_word(c) { 0 } else if c.is_whitespace() { 1 } else { 2 };
        let k = class(chars[i]);
        let (mut s, mut e) = (i, i + 1);
        while s > 0 && class(chars[s - 1]) == k {
            s -= 1;
        }
        while e < chars.len() && class(chars[e]) == k {
            e += 1;
        }
        self.anchor = Some(s);
        self.cursor = e;
    }

    fn insert(&mut self, text: &str) -> bool {
        let removed = self.delete_selection();
        let room = self.max.saturating_sub(self.len());
        let ins: String = text.chars().filter(|c| !c.is_control()).take(room).collect();
        if ins.is_empty() {
            return removed;
        }
        let at = self.byte_at(self.cursor());
        self.value.insert_str(at, &ins);
        self.cursor = self.cursor() + ins.chars().count();
        true
    }

    /// Insert pasted text at the caret (or over a selection). Newlines become spaces.
    pub fn paste(&mut self, text: &str) -> bool {
        let flat: String = text.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).collect();
        self.insert(flat.trim_end())
    }

    /// Returns true if the text changed.
    pub fn handle(&mut self, k: &Key) -> bool {
        match k {
            Key::Char(c) if !c.is_control() => self.insert(&c.to_string()),
            Key::Backspace => {
                if self.delete_selection() {
                    return true;
                }
                let c = self.cursor();
                if c == 0 {
                    return false;
                }
                let (from, to) = (self.byte_at(c - 1), self.byte_at(c));
                self.value.replace_range(from..to, "");
                self.cursor = c - 1;
                true
            }
            Key::Delete => {
                if self.delete_selection() {
                    return true;
                }
                let c = self.cursor();
                if c >= self.len() {
                    return false;
                }
                let (from, to) = (self.byte_at(c), self.byte_at(c + 1));
                self.value.replace_range(from..to, "");
                true
            }
            Key::Left => {
                match self.selection() {
                    Some((s, _)) => self.move_to(s, false),
                    None => self.move_to(self.cursor().saturating_sub(1), false),
                }
                false
            }
            Key::Right => {
                match self.selection() {
                    Some((_, e)) => self.move_to(e, false),
                    None => self.move_to(self.cursor() + 1, false),
                }
                false
            }
            Key::Home => {
                self.move_to(0, false);
                false
            }
            Key::End => {
                self.move_to(self.len(), false);
                false
            }
            // Shift extends the selection, Ctrl moves by word
            Key::Nav { nav, shift, ctrl } => {
                let c = self.cursor();
                let to = match nav {
                    Nav::Left if *ctrl => self.word_edge(c, false),
                    Nav::Left => c.saturating_sub(1),
                    Nav::Right if *ctrl => self.word_edge(c, true),
                    Nav::Right => c + 1,
                    Nav::Home => 0,
                    Nav::End => self.len(),
                    _ => return false,
                };
                self.move_to(to, *shift);
                false
            }
            Key::Ctrl('a') | Key::Ctrl('A') => {
                self.select_all();
                false
            }
            Key::Ctrl('c') | Key::Ctrl('C') | Key::Ctrl('x') | Key::Ctrl('X') => {
                if self.mask {
                    clipboard::toast("Secret fields can't be copied");
                    return false;
                }
                // the selection if there is one, otherwise the whole line
                let text = self.selected_text().unwrap_or_else(|| self.value.clone());
                if !text.is_empty() {
                    clipboard::copy(&text);
                }
                if matches!(k, Key::Ctrl('x') | Key::Ctrl('X')) {
                    if self.selection().is_some() {
                        return self.delete_selection();
                    }
                    let had = !self.value.is_empty();
                    self.value.clear();
                    self.cursor = 0;
                    self.anchor = None;
                    return had;
                }
                false
            }
            _ => false,
        }
    }

    pub fn take(&mut self) -> String {
        self.cursor = 0;
        self.anchor = None;
        std::mem::take(&mut self.value)
    }

    pub fn clear(&mut self) {
        self.take();
    }

    /// The character position nearest to `x` pixels from the field's left edge (as last drawn).
    fn pos_at(&self, x: f32) -> usize {
        let l = self.layout.borrow();
        if l.advances.is_empty() {
            return self.len();
        }
        let rel = x - l.text_x;
        let mut best = 0;
        let mut best_d = f32::MAX;
        for (i, a) in l.advances.iter().enumerate() {
            let d = (a - rel).abs();
            if d < best_d {
                best = i;
                best_d = d;
            }
        }
        (l.start + best).min(self.len())
    }

    /// A press `x` pixels from the field's left edge puts the caret there (a second press in quick succession selects the
    /// word, a third everything); the same press repeated while the pointer is dragged extends the selection.
    pub fn click(&mut self, x: f32) {
        let pos = self.pos_at(x);
        if dragging() {
            self.move_to(pos, true);
            return;
        }
        match click_count() {
            2 if !self.mask => self.select_word_at(pos),
            3 if !self.mask => self.select_all(),
            _ => {
                self.cursor = pos;
                self.anchor = Some(pos);
            }
        }
    }

    pub fn render(&self, g: &mut Gfx, r: R, focused: bool, s: f32, blink_on: bool, face: Face, id: usize) {
        self.draw(g, r, focused, s, blink_on, face, id, true);
    }

    /// Text only (no box, no clipboard button): for terminal-style lines. `r` is the text area.
    pub fn render_bare(&self, g: &mut Gfx, r: R, focused: bool, s: f32, blink_on: bool, face: Face, id: usize) {
        self.draw(g, r, focused, s, blink_on, face, id, false);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(&self, g: &mut Gfx, r: R, focused: bool, s: f32, blink_on: bool, face: Face, id: usize, boxed: bool) {
        #[cfg(test)]
        let legacy = LEGACY_RINGS.with(|c| c.get());
        #[cfg(not(test))]
        let legacy = false;
        // an input looks focused (frame, caret, clipboard button) only when it owns the keyboard of a window that has the focus
        let focused = focused && (legacy || input_focus_allowed());
        if focused && boxed {
            note_focus_ring();
        }
        g.hit(r, Event::Focus(id), "field");
        crate::ctxmenu::note_field(crate::ctxmenu::FieldNote { rect: r, id, focused, has_selection: self.selection().is_some(), empty: self.value.is_empty(), secret: self.mask });
        let (size, pad) = if boxed { (17.0 * s, 12.0 * s) } else { (16.0 * s, 0.0) };
        if boxed {
            crate::fx::field_frame(g, r, s, focused, crate::fx::CYAN);
        }
        let lh = g.line_height(size, face);
        let ty = if boxed { r.y + (r.h - lh) / 2.0 } else { r.y };
        let chars: Vec<char> = if self.mask { vec!['•'; self.len()] } else { self.value.chars().collect() };
        if chars.is_empty() && !focused {
            g.text(r.x + pad, ty, size, DIM, &self.placeholder, face);
            *self.layout.borrow_mut() = FieldLayout { start: 0, text_x: pad, advances: vec![0.0] };
            return;
        }
        let icon_room = if focused && boxed { 30.0 * s } else { 0.0 };
        let max_w = (r.w - 2.0 * pad - 4.0 * s - icon_room).max(10.0);
        let cursor = self.cursor().min(chars.len());
        let width_of = |g: &mut Gfx, from: usize, to: usize| g.text_width(size, &chars[from..to].iter().collect::<String>(), face);
        // scroll just enough to keep the caret visible
        let mut start = 0;
        while start < cursor && width_of(g, start, cursor) > max_w {
            start += 1;
        }
        let mut end = start;
        let mut advances = vec![0.0];
        let mut acc = 0.0;
        while end < chars.len() {
            let w = g.text_width(size, &chars[end].to_string(), face);
            if acc + w > max_w {
                break;
            }
            acc += w;
            advances.push(acc);
            end += 1;
        }
        let x0 = r.x + pad;
        if let Some((s0, e0)) = self.selection() {
            let (from, to) = (s0.clamp(start, end), e0.clamp(start, end));
            if to > from {
                let (xa, xb) = (advances[from - start], advances[to - start]);
                g.rect(x0 + xa, ty, (xb - xa).max(2.0), lh, rgb(52, 84, 160));
            }
        }
        let vis: String = chars[start..end].iter().collect();
        g.text(x0, ty, size, TEXT, &vis, face);
        if focused && blink_on && self.selection().is_none() {
            note_caret();
            let cx = x0 + advances.get(cursor - start).copied().unwrap_or(acc);
            g.rect(cx, ty, 2.0 * s, lh, TEXT);
        }
        *self.layout.borrow_mut() = FieldLayout { start, text_x: pad, advances };
        if focused && boxed {
            // clipboard button: opens the clipboard history picker
            let (bx, by, bs) = (r.x + r.w - 28.0 * s, r.y + (r.h - 22.0 * s) / 2.0, 22.0 * s);
            let hot = g.hovered(R::new(bx, by, bs, bs));
            g.rrect(bx, by, bs, bs, 6.0 * s, if hot { rgb(60, 72, 110) } else { rgb(34, 42, 70) });
            g.icon(crate::gfx::Icon::Clipboard, bx + bs / 2.0, by + bs / 2.0, 14.0 * s, MUTED);
            g.hit(R::new(bx, by, bs, bs), Event::Key(Key::Ctrl('V')), "clipboard");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_field_editing_with_a_caret() {
        let mut f = TextField::new("name", false);
        assert!(f.handle(&Key::Char('a')) && f.handle(&Key::Char('é')));
        assert_eq!(f.value, "aé");
        assert!(f.handle(&Key::Backspace));
        assert_eq!(f.value, "a");
        assert!(!f.handle(&Key::Char('\n')) && !f.handle(&Key::Enter) && !f.handle(&Key::Up));
        // insert in the middle
        f.set("held");
        f.handle(&Key::Left);
        f.handle(&Key::Left);
        f.handle(&Key::Char('X'));
        assert_eq!(f.value, "heXld");
        assert_eq!(f.cursor(), 3);
        f.handle(&Key::Delete);
        assert_eq!(f.value, "heXd");
        f.handle(&Key::Home);
        assert!(!f.handle(&Key::Backspace), "nothing before the caret");
        f.handle(&Key::End);
        f.handle(&Key::Char('!'));
        assert_eq!(f.value, "heXd!");
        // multibyte-safe
        f.set("añb");
        f.handle(&Key::Left);
        f.handle(&Key::Backspace);
        assert_eq!(f.value, "ab");
        f.max = 2;
        f.handle(&Key::Char('c'));
        assert_eq!(f.value, "ab", "max length respected");
        assert_eq!(f.take(), "ab");
        assert!(!f.handle(&Key::Backspace));
    }

    #[test]
    fn select_all_replaces_and_copy_cut_use_the_clipboard() {
        clipboard::clear();
        let mut f = TextField::new("", false);
        f.set("hello world");
        f.handle(&Key::Ctrl('a'));
        assert!(f.is_all_selected());
        f.handle(&Key::Char('x'));
        assert_eq!(f.value, "x", "typing over a selection replaces it");
        f.set("copy me");
        f.handle(&Key::Ctrl('c'));
        assert_eq!(clipboard::latest().as_deref(), Some("copy me"));
        assert_eq!(clipboard::take_host_copy().as_deref(), Some("copy me"));
        assert_eq!(f.value, "copy me", "copy leaves the text");
        f.handle(&Key::Ctrl('x'));
        assert_eq!(f.value, "");
        assert_eq!(clipboard::latest().as_deref(), Some("copy me"));
        f.set("abc");
        f.handle(&Key::Ctrl('a'));
        f.handle(&Key::Backspace);
        assert_eq!(f.value, "");
    }

    #[test]
    fn secret_fields_refuse_to_copy() {
        clipboard::clear();
        let mut f = TextField::new("", true);
        f.set("sk-secret");
        f.handle(&Key::Ctrl('c'));
        f.handle(&Key::Ctrl('x'));
        assert_eq!(f.value, "sk-secret");
        assert!(clipboard::latest().is_none());
        assert!(clipboard::take_toast().unwrap().contains("can't be copied"));
        // but pasting into one works
        let mut g = TextField::new("", true);
        assert!(g.paste("token"));
        assert_eq!(g.value, "token");
    }

    #[test]
    fn paste_inserts_at_the_caret_and_sanitises() {
        let mut f = TextField::new("", false);
        f.set("ab");
        f.handle(&Key::Left);
        assert!(f.paste("XY\nZ  \n"));
        assert_eq!(f.value, "aXY Zb", "newlines flattened, trailing whitespace trimmed");
        f.handle(&Key::Ctrl('a'));
        f.paste("new");
        assert_eq!(f.value, "new", "pasting over a selection replaces it");
        f.max = 5;
        f.paste("123456789");
        assert_eq!(f.value, "new12");
        assert!(!f.paste("   "), "whitespace-only paste does nothing");
        assert!(!f.paste(""));
    }

    #[test]
    fn the_clipboard_keeps_ten_by_default_and_the_limit_can_change() {
        clipboard::clear();
        for i in 0..30 {
            clipboard::remember(&format!("copy {i}"));
        }
        assert_eq!((clipboard::limit(), clipboard::history().len()), (10, 10));
        assert_eq!(clipboard::latest().as_deref(), Some("copy 29"), "the newest is what a plain paste uses");
        clipboard::set_limit(3);
        assert_eq!(clipboard::history(), vec!["copy 29", "copy 28", "copy 27"], "lowering the limit drops the oldest");
        clipboard::set_limit(500);
        assert_eq!(clipboard::limit(), clipboard::MAX_LIMIT);
        clipboard::set_limit(0);
        assert_eq!(clipboard::limit(), 1);
        clipboard::clear();
        assert_eq!(clipboard::limit(), clipboard::DEFAULT_LIMIT, "clearing the state restores the default");
    }

    #[test]
    fn clipboard_history_dedups_caps_and_hides_tokens() {
        clipboard::clear();
        clipboard::copy("one");
        clipboard::copy("two");
        clipboard::copy("one");
        assert_eq!(clipboard::history(), vec!["one", "two"]);
        for i in 0..40 {
            clipboard::remember(&format!("item {i}"));
        }
        assert_eq!(clipboard::history().len(), clipboard::DEFAULT_LIMIT);
        assert_eq!(clipboard::latest().as_deref(), Some("item 39"));
        assert!(clipboard::take_host_copy().is_some(), "copy() queued for the host, remember() did not");
        assert_eq!(clipboard::take_host_copy(), None);
        let p = clipboard::preview("sk-live-abcdefghijklmnopqrstuvwxyz");
        assert!(p.starts_with("sk-l…") && !p.contains("abcdef"), "{p}");
        assert_eq!(clipboard::preview("two words here"), "two words here");
        assert!(clipboard::preview(&"word ".repeat(40)).contains("characters"));
        assert_eq!(clipboard::preview("line1\nline2"), "line1 line2");
        clipboard::copy("");
        assert_eq!(clipboard::latest().as_deref(), Some("item 39"), "empty copies are ignored");
    }

    #[test]
    fn clicking_a_field_places_the_caret() {
        let mut g = Gfx::new(400, 100);
        let mut f = TextField::new("", false);
        f.set("abcdefgh");
        f.render(&mut g, R::new(10.0, 10.0, 300.0, 40.0), true, 1.0, false, Face::Sans, 0);
        let char_w = g.text_width(17.0, "a", Face::Sans);
        f.click(12.0 + char_w * 3.0 + 1.0); // just right of the 3rd character
        assert_eq!(f.cursor(), 3);
        f.click(0.0);
        assert_eq!(f.cursor(), 0, "clicking left of the text goes to the start");
        f.click(290.0);
        assert_eq!(f.cursor(), 8, "clicking past the end goes to the end");
        // the render registered a hit for the field and one for the clipboard button
        assert!(g.hits.iter().any(|h| h.label == "field" && h.ev == Event::Focus(0)));
        assert!(g.hits.iter().any(|h| h.label == "clipboard" && h.ev == Event::Key(Key::Ctrl('V'))));
    }

    #[test]
    fn long_text_scrolls_to_keep_the_caret_visible() {
        let mut g = Gfx::new(300, 80);
        let mut f = TextField::new("", false);
        f.set(&"0123456789".repeat(8));
        f.render(&mut g, R::new(10.0, 10.0, 200.0, 40.0), true, 1.0, true, Face::Sans, 0);
        let start = f.layout.borrow().start;
        assert!(start > 0, "view scrolled right");
        f.handle(&Key::Home);
        f.render(&mut g, R::new(10.0, 10.0, 200.0, 40.0), true, 1.0, true, Face::Sans, 0);
        assert_eq!(f.layout.borrow().start, 0, "and back when the caret moves home");
        // clicking in the scrolled view lands on the right character
        f.handle(&Key::End);
        f.render(&mut g, R::new(10.0, 10.0, 200.0, 40.0), true, 1.0, true, Face::Sans, 0);
        f.click(12.0);
        assert!(f.cursor() >= f.layout.borrow().start);
    }

    #[test]
    fn rect_hit_testing() {
        let r = R::new(10.0, 10.0, 20.0, 20.0);
        assert!(r.contains(10, 10) && r.contains(29, 29));
        assert!(!r.contains(30, 20) && !r.contains(9, 15));
        assert_eq!(r.inset(5.0), R::new(15.0, 15.0, 10.0, 10.0));
    }

    #[test]
    fn field_renders_masked_and_long_values() {
        let mut g = Gfx::new(300, 60);
        let mut f = TextField::new("pw", true);
        for c in "secret-secret-secret-secret-secret-secret".chars() {
            f.handle(&Key::Char(c));
        }
        g.clear(rgb(0, 0, 0));
        f.render(&mut g, R::new(10.0, 10.0, 280.0, 40.0), true, 1.0, true, Face::Sans, 0);
    }

    fn typed(text: &str) -> TextField {
        let mut f = TextField::new("", false);
        f.set(text);
        f
    }

    #[test]
    fn shift_arrows_select_typing_replaces_and_plain_arrows_collapse() {
        let mut f = typed("hello world");
        let sh = |nav, ctrl| Key::Nav { nav, shift: true, ctrl };
        f.handle(&Key::Home);
        f.handle(&sh(Nav::Right, false));
        f.handle(&sh(Nav::Right, false));
        assert_eq!(f.selection(), Some((0, 2)));
        assert_eq!(f.selected_text().as_deref(), Some("he"));
        f.handle(&sh(Nav::Right, true)); // to the end of the word
        assert_eq!(f.selected_text().as_deref(), Some("hello"));
        f.handle(&Key::Right);
        assert_eq!((f.selection(), f.cursor()), (None, 5), "a plain arrow collapses to the end of the selection");
        f.handle(&sh(Nav::Left, true));
        assert_eq!(f.selected_text().as_deref(), Some("hello"));
        assert!(f.handle(&Key::Char('J')));
        assert_eq!(f.value, "J world", "typing replaces the selection");
        f.handle(&sh(Nav::End, false));
        assert_eq!(f.selected_text().as_deref(), Some(" world"), "from where the caret was to the end");
        f.handle(&Key::Ctrl('a'));
        assert!(f.is_all_selected());
        assert!(f.handle(&Key::Backspace));
        assert_eq!(f.value, "");
    }

    #[test]
    fn copy_and_cut_use_the_selection_or_the_whole_line() {
        clipboard::clear();
        let mut f = typed("one two three");
        f.handle(&Key::Home);
        for _ in 0..3 {
            f.handle(&Key::Nav { nav: Nav::Right, shift: true, ctrl: false });
        }
        f.handle(&Key::Ctrl('c'));
        assert_eq!(clipboard::latest().as_deref(), Some("one"));
        assert_eq!(f.value, "one two three");
        assert!(f.handle(&Key::Ctrl('x')));
        assert_eq!((f.value.as_str(), clipboard::latest().as_deref()), (" two three", Some("one")));
        f.handle(&Key::Left);
        f.handle(&Key::Ctrl('c'));
        assert_eq!(clipboard::latest().as_deref(), Some(" two three"), "no selection copies the whole line");
        let mut secret = TextField::new("", true);
        secret.set("hunter2");
        secret.select_all();
        secret.handle(&Key::Ctrl('c'));
        assert_ne!(clipboard::latest().as_deref(), Some("hunter2"), "secrets are not copied");
    }

    #[test]
    fn the_mouse_places_the_caret_drags_a_selection_and_double_click_picks_a_word() {
        let mut g = Gfx::new(400, 60);
        let mut f = typed("alpha beta gamma");
        let r = R::new(10.0, 10.0, 300.0, 34.0);
        f.render(&mut g, r, true, 1.0, false, Face::Sans, 0);
        let w = |g: &mut Gfx, t: &str| g.text_width(17.0, t, Face::Sans) + 12.0;
        let x_beta = w(&mut g, "alpha ") + 6.0; // a little way into "beta"
        let x_end = w(&mut g, "alpha beta");
        set_click_count(1);
        f.click(x_beta);
        assert_eq!((f.cursor(), f.selection()), (7, None), "a press puts the caret between letters");
        set_dragging(true);
        f.click(x_end);
        set_dragging(false);
        assert_eq!(f.selected_text().as_deref(), Some("eta"), "dragging selects from the press to the pointer");
        set_click_count(2);
        f.click(x_beta);
        set_click_count(1);
        assert_eq!(f.selected_text().as_deref(), Some("beta"), "a double click selects the word");
        set_click_count(3);
        f.click(x_beta);
        set_click_count(1);
        assert!(f.is_all_selected(), "a triple click selects everything");
        // dragging back past the start keeps the far end fixed
        set_click_count(1);
        f.click(x_beta);
        set_dragging(true);
        f.click(-50.0);
        set_dragging(false);
        assert_eq!(f.selected_text().as_deref(), Some("alpha b"));
    }
}
