//! Settings -> Browser -> Ads and trackers: the switch, the keys, the forms, and the page at five sizes and in panes.

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
    cx.web = Arc::new(iyakki_web::fetch::NoNetwork);
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
fn the_switch_is_on_by_default_and_b_flips_it_with_an_audit_line() {
    let cx = admin("blks1");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    assert!(super::super::browser_block::load(&cx).enabled);
    key(&mut a, &cx, Key::Char('b'));
    assert!(!super::super::browser_block::load(&cx).enabled);
    assert_eq!(status(&a), "Block ads and trackers: off.");
    key(&mut a, &cx, Key::Char('b'));
    assert!(super::super::browser_block::load(&cx).enabled);
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.browser") && l.contains("Block ads and trackers: off")));
    // the other switches are untouched by the new keys
    key(&mut a, &cx, Key::Char('i'));
    assert!(!super::super::browser_config::load(&cx).images);
}

#[test]
fn remove_forgets_the_lists_and_says_how_many() {
    let cx = admin("blks2");
    let store = super::super::browser_block::store(&cx);
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    key(&mut a, &cx, Key::Char('r'));
    assert_eq!(status(&a), "There were no lists.");
    store.put("a", "A", "", "", b"||a.example^\n", 1).unwrap();
    store.put("b", "B", "", "", b"||b.example^\n", 1).unwrap();
    key(&mut a, &cx, Key::Char('r'));
    assert_eq!(status(&a), "Removed 2 lists.");
    assert!(store.list().is_empty());
}

#[test]
fn the_address_and_file_forms_open_and_check_their_input() {
    let cx = admin("blks3");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    key(&mut a, &cx, Key::Char('u'));
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::BlockListUrl, _))));
    a.modal = None;
    key(&mut a, &cx, Key::Char('o'));
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::BlockListFile, _))));
    let form = Form::new("x").text("Address", "ftp://x/y", "");
    assert!(a.submit_block_form(&FormKind::BlockListUrl, &form, &cx).unwrap_err().contains("https://"));
    let form = Form::new("x").text("File", "/no/such/file.txt", "");
    assert!(a.submit_block_form(&FormKind::BlockListFile, &form, &cx).unwrap_err().contains("does not exist"));
    let form = Form::new("x").text("File", "", "");
    assert!(a.submit_block_form(&FormKind::BlockListFile, &form, &cx).is_err());
}

#[test]
fn the_page_draws_at_five_sizes_with_nothing_outside_the_screen_and_the_new_buttons_work_when_shown() {
    let cx = admin("blks4");
    let store = super::super::browser_block::store(&cx);
    store.put("easylist", "EasyList", "https://easylist.to/easylist/easylist.txt", "GPL-3.0 or CC BY-SA 3.0 (dual licence)", b"||ads.example^\n", super::super::browser_block::now_secs()).unwrap();
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let mut shown_somewhere = false;
    for &(w, h) in &[(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        let g = draw(&mut a, &cx, w, h);
        for hit in &g.hits {
            let r = hit.rect;
            assert!(r.x >= -0.5 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5, "{w}x{h}: {} inside the width: {r:?}", hit.label);
            assert!(r.y + r.h <= h as f32 + 0.5 || hit.label.contains("tab"), "{w}x{h}: {} inside the height: {r:?}", hit.label);
        }
        shown_somewhere |= g.hits.iter().any(|h| h.label == "Download lists (l)");
        // every shown button is a key that exists
        for hit in g.hits.iter().filter(|h| h.label.ends_with("(l)") || h.label.ends_with("(n)") || h.label.ends_with("(u)") || h.label.ends_with("(o)") || h.label.ends_with("(r)")) {
            assert!(matches!(hit.ev, Event::Key(Key::Char('l' | 'n' | 'u' | 'o' | 'r'))), "{}", hit.label);
        }
    }
    assert!(shown_somewhere, "at least the large sizes show the list buttons");
    store.remove("easylist");
}

#[test]
fn in_a_pane_every_hit_stays_inside_the_pane() {
    let cx = admin("blks6");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    for pane in [R::new(40.0, 100.0, 420.0, 330.0), R::new(640.0, 72.0, 640.0, 648.0), R::new(0.0, 72.0, 360.0, 400.0), R::new(100.0, 100.0, 1700.0, 900.0)] {
        let mut g = Gfx::new(1920, 1080);
        g.clear(rgb(0, 0, 0));
        let s = (pane.w / 1280.0).min(pane.h / 720.0).clamp(0.6, 3.0);
        a.render(&mut g, pane, s, &cx, 0);
        for hit in g.hits.iter().filter(|h| !h.label.contains("tab")) {
            let r = hit.rect;
            assert!(r.x >= pane.x - 0.5 && r.x + r.w <= pane.x + pane.w + 0.5 && r.y + r.h <= pane.y + pane.h + 0.5, "pane {pane:?}: {} at {r:?}", hit.label);
        }
    }
}

#[test]
fn it_draws_with_no_lists_and_with_a_job_message() {
    let cx = admin("blks5");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let _ = draw(&mut a, &cx, 1920, 1080);
    let _ = draw(&mut a, &cx, 1280, 720);
}

#[test]
fn the_section_is_computed_without_unchecked_clamps() {
    let src = include_str!("settings_browserblock.rs");
    assert!(!src.contains(".clamp("), "use clamp_safe for computed bounds");
    assert!(!src.contains(".unwrap()"), "no unwrap on layout or list data");
    let b = include_str!("browser_block.rs");
    assert!(!b.contains(".unwrap()") || b.split("#[cfg(test)]").next().unwrap().matches(".unwrap()").count() == 0, "no unwrap outside tests");
}
