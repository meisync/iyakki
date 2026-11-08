//! Example tile: a counter with a button and a text field.
//!
//! * [`TILE_WAT`] is the verified build: hand-written WAT (`src/tile.wat`), loadable by the host
//!   runner as is. No wasm32 toolchain is needed.
//! * [`Counter`] is the same app in Rust on top of `iyakki-tile-abi`; it is tested natively and
//!   must draw the same list as the WAT. The
//!   wasm32 build of this Rust version (four exports over a static `Counter`) is NOT written or
//!   verified here: the target was not installed and nothing may be downloaded.
use iyakki_tile_abi::{measure_text, Color, DisplayList, Event, Op, Rect};

pub const TILE_WAT: &str = include_str!("tile.wat");

/// The app state and its two functions: events in, display list out.
#[derive(Default)]
pub struct Counter {
    pub count: u32,
    pub field: String,
    pub size: (i32, i32),
}

impl Counter {
    pub fn new() -> Self {
        Counter { size: (320, 200), ..Default::default() }
    }

    /// Returns true when the picture changed.
    pub fn event(&mut self, e: &Event) -> bool {
        match e {
            Event::Hit { id: 1 } => self.count += 1,
            Event::Hit { id: 2 } => self.field.clear(),
            Event::TextInput { text } => {
                for c in text.chars().filter(|c| (' '..'\u{7f}').contains(c)) {
                    if self.field.len() < 32 {
                        self.field.push(c);
                    }
                }
            }
            Event::Key { code: 8, down: true, .. } => {
                self.field.pop();
            }
            Event::Resize { w, h, .. } => self.size = (*w, *h),
            _ => return false,
        }
        true
    }

    pub fn frame(&self) -> DisplayList {
        let t = |x, y, size, c: (u8, u8, u8), text: &str| Op::Text {
            x,
            y,
            face: 0,
            size,
            color: Color::rgb(c.0, c.1, c.2),
            text: text.into(),
        };
        let btn = Rect::new(20, 70, 100, 36);
        let fld = Rect::new(20, 120, 200, 32);
        let lw = measure_text(0, 18, "+1");
        let mut l = DisplayList::new();
        l.push(Op::Rect { r: Rect::new(0, 0, self.size.0, self.size.1), color: Color::rgb(245, 245, 245) });
        l.push(t(20, 16, 16, (30, 30, 30), "TILE DEMO"));
        l.push(t(20, 44, 14, (60, 60, 60), "COUNT"));
        l.push(t(92, 44, 14, (40, 110, 220), &self.count.to_string()));
        l.push(Op::RoundRect { r: btn, radius: 8, color: Color::rgb(40, 110, 220) });
        l.push(t(20 + (100 - lw) / 2, 78, 18, (255, 255, 255), "+1"));
        l.push(Op::Hit { id: 1, r: btn });
        l.push(Op::Outline { r: fld, thickness: 1, color: Color::rgb(120, 120, 120) });
        l.push(t(28, 128, 16, (20, 20, 20), &self.field));
        l.push(Op::Hit { id: 2, r: fld });
        l
    }
}
