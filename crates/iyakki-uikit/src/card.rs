//! A dialog with a block of text that scrolls and a row of buttons that always stays inside the panel, at any window size. The tile
//! app's info and remove panels and the IDE's publish review and result use it, so what they show and where the buttons sit is one
//! piece of code with one geometry test.

use crate::fx;
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, Key, R, DIM, ERR, MUTED, OK, TEXT};

/// `Event::Tab(CARD_BASE + n)` is a click on button `n`; wheel and arrow keys scroll the text.
pub const CARD_BASE: usize = 7300;

#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub label: String,
    pub key: Key,
    pub color: [u8; 4],
}

impl Action {
    pub fn new(label: &str, key: Key, color: [u8; 4]) -> Action {
        Action { label: label.into(), key, color }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tone {
    Normal,
    Dim,
    Good,
    Warn,
    Bad,
    Bold,
}

#[derive(Clone, Debug)]
pub struct Card {
    pub title: String,
    pub lines: Vec<(String, Tone)>,
    pub actions: Vec<Action>,
    pub accent: [u8; 4],
    pub danger: bool,
    scroll: usize,
    /// Where the last frame put the panel and the buttons (tests).
    pub last_panel: R,
    pub last_buttons: Vec<R>,
}

impl Card {
    pub fn new(title: &str, accent: [u8; 4]) -> Card {
        Card { title: title.into(), lines: vec![], actions: vec![], accent, danger: false, scroll: 0, last_panel: R::new(0.0, 0.0, 0.0, 0.0), last_buttons: vec![] }
    }

    pub fn line(mut self, t: &str, tone: Tone) -> Card {
        self.lines.push((t.into(), tone));
        self
    }

    pub fn text(mut self, block: &str) -> Card {
        for l in block.lines() {
            self.lines.push((l.into(), Tone::Normal));
        }
        self
    }

    pub fn action(mut self, a: Action) -> Card {
        self.actions.push(a);
        self
    }

    pub fn danger(mut self) -> Card {
        self.danger = true;
        self
    }

    /// The key of the button this event chose (Esc for the last one when Esc is pressed and no button has it); `None` while scrolling.
    pub fn handle(&mut self, ev: &Event) -> Option<Key> {
        match ev {
            Event::Tab(n) if (CARD_BASE..CARD_BASE + self.actions.len()).contains(n) => Some(self.actions[n - CARD_BASE].key.clone()),
            Event::Scroll(d) => {
                self.scroll = (self.scroll as i32 - d * 3).max(0) as usize;
                None
            }
            Event::Key(Key::Down) => {
                self.scroll += 1;
                None
            }
            Event::Key(Key::Up) => {
                self.scroll = self.scroll.saturating_sub(1);
                None
            }
            Event::Key(Key::PageDown) => {
                self.scroll += 8;
                None
            }
            Event::Key(Key::PageUp) => {
                self.scroll = self.scroll.saturating_sub(8);
                None
            }
            Event::Key(k) => {
                let k = match k {
                    Key::Char(c) => Key::Char(c.to_ascii_lowercase()),
                    other => other.clone(),
                };
                self.actions.iter().find(|a| a.key == k).map(|a| a.key.clone())
            }
            _ => None,
        }
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32) {
        let size = 14.0 * s;
        let panel_w = (680.0 * s).min(area.w - 24.0 * s).max(200.0);
        let inner_w = panel_w - 52.0 * s;
        // the buttons first: how many rows they need at this width
        let bh = size * 2.1;
        let gap = 10.0 * s;
        let widths: Vec<f32> = self.actions.iter().map(|a| g.text_width(size, &a.label, Face::Bold) + size * 1.7).collect();
        let mut rows = 1usize;
        let mut x = 0.0;
        for w in &widths {
            if x > 0.0 && x + w > inner_w {
                rows += 1;
                x = 0.0;
            }
            x += w + gap;
        }
        let btn_h = rows as f32 * (bh + 8.0 * s);
        let mut wrapped: Vec<(String, Tone)> = Vec::new();
        for (t, tone) in &self.lines {
            if t.is_empty() {
                wrapped.push((String::new(), tone.clone()));
            }
            for l in g.wrap(size, t, if *tone == Tone::Bold { Face::Bold } else { Face::Sans }, inner_w) {
                wrapped.push((l, tone.clone()));
            }
        }
        let lh = size * 1.45;
        let want_h = 66.0 * s + wrapped.len() as f32 * lh + btn_h + 30.0 * s;
        let panel_h = want_h.min(area.h - 16.0 * s).max(120.0);
        let p = fx::modal(g, area, s, panel_w, panel_h, self.accent, self.danger);
        self.last_panel = p;
        fx::modal_title(g, p, s, &self.title, if self.danger { ERR } else { self.accent });
        let text_top = p.y + 62.0 * s;
        let text_h = (p.h - 62.0 * s - btn_h - 20.0 * s).max(lh);
        let fit = ((text_h / lh).floor() as usize).max(1);
        let max_scroll = wrapped.len().saturating_sub(fit);
        self.scroll = self.scroll.min(max_scroll);
        for (i, (l, tone)) in wrapped.iter().skip(self.scroll).take(fit).enumerate() {
            let (face, col) = match tone {
                Tone::Normal => (Face::Sans, MUTED),
                Tone::Dim => (Face::Sans, DIM),
                Tone::Good => (Face::Sans, OK),
                Tone::Warn => (Face::Sans, crate::gfx::rgb(255, 196, 94)),
                Tone::Bad => (Face::Sans, ERR),
                Tone::Bold => (Face::Bold, TEXT),
            };
            g.text(p.x + 26.0 * s, text_top + i as f32 * lh, size, col, l, face);
        }
        if max_scroll > 0 {
            let track = R::new(p.x + p.w - 14.0 * s, text_top, 4.0 * s, text_h);
            g.rrect(track.x, track.y, track.w, track.h, 2.0 * s, [255, 255, 255, 24]);
            let th = (track.h * fit as f32 / wrapped.len() as f32).max(14.0 * s);
            let ty = track.y + (track.h - th) * self.scroll as f32 / max_scroll as f32;
            g.rrect(track.x, ty, track.w, th, 2.0 * s, [255, 255, 255, 110]);
        }
        // buttons from the left, wrapping inside the panel
        self.last_buttons.clear();
        let by0 = p.y + p.h - btn_h - 12.0 * s;
        let (mut x, mut row) = (0.0, 0usize);
        for (i, (a, w)) in self.actions.iter().zip(&widths).enumerate() {
            if x > 0.0 && x + w > inner_w {
                row += 1;
                x = 0.0;
            }
            let (bx, by) = (p.x + 26.0 * s + x, by0 + row as f32 * (bh + 8.0 * s));
            g.button(bx, by, size, &a.label, a.color, Event::Tab(CARD_BASE + i));
            self.last_buttons.push(R::new(bx, by, *w, bh));
            x += w + gap;
        }
    }
}
