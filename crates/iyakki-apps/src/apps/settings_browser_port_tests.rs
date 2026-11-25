//! The Browser pages on a fake [`BrowserSettingsPort`] (lane ST-C): a change is saved and audited with the person's name, what the page asks of
//! the shell (dev tools, browser-data protection, the page cache, the custom header rules) comes from the fake, and every page draws at six
//! sizes. No shell, no sleeping: the fake keeps everything in memory and in its own temp folder.

use std::sync::atomic::Ordering;

use super::*;
use super::browser_port::BrowserSettingsPort;
use iyakki_agent_api::HeaderRuleStore;
use crate::apps::browser_badsites as bs;
use crate::apps::browser_config as bc;
use super::browser_port::FakeBrowserPort;
use crate::apps::tests::cx_with_admin_store;

fn app() -> SettingsApp {
    let cx = cx_with_admin_store("sbp-app");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    a
}

fn press(a: &mut SettingsApp, p: &FakeBrowserPort, c: char) {
    a.handle_browser(&Key::Char(c), p);
}

fn say(a: &SettingsApp) -> (String, bool) {
    a.status().cloned().unwrap_or_default()
}

fn open_form(a: &mut SettingsApp) -> (FormKind, Form) {
    match a.modal.take() {
        Some(Modal::Form(k, f)) => (k, f),
        _ => panic!("no form is open"),
    }
}

const SIZES: [(u32, u32); 6] = [(640, 400), (800, 600), (1024, 768), (1280, 720), (1920, 1080), (360, 640)];

#[test]
fn the_browser_page_draws_from_the_fake_at_six_sizes_with_its_switches_reachable() {
    let p = FakeBrowserPort::new("sbp-draw", true);
    for (w, h) in SIZES {
        let mut a = app();
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
        a.render_browser(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), s, &p);
        assert!(!g.hits.is_empty(), "{w}x{h}: nothing to press");
    }
}

#[test]
fn a_switch_is_saved_in_the_persons_file_and_audited() {
    let p = FakeBrowserPort::new("sbp-switch", true);
    let mut a = app();
    let before = bc::load(&p).images;
    press(&mut a, &p, 'i');
    assert_eq!(bc::load(&p).images, !before);
    assert!(p.audited().iter().any(|(act, what)| act == "settings.browser" && what.starts_with("Pictures:")), "{:?}", p.audited());
    assert!(say(&a).1);
}

#[test]
fn the_developer_tools_switch_is_the_fakes_and_is_audited() {
    let p = FakeBrowserPort::new("sbp-devtools", true);
    let mut a = app();
    assert!(!p.devtools.enabled("ada"));
    press(&mut a, &p, 'd');
    assert!(p.devtools.enabled("ada"));
    press(&mut a, &p, 'd');
    assert!(!p.devtools.enabled("ada"));
    let n = p.audited().iter().filter(|(act, what)| act == "settings.browser" && what.starts_with("Developer tools")).count();
    assert_eq!(n, 2);
}

#[test]
fn the_page_cache_is_emptied_through_the_port() {
    let p = FakeBrowserPort::new("sbp-cache", true);
    let mut a = app();
    press(&mut a, &p, 'x');
    assert_eq!(p.cache_cleared.load(Ordering::SeqCst), 1);
    assert!(p.audited().contains(&("settings.browser".to_string(), "Cleared the page cache".to_string())));
}

#[test]
fn protecting_browser_data_asks_first_and_the_answer_and_a_refusal_are_audited() {
    let p = FakeBrowserPort::new("sbp-protect", true);
    let mut a = app();
    press(&mut a, &p, 'P');
    let Some(Modal::Confirm(kind, _)) = a.modal.take() else { panic!("a card should ask first") };
    assert_eq!(kind, ConfirmKind::BrowserProtect);
    a.bp_confirm(&p, &kind);
    assert_eq!(p.profile_status(), iyakki_shared::profilestore::Status::Open);
    assert!(p.audited().contains(&("settings.browser".to_string(), "Browser data protection: on".to_string())));
    // now it asks to switch it off, and a locked vault refuses
    press(&mut a, &p, 'P');
    let Some(Modal::Confirm(kind, _)) = a.modal.take() else { panic!("a card should ask first") };
    assert_eq!(kind, ConfirmKind::BrowserUnprotect);
    p.profile_refuses.store(true, Ordering::SeqCst);
    a.bp_confirm(&p, &kind);
    assert!(!say(&a).1 && say(&a).0.contains("vault"), "{:?}", say(&a));
    assert!(p.audited().iter().any(|(_, what)| what == "Browser data protection: off failed"));
    assert_eq!(p.profile_status(), iyakki_shared::profilestore::Status::Open, "a refused change leaves it as it was");
    // reset deletes
    a.bp_confirm(&p, &ConfirmKind::BrowserReset);
    assert_eq!(p.profile_status(), iyakki_shared::profilestore::Status::Off);
}

#[test]
fn known_bad_sites_are_added_and_removed_with_the_site_name_only_in_the_audit() {
    let p = FakeBrowserPort::new("sbp-bad", true);
    let mut a = app();
    assert!(bs::enabled(&p));
    press(&mut a, &p, 'I');
    assert!(!bs::enabled(&p));
    press(&mut a, &p, 'J');
    let (_, mut f) = open_form(&mut a);
    f.set_value(0, "Bad.Example.org");
    f.set_value(1, "phishing page");
    assert!(a.browser_bad_add_submit(&p, &f).unwrap().contains("bad.example.org"));
    assert_eq!(bs::load_list(&p).len(), 1);
    assert!(p.audited().iter().any(|(_, what)| what.contains("added bad.example.org") && !what.contains("phishing")));
    press(&mut a, &p, 'O');
    let (_, mut f) = open_form(&mut a);
    f.set_value(0, "all");
    assert!(a.browser_bad_remove_submit(&p, &f).unwrap().contains("empty"));
    assert_eq!(bs::load_list(&p).len(), 0);
}

#[test]
fn a_short_word_is_saved_into_the_fake_persons_file() {
    let p = FakeBrowserPort::new("sbp-short", true);
    let mut a = app();
    press(&mut a, &p, 'w');
    let (_, mut f) = open_form(&mut a);
    f.set_value(0, "yt");
    f.set_value(1, "https://www.youtube.com/results?search_query=%s");
    a.short_submit(&p, true, &f).unwrap();
    assert!(bc::load(&p).short_words.contains_key("yt"));
    assert!(p.audited().iter().any(|(act, what)| act == "settings.browser" && what.contains("yt")));
    press(&mut a, &p, 'W');
    let (_, mut f) = open_form(&mut a);
    f.set_value(0, "yt");
    a.short_submit(&p, false, &f).unwrap();
    assert!(!bc::load(&p).short_words.contains_key("yt"));
}

#[test]
fn custom_header_rules_go_through_the_fake_store_and_the_transport_is_told_to_read_them_again() {
    let p = FakeBrowserPort::new("sbp-headers", true);
    let mut a = app();
    press(&mut a, &p, 'H');
    assert!(a.bh.open);
    // draws as the sub-page at six sizes
    for (w, h) in SIZES {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        a.render_browser(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), 1.0, &p);
    }
    press(&mut a, &p, 'a');
    let (kind, mut f) = open_form(&mut a);
    f.set_value(0, "Staging");
    f.set_value(1, "X-Staging");
    f.set_value(2, "secret-value");
    f.set_value(3, "staging.example.org");
    let FormKind::BrowserHeader(k) = kind else { panic!("another form") };
    a.bh_submit(&p, &k, &mut f).unwrap();
    assert_eq!(p.headers.list().len(), 1);
    assert!(p.refreshed.load(Ordering::SeqCst) >= 1, "the transport reads the rules again");
    assert!(p.audited().iter().any(|(act, what)| act == "settings.browser.headers" && what.contains("Staging") && !what.contains("secret-value")));
    // the rule is switched off with x
    press(&mut a, &p, 'x');
    assert!(!p.headers.list()[0].enabled);
    // and deleted
    let id = p.headers.list()[0].id.clone();
    a.bh_delete(&p, &id);
    assert!(p.headers.list().is_empty());
}

#[test]
fn a_site_that_is_not_in_the_list_cannot_be_forgotten_and_the_error_says_so() {
    let p = FakeBrowserPort::new("sbp-sites", true);
    let mut a = app();
    let f = Form::new("Forget a site").text("Site (as listed)", "meet.example.org", "");
    let e = a.site_forget_submit(&p, &f).unwrap_err();
    assert!(e.contains("not in the list"), "{e}");
    assert!(p.audited().is_empty());
}

#[test]
fn a_certificate_check_of_a_site_the_account_may_not_open_is_refused_before_any_connection() {
    let mut p = FakeBrowserPort::new("sbp-cert", true);
    p.policy_mut().net_hosts = vec!["only.example.org".into()];
    let mut a = app();
    let f = Form::new("Certificate of a site").text("Address", "other.example.org", "");
    let e = a.browser_cert_submit(&p, &f).unwrap_err();
    assert!(e.contains("may not open"), "{e}");
    assert!(p.audited().is_empty(), "no look, no audit line");
    let f = Form::new("Certificate of a site").text("Address", "not a site!", "");
    assert!(a.browser_cert_submit(&p, &f).is_err());
}

#[test]
fn the_proxy_form_saves_next_to_the_fake_persons_data_and_is_audited() {
    let p = FakeBrowserPort::new("sbp-proxy", true);
    let mut a = app();
    let f = Form::new("Browser proxy").text("Proxy", "", "");
    // an empty proxy turns it off and says so
    assert!(a.browser_proxy_submit(&p, &f).is_ok());
    assert!(p.audited().iter().any(|(act, what)| act == "settings.browser" && what == "Browser proxy: off"));
}

#[test]
fn video_quality_and_decoders_are_saved_and_audited() {
    let p = FakeBrowserPort::new("sbp-media", true);
    let mut a = app();
    let was = bc::load(&p);
    press(&mut a, &p, 'q');
    assert_ne!(bc::load(&p).video_max_height, was.video_max_height);
    press(&mut a, &p, 'y');
    assert_eq!(bc::load(&p).codecs, !was.codecs);
    let n = p.audited().iter().filter(|(act, _)| act == "settings.browser").count();
    assert_eq!(n, 2);
}

#[test]
fn the_graphics_keys_are_for_administrators_only() {
    let p = FakeBrowserPort::new("sbp-gfx-user", false);
    let mut a = app();
    press(&mut a, &p, 'g');
    assert!(p.audited().is_empty(), "{:?}", p.audited());
    assert!(!say(&a).1 && say(&a).0.contains("administrator"), "{:?}", say(&a));
    // an administrator changes the mode and the change is audited
    let q = FakeBrowserPort::new("sbp-gfx-admin", true);
    press(&mut a, &q, 'g');
    assert!(say(&a).1, "{:?}", say(&a));
    assert!(q.audited().iter().any(|(act, what)| act == "settings.browser" && what.starts_with("Browser graphics:")));
}
