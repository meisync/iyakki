//! The tile-only update gate (`shell_live.rs`): when the shell may paint just the browser's page rectangle, and when it must not.

use super::tests::*;
use super::*;

fn signed(tag: &str) -> Shell {
    let (mut sh, _host) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    sh
}

#[test]
fn the_page_rectangle_alone_is_painted_only_when_nothing_overlaps_it() {
    let mut sh = signed("live-gate");
    sh.mouse = (600, 400);
    assert!(sh.live_clear(), "a signed-in screen with the pointer in the page area and nothing open");
    sh.mouse = (600, 10);
    assert!(!sh.live_clear(), "the pointer in the header: the clock's tooltip hangs there");
    sh.mouse = (600, 400);
    sh.help = true;
    assert!(!sh.live_clear(), "the help card");
    sh.help = false;
    sh.toast = Some(("saved".into(), 0, true));
    assert!(!sh.live_clear(), "a toast");
    sh.toast = None;
    sh.drag_over = true;
    assert!(!sh.live_clear(), "a file being dragged over the window");
    sh.drag_over = false;
    sh.devpop.open = true;
    assert!(!sh.live_clear(), "the Devices popover");
    sh.devpop.open = false;
    assert!(sh.live_clear(), "everything closed again");
}

#[test]
fn no_one_is_signed_in_means_no_tile_update() {
    let (mut sh, _host) = shell_with_users("live-nosession");
    sh.mouse = (600, 400);
    assert!(!sh.live_clear());
}

#[test]
fn an_app_that_has_no_live_picture_never_gets_a_tile_update() {
    let mut sh = signed("live-none");
    sh.mouse = (600, 400);
    sh.set_live_paint(true);
    sh.live = true;
    let mut g = Gfx::new(1280, 720);
    assert_eq!(sh.live_update(&mut g), None, "the launcher is not an app with a live picture");
    assert!(!sh.live_pending(), "asking consumed the flag: the next whole redraw clears it too");
    sh.live = true;
    sh.render(&mut g);
    assert!(!sh.live_pending(), "a whole redraw makes a pending tile update pointless");
}

/// An app whose live picture is a rectangle: says `Live` while the flag is up, paints it once.
struct OnlyLive(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl crate::apps::App for OnlyLive {
    fn title(&self) -> &'static str {
        "Live"
    }
    fn icon(&self) -> crate::gfx::Icon {
        crate::gfx::Icon::Chat
    }
    fn accent(&self) -> [u8; 4] {
        [0, 255, 0, 255]
    }
    fn hint(&self) -> String {
        String::new()
    }
    fn handle(&mut self, _ev: &Event, _cx: &crate::apps::AppCx) -> crate::apps::Flow {
        crate::apps::Flow::Stay
    }
    fn redraw_kind(&mut self) -> crate::apps::browser_full::Redraw {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            crate::apps::browser_full::Redraw::Live
        } else {
            crate::apps::browser_full::Redraw::None
        }
    }
    fn live_draw(&mut self, _g: &mut Gfx) -> Option<(i32, i32, i32, i32)> {
        self.0.swap(false, std::sync::atomic::Ordering::SeqCst).then_some((10, 200, 50, 50))
    }
    fn render(&mut self, _g: &mut Gfx, _a: R, _s: f32, _cx: &crate::apps::AppCx, _tick: u64) {}
}

#[test]
fn only_a_front_end_that_paints_live_pictures_gets_them_without_a_whole_redraw() {
    use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
    let mut g = Gfx::new(1280, 720);
    for paint in [false, true] {
        let mut sh = signed("live-optin");
        let flag = Arc::new(AtomicBool::new(false));
        sh.screen = Screen::App(Box::new(OnlyLive(flag.clone())), PApp::Chat);
        sh.set_live_paint(paint);
        sh.mouse = (600, 400);
        sh.render(&mut g);
        sh.take_stale();
        sh.handle(&Event::Tick);
        assert!(!sh.take_stale() && !sh.live_pending(), "an idle heartbeat asks for nothing");
        flag.store(true, Ordering::SeqCst);
        sh.handle(&Event::Tick);
        if !paint {
            // the remote display, the window and the multi-display driver never call `live_update`: as before, the scene is stale
            assert!(sh.take_stale() && !sh.live_pending());
            continue;
        }
        assert!(!sh.take_stale() && sh.live_pending(), "a front end that paints live pictures keeps the scene and asks for the rectangle");
        assert_eq!(sh.live_update(&mut g), Some((10, 200, 50, 50)));
        assert_eq!(sh.live_update(&mut g), None, "once per picture");
        // something drawn over the app (the help card): no in-place paint, the shell says so and the whole scene is drawn
        flag.store(true, Ordering::SeqCst);
        sh.handle(&Event::Tick);
        sh.help = true;
        assert_eq!(sh.live_update(&mut g), None, "the help card covers the page");
        assert!(flag.load(Ordering::SeqCst), "the app was not asked to paint under the card");
        sh.set_live_paint(false);
        assert!(!sh.live_pending(), "switching the front end's mode drops a waiting update");
    }
}

/// An app that paints several small rectangles and asks for the whole scene less often.
struct ManyRects(bool);
impl crate::apps::App for ManyRects {
    fn title(&self) -> &'static str {
        "Rects"
    }
    fn icon(&self) -> crate::gfx::Icon {
        crate::gfx::Icon::Chat
    }
    fn accent(&self) -> [u8; 4] {
        [0, 255, 0, 255]
    }
    fn hint(&self) -> String {
        String::new()
    }
    fn handle(&mut self, _ev: &Event, _cx: &crate::apps::AppCx) -> crate::apps::Flow {
        crate::apps::Flow::Stay
    }
    fn redraw_kind(&mut self) -> crate::apps::browser_full::Redraw {
        crate::apps::browser_full::Redraw::Live
    }
    fn live_draw(&mut self, _g: &mut Gfx) -> Option<(i32, i32, i32, i32)> {
        self.0 = true;
        Some((10, 200, 400, 300))
    }
    fn live_rects(&self) -> Vec<(i32, i32, i32, i32)> {
        vec![(10, 200, 400, 20), (10, 480, 400, 20)]
    }
    fn live_full_every_ms(&self) -> u64 {
        1500
    }
    fn render(&mut self, _g: &mut Gfx, _a: R, _s: f32, _cx: &crate::apps::AppCx, _tick: u64) {}
}

#[test]
fn an_app_that_paints_small_rectangles_names_them_and_may_ask_for_the_whole_scene_less_often() {
    let mut g = Gfx::new(1280, 720);
    let mut sh = signed("live-rects");
    sh.screen = Screen::App(Box::new(ManyRects(false)), PApp::Chat);
    sh.set_live_paint(true);
    sh.mouse = (600, 400);
    sh.render(&mut g);
    sh.take_stale();
    assert_eq!(sh.redraw_interval_ms(), 100, "the desktop is drawn at least every 100 ms");
    sh.handle(&Event::Tick);
    assert!(sh.live_pending());
    assert_eq!(sh.redraw_interval_ms(), 1500, "the app paints live: the safety redraw is rarer, so the first live frame is not pre-empted");
    assert_eq!(sh.live_update(&mut g), Some((10, 200, 400, 300)));
    assert_eq!(sh.take_live_rects(), vec![(10, 200, 400, 20), (10, 480, 400, 20)], "the screen write copies these, not the bounding box");
    assert!(sh.take_live_rects().is_empty(), "taken once");
    // half a second without a live frame: the usual interval again
    for _ in 0..16 {
        sh.tick += 1;
    }
    assert_eq!(sh.redraw_interval_ms(), 100);
}

/// An app that wants to be polled between heartbeats and has a slow idle safety redraw (what TALES does while it reads and while it rests).
struct Pollable {
    changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl crate::apps::App for Pollable {
    fn title(&self) -> &'static str {
        "Poll"
    }
    fn icon(&self) -> crate::gfx::Icon {
        crate::gfx::Icon::Chat
    }
    fn accent(&self) -> [u8; 4] {
        [0, 255, 0, 255]
    }
    fn hint(&self) -> String {
        String::new()
    }
    fn handle(&mut self, _ev: &Event, _cx: &crate::apps::AppCx) -> crate::apps::Flow {
        crate::apps::Flow::Stay
    }
    fn wants_fast_poll(&self) -> bool {
        true
    }
    fn poll_redraw_due(&mut self) -> bool {
        self.changed.swap(false, std::sync::atomic::Ordering::SeqCst)
    }
    fn redraw_kind(&mut self) -> crate::apps::browser_full::Redraw {
        crate::apps::browser_full::Redraw::Live
    }
    fn live_draw(&mut self, _g: &mut Gfx) -> Option<(i32, i32, i32, i32)> {
        Some((10, 200, 50, 20))
    }
    fn idle_redraw_ms(&self) -> u64 {
        250
    }
    fn render(&mut self, _g: &mut Gfx, _a: R, _s: f32, _cx: &crate::apps::AppCx, _tick: u64) {}
}

#[test]
fn a_change_between_heartbeats_asks_for_a_tile_frame_at_once_and_an_idle_app_slows_the_safety_redraw() {
    use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
    let mut g = Gfx::new(1280, 720);
    let mut sh = signed("live-poll");
    let flag = Arc::new(AtomicBool::new(false));
    sh.screen = Screen::App(Box::new(Pollable { changed: flag.clone() }), PApp::Chat);
    sh.set_live_paint(true);
    sh.mouse = (600, 400);
    sh.render(&mut g);
    sh.take_stale();
    assert!(sh.fast_poll());
    assert!(!sh.poll_live() && !sh.live_pending(), "nothing changed: no frame");
    flag.store(true, Ordering::SeqCst);
    assert!(!sh.poll_live(), "a tile-only frame, not a whole one");
    assert!(sh.live_pending(), "the change is painted at once, without waiting for the 33 ms heartbeat");
    assert_eq!(sh.live_update(&mut g), Some((10, 200, 50, 20)));
    // at rest: the app's own interval; with the pointer in the header or an overlay open the usual 100 ms
    sh.tick += 40;
    assert_eq!(sh.redraw_interval_ms(), 250);
    sh.mouse = (600, 5);
    assert_eq!(sh.redraw_interval_ms(), 100, "the pointer is under the header: its tooltip may need the usual redraw");
    sh.mouse = (600, 400);
    sh.devpop.open = true;
    assert_eq!(sh.redraw_interval_ms(), 100, "an overlay is open");
    let mut sh2 = signed("live-nopoll");
    sh2.screen = Screen::Launcher;
    assert!(!sh2.fast_poll());
}
