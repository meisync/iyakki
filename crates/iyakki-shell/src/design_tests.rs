//! The design system across the whole shell: every app draws at three sizes without panicking and not blank, each app
//! has its own accent in its header, the shared widgets keep their click regions, and drawing stays cheap.

use crate::fx;
use crate::gfx::Gfx;
use crate::shell::tests::env;
use crate::shell::Shell;
use crate::ui::{Event, Key};
use iyakki_sec::policy::App as PApp;

fn signed_in() -> Shell {
    let (e, _) = env("design");
    let mut sh = Shell::new(e);
    for c in "root".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
    sh.handle(&Event::Key(Key::Tab));
    for c in "rootpass12".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
    sh.handle(&Event::Key(Key::Tab));
    for c in "rootpass12".chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
    sh.handle(&Event::Key(Key::Enter));
    assert_eq!(sh.screen_name(), "launcher");
    sh
}

/// How many distinct colours a sample of the picture holds (a blank screen has one or two).
fn colours(g: &Gfx) -> usize {
    let mut seen = std::collections::HashSet::new();
    for px in g.rgba().chunks_exact(4).step_by(37) {
        seen.insert([px[0], px[1], px[2]]);
    }
    seen.len()
}

#[test]
fn every_app_draws_at_three_sizes_and_is_not_blank() {
    let mut sh = signed_in();
    for (w, h) in [(640u32, 360u32), (1280, 720), (1920, 1080)] {
        let mut g = Gfx::new(w, h);
        sh.render(&mut g);
        assert!(colours(&g) > 20, "launcher at {w}x{h}");
        for app in PApp::ALL {
            let id = app.id();
            assert!(sh.open_for_demo(id), "{id} opens");
            sh.render(&mut g);
            sh.handle(&Event::Tick);
            sh.render(&mut g);
            assert!(colours(&g) > 20, "{id} at {w}x{h} is not blank ({} colours)", colours(&g));
            assert!(!sh.hits().is_empty(), "{id} at {w}x{h} registers click regions");
        }
    }
}

#[test]
fn each_apps_header_carries_its_own_accent() {
    let mut sh = signed_in();
    let (w, h) = (1280u32, 720u32);
    let mut g = Gfx::new(w, h);
    let mut seen: Vec<(&str, [u8; 3])> = Vec::new();
    let hh = (46.0 * crate::shell::scale_for(w, h)) as usize;
    for app in PApp::ALL {
        assert!(sh.open_for_demo(app.id()));
        sh.render(&mut g);
        // the accent line starts at full strength at the left edge, just under the header
        let i = ((hh + 1) * w as usize + 2) * 4;
        let px = [g.rgba()[i], g.rgba()[i + 1], g.rgba()[i + 2]];
        seen.push((app.id(), px));
    }
    for (i, (a, ca)) in seen.iter().enumerate() {
        for (b, cb) in &seen[i + 1..] {
            let d: i32 = ca.iter().zip(cb).map(|(x, y)| (*x as i32 - *y as i32).abs()).sum();
            assert!(d >= 24, "{a} {ca:?} and {b} {cb:?} look alike");
        }
    }
}

#[test]
fn the_backdrop_is_cached_per_size_and_a_frame_is_cheap() {
    let mut sh = signed_in();
    assert!(sh.open_for_demo("settings"));
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert_eq!(fx::background_cached_size(), Some((1280, 720)));
    let t = std::time::Instant::now();
    for _ in 0..5 {
        sh.render(&mut g);
    }
    // a debug build is an order of magnitude slower than release; this only catches a frame that got absurdly expensive
    assert!(t.elapsed().as_millis() < 5 * 600, "{:?}", t.elapsed());
    let mut small = Gfx::new(800, 450);
    sh.render(&mut small);
    assert_eq!(fx::background_cached_size(), Some((800, 450)));
}

#[test]
fn restyled_buttons_and_dialogs_keep_their_labels_and_keys() {
    use crate::apps::widgets::{render_confirm, Form};
    use crate::ui::R;
    let mut g = Gfx::new(900, 600);
    let f = Form::new("Add").text("id", "x", "").secret("key", "");
    f.render(&mut g, R::new(0.0, 70.0, 900.0, 530.0), 1.0, fx::CYAN, true);
    let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
    for want in ["backdrop", "field", "Save", "Cancel"] {
        assert!(labels.contains(&want), "{want} in {labels:?}");
    }
    assert!(g.hits.iter().any(|h| h.label == "Save" && h.ev == Event::Key(Key::Ctrl('s'))));
    g.hits.clear();
    render_confirm(&mut g, R::new(0.0, 70.0, 900.0, 530.0), 1.0, "Delete?", "Delete");
    assert!(g.hits.iter().any(|h| h.label == "Delete" && h.ev == Event::Key(Key::Char('y'))));
    assert!(g.hits.iter().any(|h| h.label == "Cancel" && h.ev == Event::Key(Key::Esc)));
}

#[test]
fn an_app_can_ask_for_data_and_back_returns_to_it() {
    use crate::apps::{App, AppCx, Flow};
    struct Asker(bool);
    impl App for Asker {
        fn title(&self) -> &'static str {
            "Asker"
        }
        fn icon(&self) -> crate::gfx::Icon {
            crate::gfx::Icon::Ide
        }
        fn accent(&self) -> [u8; 4] {
            [1, 2, 3, 255]
        }
        fn hint(&self) -> String {
            String::new()
        }
        fn handle(&mut self, _: &Event, _: &AppCx) -> Flow {
            self.0 = true;
            Flow::Stay
        }
        fn take_request(&mut self) -> Option<(String, String)> {
            std::mem::take(&mut self.0).then(|| ("data".to_string(), "/x.csv".to_string()))
        }
        fn render(&mut self, _: &mut Gfx, _: crate::ui::R, _: f32, _: &AppCx, _: u64) {}
    }
    let mut sh = signed_in();
    sh.show_app_for_test(Box::new(Asker(false)), PApp::Ide);
    sh.handle(&Event::Key(Key::Char('x')));
    assert_eq!(sh.screen_name(), "app:data", "DATA opened");
    sh.handle(&Event::Key(Key::Esc));
    assert_eq!(sh.screen_name(), "app:ide", "Back returns to the app that asked");
}

#[test]
fn files_dropped_on_the_window_offer_an_import_only_in_window_mode_and_show_where_they_go() {
    let _g = iyakki_agent::hostfiles::test_guard();
    let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-drop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("host")).unwrap();
    std::fs::write(base.join("host/notes.txt"), "hello").unwrap();
    let mut sh = signed_in();
    let mut g = Gfx::new(1280, 720);
    // not window mode: a plain sentence, no dialog
    iyakki_agent::hostfiles::set_enabled(false);
    sh.drop_files(vec![base.join("host/notes.txt")]);
    sh.render(&mut g);
    assert!(sh.toast_text().is_some_and(|t| t.contains("desktop window")), "{:?}", sh.toast_text());
    assert!(sh.host_access_chip().is_none());
    // window mode: the overlay while dragging, then the dialog on the drop
    iyakki_agent::hostfiles::set_enabled(true);
    iyakki_agent::hostfiles::set_host_home(Some(base.join("host")));
    sh.set_drag_hover(true);
    sh.render(&mut g);
    let lit = |g: &Gfx| g.rgba().chunks_exact(4).step_by(41).filter(|p| p[0] > 200 && p[1] > 200).count();
    assert!(lit(&g) > 3, "the drop overlay shows its text");
    sh.set_drag_hover(false);
    sh.drop_files(vec![base.join("host/notes.txt")]);
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label == "Cancel"), "the host-files dialog is on screen");
    // Esc leaves it without copying anything
    sh.handle(&Event::Key(Key::Esc));
    sh.render(&mut g);
    assert!(!sh.hits().iter().any(|h| h.label == "Cancel"), "Esc closed it");
    iyakki_agent::hostfiles::set_enabled(false);
    iyakki_agent::hostfiles::set_host_home(None);
}

#[test]
fn drops_are_routed_by_the_app_on_screen_the_browser_refuses_and_saves_open_the_host_dialog() {
    let _g = iyakki_agent::hostfiles::test_guard();
    let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-route-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("host")).unwrap();
    std::fs::write(base.join("host/n.txt"), "x").unwrap();
    iyakki_agent::hostfiles::set_enabled(true);
    iyakki_agent::hostfiles::set_host_home(Some(base.join("host")));
    let mut sh = signed_in();
    let mut g = Gfx::new(1280, 720);
    // the browser says what to do instead
    assert!(sh.open_for_demo("browser"));
    sh.drop_files(vec![base.join("host/n.txt")]);
    sh.render(&mut g);
    assert!(sh.toast_text().is_some_and(|t| t.contains("does not take dropped files") && t.contains("DATA")), "{:?}", sh.toast_text());
    assert!(!sh.hits().iter().any(|h| h.label == "Cancel"), "no dialog for the browser");
    // an app that wants a file saved on the host gets the save dialog
    assert!(sh.open_for_demo("edit"));
    sh.handle(&Event::Key(Key::Ctrl('H')));
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label == "Cancel"), "the host save dialog is on screen");
    sh.handle(&Event::Key(Key::Esc));
    // the media tile and the IDE ask for their own destinations
    assert!(sh.open_for_demo("media"));
    sh.drop_files(vec![base.join("host/n.txt")]);
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label == "Cancel"), "the import dialog opens over the media tile");
    sh.handle(&Event::Key(Key::Esc));
    iyakki_agent::hostfiles::set_enabled(false);
    iyakki_agent::hostfiles::set_host_home(None);
}
