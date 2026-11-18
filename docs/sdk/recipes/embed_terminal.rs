use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::terminal::TerminalPort;
use iyakki_ui_sdk::terminalview::TerminalView;

const TERM: u32 = 300; // TerminalView uses TERM .. TERM + terminalview::ACTIONS

pub struct TerminalApp {
    term: TerminalView,
}

impl TerminalApp {
    pub fn new(port: Box<dyn TerminalPort>) -> TerminalApp {
        TerminalApp { term: TerminalView::new(port, TERM) }
    }
}

impl UiApp for TerminalApp {
    fn id(&self) -> &'static str { "terminal-demo" }
    fn title(&self) -> String { "Terminal demo".into() }
    fn wants_keyboard(&self) -> bool { self.term.wants_keyboard() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => self.term.tick(),
            UiEvent::Action(a) if self.term.owns(*a) => { self.term.action(*a); }
            UiEvent::Text(t) => { self.term.text(t); }
            UiEvent::Wheel(n) => self.term.wheel(*n),
            UiEvent::Key(k) => { if !self.term.key(*k) && *k == UiKey::Esc { return Flow::Back; } }
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let m = cx.metrics();
        let g = m.gutter();
        self.term.draw(c, cx.colors(), &m, Rect::new(g, g, w - 2.0 * g, h - 2.0 * g));
    }
}
