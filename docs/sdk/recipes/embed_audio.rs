use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::audioview::AudioView;
use iyakki_ui_sdk::canvas::Canvas;
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::media::{MediaPort, Source};

const AUDIO: u32 = 500; // AudioView uses AUDIO .. AUDIO + audioview::ACTIONS

pub struct PlayerApp {
    audio: AudioView,
}

impl PlayerApp {
    pub fn new(port: Box<dyn MediaPort>) -> PlayerApp {
        PlayerApp { audio: AudioView::new(port, AUDIO) }
    }
    /// Play a file of the person's home folder (the name the port listed). A stream address asks the person first.
    pub fn play(&mut self, name: &str) {
        self.audio.open(Source::Home(name.to_string()), name);
    }
}

impl UiApp for PlayerApp {
    fn id(&self) -> &'static str { "player-demo" }
    fn title(&self) -> String { "Player demo".into() }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        match ev {
            UiEvent::Tick => self.audio.tick(),
            UiEvent::Action(a) if self.audio.owns(*a) => { self.audio.action(*a); }
            UiEvent::Wheel(n) => self.audio.wheel(*n),
            UiEvent::Key(k) => { if !self.audio.key(*k) && *k == UiKey::Esc { return Flow::Back; } }
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let m = cx.metrics();
        let g = m.gutter();
        self.audio.draw(c, cx.colors(), &m, Rect::new(g, g, w - 2.0 * g, h - 2.0 * g));
    }
}
