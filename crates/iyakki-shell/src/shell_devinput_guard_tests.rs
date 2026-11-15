//! `iyakki dev input` never answers an approval or power card: the developer-mode shell/reboot/update ask, the power cards (signed in and
//! the sign-in screen) and a flow's permission ask are answered by a person at the screen. Keys and taps from a paired computer are
//! dropped (and logged) while one is up; the person's own keys and clicks still work.

use super::tests::*;
use super::*;
use iyakki_agent::devmode::input::{self, InputEv};
use iyakki_agent::devmode::scopes::Scope;
use iyakki_agent::devmode::{DevKey, DevMode};
use iyakki_sec::policy::Role;

fn ticks(sh: &mut Shell, n: usize) {
    for _ in 0..n {
        sh.handle(&Event::Tick);
    }
}

fn send(sh: &mut Shell, evs: &[InputEv]) {
    input::push(&sh.env.services.dir().to_path_buf(), evs).unwrap();
    ticks(sh, 12);
}

const ANSWERS: [&str; 8] = ["a", "o", "d", "y", "r", "s", "l", "Enter"];

fn every_answer(sh: &mut Shell) {
    for k in ANSWERS {
        send(sh, &[InputEv::Key(k.into())]);
    }
    send(sh, &[InputEv::Text("aodyrsl\n".into())]);
}

#[test]
fn input_from_a_paired_computer_never_answers_the_power_card() {
    let (mut sh, host) = shell_with_users("dig-power");
    login(&mut sh, "root", "rootpass12");
    sh.fnk.power_card = true;
    every_answer(&mut sh);
    assert!(sh.fnk.power_card, "the card is still up: nothing closed it");
    assert!(host.powered_off.lock().unwrap().is_none(), "no restart, no power off");
    assert!(input::approval_card(sh.env.services.dir()), "the device tells the CLI an approval card is up");
    // taps do not count either
    send(&mut sh, &[InputEv::Tap(640, 360), InputEv::Tap(10, 10)]);
    assert!(sh.fnk.power_card && host.powered_off.lock().unwrap().is_none());
    // a person's own key still answers it
    sh.handle(&Event::Key(Key::Char('r')));
    assert_eq!(*host.powered_off.lock().unwrap(), Some("reboot".into()));
}

#[test]
fn a_key_that_opens_the_power_card_cannot_be_followed_by_its_answer_in_the_same_batch() {
    let (mut sh, host) = shell_with_users("dig-batch");
    login(&mut sh, "root", "rootpass12");
    sh.screen = Screen::Launcher;
    send(&mut sh, &[InputEv::Text("pr".into())]);
    assert!(host.powered_off.lock().unwrap().is_none(), "p may open the card; r is dropped");
    assert_eq!(sh.screen_name(), "power", "the power menu is still up");
    send(&mut sh, &[InputEv::Key("y".into()), InputEv::Key("o".into()), InputEv::Tap(640, 360)]);
    assert!(host.powered_off.lock().unwrap().is_none() && sh.screen_name() == "power");
    sh.handle(&Event::Key(Key::Char('r')));
    assert_eq!(*host.powered_off.lock().unwrap(), Some("reboot".into()), "the person's own key works");
}

#[test]
fn input_from_a_paired_computer_never_answers_the_sign_in_screens_power_card() {
    let (mut sh, host) = shell_with_users("dig-lpower");
    sh.open_login_power();
    assert!(sh.lbar.card.is_some());
    every_answer(&mut sh);
    assert!(sh.lbar.card.is_some() && host.powered_off.lock().unwrap().is_none());
}

#[test]
fn input_from_a_paired_computer_never_answers_the_developer_mode_ask() {
    let (mut sh, _) = shell_with_users("dig-ask");
    login(&mut sh, "root", "rootpass12");
    let dm = DevMode::open(sh.env.services.dir());
    dm.enable(&Role::Admin.template(), "root", 0).unwrap();
    std::fs::create_dir_all(dm.dir()).unwrap();
    let key = DevKey { name: "laptop".into(), key: "ab".repeat(32), account: "root".into(), added: 1, added_by: "root".into(), last_used: 0, scopes: vec!["install".into(), Scope::Shell.id().into(), Scope::Input.id().into()] };
    std::fs::write(dm.dir().join("keys.json"), serde_json::to_vec(&vec![key.clone()]).unwrap()).unwrap();
    ticks(&mut sh, 61);
    dm.shell_request(&key, false, 5, "probe").unwrap();
    ticks(&mut sh, 10);
    assert!(sh.devask.pending.is_some());
    sh.devask.open = true;
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    // taps on every button of the card, and every answer key
    let rects: Vec<R> = sh.hits().iter().filter(|h| matches!(h.ev, Event::Tab(n) if crate::devask::is_click(n))).map(|h| h.rect).collect();
    assert!(rects.len() >= 3, "the card has its buttons");
    for r in rects {
        send(&mut sh, &[InputEv::Tap((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32)]);
    }
    every_answer(&mut sh);
    assert_eq!(dm.shell_pending().len(), 1, "the request is still waiting: input from a paired computer answered nothing");
    assert!(dm.shell_pending().iter().all(|r| r.computer == "laptop"));
    // a person's own click on Deny answers it
    sh.render(&mut g);
    let r = sh.hits().iter().rev().find(|h| h.ev == Event::Tab(crate::devask::DENY)).map(|h| h.rect).expect("a Deny button");
    let (x, y) = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    assert!(dm.shell_pending().is_empty(), "the person's own click works");
}

#[test]
fn ordinary_input_still_works_when_no_card_is_up() {
    let (mut sh, _) = shell_with_users("dig-plain");
    login(&mut sh, "root", "rootpass12");
    assert!(!sh.approval_card_open());
    sh.screen = Screen::Launcher;
    send(&mut sh, &[InputEv::Key("Down".into())]);
    assert!(!input::approval_card(sh.env.services.dir()));
}
