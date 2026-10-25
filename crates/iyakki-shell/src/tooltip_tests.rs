use super::*;
use crate::gfx::Gfx;

const ROW: R = R { x: 100.0, y: 100.0, w: 300.0, h: 50.0 };
const TEXT_FULL: &str = "Ethernet · enp3s0\nconnected · 192.168.100.200 · 1000 Mb/s · aa:bb:cc:dd:ee:ff";

fn ticks(t: &mut Tooltip, n: u32) {
    for _ in 0..n {
        t.event(&Event::Tick);
    }
}

/// One frame of a list with a single cut row: the offer, then the end of the frame.
fn frame(t: &mut Tooltip, g: &mut Gfx, r: R, selected: bool, text: &str, s: f32) -> Option<R> {
    t.offer(g, r, selected, text);
    t.draw(g, s)
}

fn pointer_on(t: &mut Tooltip, g: &mut Gfx, x: i32, y: i32) {
    g.mouse = (x, y);
    t.event(&Event::MouseMove(x, y));
}

#[test]
fn an_address_is_found_with_its_port_and_not_in_a_version_or_a_mac() {
    let t = "connected · 192.168.1.9 · 1000 Mb/s";
    let (a, b) = find_ip(t).unwrap();
    assert_eq!(&t[a..b], "192.168.1.9");
    let t = "on http://10.0.0.7:11434/v1";
    let (a, b) = find_ip(t).unwrap();
    assert_eq!(&t[a..b], "10.0.0.7:11434");
    assert!(find_ip("aa:bb:cc:dd:ee:ff 1.2.3 v1.2.3.4x 300.1.1.1 1.2.3.4.5").is_none());
    assert!(find_ip("").is_none());
}

#[test]
fn a_long_name_gives_way_and_the_address_stays() {
    let mut g = Gfx::new(800, 200);
    let line = format!("{} · 192.168.100.200 · {}", "a-very-long-device-name-that-takes-all-the-room".repeat(2), "aa:bb:cc:dd:ee:ff · first seen 3 min ago");
    for w in [140.0, 200.0, 260.0, 420.0, 600.0] {
        let t = fit_keep_ip(&mut g, 13.0, &line, Face::Mono, w);
        assert!(t.contains("192.168.100.200"), "{w}: {t}");
        assert!(g.text_width(13.0, &t, Face::Mono) <= w + 1.0, "{w}: {t}");
    }
    // the plain fit would have cut the address off
    assert!(!g.fit(13.0, &line, Face::Mono, 260.0).contains("192.168"));
    // a line that fits is left alone, and one without an address is cut as before
    assert_eq!(fit_keep_ip(&mut g, 13.0, "ok 10.0.0.1", Face::Mono, 400.0), "ok 10.0.0.1");
    let plain = "x".repeat(200);
    assert_eq!(fit_keep_ip(&mut g, 13.0, &plain, Face::Mono, 100.0), g.fit(13.0, &plain, Face::Mono, 100.0));
}

#[test]
fn the_tooltip_has_the_full_text_after_the_delay_and_not_before() {
    let (mut g, mut t) = (Gfx::new(800, 600), Tooltip::new());
    pointer_on(&mut t, &mut g, 150, 120);
    ticks(&mut t, POINTER_DELAY - 1);
    assert!(frame(&mut t, &mut g, ROW, false, TEXT_FULL, 1.0).is_none(), "not yet");
    assert!(!t.take_changed() || !t.ready());
    ticks(&mut t, 1);
    assert!(t.take_changed(), "the tile is asked to draw again when it appears");
    let before = g.rgba().to_vec();
    let r = frame(&mut t, &mut g, ROW, false, TEXT_FULL, 1.0).expect("after half a second");
    assert_ne!(before, g.rgba(), "it is drawn");
    assert!(r.w > 100.0 && r.h > 30.0);
    // a row the pointer is not on shows nothing
    assert!(frame(&mut t, &mut g, R::new(500.0, 300.0, 100.0, 40.0), false, TEXT_FULL, 1.0).is_none());
}

#[test]
fn it_hides_on_move_click_release_scroll_and_escape() {
    let (mut g, mut t) = (Gfx::new(800, 600), Tooltip::new());
    let mut up = |t: &mut Tooltip, g: &mut Gfx| {
        pointer_on(t, g, 140, 110);
        pointer_on(t, g, 150, 120);
        ticks(t, POINTER_DELAY);
        assert!(frame(t, g, ROW, false, TEXT_FULL, 1.0).is_some());
    };
    up(&mut t, &mut g);
    pointer_on(&mut t, &mut g, 151, 120);
    assert!(frame(&mut t, &mut g, ROW, false, TEXT_FULL, 1.0).is_none(), "move");
    ticks(&mut t, POINTER_DELAY);
    assert!(frame(&mut t, &mut g, ROW, false, TEXT_FULL, 1.0).is_some(), "it comes back after the next rest");
    t.event(&Event::MouseDown(151, 120));
    assert!(frame(&mut t, &mut g, ROW, false, TEXT_FULL, 1.0).is_none(), "click");
    t.event(&Event::MouseUp(151, 120));
    ticks(&mut t, 100);
    assert!(frame(&mut t, &mut g, ROW, false, TEXT_FULL, 1.0).is_none(), "after a click it stays away until the pointer moves");
    up(&mut t, &mut g);
    t.event(&Event::Row(0));
    ticks(&mut t, 100);
    assert!(frame(&mut t, &mut g, ROW, false, TEXT_FULL, 1.0).is_none(), "a click turned into a row choice");
    up(&mut t, &mut g);
    t.event(&Event::Scroll(1));
    assert!(frame(&mut t, &mut g, ROW, false, TEXT_FULL, 1.0).is_none(), "scroll");
    up(&mut t, &mut g);
    t.event(&Event::Key(Key::Esc));
    ticks(&mut t, 100);
    assert!(frame(&mut t, &mut g, ROW, true, TEXT_FULL, 1.0).is_none(), "Esc, and it does not come back by itself");
    t.event(&Event::Key(Key::Down));
    ticks(&mut t, HOLD_DELAY);
    assert!(frame(&mut t, &mut g, ROW, true, TEXT_FULL, 1.0).is_some(), "the next key arms it again");
}

#[test]
fn a_keyboard_selection_shows_it_after_600_ms_for_the_selected_row_only() {
    let (mut g, mut t) = (Gfx::new(800, 600), Tooltip::new());
    g.mouse = (150, 120); // the pointer rests on the row, but the keyboard was used last
    ticks(&mut t, 200);
    assert!(frame(&mut t, &mut g, ROW, true, TEXT_FULL, 1.0).is_none(), "a tile that was just opened shows nothing by itself");
    t.event(&Event::Key(Key::Down));
    ticks(&mut t, HOLD_DELAY - 1);
    assert!(frame(&mut t, &mut g, ROW, true, TEXT_FULL, 1.0).is_none());
    ticks(&mut t, 1);
    assert!(frame(&mut t, &mut g, ROW, false, TEXT_FULL, 1.0).is_none(), "not the selected row");
    let r = frame(&mut t, &mut g, ROW, true, TEXT_FULL, 1.0).expect("600 ms");
    assert!(r.y >= ROW.y + ROW.h, "under the selected row");
}

#[test]
fn a_long_press_shows_it_while_held_and_release_puts_it_away() {
    let (mut g, mut t) = (Gfx::new(800, 600), Tooltip::new());
    g.mouse = (150, 120);
    t.event(&Event::MouseDown(150, 120));
    ticks(&mut t, HOLD_DELAY - 1);
    assert!(frame(&mut t, &mut g, ROW, false, TEXT_FULL, 1.0).is_none(), "a tap is not a long press");
    ticks(&mut t, 1);
    let r = frame(&mut t, &mut g, ROW, false, TEXT_FULL, 1.0).expect("held for 600 ms");
    assert!(!r.contains(150, 120), "it does not sit under the finger");
    t.event(&Event::MouseUp(150, 120));
    assert!(frame(&mut t, &mut g, ROW, false, TEXT_FULL, 1.0).is_none());
}

#[test]
fn it_stays_inside_the_screen_at_the_right_and_bottom_edges_at_any_scale() {
    for (s, w, h) in [(1.0f32, 800u32, 600u32), (2.0, 800, 600), (1.0, 320, 240), (3.0, 640, 480)] {
        let mut g = Gfx::new(w, h);
        for (x, y) in [(w as i32 - 2, h as i32 - 2), (w as i32 - 2, 10), (5, h as i32 - 2), (0, 0)] {
            let mut t = Tooltip::new();
            let row = R::new((x as f32 - 40.0).max(0.0), (y as f32 - 10.0).max(0.0), 80.0, 20.0);
            pointer_on(&mut t, &mut g, x, y);
            ticks(&mut t, POINTER_DELAY);
            let r = frame(&mut t, &mut g, row, false, TEXT_FULL, s).expect("shown");
            let (sw, sh) = (w as f32, h as f32);
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= sw && r.y + r.h <= sh, "scale {s} at {x},{y} on {w}x{h}: {r:?}");
        }
        // by keyboard, on the last row of the screen
        let mut t = Tooltip::new();
        t.event(&Event::Key(Key::Down));
        ticks(&mut t, HOLD_DELAY);
        let row = R::new(w as f32 - 100.0, h as f32 - 30.0, 100.0, 30.0);
        let r = frame(&mut t, &mut g, row, true, TEXT_FULL, s).expect("shown");
        assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32, "keyboard, scale {s}: {r:?}");
    }
}

#[test]
fn long_text_wraps_to_a_bounded_width_and_grows_in_height() {
    let (mut g, mut t) = (Gfx::new(800, 600), Tooltip::new());
    pointer_on(&mut t, &mut g, 100, 100);
    ticks(&mut t, POINTER_DELAY);
    let short = frame(&mut t, &mut g, ROW, false, "10.0.0.1", 1.0).unwrap();
    let long = "192.168.100.200 ".repeat(30);
    let r = frame(&mut t, &mut g, ROW, false, &long, 1.0).unwrap();
    assert!(r.w <= 360.0 + 0.5 && r.h > short.h * 3.0, "{r:?} {short:?}");
    // a screen narrower than the usual width
    let mut g = Gfx::new(200, 600);
    g.mouse = (100, 100);
    let r = frame(&mut t, &mut g, ROW, false, &long, 1.0).unwrap();
    assert!(r.x >= 0.0 && r.x + r.w <= 200.0, "{r:?}");
}

#[test]
fn it_never_takes_a_click_or_the_focus() {
    let (mut g, mut t) = (Gfx::new(800, 600), Tooltip::new());
    pointer_on(&mut t, &mut g, 150, 120);
    ticks(&mut t, POINTER_DELAY);
    let hits = g.hits.len();
    assert!(frame(&mut t, &mut g, ROW, false, TEXT_FULL, 1.0).is_some());
    assert_eq!(g.hits.len(), hits, "no click region");
}
