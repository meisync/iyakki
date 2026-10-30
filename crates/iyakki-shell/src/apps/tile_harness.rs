//! A headless harness for Rust tiles (test builds only; docs/development/TILE-TESTING.md, UI/OS split plan step 4).
//!
//! It owns the tile and an [`AppCx`], sends events, draws into a software [`Gfx`] (no framebuffer, no window) and answers the
//! questions tile tests ask over and over: click the button called X, type text, is this text on screen, do the controls overlap
//! at the four standard sizes. New tiles should be web-bundle tiles instead (see the doc); use this for tiles that need `Gfx`.

use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Gfx};
use crate::ui::{Event, Hit, Key, R};

/// The four window sizes every tile must fit.
pub const STANDARD_SIZES: [(u32, u32); 4] = [(1280, 720), (1024, 600), (800, 480), (480, 600)];
/// The shell's header above a tile's area, in pixels at scale 1.
pub const HEADER: f32 = 72.0;

pub struct TileHarness<A: App> {
    pub app: A,
    pub cx: AppCx,
    pub size: (u32, u32),
    pub tick: u64,
    last: Option<Gfx>,
}

fn partly_overlap(a: R, b: R) -> bool {
    let inter_w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let inter_h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if inter_w <= 1.0 || inter_h <= 1.0 {
        return false; // touching or a hair of overlap from rounding
    }
    let inside = |i: R, o: R| i.x >= o.x - 1.0 && i.y >= o.y - 1.0 && i.x + i.w <= o.x + o.w + 1.0 && i.y + i.h <= o.y + o.h + 1.0;
    !(inside(a, b) || inside(b, a)) // a control inside a card is fine
}

impl<A: App> TileHarness<A> {
    /// A harness at 1280x720 around `app`, which was built from `cx` (use `apps::tests::cx_with_admin_store` and friends).
    pub fn new(app: A, cx: AppCx) -> TileHarness<A> {
        TileHarness { app, cx, size: STANDARD_SIZES[0], tick: 0, last: None }
    }

    /// Send one event; the last picture is out of date afterwards.
    pub fn send(&mut self, ev: Event) -> Flow {
        self.last = None;
        self.app.handle(&ev, &self.cx)
    }

    pub fn key(&mut self, k: Key) -> Flow {
        self.send(Event::Key(k))
    }

    /// Type characters into the focused field.
    pub fn type_text(&mut self, s: &str) {
        for c in s.chars() {
            self.key(Key::Char(c));
        }
    }

    /// Draw at the current size and keep the picture (the next call draws again only after an event or a size change).
    pub fn draw(&mut self) -> &Gfx {
        if self.last.is_none() {
            let (w, h) = self.size;
            let mut g = Gfx::new(w, h);
            g.clear(rgb(9, 11, 20));
            let s = (w as f32 / 1280.0).clamp(0.6, 3.0);
            self.app.render(&mut g, R::new(0.0, HEADER, w as f32, h as f32 - HEADER), s, &self.cx, self.tick);
            self.last = Some(g);
        }
        self.last.as_ref().unwrap()
    }

    pub fn draw_at(&mut self, w: u32, h: u32) -> &Gfx {
        if self.size != (w, h) {
            self.size = (w, h);
            self.last = None;
        }
        self.draw()
    }

    /// The clickable regions of the current picture.
    pub fn hits(&mut self) -> Vec<Hit> {
        self.draw().hits.clone()
    }

    pub fn has_button(&mut self, label: &str) -> bool {
        self.draw().hits.iter().any(|h| h.label == label)
    }

    /// Click the (topmost) control called `label`: the event it registered is sent, as the shell would after a mouse click.
    /// Panics with the labels that exist when there is none.
    pub fn click_label(&mut self, label: &str) -> Flow {
        let hits = self.hits();
        let Some(hit) = hits.iter().rev().find(|h| h.label == label) else {
            let names: Vec<&str> = hits.iter().map(|h| h.label.as_str()).filter(|l| !l.is_empty()).collect();
            panic!("no control called {label:?} at {:?}; there is {names:?}", self.size);
        };
        self.send(hit.ev.clone())
    }

    /// Is `needle` part of any text the last picture drew (case-sensitive)?
    pub fn shows(&mut self, needle: &str) -> bool {
        self.draw().text_log.iter().any(|t| t.2.contains(needle))
    }

    /// Everything the last picture drew as text, in drawing order.
    pub fn texts(&mut self) -> Vec<String> {
        self.draw().text_log.iter().map(|t| t.2.clone()).collect()
    }

    /// Draw at each standard size and fail naming the size and the two controls when two clickable regions partly overlap
    /// (one inside the other is fine). Leaves the harness at the first size.
    pub fn assert_no_overlap_at_standard_sizes(&mut self) {
        for (w, h) in STANDARD_SIZES {
            let hits = self.draw_at(w, h).hits.clone();
            for (i, a) in hits.iter().enumerate() {
                for b in &hits[i + 1..] {
                    assert!(!partly_overlap(a.rect, b.rect), "{w}x{h}: {:?} {:?} overlaps {:?} {:?}", a.label, a.rect, b.label, b.rect);
                }
            }
        }
        self.draw_at(STANDARD_SIZES[0].0, STANDARD_SIZES[0].1);
    }

    /// Draw at each standard size (no assertion beyond not panicking).
    pub fn draw_at_standard_sizes(&mut self) {
        for (w, h) in STANDARD_SIZES {
            self.draw_at(w, h);
        }
        self.draw_at(STANDARD_SIZES[0].0, STANDARD_SIZES[0].1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;
    use crate::gfx::{Face, Icon};
    use crate::ui::{DIM, TEXT};

    /// A tiny tile: a title, a counter, a "More" button and a line of typed text; `overlap` makes two buttons collide.
    struct Dummy {
        n: u32,
        typed: String,
        overlap: bool,
    }

    impl App for Dummy {
        fn title(&self) -> &'static str {
            "DUMMY"
        }
        fn icon(&self) -> Icon {
            Icon::Chat
        }
        fn accent(&self) -> [u8; 4] {
            [10, 200, 200, 255]
        }
        fn hint(&self) -> String {
            "Enter more".into()
        }
        fn handle(&mut self, ev: &Event, _cx: &AppCx) -> Flow {
            match ev {
                Event::Tab(7) => self.n += 1,
                Event::Key(Key::Char(c)) => self.typed.push(*c),
                Event::Key(Key::Esc) => return Flow::Back,
                _ => {}
            }
            Flow::Stay
        }
        fn render(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &AppCx, _tick: u64) {
            g.text(area.x + 20.0 * s, area.y + 10.0 * s, 20.0 * s, TEXT, &format!("count {}", self.n), Face::Bold);
            g.text(area.x + 20.0 * s, area.y + 50.0 * s, 14.0 * s, DIM, &format!("typed: {}", self.typed), Face::Sans);
            g.hit(R::new(area.x + 20.0 * s, area.y + 100.0 * s, 120.0 * s, 36.0 * s), Event::Tab(7), "More");
            let x2 = if self.overlap { 100.0 } else { 160.0 };
            g.hit(R::new(area.x + x2 * s, area.y + 100.0 * s, 120.0 * s, 36.0 * s), Event::Tab(8), "Less");
        }
    }

    fn harness(overlap: bool) -> TileHarness<Dummy> {
        TileHarness::new(Dummy { n: 0, typed: String::new(), overlap }, cx_with_admin_store("harness"))
    }

    #[test]
    fn clicking_a_label_sends_its_event_and_the_next_picture_shows_the_result() {
        let mut h = harness(false);
        assert!(h.shows("count 0") && h.has_button("More"));
        h.click_label("More");
        h.click_label("More");
        assert!(h.shows("count 2") && !h.shows("count 0"));
    }

    #[test]
    #[should_panic(expected = "no control called \"Nope\"")]
    fn clicking_a_missing_label_says_which_controls_exist() {
        harness(false).click_label("Nope");
    }

    #[test]
    fn typing_goes_to_the_tile_and_keys_can_leave_it() {
        let mut h = harness(false);
        h.type_text("abc");
        assert!(h.shows("typed: abc"));
        assert!(matches!(h.key(Key::Esc), Flow::Back));
        assert!(h.texts().iter().any(|t| t == "count 0"));
    }

    #[test]
    fn the_four_standard_sizes_are_drawn_and_separate_controls_pass() {
        let mut h = harness(false);
        h.assert_no_overlap_at_standard_sizes();
        assert_eq!(h.size, (1280, 720));
        h.draw_at(480, 600);
        assert_eq!(h.draw().pm.width(), 480);
        h.draw_at_standard_sizes();
    }

    #[test]
    #[should_panic(expected = "overlaps")]
    fn controls_that_collide_fail_the_overlap_check_with_both_names() {
        harness(true).assert_no_overlap_at_standard_sizes();
    }

    #[test]
    fn a_control_inside_another_is_not_an_overlap_but_a_partial_overlap_is() {
        let outer = R::new(0.0, 0.0, 100.0, 100.0);
        assert!(!partly_overlap(outer, R::new(10.0, 10.0, 20.0, 20.0)));
        assert!(!partly_overlap(outer, R::new(100.0, 0.0, 50.0, 50.0)), "touching edges");
        assert!(partly_overlap(outer, R::new(50.0, 50.0, 100.0, 100.0)));
    }
}
