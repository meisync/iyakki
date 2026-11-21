//! Settings -> Browser -> Proxy (F-BROX-22): the form, the saved text the browser's network reads, the refusals in words, the audit
//! line with the address only, and the button reachable at the usual sizes.

use super::*;
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

fn submit(a: &mut SettingsApp, cx: &AppCx, v: &str) -> Result<String, String> {
    let Some(Modal::Form(kind, mut f)) = a.modal.take() else { panic!("no form is open") };
    f.set_value(0, v);
    match kind {
        FormKind::BrowserProxy => a.browser_proxy_submit(cx, &f),
        _ => panic!("another form"),
    }
}

#[test]
fn a_proxy_is_set_changed_and_turned_off_from_the_page_and_the_browsers_network_reads_it() {
    let cx = admin("bprox1");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    assert!(iyakki_proxy::proxyset::load(cx.data_dir()).is_none());
    a.handle(&Event::Key(Key::Char('N')), &cx);
    let m = submit(&mut a, &cx, "proxy.example:3128").unwrap();
    assert!(m.contains("proxy.example:3128"), "{m}");
    let p = iyakki_proxy::proxyset::load(cx.data_dir()).expect("saved");
    assert_eq!((p.host.as_str(), p.port), ("proxy.example", 3128));
    a.handle(&Event::Key(Key::Char('N')), &cx);
    assert!(submit(&mut a, &cx, "").unwrap().contains("directly"));
    assert!(iyakki_proxy::proxyset::load(cx.data_dir()).is_none());
    let lines = cx.audit.recent(20).join("\n");
    assert!(lines.contains("Browser proxy: proxy.example:3128") && lines.contains("Browser proxy: off"), "{lines}");
}

#[test]
fn a_sign_in_or_nonsense_is_refused_in_words_and_changes_nothing() {
    let cx = admin("bprox2");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    a.handle(&Event::Key(Key::Char('N')), &cx);
    submit(&mut a, &cx, "one.example:8080").unwrap();
    for bad in ["user:secret@two.example:8080", "ftp://two.example:21", "not a proxy", "two.example"] {
        a.handle(&Event::Key(Key::Char('N')), &cx);
        assert!(submit(&mut a, &cx, bad).is_err(), "{bad}");
    }
    assert_eq!(iyakki_proxy::proxyset::text(cx.data_dir()), "one.example:8080");
    assert!(!cx.audit.recent(50).join("\n").contains("secret"));
}

#[test]
fn the_button_is_reachable_at_the_usual_sizes() {
    let cx = admin("bprox3");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    // the Browser tab scrolls: draw once to learn its length, go to the end, draw again
    let draw = |a: &mut SettingsApp| {
        let mut g = Gfx::new(2560, 1600);
        g.clear(rgb(0, 0, 0));
        a.render(&mut g, R::new(0.0, 72.0, 2560.0, 1528.0), 2.0, &cx, 0);
        g
    };
    let mut seen = false;
    a.bh.scroll = 0.0;
    for _ in 0..60 {
        let g = draw(&mut a);
        seen |= g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('N')) && h.rect.y >= 72.0 && h.rect.y + h.rect.h <= 1600.5);
        if seen || a.bh.scroll >= a.bh.max - 0.5 {
            break;
        }
        a.bh.scroll = (a.bh.scroll + 150.0).min(a.bh.max);
    }
    assert!(seen, "N is drawn somewhere along the Browser tab");
}
