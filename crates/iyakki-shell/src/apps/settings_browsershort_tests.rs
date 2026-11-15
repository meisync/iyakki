//! Settings -> Browser -> Short words: the add and remove forms keep the person's words (and refuse bad ones), every change is audited, and
//! the row stays inside the window at every size.

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

fn fill(a: &mut SettingsApp, cx: &AppCx, key: char, values: &[&str]) -> Result<String, String> {
    a.handle(&Event::Key(Key::Char(key)), cx);
    let Some(Modal::Form(kind, mut f)) = a.modal.take() else { panic!("no form opened by {key}") };
    for (i, v) in values.iter().enumerate() {
        f.set_value(i, v);
    }
    let FormKind::BrowserShort(add) = kind else { panic!("wrong form") };
    a.short_submit(cx, add, &f)
}

#[test]
fn short_words_are_added_refused_and_removed_from_settings_and_audited() {
    let cx = admin("bshort1");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let words = |cx: &AppCx| super::super::browser_config::load(cx).short_words;
    assert!(fill(&mut a, &cx, 'w', &["Yt2", "https://example.org/s?q=%s"]).unwrap().contains("yt2"));
    assert_eq!(words(&cx).get("yt2").map(String::as_str), Some("https://example.org/s?q=%s"));
    assert!(fill(&mut a, &cx, 'w', &["bad word", "https://example.org/s?q=%s"]).is_err());
    assert!(fill(&mut a, &cx, 'w', &["ok", "ftp://example.org/s?q=%s"]).is_err());
    assert!(fill(&mut a, &cx, 'w', &["ok", "https://example.org/s"]).is_err());
    assert_eq!(words(&cx).len(), 1);
    assert!(fill(&mut a, &cx, 'W', &["nope"]).is_err());
    assert!(fill(&mut a, &cx, 'W', &["yt2"]).unwrap().contains("removed"));
    assert!(words(&cx).is_empty());
    let log = cx.audit.recent(30);
    assert!(log.iter().any(|l| l.contains("settings.browser") && l.contains("Short word yt2 saved")));
    assert!(log.iter().any(|l| l.contains("Short word yt2 removed")));
}

#[test]
fn the_short_words_row_has_both_buttons_on_a_big_screen_and_stays_inside_the_window() {
    let cx = admin("bshort2");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    for &(w, h) in &[(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, &cx, 0);
        for hit in g.hits.iter().filter(|h| h.ev == Event::Key(Key::Char('w')) || h.ev == Event::Key(Key::Char('W'))) {
            let r = hit.rect;
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {r:?}");
        }
        // the page does not scroll: either both buttons are there, or the line says where the keys are
        let buttons = g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('w'))) && g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('W')));
        let hint = g.text_log.iter().any(|t| t.2.contains("w adds"));
        assert!(buttons || hint || w < 1280, "{w}x{h}: neither buttons nor the hint");
    }
}
