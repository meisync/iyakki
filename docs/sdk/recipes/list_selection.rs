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
