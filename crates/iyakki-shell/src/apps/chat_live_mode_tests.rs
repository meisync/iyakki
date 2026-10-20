//! The approval mode is live: the chip and its keys stay enabled while a run goes (in CHAT and in the IDE chat), and a switch
//! applies to that run at once (see `iyakki_agent::livemode`).

use super::tests::{agent_cx, ask, run_until, turn};
use super::*;
use serde_json::json;

fn key(app: &mut ChatApp, cx: &AppCx, k: Key) {
    app.handle(&Event::Key(k), cx);
}

fn at(app: &mut ChatApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let s = crate::shell::scale_for(w, h);
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let top = 46.0 * s;
    app.render(&mut g, R::new(0.0, top, w as f32, h as f32 - top), s, cx, 0);
    g
}

fn notes(app: &ChatApp) -> Vec<String> {
    app.messages().iter().filter(|m| m.role == Role::Tool).map(|m| m.content.clone()).collect()
}

const SIZES: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480)];

/// The chip is on screen and clickable while the run waits, it does not overlap Stop run, and it sits where it sat before the run.
fn chip_while_running(app: &mut ChatApp, cx: &AppCx, before: &[(u32, u32, R)]) {
    for (w, h) in SIZES {
        let g = at(app, cx, w, h);
        let chip = g.hits.iter().find(|x| x.ev == Event::Key(Key::Ctrl('q'))).unwrap_or_else(|| panic!("the chip while running at {w}x{h}: {:?}", g.hits.iter().map(|x| &x.label).collect::<Vec<_>>()));
        let r = chip.rect;
        assert!(r.x >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y >= -0.5 && r.y + r.h <= h as f32 + 0.5, "the chip leaves {w}x{h}: {r:?}");
        let stop = g.hits.iter().find(|x| x.ev == Event::Key(Key::Ctrl('k')) && x.label.contains("Stop run")).unwrap_or_else(|| panic!("Stop run at {w}x{h}"));
        let s = stop.rect;
        assert!(r.x + r.w <= s.x + 1.0 || s.x + s.w <= r.x + 1.0, "{w}x{h}: the chip and Stop run overlap {r:?} {s:?}");
        if let Some((_, _, b)) = before.iter().find(|(bw, bh, _)| *bw == w && *bh == h) {
            assert!((b.x + b.w - (r.x + r.w)).abs() < 1.0 || (b.x - r.x).abs() < 1.0, "{w}x{h}: the chip moved when the run started: {b:?} -> {r:?}");
            assert!((b.y - r.y).abs() < 1.0, "{w}x{h}: the chip moved up or down: {b:?} -> {r:?}");
        }
    }
}

fn chip_rects(app: &mut ChatApp, cx: &AppCx) -> Vec<(u32, u32, R)> {
    SIZES.iter().map(|&(w, h)| (w, h, at(app, cx, w, h).hits.iter().find(|x| x.ev == Event::Key(Key::Ctrl('q'))).expect("chip").rect)).collect()
}

#[test]
fn in_chat_the_chip_stays_enabled_during_a_run_and_switching_to_auto_approves_the_waiting_card() {
    let (cx, home) = agent_cx("live-chat", |h| vec![turn("", vec![("w1", "write_file", json!({"path": h.join("a.txt").display().to_string(), "content": "x"}))]), turn("Saved.", vec![])]);
    let mut app = ChatApp::new();
    key(&mut app, &cx, Key::Ctrl('g'));
    let before = chip_rects(&mut app, &cx);
    ask(&mut app, &cx, "save a");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some());
    assert!(app.is_running());
    chip_while_running(&mut app, &cx, &before);
    assert!(app.mode_hint().contains("Changes apply to this run now"), "{}", app.mode_hint());
    // a click on the chip (the same event as Ctrl+Q) while the card waits
    let g = at(&mut app, &cx, 1280, 720);
    let ev = g.hits.iter().find(|x| x.ev == Event::Key(Key::Ctrl('q'))).unwrap().ev.clone();
    app.handle(&ev, &cx);
    assert_eq!(app.mode_label(), "Auto-approve safe");
    run_until(&mut app, &cx, |a| !a.is_running());
    assert!(home.join("a.txt").exists(), "the waiting write ran");
    let n = notes(&app).join("\n");
    assert!(n.contains("Approvals changed to Auto-approve safe during this run"), "{n}");
    assert!(n.contains("write_file: approved by switching to Auto-approve safe"), "{n}");
    let audit = cx.audit.recent(60).join("\n");
    assert!(audit.contains("agent.approvals.live") && audit.contains("approvals mode changed to Auto-approve safe during run ") && audit.contains(&format!("by {}", cx.session.user)), "{audit}");
    assert!(audit.contains("write_file:approved:mode-switch"), "{audit}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn an_always_ask_card_keeps_waiting_after_a_switch_and_back_to_ask_first_makes_the_next_call_ask() {
    let (mut cx, home) = agent_cx("live-hard", |h| {
        vec![
            turn("", vec![("r1", "run_command", json!({"program": "ssh", "args": ["box", "rm", "old.txt"]}))]),
            turn("", vec![("w1", "write_file", json!({"path": h.join("b.txt").display().to_string(), "content": "x"}))]),
            turn("done", vec![]),
        ]
    });
    cx.session.policy.cli = iyakki_sec::policy::CliLevel::Full;
    let mut app = ChatApp::new();
    key(&mut app, &cx, Key::Ctrl('g'));
    ask(&mut app, &cx, "clean up and save");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some());
    // Ask first -> Auto-approve safe: the destructive command still waits
    key(&mut app, &cx, Key::Ctrl('q'));
    for _ in 0..30 {
        app.handle(&Event::Tick, &cx);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(app.pending_approval().is_some_and(|p| p.1 == "run_command"), "always-ask stays pending");
    // -> kept -> Ask first, while the card still waits; then the person allows the command
    key(&mut app, &cx, Key::Ctrl('q'));
    key(&mut app, &cx, Key::Ctrl('q'));
    assert_eq!(app.mode_label(), "Ask first");
    key(&mut app, &cx, Key::Ctrl('y'));
    // the next risky call asks again
    run_until(&mut app, &cx, |a| a.pending_approval().is_some_and(|p| p.1 == "write_file"));
    key(&mut app, &cx, Key::Ctrl('n'));
    run_until(&mut app, &cx, |a| !a.is_running());
    assert!(!home.join("b.txt").exists());
    assert!(notes(&app).iter().any(|n| n.contains("Approvals changed to Ask first during this run: the next risky call asks")), "{:?}", notes(&app));
    let _ = std::fs::remove_dir_all(&home);
}

fn ws_scope(home: &std::path::Path) -> ProjectScope {
    ProjectScope { name: "Web".into(), root: home.to_path_buf(), key: "web".into(), features: home.join("FEATURES.md"), cluster: None }
}

#[test]
fn in_the_ide_chat_autopilot_switched_on_mid_turn_lets_the_turn_finish_then_works_through_the_plan() {
    let (cx, home) = agent_cx("live-ap-on", |_| {
        vec![
            // the plain turn: a write that asks in Ask first
            turn("", vec![("w0", "write_file", json!({"path": "notes.txt", "content": "plan\n"}))]),
            turn("Wrote the notes.", vec![]),
            // the Autopilot plan that follows
            turn("", vec![("a1", "board_add", json!({"prefix": "APP", "title": "Add a Dockerfile"}))]),
            turn("Planned one step.", vec![]),
            turn("", vec![("w1", "write_file", json!({"path": "Dockerfile", "content": "FROM node:20\n"}))]),
            turn("Wrote the Dockerfile.", vec![]),
            turn("Checked: the Dockerfile exists. All done.", vec![]),
        ]
    });
    let mut app = ChatApp::new().with_agent(true).with_project(ws_scope(&home));
    let before = chip_rects(&mut app, &cx);
    ask(&mut app, &cx, "add a dockerfile for the app");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some());
    chip_while_running(&mut app, &cx, &before);
    // Ctrl+Shift+A while the plain turn waits on its card: Autopilot on (it implies Auto, so the card is approved)
    key(&mut app, &cx, Key::Ctrl('A'));
    assert!(app.autopilot() && app.mode_label().starts_with("Autopilot"));
    run_until(&mut app, &cx, |a| !a.is_running() && a.messages().last().is_some_and(|m| m.content.contains("Autopilot finished")));
    assert_eq!(std::fs::read_to_string(home.join("notes.txt")).unwrap(), "plan\n");
    assert_eq!(std::fs::read_to_string(home.join("Dockerfile")).unwrap(), "FROM node:20\n");
    assert_eq!(app.messages().iter().filter(|m| m.role == Role::User).count(), 1, "the person typed once; the plan continued the turn");
    let n = notes(&app).join("\n");
    assert!(n.contains("Approvals changed to Autopilot during this run: this turn finishes, then Autopilot works through the plan"), "{n}");
    assert!(n.contains("write_file: approved by switching to Autopilot"), "{n}");
    let audit = cx.audit.recent(80).join("\n");
    assert!(audit.contains("continues the turn into the plan") && audit.contains("autopilot.start") && audit.contains("approvals mode changed to Autopilot during run"), "{audit}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn in_the_ide_chat_autopilot_switched_off_mid_run_finishes_the_step_and_hands_back_with_a_summary() {
    let (mut cx, home) = agent_cx("live-ap-off", |_| {
        vec![
            turn("", vec![("a1", "board_add", json!({"prefix": "APP", "title": "Clean the old build on the box"}))]),
            turn("Planned one step.", vec![]),
            // step 2 asks for an always-ask command; the person switches Autopilot off while it waits, then denies it
            turn("", vec![("r1", "run_command", json!({"program": "ssh", "args": ["box", "rm", "old.txt"]}))]),
            turn("Skipped the clean-up.", vec![]),
            turn("never", vec![]),
        ]
    });
    cx.session.policy.cli = iyakki_sec::policy::CliLevel::Full;
    let mut app = ChatApp::new().with_agent(true).with_project(ws_scope(&home));
    key(&mut app, &cx, Key::Ctrl('A'));
    ask(&mut app, &cx, "clean the box");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some_and(|p| p.1 == "run_command"));
    // while it runs the IDE row shows the chip and a Pause button
    let g = at(&mut app, &cx, 1280, 720);
    assert!(g.hits.iter().any(|x| x.ev == Event::Key(Key::Ctrl('q')) && x.label.contains("Autopilot")), "{:?}", g.hits.iter().map(|x| &x.label).collect::<Vec<_>>());
    assert!(g.hits.iter().any(|x| x.ev == Event::Tab(AP_PAUSE)), "Pause");
    key(&mut app, &cx, Key::Ctrl('A'));
    assert!(!app.autopilot());
    assert!(app.is_running(), "switching off does not stop the step");
    key(&mut app, &cx, Key::Ctrl('n'));
    run_until(&mut app, &cx, |a| !a.is_running());
    let last = app.messages().last().unwrap().content.clone();
    assert!(last.contains("Autopilot switched off: stopped after step 2") && last.contains("Resume"), "{last}");
    assert!(notes(&app).iter().any(|n| n.contains("Autopilot stops after the current step and hands back with a summary")), "{:?}", notes(&app));
    let saved = iyakki_agent::autopilot::State::load(&home.join(".iyakki/autopilot.json")).unwrap();
    assert_eq!(saved.status, "paused");
    let audit = cx.audit.recent(80).join("\n");
    assert!(audit.contains("autopilot.pause") && audit.contains("switched off at step 2"), "{audit}");
    let _ = std::fs::remove_dir_all(&home);
}
