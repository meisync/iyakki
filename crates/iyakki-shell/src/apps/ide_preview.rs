//! The AI IDE's docked Preview (F-IDE-25): Run on a tile app shows the app beside the editor, in the right part of the main area, instead
//! of opening it full screen. The embedding itself is [`super::embed`]; this file is the IDE's side: the header with a Pop out and a Close
//! button, the split of the main area, and the events.
//!
//! The preview needs room: in a compact window (under 1100 px) and when the editor would drop under its minimum width Run opens the app
//! full screen as before. Esc in the preview gives the keyboard back to the editor; a click in it takes the keyboard.

use super::embed::{self, Embedded, Out};
use super::ide::*;
use super::ide_layout as lay;
use super::AppCx;
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, DIM, MUTED, R, TEXT};

/// The ids of the preview header's two buttons (below [`embed::BASE`]).
pub const POP_OUT: usize = embed::BASE - 1;
pub const CLOSE: usize = embed::BASE - 2;

/// The width the preview takes of the main area, or `None` when there is no room (the editor keeps `MIN_MAIN`).
pub fn split(main: R, s: f32) -> Option<(R, R)> {
    let want = (main.w * 0.45).max(300.0 * s);
    let rest = main.w - want;
    (rest >= lay::MIN_MAIN * s).then(|| (R::new(main.x, main.y, rest, main.h), R::new(main.x + rest, main.y, want, main.h)))
}

impl IdeApp {
    /// Is there room to dock a preview in the layout drawn last?
    pub fn can_dock(&self) -> bool {
        self.preview_room
    }

    pub fn has_preview(&self) -> bool {
        self.preview.is_some()
    }

    /// Show `app` beside the editor.
    pub fn dock_preview(&mut self, app: Box<dyn super::App>) {
        self.preview = Some(Embedded::new(app));
        self.redraw = true;
    }

    pub fn close_preview(&mut self) {
        self.preview = None;
        self.redraw = true;
    }

    /// Carve the preview out of the main area of `l` (before the panes are drawn). The rectangle is kept for the drawing.
    pub(super) fn carve_preview(&mut self, l: &mut lay::Layout, s: f32) {
        self.preview_area = None;
        self.preview_room = !l.compact && split(l.main, s).is_some();
        if self.preview.is_none() || l.compact {
            return;
        }
        if let Some((ed, pv)) = split(l.main, s) {
            l.main = ed;
            self.preview_area = Some(pv);
        }
    }

    pub(super) fn draw_preview(&mut self, g: &mut Gfx, s: f32, cx: &AppCx, tick: u64, accent: [u8; 4]) {
        let (Some(area), Some(p)) = (self.preview_area, self.preview.as_mut()) else { return };
        let head = 26.0 * s;
        g.rect(area.x, area.y, area.w, area.h, [18, 20, 26, 255]);
        g.text(area.x + 8.0 * s, area.y + 5.0 * s, 13.0 * s, if p.focused { TEXT } else { MUTED }, &format!("Preview: {}", p.title()), Face::Bold);
        g.button(area.x + area.w - 4.0 * s - 60.0 * s, area.y + 2.0 * s, 12.0 * s, "Close", accent, Event::AreaAt(CLOSE, 0.0, 0.0));
        g.button(area.x + area.w - 4.0 * s - 60.0 * s - 78.0 * s, area.y + 2.0 * s, 12.0 * s, "Pop out", accent, Event::AreaAt(POP_OUT, 0.0, 0.0));
        g.text(area.x + 8.0 * s, area.y + head - 2.0 * s, 10.0 * s, DIM, if p.focused { "Esc gives the keyboard back to the editor" } else { "click in the preview to type in it" }, Face::Sans);
        let body = R::new(area.x, area.y + head + 10.0 * s, area.w, (area.h - head - 10.0 * s).max(0.0));
        p.render(g, body, s, cx, tick);
    }

    /// The preview's share of an event. True when the event was the preview's and the editor must not see it.
    pub(super) fn preview_event(&mut self, ev: &Event, cx: &AppCx) -> bool {
        match ev {
            Event::AreaAt(CLOSE, ..) => {
                self.close_preview();
                return true;
            }
            Event::AreaAt(POP_OUT, ..) => {
                if let Some(d) = self.preview_dir.clone() {
                    self.request = Some((format!("tile-preview:{}", d.display()), String::new()));
                }
                self.close_preview();
                return true;
            }
            _ => {}
        }
        let Some(p) = self.preview.as_mut() else { return false };
        let out = p.handle(ev, cx);
        if let Some((a, b)) = p.take_request() {
            self.request = Some((a, b));
        }
        match out {
            Some(Out::Closed) => {
                self.close_preview();
                true
            }
            Some(Out::GaveKeyboard) => true,
            Some(Out::Stay) => !matches!(ev, Event::Tick),
            None => false,
        }
    }
}
