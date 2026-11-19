// Visible acceptance test of task T3. Copied to tests/accept_t3.rs of the app by grade.py.
use iyakki_app_bench_t3::{BenchT3App, ID};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, UiEvent, UiKey};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;

fn host(w: u32, h: u32) -> Host<BenchT3App> {
    Host::new(BenchT3App::new(), Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}
fn centre(l: &DisplayList, name: &str) -> (f32, f32) {
    l.ops.iter().find_map(|o| if let Op::Hit(r, h) = o { (h.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}"))
}
fn tap(h: &mut Host<BenchT3App>, name: &str) {
    let l = h.record();
    let (x, y) = centre(&l, name);
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}

#[test]
fn the_list_shows_three_services_and_a_tap_selects_one() {
    let mut h = host(1280, 720);
    for n in ["Alpha", "Bravo", "Charlie"] {
        assert!(h.record().a11y().iter().any(|a| a.name == n), "{n} is a named control");
    }
    tap(&mut h, "Bravo");
    assert_eq!(h.app.selected(), 1);
    assert!(h.record().texts().contains(&"Status: slow"));
    h.event(&UiEvent::Key(UiKey::Down));
    assert_eq!(h.app.selected(), 2);
    assert!(h.record().texts().contains(&"Status: down"));
}

#[test]
fn a_narrow_pane_shows_the_list_then_the_detail_with_a_back_button() {
    let mut h = host(360, 740);
    tap(&mut h, "Charlie");
    assert!(h.app.detail_open());
    assert!(h.record().texts().contains(&"Status: down"));
    tap(&mut h, "Back");
    assert!(!h.app.detail_open());
}

#[test]
fn every_control_is_named_inside_the_pane_and_touchable_at_three_sizes_and_text_3x() {
    for (w, hh) in [(360u32, 740u32), (800, 480), (1280, 720)] {
        for open in [false, true] {
            let mut h = host(w, hh);
            h.touch = true;
            h.text_scale = 3.0;
            if open {
                tap(&mut h, "Bravo");
            }
            for n in h.record().a11y() {
                assert!(!n.name.is_empty());
                assert!(n.rect.x >= -0.5 && n.rect.x + n.rect.w <= w as f32 + 0.5 && n.rect.y + n.rect.h <= hh as f32 + 0.5, "{w}x{hh}: {} leaves the pane", n.name);
                assert!(n.rect.h >= 43.5, "{w}x{hh}: {} is too small to touch", n.name);
            }
        }
    }
}
