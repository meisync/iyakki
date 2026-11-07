// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where things go when the menu is drawn BELOW the logo (the splash stays on screen, the list sits under it on the same
//! background), and how the items split into the short first level and the "More options" second level. Pure arithmetic and
//! lists: no drawing, no firmware.
//!
//! The plan works for any pixel size from 640 x 480 to 4K: text and rows grow with the screen height, the logo shrinks (never
//! grows) so that at least `MIN_ROWS` rows fit under it, and the countdown line and the key hint sit under the list.

use crate::item::Item;
use crate::menu::{Layout, Row};
use crate::splash::fit_dims;
use alloc::string::String;
use alloc::vec::Vec;

/// Rows that must stay visible under the logo.
pub const MIN_ROWS: u32 = 3;
/// `Row::item` of the "More options" row on the first level.
pub const MORE_ROW: usize = usize::MAX;
/// `Row::item` of the "Back" row on the second level.
pub const BACK_ROW: usize = usize::MAX - 1;

/// Placement of the logo, the list and the two text lines under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Pixels per font pixel (the built-in font is 8 x 8).
    pub text_scale: u32,
    /// Height of one row (the touch target).
    pub row_h: u32,
    /// Logo size after scaling (0 x 0 when there is no logo).
    pub logo_w: u32,
    /// Logo height after scaling.
    pub logo_h: u32,
    /// Logo left edge.
    pub logo_x: u32,
    /// Logo top edge.
    pub logo_y: u32,
    /// The list: `header_h` is the top of the first row, `footer_h` what stays free below the list for the two text lines.
    pub list: Layout,
}

impl Plan {
    /// Height of one glyph.
    pub fn glyph(&self) -> u32 {
        8 * self.text_scale
    }

    /// Rows that fit.
    pub fn visible_rows(&self) -> u32 {
        self.list.height.saturating_sub(self.list.header_h + self.list.footer_h) / self.list.row_h.max(1)
    }

    /// Top of the countdown / notice line when the list shows `rows` rows: right under them, but never inside the key hint's band.
    pub fn countdown_y(&self, rows: u32) -> u32 {
        let under = self.list.header_h + rows.min(self.visible_rows()) * self.list.row_h + self.row_h / 4;
        under.min(self.hint_y().saturating_sub(self.row_h))
    }

    /// Top of the key hint line (the bottom line).
    pub fn hint_y(&self) -> u32 {
        self.list.height.saturating_sub(self.row_h)
    }
}

/// Plan the screen. `logo` is the logo's size in pixels (after any cropping), `None` for a menu without a picture.
pub fn under_logo(sw: u32, sh: u32, logo: Option<(u32, u32)>) -> Plan {
    let text_scale = (sh / 400).clamp(1, 4);
    let glyph = 8 * text_scale;
    let row_h = (glyph * 3).max(sh / 20).max(24);
    let top = sh / 24;
    // below the list: the countdown line and the hint line, one row each, plus half a row of air
    let reserve = 2 * row_h + row_h / 2;
    // the logo gets at most 40 % of the height, and never so much that fewer than MIN_ROWS rows remain
    let gap = row_h;
    let by_rows = sh.saturating_sub(top + gap + MIN_ROWS * row_h + reserve);
    let box_h = (sh * 2 / 5).min(by_rows);
    let box_w = sw * 3 / 4;
    let (logo_w, logo_h) = logo.map_or((0, 0), |(w, h)| fit_dims(w, h, box_w, box_h));
    let list_top = if logo_h > 0 { top + logo_h + gap } else { top + gap };
    // a list at least 44 characters wide (or 60 % of the screen), never wider than 90 % of it
    let list_w = (sw * 3 / 5).max(44 * glyph).min(sw * 9 / 10);
    Plan {
        text_scale,
        row_h,
        logo_w,
        logo_h,
        logo_x: (sw - logo_w) / 2,
        logo_y: top,
        list: Layout { width: sw, height: sh, header_h: list_top, footer_h: reserve, row_h, margin: (sw - list_w) / 2 },
    }
}

/// The two levels. Rows of the first level are the items that are not `more` (plus a "More options" row when the second level
/// is not empty); rows of the second level are a "Back" row and the `more` items. Hidden items are in neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Levels {
    /// First level.
    pub top: Vec<Row>,
    /// Second level.
    pub more: Vec<Row>,
}

impl Levels {
    /// Split. `more_label` and `back_label` are the texts of the two extra rows.
    pub fn split(items: &[Item], more_label: &str, back_label: &str) -> Levels {
        let all = crate::menu::rows_from_items(items);
        let (mut more, mut top): (Vec<Row>, Vec<Row>) = all.into_iter().partition(|r| items[r.item].more);
        if !more.is_empty() {
            top.push(Row { item: MORE_ROW, label: String::from(more_label), disabled: None });
            more.insert(0, Row { item: BACK_ROW, label: String::from(back_label), disabled: None });
        }
        Levels { top, more }
    }

    /// Is this item on the second level?
    pub fn on_more(&self, item: usize) -> bool {
        self.more.iter().any(|r| r.item == item)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Action;
    use alloc::string::ToString;

    const SIZES: [(u32, u32); 10] =
        [(640, 480), (800, 600), (1024, 768), (1280, 720), (1280, 1024), (1366, 768), (1920, 1080), (2560, 1440), (3440, 1440), (3840, 2160)];
    const LOGOS: [Option<(u32, u32)>; 5] = [None, Some((800, 450)), Some((300, 120)), Some((1200, 1200)), Some((4000, 90))];

    #[test]
    fn nothing_overlaps_at_any_resolution() {
        for (sw, sh) in SIZES {
            for logo in LOGOS {
                let p = under_logo(sw, sh, logo);
                let at = format!("{sw}x{sh} logo {logo:?} {p:?}");
                // the logo is inside the screen, centred, never enlarged
                assert!(p.logo_x + p.logo_w <= sw && p.logo_y + p.logo_h <= sh, "{at}");
                if let Some((w, h)) = logo {
                    assert!(p.logo_w <= w && p.logo_h <= h && p.logo_w > 0 && p.logo_h > 0, "{at}");
                    assert_eq!(p.logo_x, (sw - p.logo_w) / 2, "{at}");
                }
                // the list starts below the logo and leaves room for the two lines
                assert!(p.list.header_h >= p.logo_y + p.logo_h, "{at}");
                assert!(p.visible_rows() >= MIN_ROWS, "{at}");
                // the rows end above the hint band, and the countdown line sits right under the rows shown, the hint under it
                assert!(p.list.header_h + p.visible_rows() * p.row_h + p.row_h / 2 <= p.hint_y(), "{at}");
                for rows in [1, 2, 3, 6, 40] {
                    let c = p.countdown_y(rows);
                    assert!(c >= p.list.header_h + rows.min(p.visible_rows()) * p.row_h, "{at} rows {rows}");
                    assert!(c + p.glyph() <= p.hint_y() && p.hint_y() + p.glyph() <= sh, "{at} rows {rows}");
                }
                // the list is inside the screen
                assert!(p.list.margin * 2 < sw && p.list.margin > 0, "{at}");
            }
        }
    }

    #[test]
    fn touch_targets_are_big_enough() {
        for (sw, sh) in SIZES {
            let p = under_logo(sw, sh, Some((800, 450)));
            if sh >= 1080 {
                assert!(p.row_h >= 44, "{sw}x{sh}: row {}", p.row_h);
            }
            assert!(p.row_h >= 24 && p.row_h >= p.glyph() * 2, "{sw}x{sh}");
            // a row is wide enough for 40 characters
            assert!(sw - 2 * p.list.margin >= 40 * p.glyph().min(sw / 40), "{sw}x{sh}");
        }
        // a logo that would eat the screen is scaled down so three rows remain
        let p = under_logo(640, 480, Some((640, 480)));
        assert!(p.visible_rows() >= 3 && p.logo_h <= 480 * 2 / 5);
    }

    fn it(id: &str, more: bool, hidden: bool) -> Item {
        Item { id: id.to_string(), title: id.to_string(), hidden, order: 0, default: false, hold: None, more, disabled: None, action: Action::Linux(0) }
    }

    #[test]
    fn levels_split() {
        let items = [it("a", false, false), it("b", true, false), it("c", true, true), it("d", true, false), it("e", false, false)];
        let l = Levels::split(&items, "More options", "Back");
        assert_eq!(l.top.iter().map(|r| r.item).collect::<Vec<_>>(), [0, 4, MORE_ROW]);
        assert_eq!(l.more.iter().map(|r| r.item).collect::<Vec<_>>(), [BACK_ROW, 1, 3]);
        assert!(l.on_more(3) && !l.on_more(0) && !l.on_more(2));
        // nothing for the second level: no extra rows at all
        let l = Levels::split(&items[..1], "More options", "Back");
        assert_eq!((l.top.len(), l.more.len()), (1, 0));
    }
}
