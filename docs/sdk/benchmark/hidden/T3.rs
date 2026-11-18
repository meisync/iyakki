// HIDDEN test of task T3. Copied to tests/hidden_t3.rs by grade.py.
use iyakki_app_bench_t3::{BenchT3App, ID};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, UiEvent, UiKey};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;

fn host(w: u32, h: u32, look: Look) -> Host<BenchT3App> {
    Host::new(BenchT3App::new(), look, Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}
fn has(l: &DisplayList, name: &str) -> bool {
    l.ops.iter().any(|o| matches!(o, Op::Hit(_, h) if h.name == name))
}
fn tap(h: &mut Host<BenchT3App>, name: &str) {
    let l = h.record();
    let (x, y) = l.ops.iter().find_map(|o| if let Op::Hit(r, hh) = o { (hh.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}"));
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}

#[test]
fn compact_shows_one_column_and_wide_shows_two() {
    let mut c = host(360, 740, Look::default());
    tap(&mut c, "Bravo");
    let l = c.record();
    assert!(has(&l, "Back"));
    assert!(!has(&l, "Alpha"), "on a compact pane the list is replaced by the detail");
    let mut w = host(1280, 720, Look::default());
    tap(&mut w, "Bravo");
    let l = w.record();
    assert!(has(&l, "Alpha") && has(&l, "Charlie") && !has(&l, "Back"), "wide: list and detail together, no Back");
    assert!(l.texts().contains(&"Status: slow"));
}

#[test]
fn escape_goes_back_one_level_then_leaves() {
    let mut c = host(360, 740, Look::default());
    tap(&mut c, "Alpha");
    assert_eq!(c.event(&UiEvent::Key(UiKey::Esc)), iyakki_ui_sdk::app::Flow::Stay);
    assert!(!c.app.detail_open());
    assert_eq!(c.event(&UiEvent::Key(UiKey::Esc)), iyakki_ui_sdk::app::Flow::Back);
}

#[test]
fn status_is_text_not_only_colour_and_the_selected_row_is_findable() {
    let mut h = host(1280, 720, Look::default());
    for (n, s) in [("Alpha", "Status: ok"), ("Bravo", "Status: slow"), ("Charlie", "Status: down")] {
        tap(&mut h, n);
        assert!(h.record().texts().contains(&s), "{n} says its status in words");
    }
}

#[test]
fn the_app_follows_the_skin_and_survives_every_size_and_scale() {
    let mut looks = vec![Look::default()];
    for (_, accepted) in iyakki_ui_sdk::skins::shipped() {
        looks.push(accepted.unwrap().look);
    }
    assert!(looks.len() >= 2, "the shipped skins load");
    for look in looks {
        for (w, hh) in [(360u32, 740u32), (800, 480), (1280, 720)] {
            for scale in [1.0, 2.0, 3.0] {
                let mut h = host(w, hh, look.clone());
                h.touch = true;
                h.text_scale = scale;
                tap(&mut h, "Charlie");
                let l = h.record();
                for n in l.a11y() {
                    assert!(!n.name.is_empty());
                    assert!(n.rect.x >= -0.5 && n.rect.x + n.rect.w <= w as f32 + 0.5 && n.rect.y + n.rect.h <= hh as f32 + 0.5, "{w}x{hh} x{scale}: {} leaves the pane", n.name);
                }
            }
        }
    }
}
