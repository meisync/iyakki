// Visible acceptance test of task T2. Copied to tests/accept_t2.rs of the app by grade.py.
use iyakki_app_bench_t2::{BenchT2App, ID};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Phase, UiEvent, UiKey};
use iyakki_ui_sdk::files::MemFiles;
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;

fn host(app: BenchT2App, w: u32, h: u32) -> Host<BenchT2App> {
    Host::new(app, Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}
fn centre(l: &DisplayList, name: &str) -> (f32, f32) {
    l.ops.iter().find_map(|o| if let Op::Hit(r, h) = o { (h.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}"))
}
fn tap(h: &mut Host<BenchT2App>, name: &str) {
    let l = h.record();
    let (x, y) = centre(&l, name);
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}
fn with_file() -> BenchT2App {
    let mut f = MemFiles::new();
    f.put("a.txt", "hello world");
    BenchT2App::with_ports(Box::new(f), Box::new(iyakki_ui_sdk::assistant::NoAssistant))
}

#[test]
fn the_files_pane_lists_the_home_folder_and_opening_a_file_is_shown() {
    let mut h = host(with_file(), 800, 480);
    h.event(&UiEvent::Tick);
    assert!(h.record().texts().contains(&"No file open"));
    assert!(h.record().texts().iter().any(|t| t.contains("a.txt")));
    h.event(&UiEvent::Key(UiKey::Enter));
    h.event(&UiEvent::Tick);
    assert_eq!(h.app.opened().as_deref(), Some("a.txt"));
    assert!(h.record().texts().contains(&"Opened: a.txt"));
}

#[test]
fn every_control_is_named_inside_the_pane_and_touchable_at_three_sizes_and_text_3x() {
    for (w, hh) in [(360u32, 740u32), (800, 480), (1280, 720)] {
        for tab in ["Files", "Assistant"] {
            let mut h = host(with_file(), w, hh);
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
