//! The example tile, driven through the host runner (the real iyakki-wasm sandbox).
use iyakki_tile_abi::{Color, DisplayList, Event, Op};
use iyakki_tile_example::{Counter, TILE_WAT};
use iyakki_tile_host::export::{to_json, to_svg};
use iyakki_tile_host::render::render;
use iyakki_tile_host::{Budgets, TileManifest, TileRunner};

const THEME: Event = Event::Theme {
    dark: false,
    accent: Color(40, 110, 220, 255),
    background: Color(245, 245, 245, 255),
    foreground: Color(20, 20, 20, 255),
};

fn runner() -> TileRunner {
    let mut r = TileRunner::load(TILE_WAT.as_bytes(), TileManifest::new("counter", &[]), Budgets::default(), None, 7).unwrap();
    r.init(320, 200, 100, THEME).unwrap();
    r
}

fn texts(l: &DisplayList) -> Vec<String> {
    l.ops
        .iter()
        .filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None })
        .collect()
}

#[test]
fn click_on_the_button_changes_the_display_list() {
    let mut r = runner();
    let before = r.frame().unwrap();
    assert_eq!(texts(&before)[2], "0");
    assert_eq!(r.click(30, 80).unwrap(), Some(1), "hit region id comes back");
    let after = r.frame().unwrap();
    assert_ne!(before, after);
    assert_eq!(texts(&after)[2], "1");
    for _ in 0..11 {
        r.click(30, 80).unwrap();
    }
    assert_eq!(texts(&r.frame().unwrap())[2], "12");
}

#[test]
fn a_click_elsewhere_changes_nothing() {
    let mut r = runner();
    let before = r.frame().unwrap();
    assert_eq!(r.click(300, 190).unwrap(), None);
    assert_eq!(r.frame().unwrap(), before);
}

#[test]
fn typing_fills_the_field_and_the_field_hit_clears_it() {
    let mut r = runner();
    r.frame().unwrap();
    r.event(&[Event::TextInput { text: "hello".into() }]).unwrap();
    assert_eq!(texts(&r.frame().unwrap())[4], "hello");
    r.event(&[Event::Key { code: 8, mods: 0, down: true }]).unwrap();
    assert_eq!(texts(&r.frame().unwrap())[4], "hell");
    assert_eq!(r.click(30, 130).unwrap(), Some(2));
    assert_eq!(texts(&r.frame().unwrap())[4], "");
}

#[test]
fn resize_changes_the_background_rectangle() {
    let mut r = runner();
    r.event(&[Event::Resize { w: 640, h: 360, scale_pct: 200 }]).unwrap();
    match &r.frame().unwrap().ops[0] {
        Op::Rect { r, .. } => assert_eq!((r.w, r.h), (640, 360)),
        o => panic!("{o:?}"),
    }
}

#[test]
fn wat_and_rust_versions_draw_the_same_picture() {
    let mut r = runner();
    let mut c = Counter::new();
    let mut ev = vec![Event::TextInput { text: "abc".into() }, Event::Hit { id: 1 }, Event::Hit { id: 1 }];
    assert_eq!(r.frame().unwrap(), c.frame());
    r.event(&ev).unwrap();
    for e in ev.drain(..) {
        c.event(&e);
    }
    assert_eq!(r.frame().unwrap(), c.frame());
}

#[test]
fn reference_renderer_shows_the_button_and_the_counter() {
    let mut r = runner();
    r.click(30, 80).unwrap();
    let list = r.frame().unwrap();
    let img = render(&list, 320, 200, Color::rgb(0, 0, 0));
    assert_eq!(img.pixel(2, 2), Color(245, 245, 245, 255));
    assert_eq!(img.pixel(25, 100), Color(40, 110, 220, 255), "button body");
    assert_eq!(img.pixel(20, 70), Color(245, 245, 245, 255), "rounded corner is cut");
    // the number "1" is drawn in the accent colour around x = 92..104, y = 44..58
    let mut inked = 0;
    for y in 44..58 {
        for x in 92..104 {
            if img.pixel(x, y) == Color(40, 110, 220, 255) {
                inked += 1;
            }
        }
    }
    assert!(inked > 10, "glyph pixels: {inked}");
}

#[test]
fn exporters_carry_the_hit_regions_and_exact_text_widths() {
    let mut r = runner();
    let list = r.frame().unwrap();
    let json = to_json(&list, 320, 200);
    assert!(json.starts_with("{\"abi\":1,\"width\":320"));
    assert!(json.contains("\"op\":\"hit\",\"id\":1"));
    assert!(json.contains("\"text\":\"TILE DEMO\""));
    assert!(json.contains("\"width\":117"), "9 chars at size 16 = 9 * 13");
    let svg = to_svg(&list, 320, 200);
    assert!(svg.starts_with("<svg ") && svg.ends_with("</svg>"));
    assert!(svg.contains("data-hit-id=\"2\""));
    assert!(svg.contains("textLength=\"117\""));
}

#[test]
fn time_for_init_and_one_frame() {
    // Measurement, not an assertion on speed in CI: prints the median of 200 runs of init+frame
    // on an already compiled module's fresh instance, and the median of 200 further frames.
    let mut frames = Vec::new();
    let mut inits = Vec::new();
    let mut r = runner();
    for _ in 0..200 {
        let t = std::time::Instant::now();
        r.init(320, 200, 100, THEME).unwrap();
        let f = r.frame().unwrap();
        inits.push(t.elapsed());
        let t = std::time::Instant::now();
        r.click(30, 80).unwrap();
        let _ = r.frame().unwrap();
        frames.push(t.elapsed());
        assert!(!f.ops.is_empty());
    }
    inits.sort();
    frames.sort();
    println!("init+frame median {:?}; click+frame median {:?}", inits[100], frames[100]);
    assert!(frames[100] < std::time::Duration::from_millis(20), "far above any sane value");
}
