//! Settings → Developer mode: only administrators see it; switching on and off (and the switch-off time), the pairing code, revoking a
//! paired computer, the activity log, and the page drawn at four screen sizes.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::devmode::{self as dm, DevMode};

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

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(w, h);
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}

fn goto(a: &mut SettingsApp, cx: &AppCx, t: Tab) {
    for _ in 0..40 {
        if a.tab() == t {
            return;
        }
        key(a, cx, Key::Right);
    }
    panic!("no {t:?} page");
}

#[test]
fn only_administrators_have_the_page_and_it_switches_on_pairs_revokes_and_switches_off() {
    let mut user = admin("devset-user");
    user.session.user = "amy".into();
    user.session.policy = iyakki_sec::policy::Role::User.template();
    assert!(!SettingsApp::new(&user).tabs().contains(&Tab::Developer), "not for ordinary accounts");

    let cx = admin("devset");
    let d = DevMode::open(cx.services.dir());
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Developer);
    let g = draw(&mut a, &cx, 1280, 800);
    assert!(g.hits.iter().any(|h| h.label == "Switch on") && !g.hits.iter().any(|h| h.label == "Pair a computer"), "off by default: no pairing");
    // the switch-off time cycles; the first choice is 4 hours
    key(&mut a, &cx, Key::Char('h'));
    key(&mut a, &cx, Key::Char('e'));
    assert!(d.is_on(), "{:?}", a.status());
    assert_eq!(d.state().until - d.state().since, 3600, "the second choice is one hour");
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("devmode.on")));
    let g = draw(&mut a, &cx, 1280, 800);
    assert!(g.hits.iter().any(|h| h.label == "Switch off") && g.hits.iter().any(|h| h.label == "Pair a computer"));
    // pair: the code is open; a computer completes it (as the client would)
    key(&mut a, &cx, Key::Char('p'));
    let p = d.pairing().expect("a code is shown");
    assert_eq!(p.account, "root");
    let k = iyakki_agent::pkg::new_signing_key().unwrap();
    let pubhex = iyakki_agent::pkg::public_key_hex(&k);
    let fp = "AA:BB";
    d.complete_pairing(fp, "laptop", &pubhex, &dm::auth::pair_proof(&dm::auth::pair_key(&p.code, fp).unwrap(), fp, &pubhex, "laptop")).unwrap();
    let g = draw(&mut a, &cx, 1280, 800);
    assert!(g.hits.iter().any(|h| h.label == "laptop" && matches!(h.ev, Event::Row(0))), "the computer is listed");
    // the page draws at the usual sizes, with and without the log
    for (w, h) in [(1280, 720), (1920, 1080), (1024, 600), (800, 480)] {
        let _ = draw(&mut a, &cx, w, h);
    }
    key(&mut a, &cx, Key::Char('l'));
    let _ = draw(&mut a, &cx, 1280, 800);
    key(&mut a, &cx, Key::Char('l'));
    // revoke
    key(&mut a, &cx, Key::Char('x'));
    assert!(d.keys().is_empty(), "{:?}", a.status());
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("devmode.revoke")));
    // off again
    key(&mut a, &cx, Key::Char('e'));
    assert!(!d.is_on() && d.pairing().is_none());
    assert!(d.activity(20).iter().any(|x| x.action == "devmode.off"));
}
