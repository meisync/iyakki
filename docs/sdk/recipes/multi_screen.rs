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
