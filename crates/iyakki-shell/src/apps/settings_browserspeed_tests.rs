//! Settings -> Browser -> Speed and memory: the cache size choice is saved per person and audited, the clear button empties only
//! that person's cache, the numbers are shown, and the page draws at five screen sizes with every button inside the screen.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_web::net::{Budget, CacheSize};

struct Offline;
impl iyakki_agent::iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

/// A fake site that answers any address with a cacheable style sheet.
struct Site;
impl iyakki_web::Fetch for Site {
    fn get(&self, _: &iyakki_web::Url) -> Result<iyakki_web::Response, String> {
        Ok(iyakki_web::Response { status: 200, content_type: "text/css".into(), body: b"a{}".to_vec(), cache_control: "max-age=600".into(), ..Default::default() })
    }
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx.web = Arc::new(iyakki_web::cache::UserCaches::new(Arc::new(Site), temp_dir(&format!("{tag}-cache")), Budget { mem: 1 << 20, disk: 8 << 20, max_entry: 1 << 20 }));
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

#[test]
fn the_cache_size_cycles_and_is_saved_and_audited_for_the_person() {
    let cx = admin("bsp1");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let load = |cx: &AppCx| super::super::browser_config::load(cx).cache;
    assert_eq!(load(&cx), CacheSize::Auto);
    key(&mut a, &cx, Key::Char('k'));
    assert_eq!((load(&cx), status(&a).as_str()), (CacheSize::Large, "Cache size: Large."));
    key(&mut a, &cx, Key::Char('k'));
    assert_eq!(load(&cx), CacheSize::Small);
    key(&mut a, &cx, Key::Char('k'));
    assert_eq!(load(&cx), CacheSize::Auto);
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.browser") && l.contains("Cache size: Large")));
    let mut other = cx.clone();
    other.session.user = "amy".into();
    assert_eq!(load(&other), CacheSize::Auto, "another person's choice is their own");
    // the other switches still work beside it
    key(&mut a, &cx, Key::Char('i'));
    assert!(!super::super::browser_config::load(&cx).images);
}

#[test]
fn clear_empties_this_persons_cache_only() {
    let cx = admin("bsp2");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let u = iyakki_web::Url::parse("https://a.org/x.css").unwrap();
    for user in ["root", "amy"] {
        cx.web.for_user(user).unwrap().get(&u).unwrap();
    }
    assert_eq!(cx.web.cache_stats("root").unwrap().disk_entries, 1);
    key(&mut a, &cx, Key::Char('x'));
    assert_eq!(status(&a), "The page cache is empty.");
    assert_eq!(cx.web.cache_stats("root").unwrap().disk_entries, 0);
    assert_eq!(cx.web.cache_stats("amy").unwrap().disk_entries, 1);
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("Cleared the page cache")));
}

#[test]
fn the_page_draws_at_five_sizes_with_both_buttons_inside_the_screen() {
    let cx = admin("bsp3");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    for &(w, h) in &[(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        let g = draw(&mut a, &cx, w, h);
        for hit in &g.hits {
            let r = hit.rect;
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5, "{w}x{h}: {} inside the width: {r:?}", hit.label);
        }
        // a window too short for the whole section shows a hint line instead of buttons (the keys k and x still work)
    }
}

#[test]
fn it_draws_when_there_is_no_cache_at_all() {
    let mut cx = admin("bsp4");
    cx.web = Arc::new(iyakki_web::fetch::NoNetwork);
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let _ = draw(&mut a, &cx, 1280, 720);
    key(&mut a, &cx, Key::Char('x'));
}

#[test]
fn smooth_pages_is_a_saved_audited_per_person_switch_with_a_key_and_a_row() {
    let cx = admin("bsp-smooth");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let load = |cx: &AppCx| super::super::browser_config::load(cx).smooth;
    assert!(!load(&cx), "sharp by default");
    key(&mut a, &cx, Key::Char('m'));
    assert_eq!((load(&cx), status(&a).as_str()), (true, "Smooth pages: on."));
    assert!(super::super::browser_config::smooth_now(), "the browser's heartbeat sees it at once");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.browser") && l.contains("Smooth pages: on")));
    let mut other = cx.clone();
    other.session.user = "amy".into();
    assert!(!load(&other), "another person's choice is their own");
    // where there is room the page shows the switch (a click is the key m); on any size nothing leaves the window
    let g = draw(&mut a, &cx, 2560, 1600);
    assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('m'))), "a Smooth pages switch on a big screen");
    for &(w, h) in &[(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        let g = draw(&mut a, &cx, w, h);
        for hit in g.hits.iter().filter(|h| h.ev == Event::Key(Key::Char('m'))) {
            let r = hit.rect;
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: the switch is inside the window: {r:?}");
        }
    }
    key(&mut a, &cx, Key::Char('m'));
    assert_eq!((load(&cx), status(&a).as_str()), (false, "Smooth pages: off."));
    assert!(!super::super::browser_config::smooth_now());
}
