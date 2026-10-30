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

#[test]
fn the_page_grants_and_takes_away_scopes_per_computer_answers_shell_requests_and_disconnects() {
    use iyakki_agent::devmode::scopes::Scope;
    let cx = admin("devset-scopes");
    let d = DevMode::open(cx.services.dir());
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Developer);
    key(&mut a, &cx, Key::Char('e'));
    // the pairing card says what the new computer will be allowed, and the administrator can change it before it pairs
    key(&mut a, &cx, Key::Char('p'));
    assert_eq!(d.pairing().unwrap().scopes, vec!["install"]);
    key(&mut a, &cx, Key::Char('s')); // the cursor moves from install to debug
    key(&mut a, &cx, Key::Char('g'));
    assert_eq!(d.pairing().unwrap().scopes, vec!["install", "debug"]);
    let _ = draw(&mut a, &cx, 1280, 800);
    let p = d.pairing().unwrap();
    let k = iyakki_agent::pkg::new_signing_key().unwrap();
    let pubhex = iyakki_agent::pkg::public_key_hex(&k);
    let fp = "AA:BB";
    d.complete_pairing(fp, "laptop", &pubhex, &dm::auth::pair_proof(&dm::auth::pair_key(&p.code, fp).unwrap(), fp, &pubhex, "laptop")).unwrap();
    assert_eq!(d.keys()[0].scope_text(), "install, debug");
    // per computer: space turns the scope under the cursor on and off
    key(&mut a, &cx, Key::Char('s')); // files
    key(&mut a, &cx, Key::Char(' '));
    assert!(d.keys()[0].has(Scope::Files), "{:?}", a.status());
    key(&mut a, &cx, Key::Char(' '));
    assert!(!d.keys()[0].has(Scope::Files));
    for _ in 0..5 {
        key(&mut a, &cx, Key::Char('s'));
    }
    key(&mut a, &cx, Key::Char(' ')); // the cursor is on shell
    assert!(d.keys()[0].has(Scope::Shell));
    let g = draw(&mut a, &cx, 1280, 800);
    assert!(g.hits.iter().any(|h| h.label == "Approve shell"));
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("devmode.scope.grant")));
    // a shell request appears on the page; the administrator approves it for ten minutes, and the bar text follows
    let laptop = d.keys()[0].clone();
    d.shell_request(&laptop, false, 30, "debug audio").unwrap();
    let _ = draw(&mut a, &cx, 1280, 800);
    key(&mut a, &cx, Key::Char('a'));
    assert!(d.shell_grant("laptop").is_some(), "{:?}", a.status());
    assert_eq!(d.shell_banner().as_deref(), Some("SHELL ACCESS by laptop"));
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("devmode.shell")));
    // a request nobody made cannot be answered
    key(&mut a, &cx, Key::Char('d'));
    assert!(a.status().is_some());
    // disconnect ends the grant and the sessions
    let g = dm::sessions::open(cx.services.dir(), "laptop", "root", Scope::Shell, "shell", d.now()).unwrap();
    key(&mut a, &cx, Key::Char('k'));
    assert!(g.ended() && d.shell_grant("laptop").is_none());
    for (w, h) in [(1280, 720), (1024, 600), (800, 480)] {
        let _ = draw(&mut a, &cx, w, h);
    }
}

struct FwClock;
impl iyakki_fw::engine::Clock for FwClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }
}
struct FwNoDns;
impl iyakki_fw::engine::Resolver for FwNoDns {
    fn resolve(&self, _: &str) -> Result<Vec<std::net::IpAddr>, String> {
        Err("none".into())
    }
}
struct FwMem;
impl iyakki_fw::engine::Store for FwMem {
    fn load(&self) -> Result<Option<iyakki_fw::engine::Saved>, String> {
        Ok(None)
    }
    fn save(&self, _: &iyakki_fw::engine::Saved) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn the_page_says_when_the_firewall_blocks_the_developer_port_and_f_and_n_ask_for_the_rules() {
    let cx = admin("devset-fw");
    let d = DevMode::open(cx.services.dir());
    let mut e = iyakki_fw::Engine::new(Box::new(iyakki_fw::backend::FakeBackend::default()), Box::new(FwClock), Box::new(FwNoDns), Box::new(FwMem));
    e.boot(false);
    let mut a = SettingsApp::new(&cx);
    a.fw.handle = iyakki_fw::service::Handle::Own(Arc::new(std::sync::Mutex::new(e)));
    goto(&mut a, &cx, Tab::Developer);
    // developer mode off: no hint
    assert!(a.dev_fw_hint(&d).is_none());
    key(&mut a, &cx, Key::Char('e'));
    assert!(d.is_on());
    assert_eq!(a.dev_fw_hint(&d).as_deref(), Some("the firewall blocks the developer port 7979: Allow"));
    let g = draw(&mut a, &cx, 1280, 800);
    assert!(g.hits.iter().any(|h| h.label == "Allow in the firewall") && g.hits.iter().any(|h| h.label == "Whole network on/off"));
    // f without a code on the screen and without a blocked computer: nothing to allow
    key(&mut a, &cx, Key::Char('f'));
    assert_eq!(d.fw_want().pair_until, 0);
    // with a pairing code: the one-time request (consumed when the pairing ends)
    key(&mut a, &cx, Key::Char('p'));
    key(&mut a, &cx, Key::Char('f'));
    assert!(d.fw_want().pair_until > 0);
    d.cancel_pairing();
    assert_eq!(d.fw_want().pair_until, 0);
    // the opt-in is off by default and n turns it on and off
    assert!(!d.network_wide());
    key(&mut a, &cx, Key::Char('n'));
    assert!(d.network_wide());
    key(&mut a, &cx, Key::Char('n'));
    assert!(!d.network_wide());
}
