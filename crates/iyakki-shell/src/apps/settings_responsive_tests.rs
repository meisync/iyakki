//! Settings on phones and tablets: below 900 px wide the window is one column (the list of pages, or one page with a Back button),
//! nothing is drawn past the right edge, and the Back button works by click and by Esc.

use super::*;
use crate::apps::tests::cx_with_admin_store;

const SIZES: [(u32, u32); 6] = [(390, 844), (600, 1000), (800, 1280), (1280, 800), (1920, 1080), (844, 390)];

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(w, h);
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}

fn nav_rows(g: &Gfx) -> usize {
    g.hits.iter().filter(|x| matches!(x.ev, Event::Tab(_)) && x.rect.w > 0.0).count()
}

fn has_back(g: &Gfx) -> bool {
    g.hits.iter().any(|x| x.label == "< All settings")
}

#[test]
fn nothing_is_drawn_past_the_right_edge_on_any_size() {
    let cx = cx_with_admin_store("resp-edge");
    for tab in [Tab::Network, Tab::Account, Tab::Keyboard, Tab::Models, Tab::Shares] {
        for (w, h) in SIZES {
            let mut a = SettingsApp::new(&cx);
            a.tab = a.tabs().iter().position(|t| *t == tab).unwrap();
            let g = draw(&mut a, &cx, w, h);
            for hit in g.hits.iter().filter(|x| x.rect.w > 0.0 && x.rect.h > 0.0) {
                let r = hit.rect;
                assert!(r.x >= -0.5 && r.x + r.w <= w as f32 + 0.5, "{tab:?} {w}x{h}: {:?} {r:?}", hit.label);
            }
            // the last two columns of pixels are still the cleared background: no text or box runs off the edge
            let px = g.rgba();
            for y in 0..h as usize {
                for dx in [1usize, 2] {
                    let o = (y * w as usize + (w as usize - dx)) * 4;
                    assert_eq!(&px[o..o + 3], &px[0..3], "{tab:?} {w}x{h}: drawing reaches the right edge at row {y}");
                }
            }
        }
    }
}

#[test]
fn a_narrow_window_shows_one_page_with_a_back_button_and_a_wide_one_shows_both() {
    let cx = cx_with_admin_store("resp-nav");
    for (w, h) in SIZES {
        let s = crate::shell::scale_for(w, h);
        let mut a = SettingsApp::new(&cx);
        a.tab = a.tabs().iter().position(|t| *t == Tab::Account).unwrap();
        let g = draw(&mut a, &cx, w, h);
        if w < 900 {
            assert!(has_back(&g), "{w}x{h}: no Back button");
            assert_eq!(nav_rows(&g), 0, "{w}x{h}: the page list is drawn beside the page");
            let b = g.hits.iter().find(|x| x.label == "< All settings").unwrap().rect;
            assert!(b.h >= 44.0 * s - 0.5, "{w}x{h}: Back is {} px tall", b.h);
            // Esc goes to the list; there every row is a big target and there is no Back button
            a.handle(&Event::Key(Key::Esc), &cx);
            let g = draw(&mut a, &cx, w, h);
            assert!(!has_back(&g) && nav_rows(&g) > 3, "{w}x{h}: Esc did not show the list");
            for r in g.hits.iter().filter(|x| matches!(x.ev, Event::Tab(_)) && x.rect.w > 0.0) {
                assert!(r.rect.h >= 40.0 * s - 0.5 || r.rect.y + r.rect.h >= h as f32 - 60.0, "{w}x{h}: row {:?} is {} px tall", r.label, r.rect.h);
            }
            // choosing a page shows it, and the Back button (a click) returns to the list
            let ti = a.tabs().iter().position(|t| *t == Tab::Time).unwrap();
            let pick = g.hits.iter().find(|x| matches!(x.ev, Event::Tab(i) if i == ti)).map(|x| x.ev.clone());
            if let Some(ev) = pick {
                a.handle(&ev, &cx);
                let g = draw(&mut a, &cx, w, h);
                assert!(has_back(&g) && a.tab() == Tab::Time, "{w}x{h}: the chosen page is not shown");
                let back = g.hits.iter().find(|x| x.label == "< All settings").unwrap().ev.clone();
                a.handle(&back, &cx);
                assert!(nav_rows(&draw(&mut a, &cx, w, h)) > 3, "{w}x{h}: Back by click did not show the list");
            }
            // Esc on the list leaves Settings
            assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
        } else {
            assert!(!has_back(&g) && nav_rows(&g) > 3, "{w}x{h}: wide window should show the list beside the page");
            assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
        }
    }
}
