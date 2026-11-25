//! A scrollback of coloured text lines with a selectable, copyable pane (moved from the apps crate so tile crates can use it).

use crate::gfx::{Face, Gfx};
use crate::ui::{Event, R, TEXT};

/// Scrollback of coloured text lines, wrapped to the area width. `scroll` counts visual lines
/// from the bottom; the newest content is shown by default.
///
/// The text can be selected with the pointer (press and drag, double click a word, triple click an entry, Shift+click to
/// extend) and copied; the owner passes its events to [`ScrollLog::handle`] first.
pub struct ScrollLog {
    pub lines: Vec<(String, [u8; 4])>,
    pub scroll: usize,
    /// Selection over what was drawn last.
    pub pane: crate::textsel::Pane,
    /// The line height of the last draw (the drag scroll counts in lines).
    pub lh: f32,
    /// Keep each entry on one line (cut with an ellipsis) instead of wrapping it.
    pub nowrap: bool,
}

/// The id of the area a scroll log registers for presses (`Event::Area`).
pub const LOG_AREA: usize = 61_100;

impl Default for ScrollLog {
    fn default() -> Self {
        ScrollLog { lines: Vec::new(), scroll: 0, pane: crate::textsel::Pane::new(LOG_AREA), lh: 16.0, nowrap: false }
    }
}

impl ScrollLog {
    pub fn push(&mut self, text: impl Into<String>, color: [u8; 4]) {
        self.lines.push((text.into(), color));
        self.scroll = 0;
        if self.lines.len() > 2000 {
            self.lines.drain(..500);
        }
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.scroll = 0;
        self.pane.clear();
    }

    /// Presses, drags and Ctrl+C on the text; true when the event was used.
    pub fn handle(&mut self, ev: &Event) -> bool {
        self.pane.handle(ev)
    }

    /// One heartbeat: a drag held above or below the text scrolls it.
    pub fn on_tick(&mut self) {
        let a = self.pane.auto_scroll();
        if a != 0.0 {
            self.scroll_by((a / self.lh.max(1.0)).round() as i32 + a.signum() as i32);
            self.pane.refresh_drag();
        }
    }

    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = (self.scroll as i32 + delta).max(0) as usize;
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, size: f32, face: Face) {
        let lh = g.line_height(size, face);
        let mut visual: Vec<([u8; 4], String)> = Vec::new();
        let mut entry_of: Vec<u32> = Vec::new();
        for (n, (t, c)) in self.lines.iter().enumerate() {
            if self.nowrap {
                visual.push((*c, g.fit(size, t, face, area.w)));
                entry_of.push(n as u32);
                continue;
            }
            for l in g.wrap(size, t, face, area.w) {
                visual.push((*c, l));
                entry_of.push(n as u32);
            }
        }
        let fit = (area.h / lh).floor().max(1.0) as usize;
        let max_scroll = visual.len().saturating_sub(fit);
        self.scroll = self.scroll.min(max_scroll);
        let end = visual.len() - self.scroll;
        let start = end.saturating_sub(fit);
        for (i, (c, l)) in visual[start..end].iter().enumerate() {
            g.text(area.x, area.y + i as f32 * lh, size, if *c == [0, 0, 0, 0] { TEXT } else { *c }, l, face);
        }
        // the selectable text: every line, those out of view without geometry
        self.lh = lh;
        self.pane.begin(area);
        for (i, (_, l)) in visual.iter().enumerate() {
            self.pane.line(g, area.x, area.y + (i as f32 - start as f32) * lh, lh, size, face, l, 0, entry_of[i]);
        }
        self.pane.end(g, crate::fx::CYAN);
    }
}

