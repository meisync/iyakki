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
