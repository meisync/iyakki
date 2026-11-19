use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::browser::{BrowserPort, BrowserView};
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;

const WEB: u32 = 10_000; // a BrowserView takes WEB .. WEB + browser::ACTION_SPAN

pub struct WebApp {
    web: BrowserView,
    started: bool,
    start_url: String,
}

impl WebApp {
    pub fn new(port: Box<dyn BrowserPort>, url: &str) -> WebApp {
        WebApp { web: BrowserView::new(port, WEB), started: false, start_url: url.to_string() }
    }
}

impl UiApp for WebApp {
    fn id(&self) -> &'static str { "web-demo" }
    fn title(&self) -> String { "Web demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        if let UiEvent::Tick = ev {
            if !self.started {
                self.started = true;
                self.web.load_url(&self.start_url); // the core checks the address and the person's rules
            }
            self.web.refresh();
        }
        if !self.web.event(ev) {
            if let UiEvent::Key(UiKey::Esc) = ev { return Flow::Back; }
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let g = cx.metrics().gutter();
        self.web.draw(c, cx, Rect::new(g, g, w - 2.0 * g, h - 2.0 * g));
    }
}
