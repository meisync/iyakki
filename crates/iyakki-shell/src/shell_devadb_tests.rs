//! Developer mode's reach into the shell: what a paired computer queued with `iyakki dev input` becomes the shell's own events, the
//! shell tells the device whether a password field has focus, and the top bar shows SHELL ACCESS (in the warning colour) while a
//! shell grant is in force.

use super::tests::*;
use super::*;
use iyakki_agent::devmode::input::{self, InputEv};
use iyakki_agent::devmode::scopes::Scope;

fn ticks(sh: &mut Shell, n: usize) {
    for _ in 0..n {
        sh.handle(&Event::Tick);
    }
}

#[test]
fn queued_taps_reach_the_shell_and_the_bar_shows_shell_access_while_a_grant_is_in_force() {
    let (mut sh, _) = shell_with_users("devadb");
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    sh.render(&mut g);
    let dir = sh.env.services.dir().to_path_buf();
    // a tap on the IYAKKI name opens the menu, exactly as a click would
    let name = sh.hits().iter().find(|h| h.label == "IYAKKI menu").map(|h| h.rect).expect("the name is clickable");
    assert!(!sh.brand.open);
    input::push(&dir, &[InputEv::Tap((name.x + name.w / 2.0) as i32, (name.y + name.h / 2.0) as i32)]).unwrap();
    ticks(&mut sh, 2);
    assert!(sh.brand.open, "the tap became a click");
    input::push(&dir, &[InputEv::Key("Esc".into())]).unwrap();
    ticks(&mut sh, 2);
    assert!(!sh.brand.open, "and a key became a key");
    assert!(input::take(&dir).is_empty(), "the shell took everything");
    // the shell reports password focus to the device (none here)
    assert!(!input::secret_focus(&dir));
    // developer mode on, a computer holding the shell scope, a grant: the bar says so
    let dm = iyakki_agent::devmode::DevMode::open(&dir);
    dm.enable(&iyakki_sec::policy::Role::Admin.template(), "root", 0).unwrap();
    std::fs::create_dir_all(dm.dir()).unwrap();
    let key = iyakki_agent::devmode::DevKey { name: "laptop".into(), key: "ab".repeat(32), account: "root".into(), added: 1, added_by: "root".into(), last_used: 0, scopes: vec!["install".into(), Scope::Shell.id().into()] };
    std::fs::write(dm.dir().join("keys.json"), serde_json::to_vec(&vec![key.clone()]).unwrap()).unwrap();
    ticks(&mut sh, 31);
    assert_eq!(sh.dev_sign.as_deref(), Some("DEV"));
    dm.shell_request(&key, false, 5, "").unwrap();
    ticks(&mut sh, 30);
    assert_eq!(sh.dev_sign.as_deref(), Some("DEV \u{00b7} SHELL REQUEST from laptop"));
    dm.shell_approve(&iyakki_sec::policy::Role::Admin.template(), "root", "laptop", 5).unwrap();
    ticks(&mut sh, 30);
    assert_eq!(sh.dev_sign.as_deref(), Some("DEV \u{00b7} SHELL ACCESS by laptop"));
    for (w, h) in [(1280, 720), (1920, 1080), (1024, 600), (800, 480)] {
        let mut g = Gfx::new(w, h);
        sh.size = (w, h);
        sh.render(&mut g);
    }
    dm.shell_end(&iyakki_sec::policy::Role::Admin.template(), "root", "laptop").unwrap();
    ticks(&mut sh, 30);
    assert_eq!(sh.dev_sign.as_deref(), Some("DEV"), "the sign goes back to plain DEV when the grant ends");
    // an interrupted system update is told in the bar, red, whether developer mode is on or not
    std::fs::create_dir_all(dir.join("boot-next")).unwrap();
    std::fs::write(dir.join("boot-next/UPDATING.json"), r#"{"from":"0.1.0-alpha.1","to":"0.1.0-alpha.2","started":1,"prev_id":"x","files":[]}"#).unwrap();
    ticks(&mut sh, 30);
    assert!(sh.dev_sign.as_deref().unwrap().starts_with("UPDATE INTERRUPTED"));
    for (w, h) in [(1280, 720), (800, 480)] {
        let mut g = Gfx::new(w, h);
        sh.size = (w, h);
        sh.render(&mut g);
    }
    std::fs::remove_file(dir.join("boot-next/UPDATING.json")).unwrap();
    ticks(&mut sh, 30);
    assert_eq!(sh.dev_sign.as_deref(), Some("DEV"));
}

#[test]
fn a_frame_is_captured_only_while_a_computer_asks_and_comes_back_as_texts_and_buttons() {
    use iyakki_agent::devmode::state;
    let (mut sh, _) = shell_with_users("devstate");
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    sh.render(&mut g);
    let dir = sh.env.services.dir().to_path_buf();
    assert!(g.text_cap.is_none(), "nobody asked: nothing is kept");
    assert!(state::alive(&dir), "the shell reports that it draws");
    // a computer asks (nothing to wait for yet); the next beat redraws and the frame is handed over
    let _ = state::fetch(&dir, 0);
    ticks(&mut sh, 1);
    assert!(sh.take_stale(), "the picture is redrawn for it");
    sh.render(&mut g);
    let snap = state::fetch(&dir, 500).expect("a frame");
    assert_eq!(snap.app, "Home");
    assert!(!snap.lines.is_empty() && !snap.buttons.is_empty(), "the home screen has texts and tiles: {} lines, {} buttons", snap.lines.len(), snap.buttons.len());
    assert!(snap.buttons.iter().any(|b| b.label == "IYAKKI menu"));
    assert!(g.text_cap.is_none(), "capture is off again after the frame");
    // a button found by its label is the same place a tap would press
    let (x, y, name) = snap.target("iyakki menu").unwrap();
    assert_eq!(name, "IYAKKI menu");
    input::push(&dir, &[InputEv::Tap(x, y)]).unwrap();
    ticks(&mut sh, 2);
    assert!(sh.brand.open, "the click by label opened the menu");
}
