//! Settings -> Browser -> Developer tools for the chat agent: off by default, a saved audited per-person switch with a key and a
//! row, nothing kept while off, and the browser tile's note path feeds the buffer only while on.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};

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

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}

#[test]
fn developer_tools_for_the_chat_agent_is_an_off_by_default_audited_per_person_switch() {
    let cx = admin("bdev1");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let on = |cx: &AppCx| cx.services.devtools.enabled(&cx.session.user);
    assert!(!on(&cx), "off by default");
    a.handle(&Event::Key(Key::Char('d')), &cx);
    assert!(on(&cx));
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.browser") && l.contains("Developer tools for the chat agent: on")));
    let mut other = cx.clone();
    other.session.user = "amy".into();
    assert!(!on(&other), "another person's choice is their own");
    // the row is there on a big screen, and nothing leaves the window at any size
    let g = draw(&mut a, &cx, 2560, 1600);
    assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('d'))), "a switch on a big screen");
    for &(w, h) in &[(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        let g = draw(&mut a, &cx, w, h);
        for hit in g.hits.iter().filter(|h| h.ev == Event::Key(Key::Char('d'))) {
            let r = hit.rect;
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {r:?}");
        }
    }
    a.handle(&Event::Key(Key::Char('d')), &cx);
    assert!(!on(&cx));
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("Developer tools for the chat agent: off")));
}

#[test]
fn the_browser_notes_reach_the_buffer_only_while_the_switch_is_on() {
    let cx = admin("bdev2");
    let user = cx.session.user.clone();
    let line = "F\u{1f}GET\u{1f}404\u{1f}7\u{1f}https://a.org/x.js?token=abc\u{1f}";
    assert!(!cx.services.devtools.record_line(&user, line), "dropped while off");
    cx.services.devtools.set_enabled(&user, true);
    assert!(cx.services.devtools.record_line(&user, line));
    let out = cx.services.devtools.failed_report(&user, &iyakki_agent::devtools::Scope::All, 5);
    assert!(out.contains("https://a.org/x.js") && !out.contains("token"), "{out}");
}
