//! Settings -> Developer mode -> Connect over USB-C against the made-up machine of `iyakki_agent::directlink::fake`: who sees it, the
//! check list at five screen sizes, every button, the pairing code typed on the keyboard, and a walk over all keys.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::directlink::fake::{iface, FakeEnv};
use iyakki_agent::directlink::tb::{Device, Kind, TbView};
use iyakki_agent::directlink::{beacon::Beacon, Hello, Peer, Snapshot, SpeedSample};
use std::net::Ipv4Addr;

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

fn typ(a: &mut SettingsApp, cx: &AppCx, t: &str) {
    for c in t.chars() {
        key(a, cx, Key::Char(c));
    }
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

fn beacon() -> Beacon {
    Beacon { host: "other-box".into(), version: "0.1.0-test".into(), role: "target".into(), dev_on: true, pairing_open: true, mac: "02:aa:bb:cc:dd:ee".into(), id: "ffeeddccbbaa99887766554433221100".into(), port: 7979 }
}

/// A fake with a Thunderbolt cable to a computer that runs IYAKKI with developer mode on and a pairing code open.
fn cable() -> Arc<FakeEnv> {
    let env = Arc::new(FakeEnv::new());
    env.with(|s| {
        let mut i = iface("thunderbolt0", "thunderbolt-net", "02:11:22:33:44:55", true, true);
        i.ipv4 = vec![Ipv4Addr::new(169, 254, 5, 6)];
        s.ifaces = vec![i];
        s.tb = TbView { controller_present: true, driver_loaded: true, security: "user".into(), devices: vec![Device { name: "0-1".into(), kind: Kind::XDomain, authorized: None, vendor: "IYAKKI".into(), model: "other-box".into(), unique_id: "aa-bb".into(), rx_gbps: Some(20.0), tx_gbps: Some(20.0), lanes: Some(2) }] };
        s.snapshot = Some(Snapshot { at: 1000, links: vec![], peers: vec![Peer { iface: "thunderbolt0".into(), addr: Ipv4Addr::new(169, 254, 7, 7), beacon: beacon(), last_seen: 999 }] });
        s.hello = Ok(Hello { name: "other-box".into(), version: "0.1.0-test".into(), fingerprint: "AA:BB:CC:DD".into(), pairing_open: true });
    });
    env
}

fn open(cx: &AppCx, env: &Arc<FakeEnv>) -> SettingsApp {
    let mut a = SettingsApp::new(cx);
    goto(&mut a, cx, Tab::Developer);
    a.devel.usb.env = Some(env.clone());
    a.devel.usb.sync = true;
    key(&mut a, cx, Key::Char('u'));
    assert!(a.devel.usb_open, "u opens the page");
    a
}

fn labels(g: &Gfx) -> Vec<String> {
    g.hits.iter().map(|h| h.label.to_string()).collect()
}

#[test]
fn only_administrators_have_the_page_and_u_and_escape_open_and_close_it() {
    for role in [iyakki_sec::policy::Role::User, iyakki_sec::policy::Role::Operator, iyakki_sec::policy::Role::Guest] {
        let mut cx = admin(&format!("usbc-role-{}", role.name()));
        cx.session.user = "amy".into();
        cx.session.policy = role.template();
        assert!(!SettingsApp::new(&cx).tabs().contains(&Tab::Developer), "{role:?}: the page lives in Developer mode, which is for administrators");
    }
    let cx = admin("usbc-open");
    let env = cable();
    let mut a = open(&cx, &env);
    let g = draw(&mut a, &cx, 1280, 720);
    let l = labels(&g);
    for b in ["Check again", "Copy diagnostics", "Fixed addresses", "Allow more", "Back"] {
        assert!(l.iter().any(|x| x == b), "{b} in {l:?}");
    }
    assert!(!l.iter().any(|x| x == "Switch on"), "the developer-mode buttons are replaced by the page's own");
    // Esc closes the page first, a second Esc leaves Settings
    assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Stay) && !a.devel.usb_open);
    assert!(draw(&mut a, &cx, 1280, 720).hits.iter().any(|h| h.label == "Connect over USB-C"), "the developer page offers the way in");
    assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
    // u again, and u back
    key(&mut a, &cx, Key::Char('u'));
    key(&mut a, &cx, Key::Char('u'));
    assert!(!a.devel.usb_open);
}

#[test]
fn the_page_draws_at_five_sizes_in_every_state_with_every_control_on_screen() {
    let cx = admin("usbc-sizes");
    let states: Vec<(&str, Arc<FakeEnv>)> = vec![
        ("plain pc", {
            let e = Arc::new(FakeEnv::new());
            e.with(|s| s.ifaces = vec![iface("eth0", "e1000e", "52:54:00:00:00:01", true, true)]);
            e
        }),
        ("all but pairing", cable()),
        ("authorize", {
            let e = cable();
            e.with(|s| s.tb.devices = vec![Device { name: "0-3".into(), kind: Kind::Router, authorized: Some(0), vendor: "Acme".into(), model: "Dock".into(), unique_id: "cc".into(), rx_gbps: None, tx_gbps: None, lanes: None }]);
            e
        }),
        ("paired", {
            let e = cable();
            e.with(|s| {
                s.paired = Some("other-box".into());
                s.speed = Ok(SpeedSample { bytes: 64 << 20, secs: 0.25 });
            });
            e
        }),
    ];
    for (name, env) in states {
        let mut a = open(&cx, &env);
        if name == "paired" {
            let _ = draw(&mut a, &cx, 800, 480);
            key(&mut a, &cx, Key::Char('s'));
            assert!(a.devel.usb.note().is_some_and(|n| n.0), "{name}: {:?}", a.devel.usb.note());
        }
        // the two states that matter most get all five sizes; the others the smallest and a usual one
        let sizes: &[(u32, u32)] = if name == "plain pc" || name == "all but pairing" { &[(360, 740), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] } else { &[(360, 740), (1280, 720)] };
        for &(w, h) in sizes {
            let g = draw(&mut a, &cx, w, h);
            let sc = crate::shell::scale_for(w, h);
            for hit in &g.hits {
                let r = hit.rect;
                assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{name} {w}x{h}: {} is inside the screen: {r:?} (scale {sc})", hit.label);
            }
            // every scroll position draws too
            for _ in 0..7 {
                key(&mut a, &cx, Key::Down);
                if w <= 1280 {
                    let _ = draw(&mut a, &cx, w, h);
                }
            }
            for _ in 0..7 {
                key(&mut a, &cx, Key::Up);
            }
        }
        // in panes: the page drawn into a rectangle that is not the whole screen; the code field stays inside it
        key(&mut a, &cx, Key::Char('p'));
        for (w, h) in [(640u32, 400u32), (360, 300), (1000, 700)] {
            let mut g = Gfx::new(1280, 800);
            g.clear(rgb(0, 0, 0));
            let pane = R::new(40.0, 60.0, w as f32, h as f32);
            let s = 1.0;
            a.render_usb(&mut g, pane, s, &cx);
            for hit in g.hits.iter().filter(|x| matches!(x.ev, Event::FocusAt(..))) {
                let r = hit.rect;
                assert!(r.x >= pane.x - 0.5 && r.x + r.w <= pane.x + pane.w + 0.5 && r.y >= pane.y - 0.5 && r.y + r.h <= pane.y + pane.h + 0.5, "{name} pane {w}x{h}: the code field is inside the pane: {r:?}");
            }
        }
        key(&mut a, &cx, Key::Esc);
    }
}

#[test]
fn a_collapsed_or_tiny_area_never_panics() {
    let cx = admin("usbc-tiny");
    let env = cable();
    let mut a = open(&cx, &env);
    for (w, h) in [(0.0f32, 0.0f32), (1.0, 1.0), (20.0, 10.0), (50.0, 5000.0), (5000.0, 30.0)] {
        let mut g = Gfx::new(400, 400);
        a.render_usb(&mut g, R::new(0.0, 0.0, w, h), 1.0, &cx);
        a.render_usb(&mut g, R::new(f32::NAN, 0.0, w, h), 2.5, &cx);
    }
}

#[test]
fn authorize_pair_speed_copy_and_the_two_switches_work_from_the_keyboard_and_are_logged() {
    let cx = admin("usbc-actions");
    let env = cable();
    env.with(|s| s.tb.devices = vec![Device { name: "0-3".into(), kind: Kind::Router, authorized: Some(0), vendor: "Acme".into(), model: "Dock".into(), unique_id: "cc-dd".into(), rx_gbps: None, tx_gbps: None, lanes: None }]);
    let mut a = open(&cx, &env);
    let _ = draw(&mut a, &cx, 1280, 720);
    let r = a.devel.usb.report().unwrap();
    assert_eq!(r.checks[1].state, iyakki_agent::directlink::checks::State::Fail);
    assert!(draw(&mut a, &cx, 1280, 720).hits.iter().any(|h| h.label == "Authorize"));
    key(&mut a, &cx, Key::Char('z'));
    assert_eq!(env.calls(), vec!["authorize 0-3 1"]);
    assert!(a.devel.usb.note().is_some_and(|n| n.0 && n.1.contains("approved")));
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("directlink.authorize")));
    // the cable becomes the other computer (the dock goes away, the peer appears)
    env.with(|s| s.tb = cable().with(|c| c.tb.clone()));
    key(&mut a, &cx, Key::Char('r'));
    let r = a.devel.usb.report().unwrap();
    assert_eq!(r.checks[4].state, iyakki_agent::directlink::checks::State::Pass);
    assert!(draw(&mut a, &cx, 1280, 720).hits.iter().any(|h| h.label == "Pair"));
    // a wrong code does not pair; the right one does; typing goes to the field, not to the page's keys (r, s, u are letters of the code)
    key(&mut a, &cx, Key::Char('p'));
    assert!(a.devel.usb.code_focus);
    typ(&mut a, &cx, "WRONG-rsu");
    assert!(a.devel.usb.code_focus && a.devel.usb_open, "letters that are page keys went into the code field");
    key(&mut a, &cx, Key::Enter);
    assert!(env.with(|s| s.paired.is_none()) && a.devel.usb.note().is_some_and(|n| !n.0 && n.1.contains("wrong")), "{:?}", a.devel.usb.note());
    key(&mut a, &cx, Key::Char('p'));
    typ(&mut a, &cx, "K7QFM-2XPRT");
    key(&mut a, &cx, Key::Enter);
    assert_eq!(env.with(|s| s.paired.clone()).as_deref(), Some("other-box"), "{:?}", a.devel.usb.note());
    assert!(env.calls().iter().any(|c| c == "pair K7QFM-2XPRT AA:BB:CC:DD"), "the code goes with the certificate the other side showed");
    assert!(a.devel.usb.code.value.is_empty(), "the code is not kept");
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("directlink.pair")));
    assert!(!cx.audit.recent(20).iter().any(|l| l.contains("K7QFM")), "never the code in the audit log");
    // Esc leaves the code field only
    key(&mut a, &cx, Key::Char('p'));
    typ(&mut a, &cx, "abc");
    key(&mut a, &cx, Key::Esc);
    assert!(!a.devel.usb.code_focus && a.devel.usb_open && a.devel.usb.code.value.is_empty());
    // the speed check
    env.with(|s| s.speed = Ok(SpeedSample { bytes: 64 << 20, secs: 0.5 }));
    key(&mut a, &cx, Key::Char('s'));
    let r = a.devel.usb.report().unwrap();
    assert!(r.checks[6].text.contains("Gbit/s") && r.last == "All 7 steps passed.", "{}", r.last);
    // copy the diagnostic text
    key(&mut a, &cx, Key::Char('c'));
    let copied = crate::ui::clipboard::latest().unwrap_or_default();
    assert!(copied.contains("IYAKKI direct link report") && !copied.contains("K7QFM"), "{copied}");
    // the two switches
    key(&mut a, &cx, Key::Char('f'));
    key(&mut a, &cx, Key::Char('m'));
    let c = env.with(|s| s.config);
    assert!(c.fixed_pair && c.allow_more);
    assert!(a.status().is_some_and(|(m, ok)| *ok && m.contains("Everything is now let in")));
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("directlink.config")));
    key(&mut a, &cx, Key::Char('m'));
    assert!(!env.with(|s| s.config.allow_more));
}

#[test]
fn an_account_that_lost_the_administrator_role_changes_nothing() {
    let cx = admin("usbc-gate");
    let env = cable();
    let mut a = open(&cx, &env);
    let _ = draw(&mut a, &cx, 1280, 720);
    let mut user = admin("usbc-gate-user");
    user.session.user = "amy".into();
    user.session.policy = iyakki_sec::policy::Role::Operator.template();
    for c in ['z', 'p', 's', 'f', 'm'] {
        a.handle(&Event::Key(Key::Char(c)), &user);
    }
    assert!(env.calls().is_empty() && !env.with(|s| s.config.fixed_pair || s.config.allow_more), "{:?}", env.calls());
}

#[test]
fn every_key_walks_the_page_without_a_panic() {
    let cx = admin("usbc-walk");
    let env = cable();
    let mut a = open(&cx, &env);
    let keys = [Key::Up, Key::Down, Key::Enter, Key::Tab, Key::Backspace, Key::Delete, Key::Home, Key::End, Key::Char('r'), Key::Char('z'), Key::Char('s'), Key::Char('c'), Key::Char('f'), Key::Char('m'), Key::Char('p'), Key::Char('x'), Key::Char('é'), Key::Char(' '), Key::Esc, Key::Char('u')];
    for round in 0..6 {
        for (n, k) in keys.iter().cycle().skip(round).take(60).enumerate() {
            a.handle(&Event::Key(k.clone()), &cx);
            if n % 10 == 9 {
                let _ = draw(&mut a, &cx, 800, 480);
            }
        }
        if !a.devel.usb_open {
            key(&mut a, &cx, Key::Char('u'));
        }
    }
    a.handle(&Event::Paste("K7QFM-2XPRT".into()), &cx);
    a.handle(&Event::FocusAt(super::usbc_page::CODE_ID, 5.0), &cx);
    a.handle(&Event::Tick, &cx);
    let _ = draw(&mut a, &cx, 360, 740);
}
