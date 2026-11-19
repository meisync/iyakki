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
