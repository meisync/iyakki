//! The approval controls of the chat: one chip with three plain modes (Ask first, Auto-approve safe, Autopilot), the card with
//! Allow once / Allow for this session / Deny, and what the person allowed for the session sticking for the whole chat.

use super::tests::{agent_cx, ask, render, run_until, turn};
use super::*;
use serde_json::json;

fn write(h: &std::path::Path, name: &str) -> (&'static str, serde_json::Value) {
    ("write_file", json!({"path": h.join(name).display().to_string(), "content": "x"}))
}

fn hit<'a>(g: &'a Gfx, label: &str) -> Option<&'a crate::ui::Hit> {
    g.hits.iter().find(|h| h.label == label)
}

fn key(app: &mut ChatApp, cx: &AppCx, k: Key) {
    app.handle(&Event::Key(k), cx);
}

#[test]
fn an_answer_for_this_session_sticks_for_the_next_messages_until_reset_or_a_new_chat() {
    let (cx, home) = agent_cx("apr-sess", |h| {
        let one = |id: &str, name: &str| {
            let (n, a) = write(h, name);
            turn("", vec![(id, n, a)])
        };
        vec![one("w1", "a.txt"), turn("Saved a.", vec![]), one("w2", "b.txt"), turn("Saved b.", vec![]), one("w3", "sub/c.txt"), turn("Not saved.", vec![]), one("w4", "d.txt"), turn("Saved d.", vec![])]
    });
    let mut app = ChatApp::new();
    key(&mut app, &cx, Key::Ctrl('g'));
    ask(&mut app, &cx, "save a");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some());
    // the card: Allow once, Allow for this session (with what it covers), Deny
    let g = render(&mut app, &cx);
    for want in ["Allow once (Ctrl+Y)", "Allow for this session (Ctrl+U)", "Deny (Ctrl+N)"] {
        assert!(hit(&g, want).is_some(), "{want}");
    }
    let btn = hit(&g, "Allow for this session (Ctrl+U)").unwrap().ev.clone();
    assert_eq!(btn, Event::Key(Key::Ctrl('u')));
    app.handle(&btn, &cx);
    run_until(&mut app, &cx, |a| !a.is_running());
    assert!(home.join("a.txt").exists());
    assert_eq!(app.session_approvals().len(), 1);
    // the indicator is on screen and clickable
    let g = render(&mut app, &cx);
    let ind = g.hits.iter().find(|h| h.label.starts_with("Approved") && h.label.ends_with(": 1")).expect("Approved this session: 1");
    assert_eq!(ind.ev, Event::Key(Key::Ctrl('Q')));
    // the next message (a new run) writes another file of the same folder without asking
    ask(&mut app, &cx, "save b");
    run_until(&mut app, &cx, |a| !a.is_running());
    assert!(home.join("b.txt").exists(), "no card for the same tool in the same scope");
    assert!(cx.audit.recent(60).iter().any(|l| l.contains("write_file:approved:session")));
    // another folder is another scope: it asks, and a denial writes nothing
    ask(&mut app, &cx, "save c");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some());
    key(&mut app, &cx, Key::Ctrl('n'));
    run_until(&mut app, &cx, |a| !a.is_running());
    assert!(!home.join("sub/c.txt").exists());
    // the list: open it, it shows the grant and its Revoke row, Reset forgets everything
    key(&mut app, &cx, Key::Ctrl('Q'));
    let g = render(&mut app, &cx);
    assert!(hit(&g, "Reset approvals (Ctrl+0)").is_some() && g.hits.iter().any(|h| h.ev == Event::Row(0)), "the list is open");
    let reset = hit(&g, "Reset approvals (Ctrl+0)").unwrap().ev.clone();
    app.handle(&reset, &cx);
    assert!(app.session_approvals().is_empty());
    assert!(cx.audit.recent(60).iter().any(|l| l.contains("agent.approvals.reset")));
    // after the reset the same call asks again
    ask(&mut app, &cx, "save d");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some());
    key(&mut app, &cx, Key::Ctrl('u'));
    run_until(&mut app, &cx, |a| !a.is_running());
    assert_eq!(app.session_approvals().len(), 1);
    // a new chat ends the session: its approvals go
    app.new_chat();
    assert!(app.session_approvals().is_empty(), "approvals do not outlive the chat");
}

#[test]
fn a_session_approval_can_be_revoked_from_the_list_by_click_or_key() {
    let (cx, _home) = agent_cx("apr-revoke", |h| {
        let (n, a) = write(h, "a.txt");
        vec![turn("", vec![("w1", n, a)]), turn("ok", vec![])]
    });
    let mut app = ChatApp::new();
    key(&mut app, &cx, Key::Ctrl('g'));
    ask(&mut app, &cx, "save");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some());
    key(&mut app, &cx, Key::Ctrl('u'));
    run_until(&mut app, &cx, |a| !a.is_running());
    assert_eq!(app.session_approvals().len(), 1);
    assert!(app.session_approvals()[0].label.starts_with("write_file: files in the folder "), "{:?}", app.session_approvals());
    key(&mut app, &cx, Key::Ctrl('Q'));
    let g = render(&mut app, &cx);
    let row = g.hits.iter().find(|h| h.ev == Event::Row(0)).expect("Revoke").ev.clone();
    app.handle(&row, &cx);
    assert!(app.session_approvals().is_empty());
    assert!(cx.audit.recent(40).iter().any(|l| l.contains("agent.approvals.revoke")));
    assert!(!app.grants_open, "the empty list closes");
}

#[test]
fn a_call_that_always_asks_says_so_offers_no_session_button_and_is_never_remembered() {
    let (mut cx, _home) = agent_cx("apr-hard", |_| vec![turn("", vec![("r1", "run_command", json!({"program": "ssh", "args": ["box", "rm", "old.txt"]}))]), turn("", vec![("r2", "run_command", json!({"program": "ssh", "args": ["box", "rm", "old.txt"]}))]), turn("done", vec![])]);
    cx.session.policy.cli = iyakki_sec::policy::CliLevel::Full;
    let mut app = ChatApp::new();
    key(&mut app, &cx, Key::Ctrl('g'));
    key(&mut app, &cx, Key::Ctrl('q')); // Auto-approve safe: a destructive command still asks
    ask(&mut app, &cx, "install kubectl and clean up");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some());
    let (_, _, summary) = app.pending_approval().unwrap().clone();
    assert!(summary.contains("always asks:"), "{summary}");
    let g = render(&mut app, &cx);
    assert!(hit(&g, "Allow once (Ctrl+Y)").is_some() && hit(&g, "Deny (Ctrl+N)").is_some());
    assert!(hit(&g, "Allow for this session (Ctrl+U)").is_none(), "no session button for an always-ask call");
    // even the key only allows this once
    key(&mut app, &cx, Key::Ctrl('u'));
    run_until(&mut app, &cx, |a| a.pending_approval().is_some() || !a.is_running());
    assert!(app.pending_approval().is_some(), "the same command asks again");
    assert!(app.session_approvals().is_empty());
    key(&mut app, &cx, Key::Ctrl('n'));
    run_until(&mut app, &cx, |a| !a.is_running());
}

#[test]
fn the_chip_names_three_plain_modes_and_ctrl_q_cycles_them() {
    let (cx, home) = agent_cx("apr-modes", |_| vec![]);
    // a stand-alone chat has two steps and the kept variant: Ask first -> Auto-approve safe -> kept -> Ask first
    let mut app = ChatApp::new();
    key(&mut app, &cx, Key::Ctrl('g'));
    assert_eq!(app.mode_label(), "Ask first");
    let g = render(&mut app, &cx);
    assert!(g.hits.iter().any(|h| h.label == "Approvals: Ask first" || h.label == "Ask first"), "{:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
    key(&mut app, &cx, Key::Ctrl('q'));
    assert_eq!(app.mode_label(), "Auto-approve safe");
    key(&mut app, &cx, Key::Ctrl('q'));
    assert_eq!(app.mode_label(), "Auto-approve safe · kept");
    assert!(app.mode_hint().contains("kept"));
    key(&mut app, &cx, Key::Ctrl('q'));
    assert_eq!(app.mode_label(), "Ask first");
    assert!(app.mode_hint().starts_with("Ask first: asks before anything you have not approved; remembers your session approvals"));
    // a workspace chat has Autopilot as the third mode
    let scope = ProjectScope { name: "Web".into(), root: home.clone(), key: "web".into(), features: home.join("FEATURES.md"), cluster: None };
    let mut ws = ChatApp::new().with_agent(true).with_project(scope);
    for want in ["Auto-approve safe", "Auto-approve safe · kept", "Autopilot", "Ask first"] {
        key(&mut ws, &cx, Key::Ctrl('q'));
        assert_eq!(ws.mode_label(), want);
    }
    assert!(!ws.autopilot() && ws.approvals() == Approvals::Ask);
    assert!(!is_pinned(&cx, &ws.skey(&cx)), "the kept mode does not outlive Autopilot");
    // Autopilot still honours Ctrl+Shift+A
    key(&mut ws, &cx, Key::Ctrl('A'));
    assert_eq!(ws.mode_label(), "Autopilot");
    assert!(ws.mode_hint().contains("still stops for always-ask items"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn saved_names_old_and_new_map_to_the_modes_and_a_saved_pin_still_opens_in_the_kept_mode() {
    for (name, want) in [("ask", Approvals::Ask), ("Ask", Approvals::Ask), ("Ask first", Approvals::Ask), ("auto", Approvals::Auto), ("AUTO", Approvals::Auto), ("Auto-approve safe", Approvals::Auto), ("auto-pinned", Approvals::Pinned), ("pinned", Approvals::Pinned), ("Auto-approve safe · kept", Approvals::Pinned)] {
        let got = Approvals::from_saved(name);
        // the display form with the kept suffix is not a saved name: it maps by its prefix
        let got = got.or_else(|| name.strip_suffix(" · kept").and_then(Approvals::from_saved).map(|_| Approvals::Pinned));
        assert_eq!(got, Some(want), "{name}");
    }
    assert_eq!(Approvals::from_saved("something else"), None);
    // the audit ids never changed
    assert_eq!([Approvals::Ask.name(), Approvals::Auto.name(), Approvals::Pinned.name()], ["ask", "auto", "auto-pinned"]);
    // a pin file written by an earlier version (a list of workspace keys) still switches the kept mode on
    let (cx, _home) = agent_cx("apr-migrate", |_| vec![]);
    let app = ChatApp::new();
    let k = app.skey(&cx);
    std::fs::write(pin_file(&cx), serde_json::to_vec(&vec![k.clone()]).unwrap()).unwrap();
    assert_eq!(ChatApp::new().resumed(&cx).approvals(), Approvals::Pinned);
    assert_eq!(ChatApp::new().resumed(&cx).mode_label(), "Auto-approve safe · kept");
    std::fs::write(pin_file(&cx), b"[]").unwrap();
}

/// The chip, the indicator and the card buttons fit and stay readable at the sizes people use.
#[test]
fn the_chip_the_indicator_and_the_card_buttons_fit_at_every_size() {
    let (cx, _home) = agent_cx("apr-geom", |h| {
        let (n, a) = write(h, "a.txt");
        vec![turn("", vec![("w1", n, a)]), turn("ok", vec![]), turn("", vec![("w2", "write_file", json!({"path": "/etc/shadow", "content": "x"}))]), turn("no", vec![])]
    });
    let mut app = ChatApp::new();
    key(&mut app, &cx, Key::Ctrl('g'));
    let at = |app: &mut ChatApp, w: u32, h: u32| -> Gfx {
        let s = crate::shell::scale_for(w, h);
        let mut g = Gfx::new(w, h);
        g.clear(rgb(0, 0, 0));
        let top = 46.0 * s;
        app.render(&mut g, R::new(0.0, top, w as f32, h as f32 - top), s, &cx, 0);
        g
    };
    let sizes = [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)];
    let inside = |g: &Gfx, l: &crate::ui::Hit, w: u32, h: u32| {
        let r = l.rect;
        assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{:?} leaves {w}x{h}", l.label);
        let _ = g;
    };
    // the longest chip (kept) without grants
    key(&mut app, &cx, Key::Ctrl('q'));
    key(&mut app, &cx, Key::Ctrl('q'));
    for (w, h) in sizes {
        let g = at(&mut app, w, h);
        let chip = g.hits.iter().find(|x| x.ev == Event::Key(Key::Ctrl('q')) && (x.label.contains("Auto-approve safe"))).unwrap_or_else(|| panic!("the chip at {w}x{h}: {:?}", g.hits.iter().map(|x| &x.label).collect::<Vec<_>>()));
        inside(&g, chip, w, h);
        let s = crate::shell::scale_for(w, h);
        assert!(chip.rect.h >= 13.0 * s * 1.5, "{w}x{h}: chip height {}", chip.rect.h);
    }
    // a grant (given in Ask first), then the longest chip beside the indicator
    key(&mut app, &cx, Key::Ctrl('q'));
    assert_eq!(app.mode_label(), "Ask first");
    ask(&mut app, &cx, "save");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some());
    key(&mut app, &cx, Key::Ctrl('u'));
    run_until(&mut app, &cx, |a| !a.is_running());
    key(&mut app, &cx, Key::Ctrl('q'));
    key(&mut app, &cx, Key::Ctrl('q'));
    for (w, h) in sizes {
        let g = at(&mut app, w, h);
        let ind = g.hits.iter().find(|x| (x.label.starts_with("Approved") || x.label.starts_with("✓ ")) && x.ev == Event::Key(Key::Ctrl('Q'))).unwrap_or_else(|| panic!("the indicator at {w}x{h}: {:?}", g.hits.iter().map(|x| &x.label).collect::<Vec<_>>()));
        inside(&g, ind, w, h);
        let chip = g.hits.iter().find(|x| x.ev == Event::Key(Key::Ctrl('q')) && x.label.contains("Auto-approve safe")).expect("chip");
        let (a, b) = (chip.rect, ind.rect);
        assert!(a.x + a.w <= b.x + 1.0 || b.x + b.w <= a.x + 1.0, "{w}x{h}: the chip and the indicator overlap {a:?} {b:?}");
    }
    // the open list fits too
    key(&mut app, &cx, Key::Ctrl('Q'));
    for (w, h) in sizes {
        let g = at(&mut app, w, h);
        for want in ["Reset approvals (Ctrl+0)", "Close (Esc)"] {
            let b = hit(&g, want).unwrap_or_else(|| panic!("{want} at {w}x{h}"));
            inside(&g, b, w, h);
        }
        inside(&g, g.hits.iter().find(|x| x.ev == Event::Row(0)).expect("Revoke"), w, h);
    }
    key(&mut app, &cx, Key::Esc);
    assert!(!app.grants_open);
    key(&mut app, &cx, Key::Ctrl('q'));
    assert_eq!(app.mode_label(), "Ask first");
    // the card (the second run asks about a path outside the folder): three buttons that fit, readable text
    ask(&mut app, &cx, "save elsewhere");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some());
    for (w, h) in sizes {
        let g = at(&mut app, w, h);
        for want in ["Allow once (Ctrl+Y)", "Allow for this session (Ctrl+U)", "Deny (Ctrl+N)"] {
            let b = hit(&g, want).unwrap_or_else(|| panic!("{want} at {w}x{h}"));
            assert!(b.rect.x >= -0.5 && b.rect.x + b.rect.w <= w as f32 + 0.5, "{want} leaves {w}x{h}: {:?}", b.rect);
            let s = crate::shell::scale_for(w, h);
            assert!(b.rect.h >= 14.0 * s * 2.0, "{want} at {w}x{h} is {:.1} px high", b.rect.h);
        }
    }
    key(&mut app, &cx, Key::Ctrl('n'));
    run_until(&mut app, &cx, |a| !a.is_running());
}
