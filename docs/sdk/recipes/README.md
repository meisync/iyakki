# Recipes

One short, compiling example per pattern. Each is a file in this folder (`NAME.rs`); the block below is a copy the script `dist/sdk/gen-widget-docs.py` keeps equal to the file, and `crates/iyakki-ui-sdk/tests/docs_recipes.rs` compiles and runs every file at three pane sizes and text 3x. Copy one into your app and change it.

| I want to | Recipe |
|---|---|
| Add a tab or a pane | [tabs](#tabs-and-panes) |
| Show a list with a selection (and a detail pane when there is room) | [list](#a-list-with-a-selection) |
| Ask for input and say why it is wrong | [form](#a-form-with-validation) |
| Keep something between runs | [storage](#keep-something-through-the-core) |
| Draw my own shapes in the skin | [themed drawing](#themed-custom-drawing) |
| Let the person confirm a change | [approval card](#an-approval-card) |
| Several screens: a nav row, one focus value, Tab order, text routing, paging | [multi-screen](#several-screens) |
| Draw a barcode (Code 128, EAN-13, UPC-A) | [barcode](#barcode-labels) |
| Take a list as a chat tool argument | [list arguments](#chat-tool-arguments-and-lists) |
| Embed Files, Chat, Browser, Terminal, Editor, Audio | [embedding.md](../embedding.md) |
| Hear what happens on the machine (network, power, files, ...), online and offline | [events](#hear-events-online-and-offline) |
| Add a setting the person can change, with a chat tool | [setting with a chat tool](#a-setting-with-a-chat-tool) (a procedure, not a code file) |
| Test with a fake | [testing.md](../testing.md) and the test file `crates/iyakki-ui-sdk/tests/docs_recipes.rs` |

## Tabs and panes

A tab is a button with a name; the shown pane is a field in your state. Every button is named by its label, so the screen reader and the tests find it.

<!-- snippet: add_tabs.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::{Canvas, TextStyle};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::size::TypeRole;
use iyakki_ui_sdk::tokens::Space;
use iyakki_ui_sdk::widgets;

const A_TAB_HOME: u32 = 1;
const A_TAB_ABOUT: u32 = 2;

#[derive(Clone, Copy, PartialEq)]
enum Pane { Home, About }

pub struct TabsApp { pane: Pane }

impl TabsApp {
    pub fn new() -> TabsApp { TabsApp { pane: Pane::Home } }
    pub fn on_about(&self) -> bool { self.pane == Pane::About }
}

impl UiApp for TabsApp {
    fn id(&self) -> &'static str { "tabs-demo" }
    fn title(&self) -> String { "Tabs demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Action(A_TAB_HOME) => self.pane = Pane::Home,
            UiEvent::Action(A_TAB_ABOUT) => self.pane = Pane::About,
            UiEvent::Key(UiKey::Esc) => return Flow::Back,
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, _) = c.size();
        let (th, m) = (cx.colors(), cx.metrics());
        let g = m.gutter();
        let tab_w = (w - 2.0 * g - m.gap(Space::Md)) / 2.0;
        // the tab strip: a button per pane, the shown one primary; every button is named by its label
        widgets::button(c, th, &m, Rect::new(g, g, tab_w, m.min_target()), "Home", A_TAB_HOME, self.pane == Pane::Home);
        widgets::button(c, th, &m, Rect::new(g + tab_w + m.gap(Space::Md), g, tab_w, m.min_target()), "About", A_TAB_ABOUT, self.pane == Pane::About);
        let y = g + m.min_target() + m.gap(Space::Lg);
        let text = if self.pane == Pane::Home { "Welcome" } else { "A small app" };
        c.text(g, y, m.size(TypeRole::Title), th.text, text, TextStyle::Bold);
    }
}
```
<!-- /snippet -->

## A list with a selection

Rows are actions `A_ROW0 + i`. Arrow keys move the selection. On a compact pane the list is alone; on a wider pane the detail sits beside it (ask `cx.size_class()`, never the pixel width).

<!-- snippet: list_selection.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::{Canvas, TextStyle};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::size::{SizeClass, TypeRole};
use iyakki_ui_sdk::tokens::Space;
use iyakki_ui_sdk::widgets;

const A_ROW0: u32 = 100; // row i is the action A_ROW0 + i
const MAX_ROWS: usize = 200;

pub struct ListApp { pub items: Vec<String>, pub selected: usize }

impl ListApp {
    pub fn new(items: &[&str]) -> ListApp { ListApp { items: items.iter().map(|s| s.to_string()).collect(), selected: 0 } }
}

impl UiApp for ListApp {
    fn id(&self) -> &'static str { "list-demo" }
    fn title(&self) -> String { "List demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        let last = self.items.len().saturating_sub(1);
        match ev {
            UiEvent::Action(a) if *a >= A_ROW0 && ((*a - A_ROW0) as usize) < self.items.len() => self.selected = (*a - A_ROW0) as usize,
            UiEvent::Key(UiKey::Down) => self.selected = (self.selected + 1).min(last),
            UiEvent::Key(UiKey::Up) => self.selected = self.selected.saturating_sub(1),
            UiEvent::Key(UiKey::Esc) => return Flow::Back,
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let (th, m) = (cx.colors(), cx.metrics());
        let g = m.gutter();
        // compact: the list alone; wider: the list on the left and the detail on the right
        let list_w = if cx.size_class() == SizeClass::Compact { w - 2.0 * g } else { (w - g - g - g) / 2.0 };
        let row_h = m.min_target();
        let mut y = g;
        for (i, it) in self.items.iter().enumerate().take(MAX_ROWS) {
            if y + row_h > h { break; } // a longer list scrolls: keep an offset and a wheel handler
            widgets::row(c, th, &m, Rect::new(g, y, list_w, row_h), it, i == self.selected, A_ROW0 + i as u32);
            y += row_h + m.gap(Space::Xs);
        }
        if cx.size_class() != SizeClass::Compact {
            if let Some(it) = self.items.get(self.selected) {
                c.text(2.0 * g + list_w, g, m.size(TypeRole::Title), th.text, it, TextStyle::Bold);
            }
        }
    }
}
```
<!-- /snippet -->

## A form with validation

The rule is a plain function (testable without a screen). The app says why in words next to the field; it never fails silently.

<!-- snippet: form_validation.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::{fit, Canvas, TextStyle};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::size::TypeRole;
use iyakki_ui_sdk::tokens::Space;
use iyakki_ui_sdk::widgets;

const A_NAME: u32 = 1;
const A_SAVE: u32 = 2;

/// The rule is a plain function: the test calls it without a screen.
pub fn check_name(s: &str) -> Result<(), &'static str> {
    if s.trim().is_empty() { return Err("Give a name."); }
    if s.chars().count() > 30 { return Err("That name is too long (30 letters at most)."); }
    Ok(())
}

pub struct FormApp { pub name: String, pub saved: Option<String>, focused: bool, said: String }

impl FormApp {
    pub fn new() -> FormApp { FormApp { name: String::new(), saved: None, focused: false, said: String::new() } }
}

impl UiApp for FormApp {
    fn id(&self) -> &'static str { "form-demo" }
    fn title(&self) -> String { "Form demo".into() }
    fn wants_keyboard(&self) -> bool { self.focused }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Action(A_NAME) => self.focused = true,
            UiEvent::Text(t) if self.focused => self.name.push_str(t),
            UiEvent::Key(UiKey::Backspace) if self.focused => { self.name.pop(); }
            UiEvent::Action(A_SAVE) | UiEvent::Key(UiKey::Enter) => match check_name(&self.name) {
                Ok(()) => { self.saved = Some(self.name.clone()); self.said = "Saved.".into(); }
                Err(why) => self.said = why.into(), // say why, in the person's words, next to the field
            },
            UiEvent::Key(UiKey::Esc) => return Flow::Back,
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, _) = c.size();
        let (th, m) = (cx.colors(), cx.metrics());
        let g = m.gutter();
        let width = w - 2.0 * g;
        let fh = m.min_target();
        widgets::field(c, th, &m, Rect::new(g, g, width, fh), "Name", &self.name, self.focused, A_NAME);
        widgets::button(c, th, &m, Rect::new(g, g + fh + m.gap(Space::Md), width, fh), "Save", A_SAVE, true);
        if !self.said.is_empty() {
            let size = m.size(TypeRole::Body);
            let line = fit(c, size, &self.said, TextStyle::Regular, width);
            c.text(g, g + 2.0 * fh + 2.0 * m.gap(Space::Md), size, th.muted, &line, TextStyle::Regular);
        }
    }
}
```
<!-- /snippet -->

## Keep something through the core

`StoragePort` is the app's own saved data (256 KB, keys up to 128 bytes): on a device it is the core's `storage` namespace. A refusal (`PortError`) is shown, not swallowed.

<!-- snippet: persist_storage.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::{fit, Canvas, TextStyle};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::ports::{PortError, StoragePort};
use iyakki_ui_sdk::size::TypeRole;
use iyakki_ui_sdk::widgets;

const A_ADD: u32 = 1;
const KEY: &str = "count";

/// The count survives a restart: it is kept through the storage port, which is the core's. The app never opens a file.
pub struct KeepApp { count: u32, storage: Box<dyn StoragePort>, said: String }

impl KeepApp {
    pub fn new(mut storage: Box<dyn StoragePort>) -> KeepApp {
        let count = storage.get(KEY).and_then(|s| s.parse().ok()).unwrap_or(0);
        KeepApp { count, storage, said: String::new() }
    }
    pub fn count(&self) -> u32 { self.count }
}

impl UiApp for KeepApp {
    fn id(&self) -> &'static str { "keep-demo" }
    fn title(&self) -> String { "Keep demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Action(A_ADD) => {
                self.count += 1;
                // a port can refuse: show the sentence, do not hide it
                self.said = match self.storage.set(KEY, &self.count.to_string()) {
                    Ok(()) => String::new(),
                    Err(PortError::TooBig) => "That is too much to keep.".into(),
                    Err(_) => "The core did not allow saving.".into(),
                };
            }
            UiEvent::Key(UiKey::Esc) => return Flow::Back,
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, _) = c.size();
        let (th, m) = (cx.colors(), cx.metrics());
        let g = m.gutter();
        let size = m.size(TypeRole::Large);
        let line = fit(c, size, &format!("Count: {}", self.count), TextStyle::Bold, w - 2.0 * g);
        c.text(g, g, size, th.text, &line, TextStyle::Bold);
        widgets::button(c, th, &m, Rect::new(g, g + 2.0 * size, w - 2.0 * g, m.min_target()), "Add one", A_ADD, true);
        if !self.said.is_empty() {
            c.text(g, g + 2.0 * size + 2.0 * m.min_target(), m.size(TypeRole::Body), th.muted, &self.said, TextStyle::Regular);
        }
    }
}
```
<!-- /snippet -->

## Themed custom drawing

Gaps, radii, line widths and colours all come from the skin through `m` and `th`. Swap the skin and this drawing changes with it.

<!-- snippet: themed_drawing.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::tokens::{Line, Radius, Space};
use iyakki_ui_sdk::widgets;

pub struct BarsApp { pub values: Vec<f32> }

impl UiApp for BarsApp {
    fn id(&self) -> &'static str { "bars-demo" }
    fn title(&self) -> String { "Bars demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        if let UiEvent::Key(UiKey::Esc) = ev { return Flow::Back; }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let (th, m) = (cx.colors(), cx.metrics());
        let g = m.gutter();
        // a card in the skin's own shape (radius, edge, shadow, glass) ...
        let card = Rect::new(g, g, w - 2.0 * g, h - 2.0 * g);
        widgets::surface(c, th, &m, card, th.card, Radius::Lg, true);
        // ... and bars whose gaps, radius and colours all come from the skin: no number of your own
        let n = self.values.len().max(1) as f32;
        let gap = m.gap(Space::Md);
        let bw = ((card.w - gap * (n + 1.0)) / n).max(0.0);
        let top = m.gap(Space::Lg);
        for (i, v) in self.values.iter().enumerate() {
            let bh = (card.h - 2.0 * top) * v.clamp(0.0, 1.0);
            let r = Rect::new(card.x + gap + i as f32 * (bw + gap), card.y + card.h - top - bh, bw, bh);
            c.rrect(r, m.tokens.radius_for(Radius::Sm, r.h), th.accent);
        }
        // the baseline uses the skin's line width and a theme colour
        c.line(card.x + gap, card.y + card.h - top, card.x + card.w - gap, card.y + card.h - top, m.line(Line::Normal), th.border);
    }
}
```
<!-- /snippet -->

## An approval card

You do not draw a card for core actions: declare the tool with `ask = true` and the ChatView draws Allow and Deny while the core writes the audit line. The test `the_chat_runs_an_app_tool_after_the_person_allows_it` shows the order: the tool waits, the card is on screen, Allow runs it. The card appears by itself when a call waits (no focus needed); the field must be focused only to send a message, and it gives up the focus when a card arrives so `y` and `n` work at once (`the_card_needs_no_input_focus_and_y_answers_it_at_once`).

<!-- snippet: approval_card.rs -->
```rust
use iyakki_ui_sdk::assistant::ToolSpec;

/// An approval card is not drawn by the app: it comes from the core. Mark a tool that changes data as `ask = true` and the ChatView draws
/// the card (Allow and Deny, also `y` and `n`) and the core writes the audit line. A read-only tool is `ask = false`.
pub fn tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec::new("delete_note", "Delete one note by its number.", r#"{"type":"object","properties":{"n":{"type":"integer"}},"required":["n"]}"#, true),
        ToolSpec::new("list_notes", "List the notes.", r#"{"type":"object"}"#, false),
    ]
}
```
<!-- /snippet -->

## Several screens

A nav row of buttons (one per screen), ONE `Focus` enum for which field gets typed text (`Text` with no focus is dropped, never typed into a hidden field), a Tab order, action numbers in blocks (`A_NAV + index`, `A_FIELD + index`, `A_ROW0 + row`), a paging helper that cannot go out of range, and a test that sweeps every screen at three sizes and text 3x.

<!-- snippet: multi_screen.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::{fit, Canvas, TextStyle};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::size::TypeRole;
use iyakki_ui_sdk::tokens::Space;
use iyakki_ui_sdk::widgets;

// action numbers: one block per concern, so a number tells you where it belongs
const A_NAV: u32 = 1; // A_NAV + screen index
const A_FIELD: u32 = 10; // A_FIELD + field index
const A_PREV: u32 = 20;
const A_NEXT: u32 = 21;
const A_SAVE: u32 = 22;
const A_ROW0: u32 = 100;
const PER_PAGE: usize = 4;

#[derive(Clone, Copy, PartialEq)]
enum Screen { Home, Items, Edit }
const SCREENS: [(Screen, &str); 3] = [(Screen::Home, "Home"), (Screen::Items, "Items"), (Screen::Edit, "Edit")];

/// ONE focus value for the whole app: which text field receives typed text (None: no field).
#[derive(Clone, Copy, PartialEq)]
enum Focus { None, Name, Qty }

/// The paging helper: the slice for `page` and how many pages there are (a page is never out of range).
pub fn page_of<T>(items: &[T], page: usize, per: usize) -> (&[T], usize) {
    let pages = items.len().div_ceil(per).max(1);
    let p = page.min(pages - 1);
    (&items[(p * per).min(items.len())..((p + 1) * per).min(items.len())], pages)
}

pub struct MultiApp { screen: Screen, focus: Focus, name: String, qty: String, items: Vec<String>, page: usize }

impl MultiApp {
    pub fn new() -> MultiApp {
        MultiApp { screen: Screen::Home, focus: Focus::None, name: String::new(), qty: String::new(), items: (1..=10).map(|i| format!("Item {i}")).collect(), page: 0 }
    }
    pub fn items(&self) -> &[String] { &self.items }
    pub fn page(&self) -> usize { self.page }
    fn field_mut(&mut self) -> Option<&mut String> {
        match self.focus { Focus::Name => Some(&mut self.name), Focus::Qty => Some(&mut self.qty), Focus::None => None }
    }
}

impl UiApp for MultiApp {
    fn id(&self) -> &'static str { "multi-demo" }
    fn title(&self) -> String { "Multi demo".into() }
    fn wants_keyboard(&self) -> bool { self.focus != Focus::None }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Action(a) if *a >= A_NAV && ((*a - A_NAV) as usize) < SCREENS.len() => { self.screen = SCREENS[(*a - A_NAV) as usize].0; self.focus = Focus::None; }
            UiEvent::Action(A_FIELD) => self.focus = Focus::Name,
            UiEvent::Action(A_PREV) => self.page = self.page.saturating_sub(1),
            UiEvent::Action(A_NEXT) => { let (_, pages) = page_of(&self.items, self.page, PER_PAGE); self.page = (self.page + 1).min(pages - 1); }
            UiEvent::Action(a) if *a == A_FIELD + 1 => self.focus = Focus::Qty,
            UiEvent::Action(A_SAVE) => { if !self.name.trim().is_empty() { self.items.push(self.name.trim().to_string()); self.name.clear(); self.qty.clear(); self.focus = Focus::None; } }
            // text goes to the focused field only; with no focus it is dropped, never typed into a hidden field
            UiEvent::Text(t) => { if let Some(f) = self.field_mut() { f.push_str(t); } }
            UiEvent::Key(UiKey::Tab) if self.screen == Screen::Edit => {
                // Tab order: Name, Qty, then out of the fields
                self.focus = match self.focus { Focus::None => Focus::Name, Focus::Name => Focus::Qty, Focus::Qty => Focus::None };
            }
            UiEvent::Key(UiKey::Backspace) => { if let Some(f) = self.field_mut() { f.pop(); } }
            UiEvent::Key(UiKey::Esc) => { if self.focus != Focus::None { self.focus = Focus::None } else { return Flow::Back } }
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let (th, m) = (cx.colors(), cx.metrics());
        let (g, t, gap) = (m.gutter(), m.min_target(), m.gap(Space::Md));
        // the nav row: one button per screen, equal widths (the count is a const-like value of the array)
        let n = SCREENS.len() as f32;
        let bw = (w - 2.0 * g - gap * (n - 1.0)) / n;
        for (i, (s, label)) in SCREENS.iter().enumerate() {
            widgets::button(c, th, &m, Rect::new(g + i as f32 * (bw + gap), g, bw, t), label, A_NAV + i as u32, *s == self.screen);
        }
        let top = g + t + gap;
        let width = w - 2.0 * g;
        let body = m.size(TypeRole::Body);
        match self.screen {
            Screen::Home => { let l = fit(c, body, &format!("{} items", self.items.len()), TextStyle::Regular, width); c.text(g, top, body, th.text, &l, TextStyle::Regular); }
            Screen::Items => {
                let (rows, pages) = page_of(&self.items, self.page, PER_PAGE);
                let mut y = top;
                for (i, it) in rows.iter().enumerate() {
                    if y + t > h - g - t { break; }
                    widgets::row(c, th, &m, Rect::new(g, y, width, t), it, false, A_ROW0 + i as u32);
                    y += t + m.gap(Space::Xs);
                }
                let by = (h - g - t).max(y);
                widgets::button(c, th, &m, Rect::new(g, by, (width - gap) / 2.0, t), "Previous", A_PREV, false);
                widgets::button(c, th, &m, Rect::new(g + (width + gap) / 2.0, by, (width - gap) / 2.0, t), "Next", A_NEXT, false);
                let l = fit(c, body, &format!("Page {} of {}", self.page.min(pages - 1) + 1, pages), TextStyle::Regular, width);
                c.text(g, by - body * 1.5, body, th.muted, &l, TextStyle::Regular);
            }
            Screen::Edit => {
                widgets::field(c, th, &m, Rect::new(g, top, width, t), "Name", &self.name, self.focus == Focus::Name, A_FIELD);
                widgets::field(c, th, &m, Rect::new(g, top + t + gap, width, t), "Quantity", &self.qty, self.focus == Focus::Qty, A_FIELD + 1);
                widgets::button(c, th, &m, Rect::new(g, top + 2.0 * (t + gap), width, t), "Save", A_SAVE, true);
            }
        }
    }
}
```
<!-- /snippet -->

## Barcode labels

`iyakki_ui_sdk::barcode` has `encode_code128` (subsets A, B and C chosen for you, check symbol, stop), `encode_ean13`, `encode_upca` and a decoder for tests; `BarcodeView` draws them as dark bars on light paper in any skin, with a quiet zone and the text under the bars, as a named image. Do not write the 107-symbol table yourself: it is in the SDK, checked against independent encodings.

<!-- snippet: barcode_label.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::barcodeview::BarcodeView;
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;

/// A label with a Code 128 barcode of the order number. The table, the subsets and the check symbol are in the SDK (`iyakki_ui_sdk::barcode`),
/// tested against independent encodings: do not write your own. `BarcodeView::ean13(action, "590123412345")` and `::upca(..)` draw EAN-13 and UPC-A.
pub struct LabelApp {
    code: BarcodeView,
}

impl LabelApp {
    pub fn new(order: &str) -> Result<LabelApp, String> {
        Ok(LabelApp { code: BarcodeView::code128(1, order)? })
    }
    pub fn modules(&self) -> usize {
        self.code.modules().len()
    }
}

impl UiApp for LabelApp {
    fn id(&self) -> &'static str { "label-demo" }
    fn title(&self) -> String { "Label demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        if let UiEvent::Key(UiKey::Esc) = ev { return Flow::Back; }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let m = cx.metrics();
        let g = m.gutter();
        self.code.draw(c, cx.colors(), &m, Rect::new(g, g, w - 2.0 * g, (h - 2.0 * g).min(w / 2.0)));
    }
}
```
<!-- /snippet -->

## Chat tool arguments and lists

Arguments reach the app as strings, whole numbers (`uint`) and booleans only. A JSON array or object is kept as raw text and there is no accessor for it. Pass a list as one string of items (`SKU:QTY,SKU:QTY`), say the format in the tool's description, and parse it with a function that returns a sentence for each refusal.

<!-- snippet: tool_list_args.rs -->
```rust
use iyakki_ui_sdk::toolargs::Args;

/// A chat tool's arguments reach the app as strings, whole numbers and booleans: `Args::str`, `Args::uint`, `Args::bool`. A JSON array or
/// object is kept as raw text and there is no accessor for it. So a list is ONE string argument written as `SKU:QTY` items separated by
/// commas ("A1:2,B7:10"). Declare it in the tool's schema as a string and say the format in its description, which the model reads.
pub const SCHEMA: &str = r#"{"type":"object","properties":{"items":{"type":"string","description":"Comma separated SKU:QTY pairs, for example A1:2,B7:10"}},"required":["items"]}"#;

/// Read `SKU:QTY,SKU:QTY` into pairs; every refusal is a sentence the model can act on.
pub fn parse_items(text: &str) -> Result<Vec<(String, u32)>, String> {
    let mut out = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (sku, qty) = part.split_once(':').ok_or_else(|| format!("`{part}` is not SKU:QTY"))?;
        let qty: u32 = qty.trim().parse().map_err(|_| format!("the quantity in `{part}` is not a whole number"))?;
        if sku.trim().is_empty() || qty == 0 {
            return Err(format!("`{part}` needs a SKU and a quantity above zero"));
        }
        out.push((sku.trim().to_string(), qty));
    }
    if out.is_empty() { Err("give at least one SKU:QTY".into()) } else { Ok(out) }
}

/// In the tick loop: `let items = tool_items(&inv.args)?;` then act, then `chat.answer(&inv, Ok(..))`.
pub fn tool_items(args_json: &str) -> Result<Vec<(String, u32)>, String> {
    let a = Args::parse(args_json)?;
    parse_items(a.str("items").ok_or("give items as a string like A1:2,B7:10")?)
}
```
<!-- /snippet -->

## Hear events, online and offline

Register interest in named events, handle them in order, and handle what waited while the app was away before the first frame. A `late` event happened while the app was not running (update the screen, do not ring); a `missed` event means events were dropped (read the state again). `app-check` fails an app that registers and handles neither. See [events.md](../events.md) for the catalogue and the rules.

<!-- snippet: events_react.rs -->
```rust
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::{Canvas, TextStyle};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::events::{EventActions, EventRouter, EventsPort};
use iyakki_ui_sdk::size::TypeRole;
use std::cell::RefCell;
use std::rc::Rc;

pub struct NetApp {
    port: Box<dyn EventsPort>,
    acts: Box<dyn EventActions>,
    router: EventRouter,
    network: Rc<RefCell<String>>,
    permissions: Rc<RefCell<String>>,
}

impl NetApp {
    pub fn new(mut port: Box<dyn EventsPort>, mut acts: Box<dyn EventActions>) -> NetApp {
        let network = Rc::new(RefCell::new("not heard yet".to_string()));
        let (net, missed) = (network.clone(), network.clone());
        let permissions = Rc::new(RefCell::new(String::new()));
        let perm = permissions.clone();
        let mut router = EventRouter::new()
            .on("connectivity.changed", move |e, acts| {
                *net.borrow_mut() = e.text("state").unwrap_or("?").to_string();
                // an event that arrives `late` happened while the app was away: update the screen, but do not ring
                if !e.late && e.text("state") == Some("offline") {
                    let _ = acts.notify("The network went away");
                }
            })
            // what the app may do changed (always told, no registration needed): stop using what was taken away, read the set again
            .on_permissions(move |change, _, _| *perm.borrow_mut() = format!("{} held, {} taken away by {}", change.held, change.revoked.len(), change.by))
            // events were dropped: the mirrored state may be stale, so say so until the next one
            .on_missed(move |_, _| *missed.borrow_mut() = "unknown (events were missed)".to_string());
        // the core refuses an event the app holds no permission for, with a sentence the app may show
        let _refusals = router.register_all(port.as_mut());
        // what waited while the app was away is handled here, before the first frame
        router.begin(port.as_mut(), acts.as_mut());
        NetApp { port, acts, router, network, permissions }
    }
}

impl UiApp for NetApp {
    fn id(&self) -> &'static str { "net-demo" }
    fn title(&self) -> String { "Network demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => { self.router.pump(self.port.as_mut(), self.acts.as_mut()); }
            UiEvent::Key(UiKey::Esc) => { self.port.stop(); return Flow::Back; }
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let m = cx.metrics();
        let g = m.gutter();
        let size = m.size(TypeRole::Body);
        c.text(g, g, size, cx.colors().text, &format!("Network: {}", self.network.borrow()), TextStyle::Regular);
        c.text(g, g + m.leading(size), size, cx.colors().muted, &format!("Permissions: {}", self.permissions.borrow()), TextStyle::Regular);
    }
}
```
<!-- /snippet -->

## A setting with a chat tool

Every feature the person can change in a screen must also be reachable by chat (owner rule 2026-11-06). The steps for a setting your app owns:

1. Keep the value through the storage port (the recipe above), under a key such as `show_done`.
2. Add two tools to the `ToolSpec` list of your ChatView: a read (`ask = false`, `show_done_status`) and a change (`ask = true`, `show_done_set` with a boolean argument). The change asks every time and names the new value in its description.
3. Run both in the `tick()` loop and `answer` them. The screen switch and the tool must call the same function (one shared path); never two copies.
4. Add a test that sets the value by the tool and reads it back by the screen, and that the change tool has `ask == true`.
5. If the setting belongs on a Settings page of the OS (not only in your app), the page needs a row in `docs/development/CHAT-REACHABILITY.md` naming the tools and their risk class, and the gate test `chat_reachability_gate` (crates/iyakki-apps) checks it. For an app's own setting inside the app, the row is a line in the app's own notes in `FEATURES.md`.
