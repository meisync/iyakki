//! Settings -> Browser -> Custom headers: add, edit, enable, disable and delete with keys and with the mouse, the refusals, the
//! value kept in the vault and never in the rule file, masked in the list, the transport getting the header, and the page drawn at
//! four screen sizes with every control inside the screen.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};

struct Offline;
impl iyakki_agent::iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

const SECRET: &str = "dummy-header-secret-4711";

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn page(cx: &AppCx) -> SettingsApp {
    let mut a = SettingsApp::new(cx);
    a.go_to(Tab::Browser);
    a.handle(&Event::Key(Key::Char('H')), cx);
    assert!(a.bh.open, "H opens the list");
    a
}

fn press(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(w, h);
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}

/// Fill the open form and submit it the way Enter would.
fn submit(a: &mut SettingsApp, cx: &AppCx, set: &[(usize, &str)], ack: usize) -> Result<String, String> {
    let Some(Modal::Form(FormKind::BrowserHeader(k), mut f)) = a.modal.take() else { panic!("no header form is open") };
    for (i, v) in set {
        f.set_value(*i, v);
    }
    f.set_choice(6, ack);
    a.bh_submit(cx, &k, &mut f)
}

fn add(a: &mut SettingsApp, cx: &AppCx, name: &str, header: &str, value: &str, sites: &str) -> Result<String, String> {
    press(a, cx, Key::Char('a'));
    submit(a, cx, &[(0, name), (1, header), (2, value), (3, sites), (4, "")], 0)
}

#[test]
fn a_rule_is_added_kept_in_the_vault_and_shown_masked() {
    let cx = admin("bh1");
    let mut a = page(&cx);
    let said = add(&mut a, &cx, "Staging", "X-Api-Key", SECRET, "api.example.test, *.corp.example").unwrap();
    assert!(said.contains("Staging saved"), "{said}");
    let file = std::fs::read_to_string(super::super::browser_headers::rules_path(&cx)).unwrap();
    assert!(file.contains("X-Api-Key") && !file.contains(SECRET), "the rule file holds no value: {file}");
    let rules = super::super::browser_headers::store(&cx).list();
    assert_eq!(rules.len(), 1);
    assert_eq!(cx.services.user_get(&cx.session.user, &format!("webhdr.{}", rules[0].id)).unwrap().as_str(), SECRET);
    let g = draw(&mut a, &cx, 1280, 800);
    let texts: Vec<&String> = g.text_log.iter().map(|t| &t.2).collect();
    assert!(texts.iter().any(|t| t.contains("X-Api-Key") && t.contains("********")), "{texts:?}");
    assert!(!texts.iter().any(|t| t.contains(SECRET)), "the value is never drawn");
}

#[test]
fn apply_hands_the_header_to_the_transport_only_for_matching_sites() {
    let cx = admin("bh2");
    let mut a = page(&cx);
    add(&mut a, &cx, "Staging", "X-Api-Key", SECRET, "hdrtest.example.test").unwrap();
    super::super::browser_headers::apply(&cx);
    let p = iyakki_agent::browserheaders::provider();
    let hit = iyakki_web::Url::parse("https://hdrtest.example.test/a").unwrap();
    let miss = iyakki_web::Url::parse("https://other.example.test/a").unwrap();
    assert_eq!(p(&hit), vec![("X-Api-Key".to_string(), SECRET.to_string())]);
    assert!(p(&miss).is_empty());
    // disabled: kept, not sent
    press(&mut a, &cx, Key::Char('x'));
    assert!(status(&a).contains("is off"), "{}", status(&a));
    assert!(p(&hit).is_empty());
    press(&mut a, &cx, Key::Char('x'));
    assert_eq!(p(&hit).len(), 1);
    // deleted: gone from the transport and from the vault
    let id = super::super::browser_headers::store(&cx).list()[0].id.clone();
    press(&mut a, &cx, Key::Char('d'));
    assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::BrowserHeaderDelete(_), _))), "delete asks first");
    a.bh_delete(&cx, &id);
    assert!(p(&hit).is_empty());
    assert!(cx.services.user_get(&cx.session.user, &format!("webhdr.{id}")).is_err());
}

#[test]
fn refused_names_values_and_filters_say_why_and_save_nothing() {
    let cx = admin("bh3");
    let mut a = page(&cx);
    for h in ["Host", "Content-Length", "Connection", "Transfer-Encoding", "Upgrade", "Proxy-Authorization", "Sec-Fetch-Site"] {
        let e = add(&mut a, &cx, "Bad", h, "v", "a.example.test").unwrap_err();
        assert!(e.contains("set by the browser"), "{h}: {e}");
    }
    assert!(add(&mut a, &cx, "Bad", "X-A", "line\r\nX-Evil: 1", "a.example.test").unwrap_err().contains("printable"));
    assert!(add(&mut a, &cx, "Bad", "X-A", "v", "").unwrap_err().contains("Say which sites"));
    assert!(add(&mut a, &cx, "Bad", "X-A", "v", "*").is_err());
    let e = add(&mut a, &cx, "Auth", "Authorization", "Bearer dummy", "a.example.test").unwrap_err();
    assert!(e.contains("I know"), "{e}");
    press(&mut a, &cx, Key::Char('a'));
    let ok = submit(&mut a, &cx, &[(0, "Auth"), (1, "Authorization"), (2, "Bearer dummy"), (3, "a.example.test")], 1);
    assert!(ok.is_ok(), "{ok:?}");
    assert_eq!(super::super::browser_headers::store(&cx).list().len(), 1, "only the confirmed one was saved");
}

#[test]
fn edit_keeps_the_value_when_the_field_is_left_empty_and_rules_persist() {
    let cx = admin("bh4");
    let mut a = page(&cx);
    add(&mut a, &cx, "Staging", "X-Api-Key", SECRET, "a.example.test").unwrap();
    press(&mut a, &cx, Key::Char('e'));
    let Some(Modal::Form(_, f)) = &a.modal else { panic!("edit form") };
    assert_eq!(f.value(0), "Staging");
    assert_eq!(f.value(2), "", "the saved value is never put back into the form");
    submit(&mut a, &cx, &[(3, "b.example.test")], 0).unwrap();
    let r = super::super::browser_headers::store(&cx).list()[0].clone();
    assert_eq!(r.domains, "b.example.test");
    assert_eq!(cx.services.user_get(&cx.session.user, &format!("webhdr.{}", r.id)).unwrap().as_str(), SECRET);
    // a new page object (a new session of Settings) sees the same rule
    let b = page(&cx);
    assert_eq!(b.bh_len(&cx), 1);
}

#[test]
fn mouse_and_keyboard_reach_every_action_and_esc_goes_back() {
    let cx = admin("bh5");
    let mut a = page(&cx);
    add(&mut a, &cx, "One", "X-One", "v1", "one.example.test").unwrap();
    add(&mut a, &cx, "Two", "X-Two", "v2", "two.example.test").unwrap();
    let g = draw(&mut a, &cx, 1280, 800);
    for label in ["Add", "Edit", "Enable / Disable", "Delete", "Back"] {
        assert!(g.hits.iter().any(|h| h.label == label), "{label} button");
    }
    // click the second row, then the Enable / Disable button (a click sends the key)
    assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Row(1))), "rows are clickable");
    a.handle(&Event::Row(1), &cx);
    assert_eq!(a.sel[a.tab], 1);
    press(&mut a, &cx, Key::Enter);
    let rules = super::super::browser_headers::store(&cx).list();
    assert!(rules[0].enabled && !rules[1].enabled, "Enter switched the chosen rule");
    press(&mut a, &cx, Key::Up);
    assert_eq!(a.sel[a.tab], 0);
    // Esc leaves the list and stays in Settings
    let flow = a.handle(&Event::Key(Key::Esc), &cx);
    assert!(!a.bh.open && !matches!(flow, Flow::Back), "Esc closes the list first");
    // the Browser page has the button that opens it
    let g = draw(&mut a, &cx, 1280, 800);
    assert!(g.hits.iter().any(|h| h.label == "Custom headers"), "button on the Browser page");
}

#[test]
fn the_page_draws_inside_the_window_at_the_four_sizes() {
    let cx = admin("bh6");
    let mut a = page(&cx);
    for (w, h) in [(1280u32, 800u32), (1920, 1080), (1024, 600), (640, 360)] {
        let g = draw(&mut a, &cx, w, h);
        assert!(g.hits.iter().any(|x| x.label == "Add"), "{w}x{h}: an empty list still has Add");
    }
    for i in 0..9 {
        add(&mut a, &cx, &format!("Rule {i}"), &format!("X-H{i}"), "v", &format!("h{i}.example.test, *.long-domain-name-for-the-test.example.org")).unwrap();
    }
    for (w, h) in [(1280u32, 800u32), (1920, 1080), (1024, 600), (640, 360)] {
        for sel in [0usize, 4, 8] {
            a.sel[a.tab] = sel;
            let g = draw(&mut a, &cx, w, h);
            for hit in g.hits.iter().filter(|x| x.rect.w > 0.0) {
                let q = hit.rect;
                assert!(q.x >= -0.5 && q.y >= -0.5 && q.x + q.w <= w as f32 + 0.5 && q.y + q.h <= h as f32 + 0.5, "{w}x{h}: {:?} {:?}", hit.label, q);
            }
            assert!(g.hits.iter().any(|x| x.label == "Delete"), "{w}x{h}");
        }
    }
}
