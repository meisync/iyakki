use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::files::FilesPort;
use iyakki_ui_sdk::filesview::{self, FilesView};
use iyakki_ui_sdk::geom::Rect;

const FILES: u32 = 100; // the view owns the numbers FILES .. FILES + filesview::ACTIONS

pub struct FilesApp {
    files: FilesView,
}

impl FilesApp {
    /// `port` is the core's file port on a device, `MemFiles` in a test or the preview.
    pub fn new(port: Box<dyn FilesPort>) -> FilesApp {
        FilesApp { files: FilesView::new(port, FILES) }
    }
}

impl FilesApp {
    /// Close the text the person opened (for example when an assistant tool asks for it).
    pub fn close_preview(&mut self) {
        self.files.close();
    }
}

impl UiApp for FilesApp {
    fn id(&self) -> &'static str { "files-demo" }
    fn title(&self) -> String { "Files demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => self.files.tick(),
            UiEvent::Action(a) if self.files.owns(*a) => { self.files.action(*a); }
            UiEvent::Wheel(n) => self.files.wheel(*n),
            UiEvent::Key(k) => {
                if !self.files.key(*k) && *k == UiKey::Esc { return Flow::Back; }
            }
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let m = cx.metrics();
        let g = m.gutter();
        self.files.draw(c, cx.colors(), &m, Rect::new(g, g, w - 2.0 * g, h - 2.0 * g));
        let _ = filesview::ACTIONS;
    }
}
