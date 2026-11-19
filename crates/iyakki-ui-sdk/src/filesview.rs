//! `FilesView`: the person's home folder as a component an SDK app places in its layout (docs/development/UI-SDK.md, "Embedding the file
//! browser"). The app keeps a `FilesView` in its state, gives it a [`FilesPort`] (the core's) and a base action number, and a rectangle to
//! draw in. The view owns what is on screen: the path, the list, the selection, the text preview. It draws through [`Canvas`], so it works in
//! the shell, in a browser or client (display list) and as a WASM tile, with the colours and size class of the host.
//!
//! What the app does, in four lines:
//! ```text
//! event(Tick):   files.tick()                           (opens the home folder once)
//! event(Action): if files.owns(a) { files.action(a) }   event(Key): if files.key(k) { return }   event(Wheel): files.wheel(n)
//! view:          files.draw(c, th, &metrics, rect)      what the person chose: files.take_opened() -> Option<(path, text)>
//! ```
//! A tap or Enter on a folder goes into it; on a text file it shows the text beside the list (on a narrow pane instead of it) and hands
//! the app the path and the text once through [`FilesView::take_opened`]. Up, Backspace and Left go to the folder above.
//!
//! Changing files (version 2): the buttons "New folder", "Rename" and "Delete" and [`FilesView::save`] (the app's Save) ask the core for a
//! change; **nothing changes until the person answers the card** ("Allow" / "Deny", keys `y` and `n`). New folder and Rename first ask for the
//! name in a field. Delete moves to the trash. After an answer the view shows what happened and reads the folder again.

use crate::canvas::{fit, Canvas, Hit, Role, TextStyle};
use crate::event::UiKey;
use crate::files::{join, parent, Ask, Change, DirListing, FilesError, FilesPort, MAX_PREVIEW};
use crate::geom::Rect;
use crate::size::{Metrics, TypeRole};
use crate::theme::Theme;
use crate::tokens::{Radius, Space};
use crate::widgets;

/// Layout proportions: the widest share of a row a button may take, the width from which the preview sits beside the list, and the list's share then.
const SHARE: f32 = 1.0 / 3.0;
/// The change buttons share the row in thirds, a button beside the name field is a quarter of the width at most, the card's two buttons a half.
const THIRD: f32 = 1.0 / 3.0;
const QUARTER: f32 = 0.25;
const HALF: f32 = 0.5;
const WIDE_FROM: f32 = 560.0;
/// The width of the OK and Cancel buttons beside the name field, in text sizes.
const MIN_EM_OK: f32 = 3.5;
const LIST_SHARE: f32 = 0.45;

const A_UP: u32 = 0;
const A_REFRESH: u32 = 1;
const A_CLOSE: u32 = 2;
const A_NEWDIR: u32 = 3;
const A_RENAME: u32 = 4;
const A_DELETE: u32 = 5;
const A_ALLOW: u32 = 6;
const A_DENY: u32 = 7;
const A_NAME: u32 = 8;
const A_NAME_OK: u32 = 9;
const A_NAME_CANCEL: u32 = 10;
const A_ROW0: u32 = 12;
/// The most rows drawn at once (a row has the action `base + 8 + i`).
pub const MAX_ROWS: u32 = 64;
/// How many action numbers a files view uses, from the base the app gave it.
pub const ACTIONS: u32 = A_ROW0 + MAX_ROWS;
/// The preview shows this many lines at most.
const MAX_PREVIEW_LINES: usize = 200;

pub struct FilesView {
    port: Box<dyn FilesPort>,
    base: u32,
    /// The folder shown, relative to the home (`""` is the home).
    pub dir: String,
    listing: Option<DirListing>,
    pub selected: usize,
    scroll: usize,
    /// The text file shown beside the list: (path, text).
    preview: Option<(String, String)>,
    error: Option<String>,
    opened: Option<(String, String)>,
    visible_rows: usize,
    /// The name being asked for (a new folder, or the new name of the selected entry): what it is for, and what is typed.
    name: Option<(NameFor, String)>,
    /// The question of the core that waits for the person.
    ask: Option<Ask>,
    /// What the last answered change did (or why it did not).
    note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum NameFor {
    NewFolder,
    Rename(String),
}

impl FilesView {
    pub fn new(port: Box<dyn FilesPort>, action_base: u32) -> FilesView {
        FilesView { port, base: action_base, dir: String::new(), listing: None, selected: 0, scroll: 0, preview: None, error: None, opened: None, visible_rows: 8, name: None, ask: None, note: None }
    }

    pub fn owns(&self, action: u32) -> bool {
        (self.base..self.base + ACTIONS).contains(&action)
    }

    /// Open the home folder the first time (and nothing after).
    pub fn tick(&mut self) {
        if self.listing.is_none() && self.error.is_none() {
            self.load(String::new());
        }
    }

    fn load(&mut self, dir: String) {
        match self.port.list(&dir) {
            Ok(l) => {
                self.dir = dir;
                self.listing = Some(l);
                self.selected = 0;
                self.scroll = 0;
                self.preview = None;
                self.error = None;
            }
            Err(e) => self.error = Some(e.sentence()),
        }
    }

    /// The names in the folder shown, in order (folders end with `/`), for tests and screen readers.
    pub fn names(&self) -> Vec<String> {
        self.listing.as_ref().map(|l| l.entries.iter().map(|e| if e.is_dir { format!("{}/", e.name) } else { e.name.clone() }).collect()).unwrap_or_default()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The text being previewed with its path.
    pub fn preview(&self) -> Option<&(String, String)> {
        self.preview.as_ref()
    }

    /// Close the text preview (what Esc does), for an app that closes the file itself, for example when an assistant tool does. The folder
    /// and the selection stay.
    pub fn close(&mut self) {
        self.preview = None;
    }

    /// The path of the selected entry.
    pub fn selected_path(&self) -> Option<String> {
        let l = self.listing.as_ref()?;
        l.entries.get(self.selected).map(|e| join(&self.dir, &e.name))
    }

    /// The file the person chose since the last call (its path and text), once.
    pub fn take_opened(&mut self) -> Option<(String, String)> {
        self.opened.take()
    }

    pub fn pending_question(&self) -> Option<&Ask> {
        self.ask.as_ref()
    }

    /// What the last change did, as a sentence.
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    pub fn wants_keyboard(&self) -> bool {
        self.name.is_some()
    }

    fn ask_for(&mut self, c: Change) {
        self.note = None;
        match self.port.change(&c) {
            Ok(a) => {
                self.error = None;
                self.ask = Some(a);
            }
            Err(e) => self.error = Some(e.sentence()),
        }
    }

    /// The app saves a text file (the editor's Save): the core asks the person before the file is written. One question at a time.
    pub fn save(&mut self, path: &str, text: &str) -> bool {
        if self.ask.is_some() {
            self.error = Some("answer the question that is waiting first".into());
            return false;
        }
        self.ask_for(Change::Save { path: path.to_string(), text: text.to_string() });
        self.ask.is_some()
    }

    /// Ask for the name of a new folder in the folder shown.
    pub fn begin_new_folder(&mut self) {
        if self.ask.is_none() {
            self.name = Some((NameFor::NewFolder, String::new()));
        }
    }

    /// Ask for the new name of the selected entry (its own name to start from).
    pub fn begin_rename(&mut self) {
        if self.ask.is_some() {
            return;
        }
        let Some(l) = self.listing.as_ref() else { return };
        if let Some(e) = l.entries.get(self.selected) {
            self.name = Some((NameFor::Rename(join(&self.dir, &e.name)), e.name.clone()));
        }
    }

    /// Ask to move the selected entry to the trash.
    pub fn delete_selected(&mut self) {
        if self.ask.is_some() {
            return;
        }
        if let Some(p) = self.selected_path() {
            self.ask_for(Change::Trash { path: p });
        }
    }

    fn submit_name(&mut self) {
        let Some((what, typed)) = self.name.take() else { return };
        let t = typed.trim().to_string();
        if t.is_empty() {
            return;
        }
        let c = match what {
            NameFor::NewFolder => Change::MakeDir { dir: self.dir.clone(), name: t },
            NameFor::Rename(path) => Change::Rename { path, new_name: t },
        };
        self.ask_for(c);
    }

    /// The person's answer to the waiting question.
    pub fn decide(&mut self, approve: bool) {
        let Some(a) = self.ask.take() else { return };
        match self.port.decide(&a.call, approve) {
            Ok(t) => {
                self.note = Some(t);
                self.error = None;
                if approve {
                    // the folder is read again (a trashed or renamed entry is gone from the list, a new one is there)
                    let (d, keep) = (self.dir.clone(), self.selected);
                    self.load(d);
                    self.select(keep);
                }
            }
            // the person has not answered where they were asked: the question stays and the view says so (Allow again to look again)
            Err(FilesError::Waiting(w)) if approve => {
                self.ask = Some(a);
                self.error = None;
                self.note = Some(w);
            }
            Err(e) => self.error = Some(e.sentence()),
        }
    }

    pub fn up(&mut self) {
        if !self.dir.is_empty() {
            let p = parent(&self.dir);
            let came_from = self.dir.rsplit('/').next().unwrap_or("").to_string();
            self.load(p);
            if let Some(i) = self.listing.as_ref().and_then(|l| l.entries.iter().position(|e| e.name == came_from)) {
                self.select(i);
            }
        } else if self.preview.is_some() {
            self.preview = None;
        }
    }

    /// Go into the selected folder, or show the selected file.
    pub fn open_selected(&mut self) {
        let Some(l) = self.listing.as_ref() else { return };
        let Some(e) = l.entries.get(self.selected).cloned() else { return };
        let path = join(&self.dir, &e.name);
        if e.is_dir {
            self.load(path);
        } else {
            match self.port.read_text(&path, MAX_PREVIEW) {
                Ok(t) => {
                    self.error = None;
                    self.opened = Some((path.clone(), t.clone()));
                    self.preview = Some((path, t));
                }
                Err(err) => {
                    self.error = Some(err.sentence());
                    self.preview = None;
                }
            }
        }
    }

    fn select(&mut self, i: usize) {
        let n = self.listing.as_ref().map_or(0, |l| l.entries.len());
        self.selected = i.min(n.saturating_sub(1));
        let rows = self.visible_rows.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected + 1 - rows;
        }
    }

    pub fn action(&mut self, a: u32) -> bool {
        if !self.owns(a) {
            return false;
        }
        match a - self.base {
            A_UP => self.up(),
            A_REFRESH => {
                let d = self.dir.clone();
                self.load(d);
            }
            A_CLOSE => self.preview = None,
            A_NEWDIR => self.begin_new_folder(),
            A_RENAME => self.begin_rename(),
            A_DELETE => self.delete_selected(),
            A_ALLOW => self.decide(true),
            A_DENY => self.decide(false),
            A_NAME => {}
            A_NAME_OK => self.submit_name(),
            A_NAME_CANCEL => self.name = None,
            r if r >= A_ROW0 => {
                self.select(self.scroll + (r - A_ROW0) as usize);
                self.open_selected();
            }
            _ => {}
        }
        true
    }

    /// Keys for the list: arrows move, Enter or Right opens, Backspace or Left goes up, Esc closes the text. Returns whether the key was used.
    pub fn key(&mut self, k: UiKey) -> bool {
        if self.ask.is_some() {
            return match k {
                UiKey::Char('y') | UiKey::Char('Y') => {
                    self.decide(true);
                    true
                }
                UiKey::Char('n') | UiKey::Char('N') | UiKey::Esc => {
                    self.decide(false);
                    true
                }
                _ => false,
            };
        }
        if self.name.is_some() {
            match k {
                UiKey::Char(c) if !c.is_control() => {
                    if let Some((_, t)) = self.name.as_mut() {
                        if t.chars().count() < 120 {
                            t.push(c);
                        }
                    }
                }
                UiKey::Backspace => {
                    if let Some((_, t)) = self.name.as_mut() {
                        t.pop();
                    }
                }
                UiKey::Enter => self.submit_name(),
                UiKey::Esc => self.name = None,
                _ => return false,
            }
            return true;
        }
        match k {
            UiKey::Up => self.select(self.selected.saturating_sub(1)),
            UiKey::Down => self.select(self.selected + 1),
            UiKey::Enter | UiKey::Right => self.open_selected(),
            UiKey::Backspace | UiKey::Left => self.up(),
            UiKey::Esc if self.preview.is_some() => self.preview = None,
            _ => return false,
        }
        true
    }

    /// Typed or pasted text for the name field.
    pub fn text(&mut self, t: &str) -> bool {
        let Some((_, name)) = self.name.as_mut() else { return false };
        for c in t.chars().filter(|c| !c.is_control()) {
            if name.chars().count() < 120 {
                name.push(c);
            }
        }
        true
    }

    /// The wheel scrolls the list (positive: towards the top).
    pub fn wheel(&mut self, n: i32) {
        let len = self.listing.as_ref().map_or(0, |l| l.entries.len());
        let max = len.saturating_sub(self.visible_rows.max(1));
        self.scroll = (self.scroll as i64 - n as i64).clamp(0, max as i64) as usize;
        let rows = self.visible_rows.max(1);
        self.selected = self.selected.clamp(self.scroll, (self.scroll + rows).saturating_sub(1).max(self.scroll));
    }

    pub fn draw(&mut self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, area: Rect) {
        let t = m.min_target();
        widgets::panel(c, th, m, area);
        let (xxs, xs, sm, md) = (m.gap(Space::Xxs), m.gap(Space::Xs), m.gap(Space::Sm), m.gap(Space::Md));
        let inner = area.inset(md);
        // the path row: Up, the path, Refresh
        let row = Rect::new(inner.x, inner.y, inner.w, t);
        let btn = m.em(4.5).max(t);
        widgets::button(c, th, m, Rect::new(row.x, row.y, btn.min(row.w * SHARE), t), "Up", self.base + A_UP, false);
        widgets::button(c, th, m, Rect::new((row.right() - btn.min(row.w * SHARE)).max(row.x), row.y, btn.min(row.w * SHARE), t), "Refresh", self.base + A_REFRESH, false);
        let size = m.size(TypeRole::Body);
        let shown = if self.dir.is_empty() { "Home".to_string() } else { format!("Home / {}", self.dir.replace('/', " / ")) };
        let (left, right) = (row.x + btn.min(row.w * SHARE) + md, row.right() - btn.min(row.w * SHARE) - md);
        if right > left {
            let tx = fit(c, size, &shown, TextStyle::Bold, right - left);
            c.text(left, row.y + ((t - size) / 2.0).max(0.0), size, th.text, &tx, TextStyle::Bold);
        }
        let mut top = row.bottom() + sm;
        // the change buttons
        let cw = ((inner.w - sm - sm) * THIRD).max(0.0);
        for (i, (label, a)) in [("New folder", A_NEWDIR), ("Rename", A_RENAME), ("Delete", A_DELETE)].into_iter().enumerate() {
            widgets::button(c, th, m, Rect::new(inner.x + i as f32 * (cw + sm), top, cw, t), label, self.base + a, false);
        }
        top += t + sm;
        // the name being asked for
        if let Some((what, typed)) = self.name.clone() {
            let label = match what {
                NameFor::NewFolder => "Name of the new folder",
                NameFor::Rename(_) => "New name",
            };
            let bw = m.em(MIN_EM_OK).max(t).min(inner.w * QUARTER);
            let f = Rect::new(inner.x, top, (inner.w - 2.0 * (bw + sm)).max(0.0), t);
            widgets::field(c, th, m, f, label, &typed, true, self.base + A_NAME);
            widgets::button(c, th, m, Rect::new(f.right() + sm, top, bw, t), "OK", self.base + A_NAME_OK, true);
            widgets::button(c, th, m, Rect::new(f.right() + 2.0 * sm + bw, top, bw, t), "Cancel", self.base + A_NAME_CANCEL, false);
            top += t + sm;
        }
        // a refusal, or what the last change did
        for (text, col) in [(self.error.clone(), th.err), (self.note.clone().filter(|_| self.error.is_none()), th.ok)] {
            if let Some(e) = text {
                let s = m.size(TypeRole::Caption);
                let tx = fit(c, s, &e, TextStyle::Regular, inner.w);
                c.text(inner.x, top, s, col, &tx, TextStyle::Regular);
                top += m.leading(s) + xs;
            }
        }
        // the core's question, at the bottom
        let mut bottom = inner.bottom();
        if let Some(a) = self.ask.clone() {
            let size = m.size(TypeRole::Body);
            let shown = fit(c, size, &a.summary, TextStyle::Bold, inner.w - 2.0 * sm);
            let h = m.leading(size) + t + 2.0 * md;
            if bottom - h > top + t {
                let card = Rect::new(inner.x, bottom - h, inner.w, h);
                let rad = widgets::surface(c, th, m, card, th.card_sel, Radius::Md, true);
                c.outline(card, rad, m.tokens.focus_width(), th.amber);
                c.text(card.x + sm, card.y + sm, size, th.text, &shown, TextStyle::Bold);
                let by = card.bottom() - t - sm;
                let bw2 = ((card.w - sm - sm - sm) * HALF).max(0.0);
                widgets::button(c, th, m, Rect::new(card.x + sm, by, bw2, t), "Allow", self.base + A_ALLOW, true);
                widgets::button(c, th, m, Rect::new(card.x + 2.0 * sm + bw2, by, bw2, t), "Deny", self.base + A_DENY, false);
                bottom = card.y - sm;
            }
        }
        let body = Rect::new(inner.x, top, inner.w, (bottom - top).max(0.0));
        let wide = body.w >= WIDE_FROM;
        let preview_only = self.preview.is_some() && !wide;
        let (list_area, prev_area) = match (&self.preview, wide) {
            (Some(_), true) => {
                let (l, r) = body.split_left((body.w * LIST_SHARE).floor());
                (Some(l), Some(Rect::new(r.x + sm, r.y, (r.w - sm).max(0.0), r.h)))
            }
            (Some(_), false) => (None, Some(body)),
            _ => (Some(body), None),
        };
        let _ = preview_only;
        if let (Some(la), Some(l)) = (list_area, self.listing.clone()) {
            let rh = t.max(m.leading(m.size(TypeRole::Label)) * 1.3);
            let gap = xxs;
            let rows = (((la.h + gap) / (rh + gap)).floor() as usize).clamp(1, MAX_ROWS as usize);
            self.visible_rows = rows;
            let max_scroll = l.entries.len().saturating_sub(rows);
            self.scroll = self.scroll.min(max_scroll);
            if l.entries.is_empty() {
                let s = m.size(TypeRole::Body);
                c.text(la.x + xs, la.y + xs, s, th.dim, "This folder is empty.", TextStyle::Regular);
            }
            for (i, e) in l.entries.iter().enumerate().skip(self.scroll).take(rows) {
                let y = la.y + (i - self.scroll) as f32 * (rh + gap);
                let r = Rect::new(la.x, y, la.w, rh);
                let sel = i == self.selected;
                widgets::surface(c, th, m, r, if sel { th.card_sel } else { th.card }, Radius::Sm, false);
                let fs = m.size(TypeRole::Label);
                let meta = if e.is_dir { String::new() } else { size_text(e.size) };
                let mw = if meta.is_empty() { 0.0 } else { c.measure(fs * 0.85, &meta, TextStyle::Regular) + m.gap(Space::Lg) };
                let name = format!("{}{}", e.name, if e.is_dir { "/" } else { "" });
                let tx = fit(c, fs, &name, if e.is_dir { TextStyle::Bold } else { TextStyle::Regular }, (r.w - 2.0 * (md + xxs) - mw).max(0.0));
                c.text(r.x + md + xxs, r.y + ((r.h - fs) / 2.0).max(0.0), fs, th.text, &tx, if e.is_dir { TextStyle::Bold } else { TextStyle::Regular });
                if mw > 0.0 {
                    c.text(r.right() - mw, r.y + ((r.h - fs * 0.85) / 2.0).max(0.0), fs * 0.85, th.muted, &meta, TextStyle::Regular);
                }
                c.hit(r, Hit { action: self.base + A_ROW0 + (i - self.scroll) as u32, name: name.clone(), role: Role::ListItem });
            }
            if l.truncated {
                let s = m.size(TypeRole::Caption);
                c.text(la.x + xs, la.bottom() - m.leading(s), s, th.dim, "More files are in this folder than are listed.", TextStyle::Regular);
            }
        }
        if let (Some(pa), Some((path, text))) = (prev_area, self.preview.clone()) {
            widgets::surface(c, th, m, pa, th.field, Radius::Md, false);
            let pad = md;
            let title = fit(c, m.size(TypeRole::Body), path.rsplit('/').next().unwrap_or(&path), TextStyle::Bold, (pa.w - 2.0 * pad - btn).max(0.0));
            c.text(pa.x + pad, pa.y + pad, m.size(TypeRole::Body), th.text, &title, TextStyle::Bold);
            widgets::button(c, th, m, Rect::new(pa.right() - pad - btn.min(pa.w * SHARE), pa.y + pad / 2.0, btn.min(pa.w * SHARE), t), "Close", self.base + A_CLOSE, false);
            let fs = m.size(TypeRole::Small);
            let lh = m.leading(fs);
            let text_area = Rect::new(pa.x + pad, pa.y + t + pad, (pa.w - 2.0 * pad).max(0.0), (pa.h - t - 2.0 * pad).max(0.0));
            let lines_fit = (text_area.h / lh).floor() as usize;
            c.clipped(text_area, &mut |c| {
                for (i, line) in text.lines().take(MAX_PREVIEW_LINES.min(lines_fit)).enumerate() {
                    let tx = fit(c, fs, &line.replace('\t', "    "), TextStyle::Mono, text_area.w);
                    c.text(text_area.x, text_area.y + i as f32 * lh, fs, th.text, &tx, TextStyle::Mono);
                }
            });
        }
    }
}

/// "12 B", "3 KB", "1.5 MB".
pub fn size_text(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{} KB", n / 1024)
    } else {
        format!("{:.1} MB", n as f64 / 1048576.0)
    }
}

impl From<FilesError> for String {
    fn from(e: FilesError) -> String {
        e.sentence()
    }
}
