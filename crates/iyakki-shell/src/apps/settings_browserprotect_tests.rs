//! Settings -> Browser -> Protect browser data: off by default, a confirmation that names the cost, protection and reset through
//! the vault key, the forgotten-password path, audit lines without secrets, and hit rectangles at the five usual sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::profilestore::{self, SignIn, Status};

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

fn plain_profile(cx: &AppCx) -> std::path::PathBuf {
    let p = iyakki_agent::webengine::profile_dir(cx.services.dir(), "root");
    std::fs::create_dir_all(p.join("data")).unwrap();
    std::fs::write(p.join("data/cookies.sqlite"), "FAKE-COOKIE-JAR").unwrap();
    p
}

#[test]
fn protection_is_off_by_default_and_the_cards_name_the_cost() {
    let cx = admin("bprot1");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    assert_eq!(profilestore::status_of(&cx.services, "root"), Status::Off);
    press(&mut a, &cx, 'P');
    assert!(a.has_modal());
    let Some(Modal::Confirm(k, text)) = &a.modal else { panic!("a confirmation") };
    assert_eq!(*k, ConfirmKind::BrowserProtect);
    assert!(text.contains("forget your password") && text.contains("cannot be recovered") && text.contains("reset"), "{text}");
    press(&mut a, &cx, 'n');
    assert!(!a.has_modal());
    assert_eq!(profilestore::status_of(&cx.services, "root"), Status::Off, "cancel changes nothing");
}

#[test]
fn protect_then_reset_through_the_page_with_audit_lines_and_no_plain_files() {
    let cx = admin("bprot2");
    let plain = plain_profile(&cx);
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    press(&mut a, &cx, 'P');
    press(&mut a, &cx, 'y');
    assert_eq!(profilestore::status_of(&cx.services, "root"), Status::Open, "{:?}", a.status());
    assert!(cx.services.user_get("root", profilestore::VAULT_SECRET).is_ok(), "the key is in the vault");
    // the plain file is gone from the drive; the engine's path leads to the decrypted copy
    assert!(!cx.services.dir().join("web-profile/root.pre-protect").exists());
    assert_eq!(std::fs::read_to_string(plain.join("data/cookies.sqlite")).unwrap(), "FAKE-COOKIE-JAR");
    assert!(std::fs::symlink_metadata(&plain).unwrap().file_type().is_symlink());
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("settings.browser") && l.contains("Browser data protection: on")));
    // sign-out seals, and what the audit holds has no key
    assert_eq!(profilestore::on_sign_out(cx.services.dir(), "root"), Ok(true));
    assert_eq!(profilestore::status_of(&cx.services, "root"), Status::Sealed);
    let key = cx.services.user_get("root", profilestore::VAULT_SECRET).unwrap().to_string();
    assert!(cx.audit.recent(50).iter().all(|l| !l.contains(&key)));
    // reset (key X): everything is deleted, protection is off, the key is gone
    press(&mut a, &cx, 'X');
    press(&mut a, &cx, 'y');
    assert_eq!(profilestore::status_of(&cx.services, "root"), Status::Off);
    assert!(cx.services.user_get("root", profilestore::VAULT_SECRET).is_err());
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("Browser data reset")));
    assert!(!cx.services.dir().join("web-profile/root.sealed").exists());
}

#[test]
fn stop_protecting_brings_the_plain_files_back() {
    let cx = admin("bprot3");
    let plain = plain_profile(&cx);
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    press(&mut a, &cx, 'P');
    press(&mut a, &cx, 'y');
    press(&mut a, &cx, 'P');
    let Some(Modal::Confirm(k, _)) = &a.modal else { panic!("a confirmation") };
    assert_eq!(*k, ConfirmKind::BrowserUnprotect, "the same key now switches it off");
    press(&mut a, &cx, 'y');
    assert_eq!(profilestore::status_of(&cx.services, "root"), Status::Off);
    assert_eq!(std::fs::read_to_string(plain.join("data/cookies.sqlite")).unwrap(), "FAKE-COOKIE-JAR");
    assert!(std::fs::symlink_metadata(&plain).unwrap().file_type().is_dir());
}

#[test]
fn a_forgotten_password_leaves_the_data_locked_until_it_is_reset() {
    let cx = admin("bprot4");
    plain_profile(&cx);
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    press(&mut a, &cx, 'P');
    press(&mut a, &cx, 'y');
    profilestore::on_sign_out(cx.services.dir(), "root").unwrap();
    // an administrator reset the password: the personal vault (and with it the key) starts empty
    cx.services.reset_user_vault("root").unwrap();
    let rep = cx.services.on_login("root", "a-new-password-3", true);
    assert!(matches!(&rep.browser_data, SignIn::Locked(m) if m.contains("reset the browser data")), "{:?}", rep.browser_data);
    assert!(iyakki_agent::webengine::prepare_profile(cx.services.dir(), "root").is_err(), "the engine does not start beside it");
    press(&mut a, &cx, 'X');
    press(&mut a, &cx, 'y');
    assert!(iyakki_agent::webengine::prepare_profile(cx.services.dir(), "root").is_ok(), "after the reset a fresh profile is made");
}

#[test]
fn the_row_and_the_buttons_fit_at_the_five_usual_sizes() {
    let cx = admin("bprot5");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let g = draw(&mut a, &cx, 2560, 1600);
    assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('P'))), "a switch on a big screen");
    for &(w, h) in &[(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        let g = draw(&mut a, &cx, w, h);
        for hit in g.hits.iter().filter(|h| matches!(h.ev, Event::Key(Key::Char('P' | 'X')))) {
            let r = hit.rect;
            assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {r:?}");
        }
        // reachable at every size: the button bar always carries both keys
        assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('P'))), "{w}x{h}: P reachable");
        assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('X'))), "{w}x{h}: X reachable");
    }
}

// ---------------------------------------------------------------- layout checks over the Settings pages

fn overlap(a: R, b: R) -> bool {
    a.x < b.x + b.w - 1.0 && b.x < a.x + a.w - 1.0 && a.y < b.y + b.h - 1.0 && b.y < a.y + a.h - 1.0
}

fn contains(o: R, i: R) -> bool {
    i.x >= o.x - 1.0 && i.y >= o.y - 1.0 && i.x + i.w <= o.x + o.w + 1.0 && i.y + i.h <= o.y + o.h + 1.0
}

/// Problems of one page at one size: partly overlapping interactive rectangles (one inside the other is a row with a button and is
/// fine), a button over a heading, a rectangle past the content's right edge.
fn layout_problems(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Vec<String> {
    controls::HEADINGS.with(|v| v.borrow_mut().clear());
    let g = draw(a, cx, w, h);
    let heads = controls::HEADINGS.with(|v| v.borrow().clone());
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    let content_left = a.nav_rect.x + a.nav_rect.w + 1.0;
    let right = w as f32 - 20.0 * s + 1.0;
    let hits: Vec<_> = g.hits.iter().filter(|x| x.rect.x >= content_left && x.rect.w > 0.0 && x.rect.h > 0.0 && !matches!(x.ev, Event::FocusAt(..))).collect();
    let mut out = Vec::new();
    for (i, p) in hits.iter().enumerate() {
        if p.rect.x + p.rect.w > right {
            out.push(format!("{:?} passes the right edge ({:.0} > {:.0})", p.ev, p.rect.x + p.rect.w, right));
        }
        for hd in &heads {
            if overlap(p.rect, *hd) && !contains(p.rect, *hd) && !contains(*hd, p.rect) || (overlap(p.rect, *hd) && p.rect.h < 60.0 * s) {
                out.push(format!("{:?} covers a heading", p.ev));
            }
        }
        for q in hits.iter().skip(i + 1) {
            if overlap(p.rect, q.rect) && !contains(p.rect, q.rect) && !contains(q.rect, p.rect) {
                out.push(format!("{:?} and {:?} overlap", p.ev, q.ev));
            }
        }
    }
    out
}

const SIZES: [(u32, u32); 5] = [(800, 480), (1280, 720), (1920, 1080), (2560, 1600), (360, 740)];

#[test]
fn the_browser_page_has_no_overlaps_and_nothing_past_the_content_edge_at_five_sizes() {
    let cx = admin("bprot-lay");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    let mut bad = Vec::new();
    for (w, h) in SIZES {
        for p in layout_problems(&mut a, &cx, w, h) {
            bad.push(format!("{w}x{h}: {p}"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn every_settings_page_layout_is_reported() {
    let cx = admin("bprot-all");
    let tabs: Vec<Tab> = SettingsApp::new(&cx).tabs().to_vec();
    let mut report = Vec::new();
    for t in tabs {
        let mut a = SettingsApp::new(&cx);
        a.go_to(t);
        for (w, h) in SIZES {
            let p = layout_problems(&mut a, &cx, w, h);
            if !p.is_empty() {
                report.push(format!("{:?} {w}x{h}: {}", t, p.iter().take(2).cloned().collect::<Vec<_>>().join("; ")));
            }
        }
    }
    eprintln!("LAYOUT REPORT ({} findings)\n{}", report.len(), report.join("\n"));
}

#[test]
fn a_search_with_no_match_shows_a_message_not_a_page_and_esc_gives_the_keys_back() {
    let cx = admin("bprot-nav");
    let mut a = SettingsApp::new(&cx);
    a.go_to(Tab::Browser);
    press(&mut a, &cx, '/');
    for c in "zzqx".chars() {
        press(&mut a, &cx, c);
    }
    assert!(a.nav_focus && a.nav_matches().is_empty());
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(!g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('P'))), "no page buttons under 'Nothing matches'");
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(!a.nav_focus && a.nav_query.value.is_empty(), "one Esc clears and unfocuses");
    a.go_to(Tab::Browser); // typing moved to the first page that matched the first letter
    press(&mut a, &cx, 'P');
    assert!(a.has_modal(), "the page key works again");
}
