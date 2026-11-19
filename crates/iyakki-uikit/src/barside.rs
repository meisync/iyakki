//! Which edge the bar sits on, for the panels that hang from it (F-LAUNCH-24). The top bar is at the top by default; a Look may put it at the
//! bottom (`topbar_position = bottom`). The popovers that open from it (Devices, the IYAKKI menu, the installer list, the battery, the volume
//! dropdown, the clipboard list) were all written to open *below* their anchor. Each now asks this module: with the bar at the bottom a panel
//! opens *above* its anchor, with the same size rules and kept on screen. With the bar at the top (the default) nothing here changes a pixel.

use std::cell::Cell;

/// The height of the bar's band at the base scale (the name box, the buttons and their margins).
pub const BAR_BAND: f32 = 76.0;

thread_local! {
    static BOTTOM: Cell<bool> = const { Cell::new(false) };
    static DY: Cell<f32> = const { Cell::new(0.0) };
}

/// Set once a frame by the shell from the person's Look (tests of the panels set it directly).
pub fn set_bar_at_bottom(on: bool) {
    BOTTOM.with(|c| c.set(on));
    if !on {
        DY.with(|c| c.set(0.0));
    }
}

pub fn bar_at_bottom() -> bool {
    BOTTOM.with(Cell::get)
}

/// How far down the bar's band is from its place at the top (0 with the bar at the top): the layout origin of the bar. The shell sets it
/// with the screen's size once a frame, on the home screen only (an app keeps its own header at the top).
pub fn set_bar_dy(dy: f32) {
    DY.with(|c| c.set(dy.max(0.0)));
    BOTTOM.with(|c| c.set(dy > 0.0));
}

pub fn bar_dy() -> f32 {
    DY.with(Cell::get)
}

/// For a panel that opens from `anchor` (its top and bottom edge) on a screen `h` high: the most height it may have and where its top is when
/// it is `ph` high, when the bar is at the bottom: above the anchor, 8 units clear of it, never above 8 units from the top edge.
pub fn up_room(anchor_top: f32, s: f32) -> f32 {
    (anchor_top - 16.0 * s).max(60.0 * s)
}

pub fn up_top(anchor_top: f32, ph: f32, s: f32) -> f32 {
    (anchor_top - 8.0 * s - ph).max(8.0 * s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panel_above_its_anchor_has_room_and_a_top_that_keeps_it_on_screen() {
        assert!(!bar_at_bottom());
        set_bar_at_bottom(true);
        assert!(bar_at_bottom());
        set_bar_at_bottom(false);
        let (s, anchor_top) = (1.0, 600.0);
        let room = up_room(anchor_top, s);
        assert_eq!(room, 584.0);
        let ph = room.min(300.0);
        let top = up_top(anchor_top, ph, s);
        assert_eq!(top, 292.0);
        assert!(top + ph + 8.0 <= anchor_top, "clear of the anchor");
        // a tall panel is cut to the room and sits at the top margin
        assert_eq!(up_top(anchor_top, 900.0, s), 8.0);
        // an anchor very near the top still leaves a usable panel
        assert_eq!(up_room(20.0, s), 60.0);
    }
}
