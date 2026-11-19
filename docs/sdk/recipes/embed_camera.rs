use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::cameras::CameraPort;
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cameraview::{CameraView, ViewConfig};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;

const CAM: u32 = 600; // the view uses CAM .. CAM + cameraview::ACTIONS

pub struct DoorbellApp {
    cams: CameraView,
    now_ms: u64,
}

impl DoorbellApp {
    pub fn new(port: Box<dyn CameraPort>) -> DoorbellApp {
        // the person sees a consent card (Allow, Deny) before any picture is pulled
        let cfg = ViewConfig { label: "Doorbell panel".into(), ..ViewConfig::default() };
        DoorbellApp { cams: CameraView::new(port, cfg, CAM), now_ms: 0 }
    }
}

impl UiApp for DoorbellApp {
    fn id(&self) -> &'static str { "doorbell-demo" }
    fn title(&self) -> String { "Doorbell demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => { self.now_ms += 33; self.cams.tick(self.now_ms); } // about 30 ticks a second
            UiEvent::Action(a) if self.cams.owns(*a) => { self.cams.action(*a); }
            UiEvent::Key(k) => { if !self.cams.key(*k) && *k == UiKey::Esc { self.cams.close(); return Flow::Back; } }
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let m = cx.metrics();
        let g = m.gutter();
        self.cams.draw(c, cx.colors(), &m, Rect::new(g, g, w - 2.0 * g, h - 2.0 * g));
    }
}
