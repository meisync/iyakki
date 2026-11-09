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
    for _ in 0..60 {
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

/// Press the hit region that carries `ev` the way the shell does for a click: from_click is on while it is handled.
fn tap(a: &mut SettingsApp, cx: &AppCx, g: &Gfx, pick: impl Fn(&crate::ui::Hit) -> bool) {
    let h = g.hits.iter().find(|h| pick(h)).unwrap_or_else(|| panic!("no such hit region; have {:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>()));
    assert!(h.rect.h >= 30.0 || h.label.ends_with(": shell"), "{} is too small to tap: {:?}", h.label, h.rect);
    crate::ui::set_from_click(true);
    a.handle(&h.ev.clone(), cx);
    crate::ui::set_from_click(false);
}

fn pair(d: &DevMode, name: &str) {
    let p = d.pairing().expect("a code is shown");
    let k = iyakki_agent::pkg::new_signing_key().unwrap();
    let pubhex = iyakki_agent::pkg::public_key_hex(&k);
    d.complete_pairing("AA:BB", name, &pubhex, &dm::auth::pair_proof(&dm::auth::pair_key(&p.code, "AA:BB").unwrap(), "AA:BB", &pubhex, name)).unwrap();
}

#[test]
fn every_part_of_the_page_answers_a_click_like_its_key() {
    use iyakki_agent::devmode::scopes::Scope;
    let cx = admin("devset-click");
    let d = DevMode::open(cx.services.dir());
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Developer);
    // off: the empty list invites switching on; the cycle line and the Switch on button work by click
    let g = draw(&mut a, &cx, 1280, 800);
    tap(&mut a, &cx, &g, |h| matches!(h.ev, Event::Key(Key::Char('h'))) && h.label.starts_with("Port"));
    assert_eq!(a.devel.hours, 1, "a tap on the switch-off line is h");
    tap(&mut a, &cx, &g, |h| h.label == "Switch-off time");
    assert_eq!(a.devel.hours, 2);
    a.devel.hours = 0;
    tap(&mut a, &cx, &g, |h| h.label == "Switch on");
    assert!(d.is_on(), "{:?}", a.status());
    // pair from the empty list's hint, copy by tap, scopes of the new computer by tap
    let g = draw(&mut a, &cx, 1280, 800);
    tap(&mut a, &cx, &g, |h| matches!(h.ev, Event::Key(Key::Char('p'))) && h.label.starts_with("None yet"));
    assert!(d.pairing().is_some());
    let g = draw(&mut a, &cx, 1280, 1400);
    tap(&mut a, &cx, &g, |h| h.label == "New computer: debug");
    assert_eq!(d.pairing().unwrap().scopes, vec!["install", "debug"]);
    tap(&mut a, &cx, &g, |h| matches!(h.ev, Event::Key(Key::Char('C'))));
    assert!(a.status().is_some_and(|s| s.0.contains("copied")), "{:?}", a.status());
    pair(&d, "laptop");
    pair_again(&mut a, &cx, &d, "desk");
    // two computers: a click on a row chooses it, a click on a scope line turns that scope on for that computer only
    let g = draw(&mut a, &cx, 1280, 1400);
    tap(&mut a, &cx, &g, |h| h.label == "desk" && matches!(h.ev, Event::Row(1)));
    assert_eq!(a.sel[a.tab], 1);
    let g = draw(&mut a, &cx, 1280, 1400);
    tap(&mut a, &cx, &g, |h| h.label == "desk: files");
    let keys = d.keys();
    assert!(keys.iter().find(|k| k.name == "desk").unwrap().has(Scope::Files) && !keys.iter().find(|k| k.name == "laptop").unwrap().has(Scope::Files));
    let g = draw(&mut a, &cx, 1280, 1400);
    tap(&mut a, &cx, &g, |h| h.label == "desk: files");
    assert!(!d.keys().iter().find(|k| k.name == "desk").unwrap().has(Scope::Files), "a second tap turns it off");
    // pending shell request: its buttons sit on the page and reach the same handler as the keys
    let desk = d.keys().into_iter().find(|k| k.name == "desk").unwrap();
    let mut desk = desk;
    d.set_scope(&cx.session.policy, "root", "desk", Scope::Shell, true).unwrap();
    desk = d.keys().into_iter().find(|k| k.name == desk.name).unwrap();
    d.shell_request(&desk, false, 30, "debug").unwrap();
    for (label, hours) in [("4 hours", 4), ("8 hours", 8), ("12 hours", 12)] {
        let g = draw(&mut a, &cx, 1280, 1400);
        tap(&mut a, &cx, &g, |h| h.label == label);
        let gr = d.shell_grant("desk").unwrap_or_else(|| panic!("{label}: {:?}", a.status()));
        assert!(gr.expires - d.now() > hours * 3600 - 120 && gr.expires - d.now() <= hours * 3600, "{label}");
        d.shell_end(&cx.session.policy, "root", "desk").unwrap();
        d.shell_request(&desk, false, 30, "debug").unwrap();
    }
    let g = draw(&mut a, &cx, 1280, 1400);
    tap(&mut a, &cx, &g, |h| h.label == "Refuse");
    assert!(d.shell_pending().is_empty() && d.shell_grant("desk").is_none());
    // the log by button, the whole page at a small size still has every row reachable by wheel
    let g = draw(&mut a, &cx, 1280, 800);
    tap(&mut a, &cx, &g, |h| h.label == "Activity log");
    assert!(a.devel.log);
    let g = draw(&mut a, &cx, 1280, 800);
    tap(&mut a, &cx, &g, |h| h.label == "Activity log");
    assert!(!a.devel.log);
}

fn pair_again(a: &mut SettingsApp, cx: &AppCx, d: &DevMode, name: &str) {
    key(a, cx, Key::Char('p'));
    pair(d, name);
}

#[test]
fn the_wheel_scrolls_the_page_and_every_row_can_be_reached_at_a_small_size() {
    let cx = admin("devset-wheel");
    let d = DevMode::open(cx.services.dir());
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Developer);
    key(&mut a, &cx, Key::Char('e'));
    key(&mut a, &cx, Key::Char('p'));
    pair(&d, "laptop");
    key(&mut a, &cx, Key::Char('p'));
    a.mouse = (900, 400);
    let (w, h) = (1024, 600);
    let g = draw(&mut a, &cx, w, h);
    assert!(!g.hits.iter().any(|h| h.label == "laptop: shell"), "the last scope is below the window at first");
    assert!(a.devel.max > 0.0);
    let mut seen = false;
    for _ in 0..60 {
        a.handle(&Event::Scroll(-1), &cx);
        let g = draw(&mut a, &cx, w, h);
        if g.hits.iter().any(|x| x.label == "laptop: shell" && x.rect.h >= 30.0) {
            seen = true;
            tap(&mut a, &cx, &g, |x| x.label == "laptop: shell");
            break;
        }
    }
    assert!(seen, "scrolled to the end: {} of {}", a.devel.scroll, a.devel.max);
    assert!(d.keys()[0].has(iyakki_agent::devmode::scopes::Scope::Shell));
    // every region that is registered lies inside the page (not under the button bar)
    let g = draw(&mut a, &cx, w, h);
    let bar_top = g.hits.iter().filter(|x| x.label == "Switch off").map(|x| x.rect.y).fold(f32::MAX, f32::min);
    for x in g.hits.iter().filter(|x| matches!(x.ev, Event::Sub(..) | Event::Row(_))) {
        assert!(x.rect.y + x.rect.h <= bar_top + 1.0, "{} reaches under the buttons", x.label);
    }
    a.handle(&Event::Scroll(100), &cx);
    assert_eq!(a.devel.scroll, 0.0, "wheel up goes back to the top");
}

#[test]
fn the_usb_c_page_answers_clicks_and_the_wheel_and_a_click_leaves_the_code_box() {
    let cx = admin("devset-usb");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Developer);
    let g = draw(&mut a, &cx, 1280, 800);
    tap(&mut a, &cx, &g, |h| h.label == "Connect over USB-C");
    assert!(a.devel.usb_open);
    let g = draw(&mut a, &cx, 1280, 800);
    assert!(g.hits.iter().any(|h| h.label == "Check again"), "the bar has its buttons");
    a.handle(&Event::Scroll(-2), &cx);
    assert_eq!(a.devel.usb.top, 2, "the wheel scrolls the checks");
    a.handle(&Event::Scroll(10), &cx);
    assert_eq!(a.devel.usb.top, 0);
    // typing a code, then a click on Back: leaves the box and goes back, no 'u' typed
    a.devel.usb.code_focus = true;
    let g = draw(&mut a, &cx, 1280, 800);
    tap(&mut a, &cx, &g, |h| h.label == "Back");
    assert!(!a.devel.usb_open && !a.devel.usb.code_focus);
    assert_eq!(a.devel.usb.code.value, "");
}

fn paired_laptop(cx: &AppCx, d: &DevMode, a: &mut SettingsApp) {
    if !d.is_on() {
        key(a, cx, Key::Char('e'));
    }
    key(a, cx, Key::Char('p'));
    let p = d.pairing().expect("a code is shown");
    let k = iyakki_agent::pkg::new_signing_key().unwrap();
    let pubhex = iyakki_agent::pkg::public_key_hex(&k);
    let fp = "AA:BB";
    d.complete_pairing(fp, "laptop", &pubhex, &dm::auth::pair_proof(&dm::auth::pair_key(&p.code, fp).unwrap(), fp, &pubhex, "laptop")).unwrap();
}

#[test]
fn the_g_key_grants_every_diagnostic_scope_and_never_the_dangerous_ones_and_t_extends_the_timer() {
    use iyakki_agent::devmode::scopes::Scope;
    let cx = admin("devset-gt");
    let d = DevMode::open(cx.services.dir());
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Developer);
    // G with no computer paired says so and grants nothing
    key(&mut a, &cx, Key::Char('e'));
    key(&mut a, &cx, Key::Char('G'));
    assert!(a.status().is_some_and(|s| s.0.contains("No computer")), "{:?}", a.status());
    paired_laptop(&cx, &d, &mut a);
    key(&mut a, &cx, Key::Char('G'));
    let k = d.keys()[0].clone();
    for s in [Scope::Debug, Scope::Files, Scope::Input, Scope::Kube, Scope::Shell] {
        assert!(k.has(s), "{s:?} is a diagnostic scope");
    }
    for s in Scope::ALL {
        if !matches!(s, Scope::Debug | Scope::Files | Scope::Input | Scope::Kube | Scope::Shell) && s.id() != "install" {
            assert!(!k.scopes.iter().any(|x| x == s.id()), "{s:?} must stay off: {}", k.scope_text());
        }
    }
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("devmode.scope.grant")));
    // t pushes the switch-off time further and writes an audit line
    let before = d.state().until;
    key(&mut a, &cx, Key::Char('t'));
    assert!(d.state().until > before, "{:?}", a.status());
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("devmode.extend")));
}

#[test]
fn the_keys_4_8_and_1_approve_a_shell_for_4_8_and_12_hours_and_y_uses_the_8_hour_default() {
    let cx = admin("devset-hours");
    let d = DevMode::open(cx.services.dir());
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Developer);
    paired_laptop(&cx, &d, &mut a);
    key(&mut a, &cx, Key::Char('G')); // the shell scope (still approved here each time)
    let laptop = d.keys()[0].clone();
    for (c, hours) in [('4', 4i64), ('8', 8), ('1', 12)] {
        d.shell_request(&laptop, false, 30, "t").unwrap();
        key(&mut a, &cx, Key::Char(c));
        let g = d.shell_grant("laptop").unwrap_or_else(|| panic!("key {c}: {:?}", a.status()));
        assert_eq!(g.expires - d.now(), hours * 3600, "key {c}");
        d.shell_end(&iyakki_sec::policy::Role::Admin.template(), "root", "laptop").unwrap();
    }
    assert_eq!(iyakki_agent::devmode::grants::AUTO_DEFAULT_HOURS, 8, "the automatic approval defaults to 8 hours");
    assert_eq!(iyakki_agent::devmode::grants::MAX_MINUTES, 720);
}
