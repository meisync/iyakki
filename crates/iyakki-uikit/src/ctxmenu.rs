//! The context menu: a small glass popup opened by a right click at the pointer.
//!
//! The shell owns it (so every app and every front end gets it): it asks the app for its own entries (see
//! `App::context_items`), adds Cut / Copy / Paste / Select all when the pointer is over a text field, and always ends with
//! "Clipboard history…". This file holds the menu itself (entries, layout, keys, drawing) and the small registry through
//! which text fields tell the shell, while they draw, where they are and what they hold.

use std::cell::RefCell;

use crate::fx;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Key, R, DIM, MUTED, TEXT};

/// Ids of the entries the shell knows how to carry out itself.
pub const CUT: &str = "cut";
pub const COPY: &str = "copy";
pub const PASTE: &str = "paste";
pub const SELECT_ALL: &str = "selall";
pub const HISTORY: &str = "history";

/// One line of the menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// What the shell (or the app) gets back when it is chosen. Empty for a separator.
    pub id: String,
    pub label: String,
    /// Shown at the right, like `Ctrl+C`.
    pub shortcut: String,
    pub enabled: bool,
    /// Only usable while the clipboard holds text (checked each time the menu is drawn or used).
    pub needs_clip: bool,
}

impl Item {
    pub fn new(id: &str, label: &str, shortcut: &str) -> Item {
        Item { id: id.into(), label: label.into(), shortcut: shortcut.into(), enabled: true, needs_clip: false }
    }
    pub fn separator() -> Item {
        Item { id: String::new(), label: String::new(), shortcut: String::new(), enabled: false, needs_clip: false }
    }
    pub fn enabled(mut self, on: bool) -> Item {
        self.enabled = on;
        self
    }
    pub fn needs_clip(mut self) -> Item {
        self.needs_clip = true;
        self
    }
    pub fn is_separator(&self) -> bool {
        self.id.is_empty()
    }
}

/// What a key or click on the open menu did.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Still open.
    Stay,
    Close,
    /// The entry with this id was chosen (the menu is closed).
    Pick(String),
}

#[derive(Clone, Debug)]
pub struct Menu {
    pub items: Vec<Item>,
    /// The highlighted entry.
    pub sel: usize,
    /// Where the pointer was when it opened.
    pub at: (i32, i32),
    pub rect: R,
    rects: Vec<R>,
    clip_text: bool,
}

const ITEM_H: f32 = 30.0;
const SEP_H: f32 = 9.0;
const PAD: f32 = 6.0;
/// An entry's height when a finger asked for the menu: big enough to tap.
const TOUCH_ITEM_H: f32 = 44.0;

impl Menu {
    /// Opens a menu with its corner at the pointer, moved back inside the `screen` (w, h) when it would stick out.
    pub fn open(items: Vec<Item>, at: (i32, i32), screen: (u32, u32), s: f32, clip_text: bool) -> Menu {
        Menu::open_sized(items, at, screen, s, clip_text, ITEM_H)
    }

    /// [`Menu::open`] with entries at least 44 px tall, for a menu a finger opened.
    pub fn open_touch(items: Vec<Item>, at: (i32, i32), screen: (u32, u32), s: f32, clip_text: bool) -> Menu {
        Menu::open_sized(items, at, screen, s, clip_text, TOUCH_ITEM_H)
    }

    fn open_sized(items: Vec<Item>, at: (i32, i32), screen: (u32, u32), s: f32, clip_text: bool, item_h: f32) -> Menu {
        let longest = items.iter().map(|i| i.label.chars().count()).max().unwrap_or(0) as f32;
        let sc = items.iter().map(|i| i.shortcut.chars().count()).max().unwrap_or(0) as f32;
        let w = ((longest * 8.4 + sc * 7.6 + 70.0) * s).max(200.0 * s).min(screen.0 as f32);
        let h: f32 = items.iter().map(|i| if i.is_separator() { SEP_H * s } else { item_h * s }).sum::<f32>() + 2.0 * PAD * s;
        let x = (at.0 as f32).min(screen.0 as f32 - w).max(0.0);
        let y = (at.1 as f32).min(screen.1 as f32 - h).max(0.0);
        let rect = R::new(x, y, w, h);
        let mut rects = Vec::new();
        let mut cy = y + PAD * s;
        for i in &items {
            let ih = if i.is_separator() { SEP_H * s } else { item_h * s };
            rects.push(R::new(x + PAD * s, cy, w - 2.0 * PAD * s, ih));
            cy += ih;
        }
        let mut m = Menu { items, sel: 0, at, rect, rects, clip_text };
        m.sel = m.step(usize::MAX, true);
        m
    }

    /// The clipboard changed (text arrived from the system): `Paste` may become usable.
    pub fn set_clip_text(&mut self, on: bool) {
        self.clip_text = on;
    }

    pub fn is_enabled(&self, i: usize) -> bool {
        self.items.get(i).is_some_and(|it| it.enabled && (!it.needs_clip || self.clip_text))
    }

    pub fn item_rect(&self, i: usize) -> Option<R> {
        self.rects.get(i).copied()
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.items.iter().position(|i| i.id == id)
    }

    /// The next usable entry after `from` (or before it), wrapping round. `usize::MAX` means "from the edge".
    fn step(&self, from: usize, down: bool) -> usize {
        let n = self.items.len();
        if n == 0 {
            return 0;
        }
        let mut i = from;
        for _ in 0..n {
            i = if down { if i == usize::MAX || i + 1 >= n { 0 } else { i + 1 } } else if i == usize::MAX || i == 0 { n - 1 } else { i - 1 };
            if self.is_enabled(i) {
                return i;
            }
        }
        from.min(n - 1)
    }

    fn pick(&mut self) -> Outcome {
        if self.is_enabled(self.sel) {
            Outcome::Pick(self.items[self.sel].id.clone())
        } else {
            Outcome::Stay
        }
    }

    pub fn key(&mut self, k: &Key) -> Outcome {
        match k {
            Key::Esc => Outcome::Close,
            Key::Down | Key::Tab => {
                self.sel = self.step(self.sel, true);
                Outcome::Stay
            }
            Key::Up | Key::BackTab => {
                self.sel = self.step(self.sel, false);
                Outcome::Stay
            }
            Key::Home => {
                self.sel = self.step(usize::MAX, true);
                Outcome::Stay
            }
            Key::End => {
                self.sel = self.step(usize::MAX, false);
                Outcome::Stay
            }
            Key::Enter => self.pick(),
            _ => Outcome::Close,
        }
    }

    /// The pointer moved: the entry under it is highlighted.
    pub fn hover(&mut self, x: i32, y: i32) {
        if let Some(i) = self.rects.iter().position(|r| r.contains(x, y)) {
            if self.is_enabled(i) {
                self.sel = i;
            }
        }
    }

    /// A left click: on a usable entry it chooses it, on a disabled one nothing happens, anywhere else the menu closes.
    pub fn click(&mut self, x: i32, y: i32) -> Outcome {
        if !self.rect.contains(x, y) {
            return Outcome::Close;
        }
        match self.rects.iter().position(|r| r.contains(x, y)) {
            Some(i) if self.is_enabled(i) => Outcome::Pick(self.items[i].id.clone()),
            _ => Outcome::Stay,
        }
    }

    pub fn draw(&self, g: &mut Gfx, s: f32, accent: [u8; 4]) {
        fx::card(g, self.rect, s, accent, 0.25);
        for (i, it) in self.items.iter().enumerate() {
            let r = self.rects[i];
            if it.is_separator() {
                g.rect(r.x + 8.0 * s, r.y + r.h / 2.0, r.w - 16.0 * s, 1.0, [255, 255, 255, 28]);
                continue;
            }
            let on = self.is_enabled(i);
            if on && i == self.sel {
                g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, fx::over(crate::ui::CARD_SEL, fx::with_alpha(accent, 50)));
                g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, 1.0, fx::with_alpha(accent, 150));
            }
            let size = 14.5 * s;
            let ty = r.y + (r.h - g.line_height(size, Face::Sans)) / 2.0;
            g.text(r.x + 12.0 * s, ty, size, if on { TEXT } else { DIM }, &it.label, Face::Sans);
            if !it.shortcut.is_empty() {
                let sz = 12.5 * s;
                let w = g.text_width(sz, &it.shortcut, Face::Sans);
                g.text(r.x + r.w - 12.0 * s - w, r.y + (r.h - g.line_height(sz, Face::Sans)) / 2.0, sz, if on { MUTED } else { rgb(96, 104, 134) }, &it.shortcut, Face::Sans);
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------ text field registry

/// What a text field says about itself each time it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldNote {
    pub rect: R,
    /// The id its clicks carry (`Event::FocusAt(id, x)`).
    pub id: usize,
    pub focused: bool,
    pub has_selection: bool,
    pub empty: bool,
    /// A password: it cannot be copied.
    pub secret: bool,
}

thread_local! {
    static FIELDS: RefCell<Vec<FieldNote>> = const { RefCell::new(Vec::new()) };
    static UNDER: RefCell<Vec<crate::ui::Hit>> = const { RefCell::new(Vec::new()) };
}

/// The click regions under the pointer when the menu was asked for, topmost first. An app reads them from
/// `App::context_items` to learn what the pointer is over (the browser: which link).
pub fn under() -> Vec<crate::ui::Hit> {
    UNDER.with(|u| u.borrow().clone())
}

pub fn set_under(h: Vec<crate::ui::Hit>) {
    UNDER.with(|u| *u.borrow_mut() = h);
}

/// Called by a text field while it draws.
pub fn note_field(n: FieldNote) {
    FIELDS.with(|f| f.borrow_mut().push(n));
}

/// Forget the fields of the last frame (the shell calls this before it draws).
pub fn start_frame() {
    FIELDS.with(|f| f.borrow_mut().clear());
}

/// The fields drawn this frame, bottom to top.
pub fn fields() -> Vec<FieldNote> {
    FIELDS.with(|f| f.borrow().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<Item> {
        vec![Item::new(CUT, "Cut", "Ctrl+X").enabled(false), Item::new(COPY, "Copy", "Ctrl+C"), Item::new(PASTE, "Paste", "Ctrl+V").needs_clip(), Item::separator(), Item::new(HISTORY, "Clipboard history…", "Ctrl+Shift+V")]
    }

    #[test]
    fn it_opens_at_the_pointer_and_stays_inside_the_screen() {
        let m = Menu::open(items(), (100, 80), (1280, 720), 1.0, true);
        assert_eq!((m.rect.x, m.rect.y), (100.0, 80.0));
        let m = Menu::open(items(), (1270, 715), (1280, 720), 1.0, true);
        assert!(m.rect.x + m.rect.w <= 1280.0 && m.rect.y + m.rect.h <= 720.0, "{:?}", m.rect);
        assert!(m.rect.x >= 0.0 && m.rect.y >= 0.0);
        let tiny = Menu::open(items(), (5, 5), (200, 100), 1.0, true);
        assert!(tiny.rect.x >= 0.0 && tiny.rect.y >= 0.0);
    }

    #[test]
    fn a_menu_opened_by_a_finger_has_entries_44_pixels_tall_and_a_tap_chooses() {
        let m = Menu::open_touch(items(), (100, 80), (1280, 720), 1.0, true);
        assert_eq!(m.item_rect(1).unwrap().h, 44.0);
        assert!(m.item_rect(1).unwrap().h > Menu::open(items(), (100, 80), (1280, 720), 1.0, true).item_rect(1).unwrap().h);
        let r = m.item_rect(1).unwrap();
        let mut m = m;
        assert_eq!(m.click((r.x + 5.0) as i32, (r.y + 40.0) as i32), Outcome::Pick(COPY.into()));
    }

    #[test]
    fn keys_skip_disabled_entries_and_separators_and_enter_chooses() {
        let mut m = Menu::open(items(), (10, 10), (800, 600), 1.0, true);
        assert_eq!(m.sel, 1, "the first usable entry is highlighted (Cut is off)");
        m.key(&Key::Down);
        assert_eq!(m.sel, 2);
        m.key(&Key::Down);
        assert_eq!(m.sel, 4, "the separator is skipped");
        m.key(&Key::Down);
        assert_eq!(m.sel, 1, "wraps round, skipping Cut");
        m.key(&Key::Up);
        assert_eq!(m.sel, 4);
        assert_eq!(m.key(&Key::Enter), Outcome::Pick(HISTORY.into()));
        assert_eq!(m.key(&Key::Esc), Outcome::Close);
        assert_eq!(m.key(&Key::Char('x')), Outcome::Close);
    }

    #[test]
    fn paste_needs_text_on_the_clipboard_and_clicks_choose_or_close() {
        let mut m = Menu::open(items(), (10, 10), (800, 600), 1.0, false);
        let paste = m.index_of(PASTE).unwrap();
        assert!(!m.is_enabled(paste));
        let r = m.item_rect(paste).unwrap();
        assert_eq!(m.click((r.x + 5.0) as i32, (r.y + 5.0) as i32), Outcome::Stay, "a disabled entry does nothing");
        m.set_clip_text(true);
        assert_eq!(m.click((r.x + 5.0) as i32, (r.y + 5.0) as i32), Outcome::Pick(PASTE.into()));
        let cut = m.item_rect(0).unwrap();
        assert_eq!(m.click((cut.x + 5.0) as i32, (cut.y + 5.0) as i32), Outcome::Stay);
        assert_eq!(m.click(790, 590), Outcome::Close, "a click elsewhere closes it");
    }

    #[test]
    fn it_draws_inside_its_rectangle() {
        let m = Menu::open(items(), (50, 50), (400, 300), 1.0, true);
        let mut g = Gfx::new(400, 300);
        g.clear(rgb(9, 11, 20));
        m.draw(&mut g, 1.0, rgb(79, 140, 255));
        assert!(m.rect.h > 100.0);
    }
}
