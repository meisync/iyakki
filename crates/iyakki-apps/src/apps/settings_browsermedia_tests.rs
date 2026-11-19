//! Settings -> Browser -> Video and sound (F-BROX-18, F-BROX-20): the two choices, their keys, the buttons at the usual sizes, the audit
//! lines, and that the choices are kept.

use super::*;
use crate::apps::browser_config as bc;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};

struct Offline;
impl iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
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

fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

#[test]
fn q_walks_the_video_caps_in_a_ring_and_y_turns_the_decoders_off_and_on_and_both_are_kept_and_audited() {
    let cx = admin("bmedia1");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let c = bc::load(&cx);
    assert_eq!((c.video_max_height, c.codecs), (0, true), "no limit and decoders allowed by default");
    for want in [1080, 720, 480, 360, 0] {
        press(&mut a, &cx, 'q');
        assert_eq!(bc::load(&cx).video_max_height, want);
    }
    press(&mut a, &cx, 'q');
    press(&mut a, &cx, 'y');
    let c = bc::load(&cx);
    assert_eq!((c.video_max_height, c.codecs), (1080, false));
    press(&mut a, &cx, 'y');
    assert!(bc::load(&cx).codecs);
    let lines = cx.audit.recent(40).join("\n");
    assert!(lines.contains("Video quality: up to 1080 pixels tall") && lines.contains("decoders: off"), "{lines}");
    // other choices are not disturbed
    assert!(bc::load(&cx).images && bc::load(&cx).js_all);
}

#[test]
fn a_hand_edited_file_with_an_odd_cap_falls_back_to_no_limit() {
    let cx = admin("bmedia2");
    let mut c = bc::load(&cx);
    c.video_max_height = 720;
    bc::save(&cx, &c);
    let path = bc::path(&cx);
    let text = std::fs::read_to_string(&path).unwrap().replace("\"video_max_height\":720", "\"video_max_height\":99999999");
    std::fs::write(&path, text).unwrap();
    assert_eq!(bc::load(&cx).video_max_height, 0);
}

#[test]
fn the_keys_are_reachable_at_the_five_usual_sizes() {
    let cx = admin("bmedia3");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    for &(w, h) in &[(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, &cx, 0);
        for k in ['q', 'y'] {
            let hit = g.hits.iter().find(|h| h.ev == Event::Key(Key::Char(k))).unwrap_or_else(|| panic!("{w}x{h}: {k} reachable"));
            let r = hit.rect;
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {k} {r:?}");
        }
    }
}
