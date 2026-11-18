//! The shell's side of smooth scrolling: pixel steps reach a tile that can use them as pixels, every other tile (and every popover)
//! as whole wheel notches with nothing lost, a tile that says no falls back, and the browser's tile asks for un-eased steps.

use super::tests::*;
use super::*;
use crate::scrolleng::NOTCH_PX;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Seen {
    px: Vec<i32>,
    notches: Vec<i32>,
}

/// A tile with pixel scrolling (`takes`) or only the classic notches.
struct Fake {
    seen: Arc<Mutex<Seen>>,
    takes: bool,
    raw: bool,
}

impl App for Fake {
    fn title(&self) -> &'static str {
        "Fake"
    }
    fn icon(&self) -> Icon {
        Icon::Chat
    }
    fn accent(&self) -> [u8; 4] {
        rgb(0, 255, 0)
    }
    fn hint(&self) -> String {
        String::new()
    }
    fn handle(&mut self, ev: &Event, _cx: &AppCx) -> Flow {
        if let Event::Scroll(d) = ev {
            self.seen.lock().unwrap().notches.push(*d);
        }
        Flow::Stay
    }
    fn scroll_px(&mut self, px: i32, _cx: &AppCx) -> bool {
        if self.takes {
            self.seen.lock().unwrap().px.push(px);
        }
        self.takes
    }
    fn scroll_raw(&self) -> bool {
        self.raw
    }
    fn render(&mut self, _g: &mut Gfx, _a: R, _s: f32, _cx: &AppCx, _tick: u64) {}
}

fn shell_with(tag: &str, takes: bool, raw: bool) -> (Shell, Arc<Mutex<Seen>>) {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    let seen = Arc::new(Mutex::new(Seen::default()));
    sh.screen = Screen::App(Box::new(Fake { seen: seen.clone(), takes, raw }), PApp::Chat);
    sh.take_stale();
    (sh, seen)
}

#[test]
fn a_tile_with_pixel_scrolling_gets_pixels_and_a_frame_is_asked_for() {
    let (mut sh, seen) = shell_with("scr-px", true, false);
    sh.handle(&Event::ScrollPx(7));
    sh.handle(&Event::ScrollPx(-3));
    let s = seen.lock().unwrap();
    assert_eq!(s.px, vec![7, -3]);
    assert!(s.notches.is_empty(), "no notch on top of the pixels");
    drop(s);
    assert!(sh.take_stale(), "each step draws a new picture");
}

#[test]
fn a_tile_with_only_notches_gets_whole_notches_and_loses_no_distance() {
    let (mut sh, seen) = shell_with("scr-notch", false, false);
    // a click as nine small steps: one notch, at about half-way, the rest kept
    for _ in 0..9 {
        sh.handle(&Event::ScrollPx(6));
    }
    assert_eq!(seen.lock().unwrap().notches, vec![1], "54 px = one notch");
    // a big flick: 100 px = two notches and a borrowed 8 px, which the next step repays
    seen.lock().unwrap().notches.clear();
    sh.handle(&Event::ScrollPx(100));
    sh.handle(&Event::ScrollPx(62));
    let n: i32 = seen.lock().unwrap().notches.iter().sum();
    assert_eq!(n, 3, "162 px = three notches in all");
    // down is negative
    seen.lock().unwrap().notches.clear();
    sh.handle(&Event::ScrollPx(-(NOTCH_PX as i32) * 2));
    assert_eq!(seen.lock().unwrap().notches, vec![-2]);
}

#[test]
fn a_tile_that_cannot_use_the_step_falls_back_to_notches() {
    let (mut sh, seen) = shell_with("scr-refuse", false, false);
    sh.handle(&Event::ScrollPx(54));
    let s = seen.lock().unwrap();
    assert!(s.px.is_empty() && s.notches == vec![1]);
}

#[test]
fn with_a_popup_open_the_tile_gets_no_pixels() {
    let (mut sh, seen) = shell_with("scr-help", true, false);
    sh.help = true;
    sh.handle(&Event::ScrollPx(7));
    assert!(seen.lock().unwrap().px.is_empty(), "the help card is over the tile");
}

#[test]
fn a_left_over_part_of_a_notch_is_forgotten_after_a_pause() {
    let (mut sh, seen) = shell_with("scr-rem", false, false);
    sh.handle(&Event::ScrollPx(20)); // under half a notch: kept
    assert!(seen.lock().unwrap().notches.is_empty());
    for _ in 0..30 {
        sh.handle(&Event::Tick); // a second of nothing
    }
    sh.handle(&Event::ScrollPx(20)); // would be 40 together: a notch; alone it is not
    assert!(seen.lock().unwrap().notches.is_empty());
}

#[test]
fn the_browsers_tile_asks_for_uneased_steps_and_others_for_eased_ones() {
    let (mut sh, _) = shell_with("scr-raw", true, true);
    sh.handle(&Event::Tick);
    assert!(crate::scrolleng::raw(), "the page engine makes its own smooth pictures");
    let (mut sh2, _) = shell_with("scr-raw2", true, false);
    sh2.handle(&Event::Tick);
    assert!(!crate::scrolleng::raw());
    crate::scrolleng::set_raw(false);
}
