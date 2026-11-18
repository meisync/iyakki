//! Which rows of a list are on the page: the one rule for every list of clickable rows that does not fit its room.
//!
//! The bug it ends: a page that recomputed its first visible row from the SELECTED row on every draw moved the whole list when a
//! row was clicked (the click changes the selection, the "selected row must be visible" rule re-ran from the top), so the row
//! under the pointer slid away and the next click landed on the wrong one. The rule here, the one the radio list always had:
//!
//! - a click selects without moving the list ([`ListWindow::click_select`]);
//! - a key that moves the choice scrolls only as far as needed to show it ([`ListWindow::key_select`]; [`ListWindow::layout`]
//!   notices a changed selection by itself, so a page needs no call for keys);
//! - the wheel scrolls the list, not the choice ([`ListWindow::wheel_by`]);
//! - rows that shrink away or a window that grows pull the list back so no empty room is left under the last row.
//!
//! Rows may differ in height: the caller gives one height per row (the step to the next row, gap included) and the room they
//! must fit in. Nothing is drawn here; the page draws rows `top()..` while they fit.

use crate::safeclamp::SafeClamp;

/// How many rows one wheel notch moves.
pub const ROWS_PER_NOTCH: i32 = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListWindow {
    top: usize,
    /// The selection the window last accounted for: a different one at the next [`ListWindow::layout`] means a key moved it.
    seen: usize,
    /// The highest `top` that still fills the room (set when laid out).
    most: usize,
    /// How many rows fit from `top` (set when laid out).
    fit: usize,
}

impl ListWindow {
    /// The first row on the page.
    pub fn top(&self) -> usize {
        self.top
    }

    /// How many rows were on the page at the last layout (at least one once laid out).
    pub fn fit(&self) -> usize {
        self.fit
    }

    /// A click chose `sel` where it is: the list stays put.
    pub fn click_select(&mut self, sel: usize) {
        self.seen = sel;
    }

    /// A key moved the choice to `sel`: scroll up to it, or down just far enough that it fits whole in `room`.
    pub fn key_select(&mut self, sel: usize, heights: &[f32], room: f32) {
        self.seen = sel;
        if heights.is_empty() {
            return;
        }
        let sel = sel.min(heights.len() - 1);
        if sel < self.top {
            self.top = sel;
        }
        while self.top < sel && heights[self.top..=sel].iter().sum::<f32>() > room {
            self.top += 1;
        }
    }

    /// The wheel: `rows` rows down (negative: up). The choice stays where it is. Uses the limit of the last layout.
    pub fn wheel_by(&mut self, rows: i32) {
        self.top = (self.top as i64 + rows as i64).clamp_safe(0, self.most as i64) as usize;
    }

    /// Settle the window for this draw and give the first row to show. Call it every draw with the current selection, one
    /// height per row and the room the rows share.
    pub fn layout(&mut self, sel: usize, heights: &[f32], room: f32) -> usize {
        if heights.is_empty() {
            *self = ListWindow { seen: sel, ..Default::default() };
            return 0;
        }
        if sel != self.seen {
            self.key_select(sel, heights, room);
        }
        // the highest top that still fills the room: the longest run of rows at the end that fits (the last row alone if none does)
        let (mut most, mut used) = (heights.len(), 0.0f32);
        while most > 0 && used + heights[most - 1] <= room {
            used += heights[most - 1];
            most -= 1;
        }
        let most = most.min(heights.len() - 1);
        self.most = most;
        self.top = self.top.min(most);
        let (mut fit, mut used) = (0usize, 0.0f32);
        for h in &heights[self.top..] {
            if fit > 0 && used + h > room {
                break;
            }
            used += h;
            fit += 1;
        }
        self.fit = fit.max(1);
        self.top
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(n: usize) -> Vec<f32> {
        vec![64.0; n]
    }

    #[test]
    fn everything_fits_so_nothing_scrolls() {
        let mut w = ListWindow::default();
        assert_eq!(w.layout(0, &rows(5), 400.0), 0);
        assert_eq!(w.fit(), 5);
        w.wheel_by(3);
        assert_eq!(w.layout(0, &rows(5), 400.0), 0);
        w.key_select(4, &rows(5), 400.0);
        assert_eq!(w.top(), 0);
    }

    #[test]
    fn a_click_near_the_bottom_does_not_move_the_rows_above() {
        let h = rows(10);
        let mut w = ListWindow::default();
        w.layout(0, &h, 64.0 * 4.0);
        assert_eq!((w.top(), w.fit()), (0, 4));
        // the user clicks the fourth visible row; the page sets the selection and tells the window
        w.click_select(3);
        assert_eq!(w.layout(3, &h, 64.0 * 4.0), 0, "the row under the pointer stays under it");
        // and the next click on the row that is now drawn there lands where it looks
        w.click_select(2);
        assert_eq!(w.layout(2, &h, 64.0 * 4.0), 0);
    }

    #[test]
    fn a_click_after_scrolling_keeps_the_scrolled_rows() {
        let h = rows(10);
        let mut w = ListWindow::default();
        w.layout(0, &h, 64.0 * 4.0);
        w.wheel_by(3);
        assert_eq!(w.layout(0, &h, 64.0 * 4.0), 3, "the wheel moved the list, not the choice");
        w.click_select(5);
        assert_eq!(w.layout(5, &h, 64.0 * 4.0), 3);
    }

    #[test]
    fn a_key_scrolls_only_as_needed() {
        let h = rows(10);
        let room = 64.0 * 4.0;
        let mut w = ListWindow::default();
        w.layout(0, &h, room);
        for sel in 1..=3 {
            assert_eq!(w.layout(sel, &h, room), 0, "still on the page");
        }
        assert_eq!(w.layout(4, &h, room), 1, "one row, no more");
        assert_eq!(w.layout(5, &h, room), 2);
        // down to the end and past it (the selection stops at the last row)
        assert_eq!(w.layout(9, &h, room), 6);
        assert_eq!(w.layout(9, &h, room), 6);
        // back up: the list stays until the choice leaves the top
        assert_eq!(w.layout(8, &h, room), 6);
        assert_eq!(w.layout(6, &h, room), 6);
        assert_eq!(w.layout(5, &h, room), 5);
    }

    #[test]
    fn a_key_after_a_wheel_scrolls_to_the_choice() {
        let h = rows(10);
        let room = 64.0 * 4.0;
        let mut w = ListWindow::default();
        w.layout(0, &h, room);
        w.wheel_by(6);
        assert_eq!(w.layout(0, &h, room), 6);
        assert_eq!(w.layout(1, &h, room), 1, "the choice moved by a key: show it");
    }

    #[test]
    fn the_wheel_stops_at_both_ends() {
        let h = rows(10);
        let room = 64.0 * 4.0;
        let mut w = ListWindow::default();
        w.layout(0, &h, room);
        w.wheel_by(-5);
        assert_eq!(w.top(), 0);
        w.wheel_by(100);
        assert_eq!(w.layout(0, &h, room), 6, "the last row ends at the bottom of the room");
        w.wheel_by(-ROWS_PER_NOTCH);
        assert_eq!(w.top(), 3);
    }

    #[test]
    fn rows_of_different_heights() {
        // the slider row (index 1) is taller than the others
        let h = vec![64.0, 90.0, 64.0, 64.0, 64.0, 64.0];
        let room = 200.0;
        let mut w = ListWindow::default();
        assert_eq!(w.layout(0, &h, room), 0);
        assert_eq!(w.fit(), 2, "64 + 90 fit, a third row does not (218 > 200)");
        assert_eq!(w.layout(3, &h, room), 2, "rows 2 and 3 are 128 tall: row 3 fits from 1 only if 90+64+64 <= 200, it does not");
        w.wheel_by(100);
        assert_eq!(w.layout(3, &h, room), 3, "the last three rows are 192 tall and fill the room");
    }

    #[test]
    fn a_window_that_grows_pulls_the_list_back() {
        let h = rows(10);
        let mut w = ListWindow::default();
        w.layout(0, &h, 64.0 * 4.0);
        assert_eq!(w.layout(9, &h, 64.0 * 4.0), 6);
        // twice the room: no empty room under the last row
        assert_eq!(w.layout(9, &h, 64.0 * 8.0), 2);
        assert_eq!(w.fit(), 8);
        // everything fits
        assert_eq!(w.layout(9, &h, 64.0 * 12.0), 0);
    }

    #[test]
    fn a_window_that_shrinks_keeps_the_choice_in_view_on_the_next_key_only() {
        let h = rows(10);
        let mut w = ListWindow::default();
        w.layout(0, &h, 64.0 * 6.0);
        w.click_select(4);
        assert_eq!(w.layout(4, &h, 64.0 * 6.0), 0);
        // smaller: a click does not move the list even if the choice is now below the page
        assert_eq!(w.layout(4, &h, 64.0 * 3.0), 0);
        // a key shows it
        assert_eq!(w.layout(5, &h, 64.0 * 3.0), 3);
    }

    #[test]
    fn rows_removed_clamp_the_top() {
        let mut w = ListWindow::default();
        w.layout(0, &rows(10), 64.0 * 4.0);
        assert_eq!(w.layout(9, &rows(10), 64.0 * 4.0), 6);
        assert_eq!(w.layout(9, &rows(7), 64.0 * 4.0), 3);
        assert_eq!(w.layout(2, &rows(3), 64.0 * 4.0), 0);
        assert_eq!(w.layout(0, &[], 64.0 * 4.0), 0);
        assert_eq!(w.top(), 0);
        // and a selection past the end (the page clamps it, the window must not panic)
        assert_eq!(w.layout(9, &rows(2), 64.0), 1);
    }

    #[test]
    fn a_room_smaller_than_one_row_still_shows_one() {
        let h = rows(4);
        let mut w = ListWindow::default();
        assert_eq!(w.layout(0, &h, 10.0), 0);
        assert_eq!(w.fit(), 1);
        assert_eq!(w.layout(2, &h, 10.0), 2);
        assert_eq!(w.fit(), 1);
        w.wheel_by(100);
        assert_eq!(w.top(), 3);
    }
}
