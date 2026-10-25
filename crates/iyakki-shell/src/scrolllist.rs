//! One scrolling list, the same everywhere: the mouse wheel, the trackpad's two-finger scroll (the touchpad driver turns it into the
//! same [`Event::Scroll`], with the "natural scrolling" choice of Settings -> Keyboard already applied), a scroll bar that can be
//! clicked and dragged, and PageUp / PageDown / Home / End.
//!
//! Why it exists: the old Quick install page listened to keys only, so a wheel or a trackpad did nothing there. A list that keeps a
//! [`ScrollList`] gets all of it by calling four things: [`ScrollList::layout`] while drawing, [`ScrollList::draw_bar`] to show and
//! register the bar, [`ScrollList::event`] for the pointer events and [`ScrollList::key`] for the keys.
//!
//! `Event::Scroll` carries no position, so "over the list" is decided from the last pointer position the screen was told
//! (`MouseMove`): a wheel with the pointer outside the list's rectangle is not taken. Without any known position it is taken.

use crate::safeclamp::SafeClamp;
use crate::gfx::{rgb, Gfx};
use crate::ui::{Event, Key, R};

/// How many rows one wheel notch moves.
pub const ROWS_PER_NOTCH: f32 = 3.0;

#[derive(Clone, Debug, Default)]
pub struct ScrollList {
    /// How far the content is scrolled (pixels from its top).
    pub offset: f32,
    /// The most it can be scrolled (content height minus the view's); 0 when everything fits.
    pub max: f32,
    /// Where the list is drawn (the wheel only counts over it).
    pub view: R,
    /// One row, in pixels: a wheel notch moves [`ROWS_PER_NOTCH`] of them.
    pub row: f32,
    /// The scroll bar's track (empty when nothing scrolls).
    pub track: R,
    /// While the thumb is dragged: how far below the thumb's top the pointer grabbed it.
    grab: Option<f32>,
    /// The last pointer position seen.
    pub pointer: Option<(i32, i32)>,
}

/// The hit id the bar registers is the caller's; the width of the clickable strip (wider than the drawn bar, for fingers).
const STRIP: f32 = 16.0;

impl ScrollList {
    /// Tell the list where it is drawn and how tall its content is (call while drawing, before [`ScrollList::draw_bar`]).
    pub fn layout(&mut self, view: R, content_h: f32, row: f32, s: f32) {
        self.view = view;
        self.row = row.max(1.0);
        self.max = (content_h - view.h).max(0.0);
        self.offset = self.offset.clamp_safe(0.0, self.max);
        self.track = if self.max > 0.0 { R::new(view.x + view.w - STRIP * s, view.y, STRIP * s, view.h) } else { R::new(view.x + view.w, view.y, 0.0, 0.0) };
    }

    pub fn scrolls(&self) -> bool {
        self.max > 0.0
    }

    pub fn by(&mut self, px: f32) {
        self.offset = (self.offset + px).clamp_safe(0.0, self.max);
    }

    pub fn to_top(&mut self) {
        self.offset = 0.0;
    }

    /// The wheel / trackpad: `d` > 0 is up (towards the start). True when taken.
    pub fn wheel(&mut self, d: i32) -> bool {
        if self.max <= 0.0 {
            return false;
        }
        if let Some((x, y)) = self.pointer {
            if !self.view.contains(x, y) {
                return false;
            }
        }
        self.by(-(d as f32) * self.row * ROWS_PER_NOTCH);
        true
    }

    /// A smooth scroll step of `px` pixels of a nominal screen ([`crate::scrolleng::NOTCH_PX`] is one wheel click, which moves
    /// [`ROWS_PER_NOTCH`] rows here); positive = up. Taken under the same rules as [`ScrollList::wheel`]. When the list is at the end
    /// the step pushes towards, the scroll engine is told, so a trackpad's momentum stops there.
    pub fn wheel_px(&mut self, px: i32) -> bool {
        if self.max <= 0.0 {
            return false;
        }
        if let Some((x, y)) = self.pointer {
            if !self.view.contains(x, y) {
                return false;
            }
        }
        let before = self.offset;
        self.by(-(px as f32) / crate::scrolleng::NOTCH_PX * self.row * ROWS_PER_NOTCH);
        if self.offset == before {
            crate::scrolleng::note_bound();
        }
        true
    }

    /// PageUp / PageDown (a screen less a row), Home / End. True when the key was used.
    pub fn key(&mut self, k: &Key) -> bool {
        if self.max <= 0.0 {
            return false;
        }
        let page = (self.view.h - self.row).max(self.row);
        match k {
            Key::PageDown => self.by(page),
            Key::PageUp => self.by(-page),
            Key::Home => self.offset = 0.0,
            Key::End => self.offset = self.max,
            _ => return false,
        }
        true
    }

    /// Keep the rows between `top` and `bottom` (content coordinates) in view.
    pub fn show(&mut self, top: f32, bottom: f32) {
        if top < self.offset {
            self.offset = top.max(0.0);
        } else if bottom > self.offset + self.view.h {
            self.offset = (bottom - self.view.h).clamp_safe(0.0, self.max);
        }
        self.offset = self.offset.clamp_safe(0.0, self.max);
    }

    /// The thumb: (top, height) in screen pixels.
    pub fn thumb(&self) -> (f32, f32) {
        let h = self.track.h;
        if self.max <= 0.0 || h <= 0.0 {
            return (self.track.y, 0.0);
        }
        let total = self.view.h + self.max;
        let th = (h * self.view.h / total).max(24.0).min(h);
        (self.track.y + (h - th) * (self.offset / self.max), th)
    }

    /// Draw the track and thumb and register the clickable strip as `Event::Area(id)` (the shell turns it into `AreaAt(id, x, y)`).
    pub fn draw_bar(&self, g: &mut Gfx, s: f32, id: usize, accent: [u8; 4]) {
        if self.max <= 0.0 {
            return;
        }
        let (ty, th) = self.thumb();
        let w = 4.0 * s;
        let x = self.track.x + self.track.w - w - 1.0 * s;
        g.rrect(x, self.track.y, w, self.track.h, w / 2.0, rgb(24, 30, 54));
        g.rrect(x, ty, w, th, w / 2.0, crate::fx::with_alpha(accent, if self.grab.is_some() { 255 } else { 190 }));
        g.hit(self.track, Event::Area(id), "scroll bar");
    }

    /// A press on the strip at `y` pixels from the track's top: on the thumb it is grabbed, elsewhere the thumb jumps there
    /// (centred on the press) and is grabbed too.
    pub fn bar_down(&mut self, y: f32) {
        let (ty, th) = self.thumb();
        let abs = self.track.y + y;
        if abs >= ty && abs <= ty + th {
            self.grab = Some(abs - ty);
        } else {
            self.grab = Some(th / 2.0);
            self.drag_to(abs as i32);
        }
    }

    /// The pointer moved to `y` (screen) while the thumb is held.
    pub fn drag_to(&mut self, y: i32) {
        let Some(grab) = self.grab else { return };
        let (_, th) = self.thumb();
        let room = (self.track.h - th).max(1.0);
        let top = (y as f32 - grab - self.track.y).clamp_safe(0.0, room);
        self.offset = (top / room * self.max).clamp_safe(0.0, self.max);
    }

    pub fn release(&mut self) {
        self.grab = None;
    }

    pub fn dragging(&self) -> bool {
        self.grab.is_some()
    }

    /// The pointer events of a list: remembers the pointer, drags the thumb, takes the wheel. True when the event was used.
    pub fn event(&mut self, ev: &Event, bar_id: usize) -> bool {
        match ev {
            Event::MouseMove(x, y) => {
                self.pointer = Some((*x, *y));
                if self.grab.is_some() {
                    self.drag_to(*y);
                    return true;
                }
                false
            }
            Event::MouseUp(..) => std::mem::take(&mut self.grab).is_some(),
            Event::AreaAt(id, _, y) if *id == bar_id => {
                self.bar_down(*y);
                true
            }
            Event::Scroll(d) => self.wheel(*d),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> ScrollList {
        let mut l = ScrollList::default();
        l.layout(R::new(10.0, 100.0, 400.0, 300.0), 1000.0, 30.0, 1.0);
        l
    }

    #[test]
    fn the_wheel_moves_three_rows_a_notch_and_only_over_the_list() {
        let mut l = list();
        assert!(l.scrolls() && (l.max - 700.0).abs() < 0.01);
        l.pointer = Some((50, 150));
        assert!(l.wheel(-1));
        assert!((l.offset - 90.0).abs() < 0.01, "three rows of 30 down: {}", l.offset);
        assert!(l.wheel(1) && l.offset.abs() < 0.01, "and back up");
        l.pointer = Some((50, 20));
        assert!(!l.wheel(-1) && l.offset == 0.0, "outside the list: not taken");
        l.pointer = None;
        assert!(l.wheel(-2) && l.offset > 0.0, "no known pointer: taken");
        assert!(l.wheel(-1000) && (l.offset - l.max).abs() < 0.01, "clamped at the end");
        // pixel steps: one notch's worth in small steps is the same distance, the same rules apply
        let mut p = list();
        p.pointer = Some((50, 150));
        for _ in 0..9 {
            assert!(p.wheel_px(-6)); // 54 px of nominal travel = one notch = 90 px of the list
        }
        assert!((p.offset - 90.0).abs() < 0.01, "{}", p.offset);
        p.pointer = Some((50, 20));
        assert!(!p.wheel_px(-6), "outside the list: not taken");
        p.pointer = None;
        assert!(p.wheel_px(100000) && p.offset == 0.0, "clamped at the start, never negative");
    }

    #[test]
    fn a_list_that_fits_takes_nothing() {
        let mut l = ScrollList::default();
        l.layout(R::new(0.0, 0.0, 100.0, 300.0), 200.0, 30.0, 1.0);
        assert!(!l.scrolls() && !l.wheel(-1) && !l.key(&Key::PageDown));
    }

    #[test]
    fn keys_move_a_page_and_the_ends() {
        let mut l = list();
        assert!(l.key(&Key::PageDown) && (l.offset - 270.0).abs() < 0.01);
        assert!(l.key(&Key::End) && l.offset == l.max);
        assert!(l.key(&Key::PageUp) && (l.offset - 430.0).abs() < 0.01);
        assert!(l.key(&Key::Home) && l.offset == 0.0);
        assert!(!l.key(&Key::Enter));
    }

    #[test]
    fn the_thumb_is_grabbed_dragged_and_a_press_on_the_track_jumps() {
        let mut l = list();
        let (ty, th) = l.thumb();
        assert!(th > 0.0 && th < l.track.h && ty == l.track.y);
        // press on the thumb, drag to the bottom
        assert!(l.event(&Event::AreaAt(9, 5.0, 5.0), 9) && l.dragging());
        assert!(l.event(&Event::MouseMove(300, 10_000), 9));
        assert!((l.offset - l.max).abs() < 0.01, "dragged to the end: {}", l.offset);
        assert!(l.event(&Event::MouseUp(300, 10_000), 9) && !l.dragging());
        // a press low on the track jumps there
        l.offset = 0.0;
        assert!(l.event(&Event::AreaAt(9, 5.0, l.track.h * 0.5), 9));
        l.release();
        assert!(l.offset > l.max * 0.3 && l.offset < l.max * 0.7, "{}", l.offset);
        assert!(!l.event(&Event::AreaAt(8, 5.0, 5.0), 9), "another area is not ours");
    }

    #[test]
    fn show_keeps_a_row_in_view() {
        let mut l = list();
        l.show(500.0, 540.0);
        assert!(l.offset + l.view.h >= 540.0 && l.offset <= 500.0);
        l.show(0.0, 30.0);
        assert_eq!(l.offset, 0.0);
    }
}
