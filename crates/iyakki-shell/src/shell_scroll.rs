//! The shell's side of scrolling: the pixel steps of the scroll engine (`scrolleng.rs`, run by the input thread) reach the open tile
//! as the finest thing it understands.
//!
//! [`Event::ScrollPx`] never goes to a tile. [`Shell::handle`] gives it to [`Shell::scroll_px`]:
//! * a tile that can scroll something by pixels under the pointer takes it through [`App::scroll_px`] (and says so);
//! * everything else (a popover, a menu, panes, a tile with only whole-line scrolling) gets the classic [`Event::Scroll`], one
//!   per [`NOTCH_PX`] of movement, the part of a notch left over kept for the next step. Nothing is lost and no tile had to change.

use super::*;
use crate::scrolleng::NOTCH_PX;

/// A part of a notch left over for longer than this many heartbeats (about half a second) is forgotten.
const REM_KEEP_TICKS: u64 = 15;

impl Shell {
    /// One pixel step of the scroll engine (positive = up, like [`Event::Scroll`]).
    pub(super) fn scroll_px(&mut self, px: i32) {
        let ev = Event::ScrollPx(px);
        if self.saver_intercept(&ev) {
            self.saver_after();
            return; // the first input only wakes the screen
        }
        self.tbar_activity();
        if self.scroll_px_direct() {
            let cx = self.cx();
            let taken = match (&mut self.screen, cx) {
                (Screen::App(app, _), Some(cx)) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.scroll_px(px, &cx))).unwrap_or(false),
                _ => false,
            };
            if taken {
                self.stale = true;
                self.saver_after();
                return;
            }
        }
        if self.tick.saturating_sub(self.scroll_rem_at) > REM_KEEP_TICKS {
            self.scroll_rem = 0.0;
        }
        self.scroll_rem_at = self.tick;
        self.scroll_rem += px as f32;
        let n = (self.scroll_rem / NOTCH_PX).round() as i32;
        if n != 0 {
            self.scroll_rem -= n as f32 * NOTCH_PX;
            self.handle(&Event::Scroll(n));
        } else {
            self.saver_after();
        }
    }

    /// Nothing is drawn over the open tile (no menu, popover, card, dialog or pane layout): it can be handed pixels.
    fn scroll_px_direct(&self) -> bool {
        matches!(self.screen, Screen::App(..)) && self.session.is_some() && !self.overlay_open() && !self.ws_active() && self.lay.menu.is_none() && self.ring.is_none()
    }

    /// Tell the input thread what the open tile needs: whether it makes its own smooth pictures (the browser's engine), so the
    /// wheel's steps are not eased twice. Called with every heartbeat.
    pub(super) fn scroll_sync(&mut self) {
        let raw = match &self.screen {
            Screen::App(app, _) => app.scroll_raw(),
            _ => false,
        };
        crate::scrolleng::set_raw(raw);
    }
}
