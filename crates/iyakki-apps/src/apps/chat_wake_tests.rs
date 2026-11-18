//! The waker of pending work in the chat (F-AI-16): work that ends wakes the assistant with one synthetic turn, once.

use super::tests::{agent_cx, run_until, turn};
use super::*;
use iyakki_agent::pending::{Ledger, Source, State};

fn woken(app: &ChatApp) -> usize {
    app.messages().iter().filter(|m| m.role == Role::User && m.content.contains("Work you were waiting for has ended")).count()
}

#[test]
fn work_that_ends_wakes_the_agent_chat_with_one_turn_and_not_a_second_time() {
    // home-ok: the chat's own test home under the process temp folder
    let (cx, _home) = agent_cx("wake1", |_| vec![turn("Noted, the pull is done.", vec![])]);
    let mut app = ChatApp::new();
    app.handle(&Event::Key(Key::Ctrl('g')), &cx);
    let ledger = Ledger::open(&cx.data_dir().join("pending"), &cx.session.user);
    ledger.report(Source::Pull, "pull-1", "pull of a model", "", State::Running, "");
    // the waker exists after the first tick; work that is still running wakes nobody
    for _ in 0..20 {
        app.handle(&Event::Tick, &cx);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(woken(&app), 0);
    assert!(!app.is_running());
    ledger.report(Source::Pull, "pull-1", "pull of a model", "", State::FinishedOk, "all layers downloaded");
    run_until(&mut app, &cx, |a| woken(a) == 1);
    run_until(&mut app, &cx, |a| !a.is_running());
    let text = app.messages().iter().find(|m| m.content.contains("Work you were waiting for has ended")).map(|m| m.content.clone()).unwrap_or_default();
    assert!(text.contains("pull of a model"), "{text}");
    // the ticket is told: more ticks start nothing
    for _ in 0..30 {
        app.handle(&Event::Tick, &cx);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(woken(&app), 1);
    assert!(!app.is_running());
}

#[test]
fn nothing_wakes_a_chat_that_is_not_the_agent() {
    let (cx, _home) = agent_cx("wake2", |_| vec![turn("unused", vec![])]);
    let mut app = ChatApp::new();
    let ledger = Ledger::open(&cx.data_dir().join("pending"), &cx.session.user);
    ledger.report(Source::Flow, "flow-1", "a flow run", "", State::FinishedFailed, "step 2 failed");
    for _ in 0..40 {
        app.handle(&Event::Tick, &cx);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(woken(&app), 0);
    assert!(!app.is_running());
}

#[test]
fn the_waiting_line_shows_what_the_assistant_waits_for() {
    let (cx, _home) = agent_cx("wake3", |_| vec![turn("ok", vec![])]);
    let mut app = ChatApp::new();
    app.handle(&Event::Key(Key::Ctrl('g')), &cx);
    let ledger = Ledger::open(&cx.data_dir().join("pending"), &cx.session.user);
    ledger.report(Source::Rollout, "ro-1", "rollout of the web app", "", State::Running, "");
    run_until(&mut app, &cx, |a| a.wait_line.is_some());
    let line = app.wait_line.clone().unwrap_or_default();
    assert!(line.starts_with("Waiting for") && line.contains("rollout"), "{line}");
    // drawn: the line's text is on the screen whatever the font (a text op is recorded)
    let g = super::tests::render_with(&mut app, &cx, 1280, 720);
    assert!(g.layout_log.texts.iter().any(|t| t.3.starts_with("Waiting for")), "the Waiting line was not drawn");
    ledger.report(Source::Rollout, "ro-1", "rollout of the web app", "", State::Stopped, "stopped");
}
