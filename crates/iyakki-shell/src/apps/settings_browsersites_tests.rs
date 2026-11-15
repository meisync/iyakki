//! Settings -> Browser -> Sites you allowed or blocked: the list shows what is remembered, nothing when nothing is, F forgets one site
//! (and the device chosen for it), a site that is not listed is refused, the change is audited, and the button fits at the usual sizes.

use super::*;
use crate::apps::browser_media::{self, Answer, MediaPerms};
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

fn forget(a: &mut SettingsApp, cx: &AppCx, site: Option<&str>) -> Result<String, String> {
    a.handle(&Event::Key(Key::Char('F')), cx);
    let Some(Modal::Form(kind, mut f)) = a.modal.take() else { return Err("no form opened".into()) };
    assert!(matches!(kind, FormKind::BrowserSite));
    if let Some(s) = site {
        f.set_value(0, s);
    }
    a.site_forget_submit(cx, &f)
}

#[test]
fn one_site_is_forgotten_with_its_device_and_a_site_that_is_not_listed_is_refused() {
    let cx = admin("bsites1");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    // nothing remembered: F says so and opens no form
    assert!(forget(&mut a, &cx, None).is_err());
    assert!(a.status().is_some_and(|s| s.0.contains("No site is remembered")));
    let mut m = MediaPerms::default();
    m.apply("https://meet.example/r", iyakki_webview::proto::media_kind::CAMERA, Answer::Always);
    m.set_site_device("meet.example", iyakki_webview::proto::media_kind::CAMERA, "USB Cam: x");
    m.apply("https://shop.example/", iyakki_webview::proto::media_kind::MICROPHONE, Answer::Block);
    browser_media::save(&cx, &m);
    assert_eq!(browser_media::load(&cx).list().len(), 2);
    assert!(forget(&mut a, &cx, Some("nowhere.example")).is_err());
    assert_eq!(browser_media::load(&cx).list().len(), 2, "a refused name changes nothing");
    let ok = forget(&mut a, &cx, Some("  MEET.example ")).unwrap();
    assert!(ok.contains("ask again"), "{ok}");
    let after = browser_media::load(&cx);
    assert_eq!(after.list(), vec![("shop.example".to_string(), "microphone blocked".to_string())]);
    assert_eq!(after.site_device("meet.example", iyakki_webview::proto::media_kind::CAMERA), "", "the device chosen for the site is forgotten too");
    assert!(cx.audit.recent(30).iter().any(|l| l.contains("settings.browser") && l.contains("meet.example forgotten")));
    // with one site left the form is filled in
    a.handle(&Event::Key(Key::Char('F')), &cx);
    let Some(Modal::Form(_, f)) = a.modal.take() else { panic!("no form") };
    assert_eq!(f.value(0), "shop.example");
}

#[test]
fn the_list_is_drawn_only_when_something_is_remembered_and_the_button_fits() {
    use crate::apps::tile_harness::TileHarness;
    let cx = admin("bsites2");
    let mut t = TileHarness::new(SettingsApp::new(&cx), cx.clone());
    t.app.go_to(Tab::Browser);
    let has = |g: &crate::gfx::Gfx| g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('F')));
    assert!(!has(&t.draw_at(1920, 1080)), "nothing remembered: no list and no button");
    let mut m = MediaPerms::default();
    m.apply("https://meet.example/r", iyakki_webview::proto::media_kind::CAMERA, Answer::Always);
    browser_media::save(&cx, &m);
    for (w, h) in [(2560u32, 1600u32), (1920, 1080), (1280, 720)] {
        let g = t.draw_at(w, h);
        let r = g.hits.iter().find(|x| x.ev == Event::Key(Key::Char('F'))).map(|x| x.rect);
        // the page scrolls: a button below the window is reached by scrolling, one that is drawn must be inside the window sideways
        if let Some(r) = r {
            assert!(r.x >= 0.0 && r.x + r.w <= w as f32, "{w}x{h}: inside sideways {r:?}");
        }
    }
    // the list is at the end of the page, which scrolls: End brings it into the window
    let _ = t.draw_at(1920, 1080);
    t.send(Event::Key(Key::End));
    assert!(has(&t.draw_at(1920, 1080)), "the button is there at the usual size once the page is scrolled to its end");
}
