//! Settings -> Browser -> Graphics: the choice is saved and audited (administrators only), the state is shown in plain words, the
//! "Try hardware again" button exists only after a fall back, and the row draws inside the screen at four sizes without overlapping.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::webgraphics::{self, Mode};

struct Offline;
impl iyakki_agent::iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32, s: f32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    a.render(&mut g, R::new(0.0, 72.0 * s, w as f32, h as f32 - 72.0 * s), s, cx, 0);
    g
}

/// Pretend the engine fell back on this machine's card.
fn fall_back(cx: &AppCx) {
    let d = cx.services.dir();
    let st = webgraphics::State { schema: 1, fallbacks: vec![webgraphics::Fallback { gpu: webgraphics::status(d).gpu, since: 1, reason: "test".into() }], ..Default::default() };
    std::fs::create_dir_all(webgraphics::root(d)).unwrap();
    std::fs::write(webgraphics::root(d).join(webgraphics::STATE_FILE), serde_json::to_string(&st).unwrap()).unwrap();
}

#[test]
fn g_cycles_the_choice_saves_it_and_audits_it() {
    let cx = admin("bgfx1");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let mode = |cx: &AppCx| webgraphics::load_mode(cx.services.dir()).0;
    assert_eq!(mode(&cx), Mode::Automatic);
    key(&mut a, &cx, Key::Char('g'));
    assert_eq!(mode(&cx), Mode::Software);
    assert!(status(&a).starts_with("Browser graphics: Software (safe)."), "{}", status(&a));
    key(&mut a, &cx, Key::Char('g'));
    assert_eq!(mode(&cx), Mode::Hardware);
    key(&mut a, &cx, Key::Char('g'));
    assert_eq!(mode(&cx), Mode::Automatic);
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.browser") && l.contains("Browser graphics: Hardware")));
    // the neighbouring keys still work
    key(&mut a, &cx, Key::Char('i'));
    assert!(!super::super::browser_config::load(&cx).images);
}

#[test]
fn a_person_without_settings_rights_sees_the_state_but_cannot_change_it() {
    let mut cx = admin("bgfx2");
    cx.session.policy.manage_settings = false;
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    key(&mut a, &cx, Key::Char('g'));
    assert_eq!(webgraphics::load_mode(cx.services.dir()).0, Mode::Automatic);
    assert!(status(&a).contains("administrator"), "{}", status(&a));
    let g = draw(&mut a, &cx, 1280, 720, 1.0);
    assert!(!g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('g'))), "no button for a person who cannot use it");
}

#[test]
fn try_hardware_again_exists_only_after_a_fall_back_and_forgets_it() {
    let cx = admin("bgfx3");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let g = draw(&mut a, &cx, 1920, 1080, 1.5);
    assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('g'))));
    assert!(!g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('t'))), "nothing to try again");
    key(&mut a, &cx, Key::Char('t'));
    assert!(status(&a).contains("no fall back"), "{}", status(&a));
    fall_back(&cx);
    assert!(webgraphics::status(cx.services.dir()).line.starts_with("Automatic: software because the last session stopped"));
    let g = draw(&mut a, &cx, 1920, 1080, 1.5);
    assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('t'))));
    key(&mut a, &cx, Key::Char('t'));
    assert_eq!(webgraphics::status(cx.services.dir()).line, "Automatic: hardware");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("trying the graphics card again")));
}

#[test]
fn the_row_draws_inside_the_screen_at_four_sizes_without_overlapping() {
    for fallback in [false, true] {
        let cx = admin(&format!("bgfx4{fallback}"));
        if fallback {
            fall_back(&cx);
        }
        let mut a = SettingsApp::new(&cx);
        a.go_to(Tab::Browser);
        for &(w, h, s) in &[(1280u32, 720u32, 1.0f32), (1920, 1080, 1.5), (1024, 600, 0.8), (800, 480, 0.62)] {
            let g = draw(&mut a, &cx, w, h, s);
            let mine: Vec<_> = g.hits.iter().filter(|h| h.ev == Event::Key(Key::Char('g')) || h.ev == Event::Key(Key::Char('t'))).collect();
            assert!(mine.iter().any(|h| h.ev == Event::Key(Key::Char('g'))), "{w}x{h} s{s}: the Change button is there (touch and mouse)");
            if fallback {
                assert!(mine.iter().any(|h| h.ev == Event::Key(Key::Char('t'))), "{w}x{h} s{s}: Try hardware again is there");
            }
            for hit in &mine {
                let r = hit.rect;
                assert!(r.x >= 0.0 && r.y >= 72.0 * s - 0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h} s{s}: {} inside the screen: {r:?}", hit.label);
            }
            // the buttons do not overlap each other or any other control on the page
            for (i, x) in g.hits.iter().enumerate() {
                for y in g.hits.iter().skip(i + 1) {
                    if mine.iter().any(|m| std::ptr::eq(*m, x) || std::ptr::eq(*m, y)) {
                        let (p, q) = (x.rect, y.rect);
                        let overlap = p.x < q.x + q.w - 0.5 && q.x < p.x + p.w - 0.5 && p.y < q.y + q.h - 0.5 && q.y < p.y + p.h - 0.5;
                        assert!(!overlap, "{w}x{h} s{s}: {} overlaps {}: {p:?} {q:?}", x.label, y.label);
                    }
                }
            }
        }
    }
}

#[test]
fn a_newer_setting_file_is_shown_and_not_changed() {
    let cx = admin("bgfx5");
    let f = webgraphics::root(cx.services.dir());
    std::fs::create_dir_all(&f).unwrap();
    std::fs::write(f.join(webgraphics::SETTING_FILE), "{\"schema\": 9, \"mode\": \"software\"}").unwrap();
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    key(&mut a, &cx, Key::Char('g'));
    assert!(status(&a).contains("newer IYAKKI"), "{}", status(&a));
    assert_eq!(std::fs::read_to_string(f.join(webgraphics::SETTING_FILE)).unwrap(), "{\"schema\": 9, \"mode\": \"software\"}");
    let g = draw(&mut a, &cx, 1280, 720, 1.0);
    assert!(!g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('g'))), "no button that cannot work");
}
