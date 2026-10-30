//! The developer-mode approval on every screen: pulse and banner while a request waits, the inline card from the DEV sign, and the
//! three answers; nothing animates when nothing waits.

use super::tests::*;
use super::*;
use iyakki_agent::devmode::scopes::Scope;
use iyakki_agent::devmode::{DevKey, DevMode};
use iyakki_sec::policy::Role;

const SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];

fn ticks(sh: &mut Shell, n: usize) {
    for _ in 0..n {
        sh.handle(&Event::Tick);
    }
}

/// A signed-in administrator, developer mode on, one paired computer holding the shell scope.
fn setup(tag: &str, account: &str) -> (Shell, DevMode, DevKey) {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    let dm = DevMode::open(sh.env.services.dir());
    dm.enable(&Role::Admin.template(), "root", 0).unwrap();
    std::fs::create_dir_all(dm.dir()).unwrap();
    let key = DevKey { name: "laptop".into(), key: "ab".repeat(32), account: account.into(), added: 1, added_by: "root".into(), last_used: 0, scopes: vec!["install".into(), Scope::Shell.id().into()] };
    std::fs::write(dm.dir().join("keys.json"), serde_json::to_vec(&vec![key.clone()]).unwrap()).unwrap();
    ticks(&mut sh, 61);
    (sh, dm, key)
}

fn hit(sh: &Shell, id: usize) -> Option<R> {
    sh.hits().iter().rev().find(|h| h.ev == Event::Tab(id)).map(|h| h.rect)
}

fn press(sh: &mut Shell, g: &mut Gfx, id: usize) {
    sh.render(g);
    let r = hit(sh, id).unwrap_or_else(|| panic!("no region {id}"));
    let (x, y) = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
}

fn open_card(sh: &mut Shell, g: &mut Gfx) {
    sh.render(g);
    // on a narrow bar the sign gives way to the title; the pulsing banner then opens the card
    let r = sh.hits().iter().rev().find(|h| h.label == "developer mode on").map(|h| h.rect).or_else(|| hit(sh, crate::devask::OPEN));
    let r = r.expect("the DEV sign or the banner");
    let (x, y) = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
}

#[test]
fn nothing_animates_while_nothing_waits_and_a_request_makes_the_sign_pulse_and_shows_the_banner_on_every_screen() {
    let (mut sh, dm, key) = setup("devask1", "root");
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(sh.devask.pending.is_none());
    assert_eq!(sh.devask.pulse(7), 0.0, "no pulse without a request");
    assert!(hit(&sh, crate::devask::OPEN).is_none());
    sh.stale = false;
    ticks(&mut sh, 9);
    assert!(!sh.stale, "nothing waits: the heartbeat does not redraw");
    dm.shell_request(&key, false, 5, "mic test").unwrap();
    ticks(&mut sh, 10);
    assert!(sh.devask.pending.is_some());
    assert!(sh.stale, "a waiting request redraws");
    let phases: Vec<f32> = (0..24).map(|t| sh.devask.pulse(t)).collect();
    assert!(phases.iter().all(|p| *p >= 0.25 && *p <= 1.0));
    assert!(phases.iter().cloned().fold(0.0, f32::max) - phases.iter().cloned().fold(2.0, f32::min) > 0.5, "the pulse swells and fades");
    for (w, h) in SIZES {
        let mut g = Gfx::new(w, h);
        sh.size = (w, h);
        sh.screen = Screen::Launcher;
        sh.render(&mut g);
        let b = hit(&sh, crate::devask::OPEN).unwrap_or_else(|| panic!("no banner at {w}x{h}"));
        assert!(b.x >= 0.0 && b.y >= 0.0 && b.x + b.w <= w as f32 + 0.5 && b.y + b.h <= h as f32 + 0.5, "banner inside {w}x{h}");
        open_app(&mut sh, "chat");
        sh.render(&mut g);
        assert!(hit(&sh, crate::devask::OPEN).is_some(), "banner over an app at {w}x{h}");
    }
    // the request is answered elsewhere: the animation stops
    dm.shell_deny(&Role::Admin.template(), "root", "laptop").unwrap();
    ticks(&mut sh, 10);
    assert!(sh.devask.pending.is_none());
    sh.stale = false;
    ticks(&mut sh, 9);
    assert!(!sh.stale);
}

fn open_app(sh: &mut Shell, id: &str) {
    sh.jump_to(id, "");
}

#[test]
fn clicking_the_dev_sign_opens_the_card_inline_and_it_fits_at_every_size() {
    let (mut sh, dm, key) = setup("devask2", "root");
    let mut g = Gfx::new(1280, 720);
    // without a request the sign still goes to Settings
    open_card(&mut sh, &mut g);
    assert!(!sh.devask.open);
    sh.screen = Screen::Launcher;
    dm.shell_request(&key, false, 5, "a rather long reason that has to wrap over several lines of the card so that it is cut and never overflows the screen").unwrap();
    ticks(&mut sh, 10);
    for (w, h) in SIZES {
        let mut g = Gfx::new(w, h);
        sh.size = (w, h);
        sh.screen = Screen::Launcher;
        sh.devask.open = false;
        open_card(&mut sh, &mut g);
        assert!(sh.devask.open, "the sign opens the card at {w}x{h}");
        assert!(matches!(sh.screen, Screen::Launcher), "and does not leave the screen");
        sh.render(&mut g);
        let req = sh.devask.pending.clone().unwrap();
        let s = scale_for(w, h);
        let l = crate::devask::layout(&mut g, w as f32, h as f32, s, &req);
        assert!(l.card.x >= 0.0 && l.card.y >= 0.0 && l.card.x + l.card.w <= w as f32 + 0.5 && l.card.y + l.card.h <= h as f32 + 0.5, "card inside {w}x{h}: {:?}", l.card);
        for (i, (_, label, r, _)) in l.buttons.iter().enumerate() {
            assert!(r.x >= l.card.x - 0.5 && r.x + r.w <= l.card.x + l.card.w + 0.5 && r.y + r.h <= l.card.y + l.card.h + 0.5, "{label} inside the card at {w}x{h}");
            for (_, _, o, _) in l.buttons.iter().skip(i + 1) {
                assert!(r.x + r.w <= o.x || o.x + o.w <= r.x || r.y + r.h <= o.y || o.y + o.h <= r.y, "buttons overlap at {w}x{h}");
            }
        }
        for id in [crate::devask::ONCE, crate::devask::AUTO, crate::devask::DENY, crate::devask::CLOSE] {
            assert!(hit(&sh, id).is_some(), "region {id} at {w}x{h}");
        }
    }
    // Esc and the close box close it without answering
    press(&mut sh, &mut g, crate::devask::CLOSE);
    assert!(!sh.devask.open && sh.devask.pending.is_some());
    open_card(&mut sh, &mut g);
    sh.handle(&Event::Key(Key::Esc));
    assert!(!sh.devask.open && sh.devask.pending.is_some());
}

#[test]
fn approve_deny_and_approve_with_auto_do_what_settings_does() {
    // Approve: one session
    let (mut sh, dm, key) = setup("devask3", "root");
    let mut g = Gfx::new(1280, 720);
    dm.shell_request(&key, false, 5, "").unwrap();
    ticks(&mut sh, 10);
    open_card(&mut sh, &mut g);
    press(&mut sh, &mut g, crate::devask::ONCE);
    let gr = dm.shell_grant("laptop").expect("approved");
    assert!(gr.once && !gr.as_root);
    assert!(!sh.devask.open && sh.devask.pending.is_none(), "the card is gone and the animation stops");
    // Deny
    dm.shell_end(&Role::Admin.template(), "root", "laptop").unwrap();
    dm.shell_request(&key, false, 5, "").unwrap();
    ticks(&mut sh, 10);
    open_card(&mut sh, &mut g);
    press(&mut sh, &mut g, crate::devask::DENY);
    assert!(dm.shell_grant("laptop").is_none());
    assert_eq!(dm.shell_status("laptop"), iyakki_agent::devmode::grants::ShellStatus::Denied);
    assert!(sh.devask.pending.is_none());
    // Approve 10 min + auto 4 h
    dm.shell_request(&key, false, 5, "").unwrap();
    ticks(&mut sh, 10);
    open_card(&mut sh, &mut g);
    press(&mut sh, &mut g, crate::devask::AUTO);
    let gr = dm.shell_grant("laptop").expect("approved");
    assert!(!gr.once && gr.expires - gr.since == 600);
    assert_eq!(dm.shell_auto_list().len(), 1, "the next requests are approved by themselves");
}

#[test]
fn a_root_request_cannot_be_approved_automatically() {
    let (mut sh, dm, key) = setup("devask4", "root");
    let mut g = Gfx::new(1280, 720);
    dm.shell_request(&key, true, 5, "").unwrap();
    ticks(&mut sh, 10);
    assert!(sh.devask.pending.as_ref().unwrap().as_root);
    open_card(&mut sh, &mut g);
    sh.render(&mut g);
    assert!(hit(&sh, crate::devask::AUTO).is_none(), "the auto button is greyed out and has no click");
    // even a stray event is refused
    sh.handle(&Event::Tab(crate::devask::AUTO));
    assert!(dm.shell_grant("laptop").is_none() && dm.shell_auto_list().is_empty());
    press(&mut sh, &mut g, crate::devask::ONCE);
    let gr = dm.shell_grant("laptop").unwrap();
    assert!(gr.as_root && gr.once && dm.shell_auto_list().is_empty());
}

#[test]
fn only_an_administrator_sees_the_request() {
    let (mut sh, dm, key) = setup("devask5", "root");
    dm.shell_request(&key, false, 5, "").unwrap();
    ticks(&mut sh, 10);
    assert!(sh.devask.pending.is_some());
    sh.handle(&Event::Key(Key::F(11)));
    // sign out and in as an ordinary user
    sh.session = None;
    ticks(&mut sh, 10);
    assert!(sh.devask.pending.is_none());
}
