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
