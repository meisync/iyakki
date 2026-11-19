//! Settings -> Browser -> Developer tools for the chat agent: off by default, a saved audited per-person switch with a key and a
//! row, nothing kept while off, and the browser tile's note path feeds the buffer only while on.

use super::*;
use crate::apps::tile_harness::TileHarness;
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

#[test]
fn developer_tools_for_the_chat_agent_is_an_off_by_default_audited_per_person_switch() {
    let cx = admin("bdev1");
    let mut t = TileHarness::new(SettingsApp::new(&cx), cx.clone());
    t.app.go_to(Tab::Browser);
    let on = |cx: &AppCx| cx.services.devtools().enabled(&cx.session.user);
    assert!(!on(&cx), "off by default");
    t.send(Event::Key(Key::Char('d')));
    assert!(on(&cx));
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.browser") && l.contains("Developer tools for the chat agent: on")));
    let mut other = cx.clone();
    other.session.user = "amy".into();
    assert!(!on(&other), "another person's choice is their own");
    // the row is there on a big screen, and nothing leaves the window at any size
    // (the Browser page scrolls since F-BROW-92: on a big screen the switch is on the page, reached by the Page keys when it is below the window)
    let mut seen = false;
    for _ in 0..12 {
        let g = t.draw_at(2560, 1600);
        if g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('d'))) {
            seen = true;
            break;
        }
        t.send(Event::Key(Key::PageDown));
    }
    assert!(seen, "a switch on a big screen");
    t.send(Event::Key(Key::Home));
    for &(w, h) in &[(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        let g = t.draw_at(w, h);
        for hit in g.hits.iter().filter(|h| h.ev == Event::Key(Key::Char('d'))) {
            let r = hit.rect;
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {r:?}");
        }
    }
    t.send(Event::Key(Key::Char('d')));
    assert!(!on(&cx));
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("Developer tools for the chat agent: off")));
}

#[test]
fn the_browser_notes_reach_the_buffer_only_while_the_switch_is_on() {
    let cx = admin("bdev2");
    let user = cx.session.user.clone();
    let line = "F\u{1f}GET\u{1f}404\u{1f}7\u{1f}https://a.org/x.js?token=abc\u{1f}";
    assert!(!cx.services.devtools().record_line(&user, line), "dropped while off");
    cx.services.devtools().set_enabled(&user, true);
    assert!(cx.services.devtools().record_line(&user, line));
    let out = cx.services.devtools().failed_report(&user, &iyakki_agent::devtools::Scope::All, 5);
    assert!(out.contains("https://a.org/x.js") && !out.contains("token"), "{out}");
}
