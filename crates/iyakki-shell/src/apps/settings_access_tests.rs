//! Settings -> Accessibility: every row is a switch with a key and a click, the choices are kept per person in the files the shell and the
//! assistant share, the page is in the list for everybody, and every Settings page can be walked from the keyboard (the walk finds each
//! clickable region of the page). A listing of what a page has by way of keys of its own is printed for the owner.

use super::*;
use crate::access::{self, Entry};
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_agent::accessibility as acc;

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
    let s = crate::shell::scale_for(w, h);
    access::start_frame();
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}

fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

#[test]
fn the_page_is_there_for_everybody_and_found_by_its_words() {
    let cx = admin("acc-set-tab");
    let a = SettingsApp::new(&cx);
    assert!(a.tabs().contains(&Tab::Accessibility));
    assert_eq!(Tab::Accessibility.group(), CATEGORIES[4]);
    for w in ["screen reader", "high contrast", "sticky keys", "slow keys", "blind"] {
        assert!(Tab::Accessibility.keywords().contains(w), "{w}");
    }
    let mut cx2 = cx_with_admin_store("acc-set-user");
    cx2.session.role = iyakki_sec::policy::Role::User;
    cx2.session.policy = iyakki_sec::policy::Role::User.template();
    assert!(SettingsApp::new(&cx2).tabs().contains(&Tab::Accessibility), "no administrator needed");
    assert!(Tab::ORDER.contains(&Tab::Accessibility));
}

#[test]
fn each_key_changes_one_choice_keeps_it_for_this_person_and_audits_it() {
    let cx = admin("acc-set-keys");
    let dir = cx.services.dir().to_path_buf();
    let mut a = SettingsApp::new(&cx).on(Tab::Accessibility);
    for (c, check) in [
        ('n', (|x: &acc::Access| !x.speak_notices) as fn(&acc::Access) -> bool),
        ('p', |x| x.large_pointer),
        ('f', |x| x.big_ring),
        ('m', |x| x.reduce_motion),
        ('k', |x| x.sticky_keys),
        ('r', |x| x.screen_reader),
    ] {
        press(&mut a, &cx, c);
        assert!(check(&acc::load(&dir, "root")), "{c}");
    }
    assert!(acc::machine_reader(&dir), "the reader is also the computer's choice");
    assert!(!acc::load(&dir, "amy").screen_reader, "another person is untouched");
    // slow keys cycle through the choices and come back to off
    let mut seen = Vec::new();
    for _ in 0..acc::SLOW_CHOICES.len() {
        press(&mut a, &cx, 'w');
        seen.push(acc::load(&dir, "root").slow_keys_ms);
    }
    assert_eq!(seen, vec![300, 500, 800, 1200, 0]);
    // high contrast is the theme of Settings -> Display
    press(&mut a, &cx, 'c');
    assert!(acc::high_contrast(&dir, "root") && crate::theme::is_high_contrast());
    press(&mut a, &cx, 'c');
    assert!(!acc::high_contrast(&dir, "root") && !crate::theme::is_high_contrast());
    // text size is the one of Display
    press(&mut a, &cx, 'u');
    assert_eq!(crate::uisize::load(&dir, "root"), crate::uisize::NORMAL + 1);
    press(&mut a, &cx, 'U');
    crate::uisize::apply(None);
    let audit = std::fs::read_to_string(cx.audit.path()).unwrap_or_default();
    assert!(audit.contains("settings.accessibility"), "{audit}");
    assert!(!audit.contains("rootpass"));
}

#[test]
fn the_reader_without_a_voice_is_said_on_the_page_and_in_the_message() {
    let cx = admin("acc-set-novoice");
    let mut a = SettingsApp::new(&cx).on(Tab::Accessibility);
    press(&mut a, &cx, 'r');
    let (msg, ok) = a.status.clone().expect("a message");
    assert!(!ok && msg.contains("no reading voice is installed") && msg.contains("Settings, Voice"), "{msg}");
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.text_log.iter().any(|(_, _, t)| t.contains("No reading voice is installed")), "the row says it too");
    // the test key says what went wrong instead of pretending
    press(&mut a, &cx, 't');
    assert!(a.status.clone().unwrap().0.contains("could not speak"), "{:?}", a.status);
    // what the page says is queued for the reader (it is dropped when the reader is off)
    assert!(!access::take_announcements().is_empty());
}

#[test]
fn the_page_draws_eight_switches_that_describe_themselves_and_clicking_one_sends_its_key() {
    let cx = admin("acc-set-draw");
    let mut a = SettingsApp::new(&cx).on(Tab::Accessibility);
    press(&mut a, &cx, 'k');
    for (w, h) in [(1280, 720), (1920, 1080)] {
        let g = draw(&mut a, &cx, w, h);
        let said: Vec<String> = g.hits.iter().filter_map(access::node_of).map(|n| n.speak()).collect();
        for want in ["Screen reader, switch, off", "Sticky keys, switch, on", "High contrast, switch, off", "Large mouse pointer, switch, off", "Big focus ring, switch, off", "Reduce motion, switch, off", "Slow keys: off, switch, off", "Read messages and errors aloud, switch, on"] {
            assert!(said.iter().any(|s| s == want), "{w}x{h}: {want:?} not in {said:?}");
        }
        let hit = g.hits.iter().find(|h| h.label == "Reduce motion").unwrap();
        assert_eq!(hit.ev, Event::Key(Key::Char('m')));
    }
}

/// The controls of every page against the walk: nothing a mouse can press is missing from it, and the keys the page has of its own are
/// listed (a page without any is reachable through the walk only).
#[test]
fn every_clickable_region_of_every_settings_page_is_in_the_keyboard_walk() {
    let cx = admin("acc-set-reach");
    let mut report: Vec<String> = Vec::new();
    let mut walk_only_pages = Vec::new();
    let tabs: Vec<Tab> = SettingsApp::new(&cx).tabs().to_vec();
    for tab in tabs {
        let mut a = SettingsApp::new(&cx).on(tab);
        for (w, h) in [(1280u32, 720u32), (1920, 1080)] {
            let g = draw(&mut a, &cx, w, h);
            let screen = (w as f32, h as f32);
            let walk: Vec<Entry> = access::entries(&g.hits, screen);
            let mut left_out = Vec::new();
            let mut native = 0;
            for hit in &g.hits {
                let r = hit.rect;
                let on_screen = r.w >= 2.0 && r.h >= 2.0 && r.x + r.w / 2.0 >= 0.0 && r.y + r.h / 2.0 >= 0.0 && r.x + r.w / 2.0 < screen.0 && r.y + r.h / 2.0 < screen.1;
                let control = !matches!(hit.ev, Event::MouseMove(..) | Event::MouseDown(..) | Event::MouseUp(..) | Event::Scroll(_) | Event::ScrollPx(_) | Event::Tick | Event::Paste(_));
                if !on_screen || !control {
                    continue;
                }
                if matches!(hit.ev, Event::Key(_)) {
                    native += 1;
                }
                if !walk.iter().any(|e| g.hits[e.hit].label == hit.label && g.hits[e.hit].ev == hit.ev) {
                    left_out.push(format!("{:?}/{:?}", hit.label, hit.ev));
                }
            }
            // the walk, going round once, stands on each of its controls exactly once
            let mut c = access::Cursor::default();
            let mut order = Vec::new();
            for _ in 0..walk.len() {
                order.push(c.step(&walk, &g.hits, false).unwrap());
            }
            assert_eq!(order, (0..walk.len()).collect::<Vec<_>>(), "{tab:?} {w}x{h}");
            if (w, h) == (1280, 720) {
                report.push(format!("{:<18} {:>3} controls in the walk, {:>3} with a key of their own, left out: {:?}", tab.label(), walk.len(), native, left_out));
                if walk.len() > native {
                    walk_only_pages.push(tab.label());
                }
            }
            assert!(left_out.is_empty(), "{tab:?} {w}x{h}: clickable regions the keyboard walk cannot reach: {left_out:?}");
        }
    }
    eprintln!("{}\npages with controls that only the walk reaches: {walk_only_pages:?}", report.join("\n"));
}
