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
    let hh = (46.0 * (w as f32 / 1280.0).min(h as f32 / 720.0)) as usize;
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
