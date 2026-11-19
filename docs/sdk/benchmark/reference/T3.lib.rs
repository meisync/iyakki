//! Reference solution of T3 (themed dashboard: list and detail, accessible at three sizes).
use iyakki_ui_sdk::app::{Flow, UiApp};
use iyakki_ui_sdk::canvas::{fit, Canvas, TextStyle};
use iyakki_ui_sdk::cx::UiCx;
use iyakki_ui_sdk::event::{UiEvent, UiKey};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::size::{SizeClass, TypeRole};
use iyakki_ui_sdk::tokens::{Radius, Space};
use iyakki_ui_sdk::widgets;

pub mod ports;

pub const ID: &str = "bench-t3";
const A_BACK: u32 = 1;
const A_ROW0: u32 = 10;
const DOT: f32 = 0.5; // a status dot is half a text size across

struct Service {
    name: &'static str,
    status: &'static str,
    note: &'static str,
}

const SERVICES: [Service; 3] = [
    Service { name: "Alpha", status: "ok", note: "All systems normal." },
    Service { name: "Bravo", status: "slow", note: "Latency above 300 ms." },
    Service { name: "Charlie", status: "down", note: "Not responding." },
];

pub struct BenchT3App {
    selected: usize,
    detail: bool,
}

impl Default for BenchT3App {
    fn default() -> Self {
        BenchT3App::new()
    }
}

impl BenchT3App {
    pub fn new() -> BenchT3App {
        BenchT3App { selected: 0, detail: false }
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// True while a compact pane shows the detail screen instead of the list.
    pub fn detail_open(&self) -> bool {
        self.detail
    }
}

impl UiApp for BenchT3App {
    fn id(&self) -> &'static str {
        ID
    }
    fn title(&self) -> String {
        "BENCH T3".into()
    }
    fn event(&mut self, ev: &UiEvent, _cx: &mut UiCx) -> Flow {
        let last = SERVICES.len() - 1;
        match ev {
            UiEvent::Action(A_BACK) => self.detail = false,
            UiEvent::Action(a) if *a >= A_ROW0 && ((*a - A_ROW0) as usize) <= last => {
                self.selected = (*a - A_ROW0) as usize;
                self.detail = true;
            }
            UiEvent::Key(UiKey::Down) => self.selected = (self.selected + 1).min(last),
            UiEvent::Key(UiKey::Up) => self.selected = self.selected.saturating_sub(1),
            UiEvent::Key(UiKey::Enter) => self.detail = true,
            UiEvent::Key(UiKey::Esc) => {
                if self.detail {
                    self.detail = false;
                } else {
                    return Flow::Back;
                }
            }
            _ => {}
        }
        Flow::Stay
    }
    fn view(&mut self, c: &mut dyn Canvas, cx: &UiCx) {
        let (w, h) = c.size();
        let (th, m) = (cx.colors(), cx.metrics());
        c.gradient(Rect::new(0.0, 0.0, w, h), th.bg_top, th.bg_bottom);
        let g = m.gutter();
        let wide = cx.size_class() != SizeClass::Compact;
        let t = m.min_target();
        let list_w = if wide { (w - g - g - g) / 2.0 } else { w - 2.0 * g };
        let show_list = wide || !self.detail;
        let show_detail = wide || self.detail;
        if show_list {
            let mut y = g;
            for (i, s) in SERVICES.iter().enumerate() {
                let r = widgets::row(c, th, &m, Rect::new(g, y, list_w, t), s.name, i == self.selected, A_ROW0 + i as u32);
                let col = match s.status {
                    "ok" => th.ok,
                    "down" => th.err,
                    _ => th.amber,
                };
                let d = m.size(TypeRole::Body) * DOT;
                c.rrect(Rect::new(r.right() - d - m.gap(Space::Md), r.y + (r.h - d) / 2.0, d, d), d / 2.0, col);
                y += r.h + m.gap(Space::Xs);
            }
        }
        if show_detail {
            let x = if wide { g + list_w + g } else { g };
            let mut y = g;
            if !wide {
                let r = widgets::button(c, th, &m, Rect::new(x, y, list_w, t), "Back", A_BACK, false);
                y += r.h + m.gap(Space::Lg);
            }
            let s = &SERVICES[self.selected.min(SERVICES.len() - 1)];
            let pw = if wide { w - x - g } else { list_w };
            let ph = (h - y - g).max(0.0);
            widgets::surface(c, th, &m, Rect::new(x, y, pw, ph), th.card, Radius::Lg, true);
            let pad = m.gap(Space::Lg);
            let title = m.size(TypeRole::Title);
            let name = fit(c, title, s.name, TextStyle::Bold, pw - 2.0 * pad);
            c.text(x + pad, y + pad, title, th.text, &name, TextStyle::Bold);
            let body = m.size(TypeRole::Body);
            let st = fit(c, body, &format!("Status: {}", s.status), TextStyle::Regular, pw - 2.0 * pad);
            c.text(x + pad, y + pad + m.leading(title), body, th.text, &st, TextStyle::Regular);
            let note = fit(c, body, s.note, TextStyle::Regular, pw - 2.0 * pad);
            c.text(x + pad, y + pad + m.leading(title) + m.leading(body), body, th.muted, &note, TextStyle::Regular);
        }
    }
}
