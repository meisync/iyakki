//! Settings -> Browser -> Known-bad sites (F-BROW-98): the switch, the add and remove forms, the audit lines, the guard the browser
//! consults, and the keys reachable at the usual sizes.

use super::*;
use crate::apps::browser_badsites as bs;
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

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, cx, 0);
    g
}

fn submit(a: &mut SettingsApp, cx: &AppCx, set: &[(usize, &str)]) -> Result<String, String> {
    let Some(Modal::Form(kind, mut f)) = a.modal.take() else { panic!("no form is open") };
    for (i, v) in set {
        f.set_value(*i, v);
    }
    match kind {
        FormKind::BrowserBadAdd => a.browser_bad_add_submit(cx, &f),
        FormKind::BrowserBadRemove => a.browser_bad_remove_submit(cx, &f),
        _ => panic!("another form"),
    }
}

#[test]
fn a_site_is_put_on_the_list_and_taken_off_from_the_page_and_each_change_is_audited_by_name_only() {
    let cx = admin("bbad1");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    assert!(bs::enabled(&cx) && bs::load_list(&cx).len() == 0, "on and empty by default");
    press(&mut a, &cx, 'J');
    assert!(submit(&mut a, &cx, &[(0, "")]).is_err(), "a name is needed");
    press(&mut a, &cx, 'J');
    let m = submit(&mut a, &cx, &[(0, "Bad.Example.org"), (1, "phishing")]).unwrap();
    assert!(m.contains("bad.example.org"), "{m}");
    assert_eq!(bs::load_list(&cx).len(), 1);
    press(&mut a, &cx, 'J');
    assert!(submit(&mut a, &cx, &[(0, "https://x.org/a/file.html")]).is_err(), "a full address is refused, not guessed at");
    press(&mut a, &cx, 'O');
    assert!(submit(&mut a, &cx, &[(0, "nothing.example")]).unwrap_err().contains("not on the list"));
    press(&mut a, &cx, 'O');
    assert!(submit(&mut a, &cx, &[(0, "bad.example.org")]).is_ok());
    assert_eq!(bs::load_list(&cx).len(), 0);
    press(&mut a, &cx, 'J');
    submit(&mut a, &cx, &[(0, "one.example"), (1, "")]).unwrap();
    press(&mut a, &cx, 'J');
    submit(&mut a, &cx, &[(0, "two.example"), (1, "")]).unwrap();
    press(&mut a, &cx, 'O');
    assert!(submit(&mut a, &cx, &[(0, "ALL")]).unwrap().contains("2 removed"));
    let lines = cx.audit.recent(50).join("\n");
    assert!(lines.contains("added bad.example.org") && lines.contains("removed bad.example.org") && lines.contains("emptied"), "{lines}");
    assert!(!lines.contains("phishing"), "the reason is not in the audit");
}

#[test]
fn the_switch_turns_the_list_off_and_on_and_the_guard_the_browser_asks_follows() {
    let cx = admin("bbad2");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    bs::add(&cx, "evil.example", "test").unwrap();
    assert_eq!(bs::load_list(&cx).len(), 1);
    press(&mut a, &cx, 'I');
    assert!(!bs::enabled(&cx), "off");
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("Known-bad site list: off")));
    press(&mut a, &cx, 'I');
    assert!(bs::enabled(&cx), "on again");
}

#[test]
fn the_row_shows_its_state_and_the_keys_are_reachable_at_the_usual_sizes() {
    let cx = admin("bbad3");
    bs::add(&cx, "evil.example", "").unwrap();
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let g = draw(&mut a, &cx, 2560, 1600);
    assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('I'))), "a switch on a big screen");
    for &(w, h) in &[(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        let g = draw(&mut a, &cx, w, h);
        for k in ['I', 'J', 'O'] {
            let hit = g.hits.iter().find(|h| h.ev == Event::Key(Key::Char(k))).unwrap_or_else(|| panic!("{w}x{h}: {k} reachable"));
            let r = hit.rect;
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {k} {r:?}");
        }
    }
}
