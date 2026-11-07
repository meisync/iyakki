//! One place for "the wheel, a two-finger swipe or a finger drag scrolls this page". Every Settings page that is taller than the
//! window keeps its scroll position in a field of its own (Share internet, Remote access, Voice, the Session log); this module maps
//! the pointer's scroll events (whole notches or smooth pixels) onto them, so no page is left with only PageUp/PageDown.

use crate::safeclamp::SafeClamp;
use super::*;

impl SettingsApp {
    /// True when this page has its own scroll position (the pages whose rows are windowed or that hold a list have their own wheel rules).
    pub(super) fn page_scrolls_by_pixels(&self) -> bool {
        match self.tab() {
            Tab::Network => self.share.open,
            Tab::Remote | Tab::Voice | Tab::SessionLog => true,
            _ => false,
        }
    }

    /// Scroll the page by `px` pixels at scale 1 (positive = further down). False when this page does not scroll that way.
    pub(super) fn page_scroll_by(&mut self, px: f32) -> bool {
        let s = self.nav_s.max(0.5);
        match self.tab() {
            Tab::Network if self.share.open => self.share.scroll = (self.share.scroll + px * s).clamp_safe(0.0, self.share.max),
            Tab::Remote => self.remote_scroll_by(px * s),
            Tab::Voice => self.vo_scroll = (self.vo_scroll + px * s).clamp_safe(0.0, self.vo_max),
            Tab::SessionLog => {
                // lines, not pixels: a notch is three lines
                let lines = (px / crate::scrolleng::SCREEN_NOTCH_PX * 3.0).round() as i64;
                let last = self.crash.lines.len().saturating_sub(1) as i64;
                self.crash.scroll = (self.crash.scroll as i64 + lines).clamp_safe(0, last) as usize;
            }
            _ => return false,
        }
        true
    }

    #[cfg_attr(not(test), allow(dead_code))]
    /// Where the page is scrolled to (pixels, or lines on the Session log), for tests and for the "at an end" check.
    pub(super) fn page_scroll_pos(&self) -> f32 {
        match self.tab() {
            Tab::Network => self.share.scroll,
            Tab::Remote => self.remote.scroll,
            Tab::Voice => self.vo_scroll,
            Tab::SessionLog => self.crash.scroll as f32,
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tall page: the wheel, smooth pixels and the keys all move it, and every page that has a scroll position answers the wheel.
    #[test]
    fn the_wheel_and_smooth_scroll_move_every_page_that_scrolls() {
        let cx = crate::apps::tests::cx_with_admin_store("pagescroll");
        let mut a = SettingsApp::new(&cx);
        a.nav_s = 1.0;
        a.mouse = (900, 500); // over the page, not the page list
        a.nav_rect = R::new(0.0, 0.0, 300.0, 800.0);
        a.share.open = true;
        a.share.max = 2000.0;
        a.remote.max = 2000.0;
        a.vo_max = 2000.0;
        a.crash.lines = (0..200).map(|i| format!("line {i}")).collect();
        let mut seen = 0;
        for i in 0..a.tabs().len() {
            a.tab = i;
            if !a.page_scrolls_by_pixels() {
                continue;
            }
            seen += 1;
            let t = a.tab();
            let p0 = a.page_scroll_pos();
            a.handle(&Event::Scroll(-2), &cx); // wheel down
            let p1 = a.page_scroll_pos();
            assert!(p1 > p0, "{t:?}: wheel down did not scroll ({p0} -> {p1})");
            assert!(a.scroll_px(-120, &cx) || a.scroll_px(120, &cx), "{t:?}: smooth scroll not taken");
            a.handle(&Event::Scroll(10), &cx); // wheel up, past the top
            assert!(a.page_scroll_pos() < p1, "{t:?}: wheel up did not scroll back");
        }
        assert_eq!(seen, 4, "Share internet, Remote access, Voice and the Session log scroll by position");
    }
}
