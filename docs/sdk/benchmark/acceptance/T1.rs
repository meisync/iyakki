// Visible acceptance test of task T1 (the model may run it). Copied to tests/accept_t1.rs of the app by grade.py.
use iyakki_app_bench_t1::{BenchT1App, ID};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, UiEvent};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;
use iyakki_ui_sdk::ports::{MemStorage, StoragePort};

fn host(app: BenchT1App, w: u32, h: u32) -> Host<BenchT1App> {
    Host::new(app, Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}
fn centre(l: &DisplayList, name: &str) -> (f32, f32) {
    l.ops.iter().find_map(|o| if let Op::Hit(r, h) = o { (h.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}"))
}
fn tap(h: &mut Host<BenchT1App>, name: &str) {
    let l = h.record();
    let (x, y) = centre(&l, name);
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}

#[test]
fn the_counter_counts_and_is_kept_through_the_storage_port() {
    let mut h = host(BenchT1App::new(), 360, 740);
    tap(&mut h, "More");
    tap(&mut h, "More");
    assert_eq!(h.app.count(), 2);
    assert!(h.record().texts().contains(&"Pressed 2 times"));
    let mut s = MemStorage::default();
    s.set("count", "41").unwrap();
    assert_eq!(BenchT1App::with_storage(Box::new(s)).count(), 41);
}

#[test]
fn a_note_is_added_listed_and_kept_and_an_empty_note_is_refused() {
    let mut h = host(BenchT1App::new(), 360, 740);
    tap(&mut h, "Notes");
    tap(&mut h, "Add");
    assert!(h.app.notes().is_empty(), "an empty note is not added");
    tap(&mut h, "Note");
    h.event(&UiEvent::Text("buy milk".into()));
    tap(&mut h, "Add");
    assert_eq!(h.app.notes(), vec!["buy milk".to_string()]);
    assert!(h.record().texts().contains(&"buy milk"));
}

#[test]
fn the_compact_switch_works_on_the_screen_and_is_kept() {
    let mut h = host(BenchT1App::new(), 360, 740);
    assert!(!h.app.compact());
    tap(&mut h, "Notes");
    tap(&mut h, "Compact rows");
    assert!(h.app.compact());
    tap(&mut h, "Compact rows");
    assert!(!h.app.compact());
}

#[test]
fn every_control_is_named_inside_the_pane_and_touchable_at_three_sizes_and_text_3x() {
    for (w, hh) in [(360u32, 740u32), (800, 480), (1280, 720)] {
        for tab in ["Counter", "Notes", "Assistant"] {
            let mut h = host(BenchT1App::new(), w, hh);
            h.touch = true;
            h.text_scale = 3.0;
            tap(&mut h, tab);
            h.event(&UiEvent::Tick);
            for n in h.record().a11y() {
                assert!(!n.name.is_empty());
                assert!(n.rect.x >= -0.5 && n.rect.x + n.rect.w <= w as f32 + 0.5 && n.rect.y + n.rect.h <= hh as f32 + 0.5, "{tab} {w}x{hh}: {} leaves the pane", n.name);
                assert!(n.rect.h >= 43.5, "{tab} {w}x{hh}: {} is too small to touch", n.name);
            }
        }
    }
}
