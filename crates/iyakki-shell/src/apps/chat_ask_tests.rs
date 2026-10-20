//! Tests for the answer buttons under a question the assistant ends with (yes/no, a choice, "Other…"), their keys, their place
//! in the layout, and the `ask_user` tool.

use super::tests::{agent_cx, ask, render_with, run_until, turn};
use super::*;
use crate::apps::tests::cx_with_backend;
use crate::apps::App;
use serde_json::json;

/// A plain-chat model that answers from a queue.
struct Queue(std::sync::Mutex<std::collections::VecDeque<String>>);
impl ChatBackend for Queue {
    fn complete(&self, _: &[Message]) -> Result<String, String> {
        self.0.lock().unwrap().pop_front().ok_or_else(|| "no more answers".to_string())
    }
}

fn plain_chat(tag: &str, answers: &[&str]) -> (ChatApp, AppCx) {
    let (cx, _d) = cx_with_backend(tag, Queue(std::sync::Mutex::new(answers.iter().map(|s| s.to_string()).collect())));
    (ChatApp::new(), cx)
}

fn settle(app: &mut ChatApp, cx: &AppCx) {
    for _ in 0..400 {
        app.handle(&Event::Tick, cx);
        if !app.is_waiting() && !app.is_running() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the chat did not settle");
}

fn chips(g: &Gfx) -> Vec<(String, Event, R)> {
    g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(n) if (CHIP_BASE..CHIP_BASE + 12).contains(&n))).map(|h| (h.label.clone(), h.ev.clone(), h.rect)).collect()
}

fn overlap(a: R, b: R) -> bool {
    a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5
}

fn last_user(app: &ChatApp) -> String {
    app.messages().iter().rev().find(|m| m.role == Role::User).map(|m| m.content.clone()).unwrap_or_default()
}

#[test]
fn a_yes_no_question_shows_yes_no_and_other_and_a_click_sends_the_answer_like_typing() {
    let (mut app, cx) = plain_chat("askchip1", &["The build is still running. Would you like me to check the status again?", "Checking.", "Fine."]);
    ask(&mut app, &cx, "is the build done");
    settle(&mut app, &cx);
    let g = render_with(&mut app, &cx, 1280, 720);
    let labels: Vec<String> = chips(&g).into_iter().map(|c| c.0).collect();
    assert_eq!(labels, vec!["Yes (Y)", "No (N)", "Other…"]);
    // a redraw keeps them
    assert_eq!(chips(&render_with(&mut app, &cx, 1280, 720)).len(), 3);
    // the first one sends "Yes" as the next user message and the buttons go away
    let ev = chips(&g)[0].1.clone();
    app.handle(&ev, &cx);
    assert_eq!(last_user(&app), "Yes");
    assert!(app.is_waiting());
    assert!(chips(&render_with(&mut app, &cx, 1280, 720)).is_empty());
    settle(&mut app, &cx);
    assert_eq!(app.messages().last().unwrap().content, "Checking.");
    assert!(chips(&render_with(&mut app, &cx, 1280, 720)).is_empty(), "a statement has no buttons");
    // "Other…" only hides them, and sends nothing
    let (mut app, cx) = plain_chat("askchip1b", &["Shall I proceed?"]);
    ask(&mut app, &cx, "go");
    settle(&mut app, &cx);
    let n = app.messages().len();
    let other = chips(&render_with(&mut app, &cx, 1280, 720)).last().unwrap().1.clone();
    app.handle(&other, &cx);
    assert!(chips(&render_with(&mut app, &cx, 1280, 720)).is_empty() && app.messages().len() == n && !app.is_waiting());
}

#[test]
fn keys_pick_an_answer_only_while_the_box_is_empty_and_the_buttons_show() {
    let (mut app, cx) = plain_chat("askchip2", &["Shall I proceed?", "Done.", "Which one do you want: A or B?", "Ok.", "Shall I proceed?", "x", "Shall I proceed?"]);
    ask(&mut app, &cx, "go");
    settle(&mut app, &cx);
    app.handle(&Event::Key(Key::Char('y')), &cx);
    assert_eq!(last_user(&app), "Yes");
    settle(&mut app, &cx);
    // a choice: 2 picks the second option; Y is just a letter there
    ask(&mut app, &cx, "pick");
    settle(&mut app, &cx);
    let labels: Vec<String> = chips(&render_with(&mut app, &cx, 1280, 720)).into_iter().map(|c| c.0).collect();
    assert_eq!(labels, vec!["1 · A", "2 · B", "Other…"]);
    app.handle(&Event::Key(Key::Char('y')), &cx);
    assert_eq!(app.input_text(), "y", "Y types when the question is not yes/no");
    app.handle(&Event::Key(Key::Backspace), &cx);
    app.handle(&Event::Key(Key::Char('9')), &cx);
    assert_eq!(app.input_text(), "9", "a number with no such button types");
    app.handle(&Event::Key(Key::Backspace), &cx);
    app.handle(&Event::Key(Key::Char('2')), &cx);
    assert_eq!(last_user(&app), "B");
    settle(&mut app, &cx);
    // with text already typed the keys are ordinary typing
    ask(&mut app, &cx, "again");
    settle(&mut app, &cx);
    app.handle(&Event::Key(Key::Char('h')), &cx);
    app.handle(&Event::Key(Key::Char('n')), &cx);
    assert_eq!(app.input_text(), "hn");
    assert_eq!(last_user(&app), "again");
    // and N says no
    let (mut app, cx) = plain_chat("askchip2b", &["Shall I proceed?"]);
    ask(&mut app, &cx, "go");
    settle(&mut app, &cx);
    app.handle(&Event::Key(Key::Char('N')), &cx);
    assert_eq!(last_user(&app), "No");
}

#[test]
fn a_half_typed_draft_survives_a_click_and_a_new_chat_drops_the_buttons() {
    let (mut app, cx) = plain_chat("askchip3", &["Shall I proceed?"]);
    ask(&mut app, &cx, "go");
    settle(&mut app, &cx);
    app.handle(&Event::Key(Key::Char('h')), &cx);
    let g = render_with(&mut app, &cx, 1280, 720);
    app.handle(&chips(&g)[1].1.clone(), &cx);
    assert_eq!(last_user(&app), "No");
    assert_eq!(app.input_text(), "h", "the draft is still there");
    let (mut app, cx) = plain_chat("askchip3b", &["Shall I proceed?"]);
    ask(&mut app, &cx, "go");
    settle(&mut app, &cx);
    app.new_chat();
    assert!(chips(&render_with(&mut app, &cx, 1280, 720)).is_empty(), "buttons are not kept across a reload");
}

#[test]
fn options_come_from_a_numbered_list_next_to_the_question() {
    let (mut app, cx) = plain_chat("askchip4", &["Registries:\n\n1. Docker Hub: the default\n2. GHCR\n\nWhich one should I use?"]);
    ask(&mut app, &cx, "deploy");
    settle(&mut app, &cx);
    let g = render_with(&mut app, &cx, 1280, 720);
    let labels: Vec<String> = chips(&g).into_iter().map(|c| c.0).collect();
    assert_eq!(labels, vec!["1 · Docker Hub", "2 · GHCR", "Other…"]);
    app.handle(&chips(&g)[0].1.clone(), &cx);
    assert_eq!(last_user(&app), "Docker Hub");
}

#[test]
fn the_ask_user_tool_round_trip_shows_exact_buttons_and_the_click_continues_the_run() {
    let (cx, _home) = agent_cx("askchip5", |_| {
        vec![
            turn("", vec![("q1", "ask_user", json!({"question": "Which registry should I push to?", "options": ["Docker Hub", "GHCR"]}))]),
            turn("Pushing to GHCR now.", vec![]),
        ]
    });
    let mut app = ChatApp::new();
    app.handle(&Event::Key(Key::Ctrl('g')), &cx);
    ask(&mut app, &cx, "ship it");
    run_until(&mut app, &cx, |a| !a.is_running());
    // no approval was asked; the question is in the chat with its options
    assert!(app.pending_approval().is_none());
    assert_eq!(app.messages().last().unwrap().content, "Which registry should I push to?\n\n1. Docker Hub\n2. GHCR");
    let g = render_with(&mut app, &cx, 1280, 720);
    let labels: Vec<String> = chips(&g).into_iter().map(|c| c.0).collect();
    assert_eq!(labels, vec!["1 · Docker Hub", "2 · GHCR", "Other…"]);
    app.handle(&chips(&g)[1].1.clone(), &cx);
    assert_eq!(last_user(&app), "GHCR");
    run_until(&mut app, &cx, |a| !a.is_running());
    assert_eq!(app.messages().last().unwrap().content, "Pushing to GHCR now.");
    assert!(chips(&render_with(&mut app, &cx, 1280, 720)).is_empty());
}

#[test]
fn the_agent_chat_finds_a_question_in_prose_too() {
    let (cx, _home) = agent_cx("askchip6", |_| vec![turn("Plan ready. Shall I proceed?", vec![])]);
    let mut app = ChatApp::new();
    app.handle(&Event::Key(Key::Ctrl('g')), &cx);
    ask(&mut app, &cx, "plan it");
    run_until(&mut app, &cx, |a| !a.is_running());
    let labels: Vec<String> = chips(&render_with(&mut app, &cx, 1280, 720)).into_iter().map(|c| c.0).collect();
    assert_eq!(labels, vec!["Yes (Y)", "No (N)", "Other…"]);
}

/// Nothing the buttons use overlaps anything else that is clickable, stays inside the pane, and the conversation ends above them.
fn assert_no_overlap(g: &Gfx, pane: R, what: &str) {
    let cs = chips(g);
    assert!(cs.len() >= 3, "{what}: {cs:?}");
    for (i, (la, _, ra)) in cs.iter().enumerate() {
        assert!(ra.x >= pane.x - 0.5 && ra.x + ra.w <= pane.x + pane.w + 0.5 && ra.y >= pane.y && ra.y + ra.h <= pane.y + pane.h, "{what}: {la} {ra:?} outside {pane:?}");
        for (lb, _, rb) in cs.iter().skip(i + 1) {
            assert!(!overlap(*ra, *rb), "{what}: {la} overlaps {lb}");
        }
        for h in &g.hits {
            if matches!(h.ev, Event::Tab(n) if (CHIP_BASE..CHIP_BASE + 12).contains(&n)) {
                continue;
            }
            assert!(!overlap(*ra, h.rect), "{what}: the button {la} {ra:?} overlaps {:?} {:?}", h.label, h.rect);
        }
    }
}

fn long_answer() -> String {
    let mut s = String::from("Here are the registries I can push to:\n\n");
    for (i, n) in ["Docker Hub (the public default registry)", "GitHub Container Registry for this organisation", "Quay", "A private registry on the cluster", "Amazon ECR in the staging account"].iter().enumerate() {
        s += &format!("{}. {n}\n", i + 1);
    }
    s + "\nWhich one should I use?"
}

#[test]
fn the_buttons_do_not_overlap_anything_at_1280x720_with_a_long_conversation() {
    let (mut app, cx) = plain_chat("askchip7", &[&long_answer()]);
    for i in 0..12 {
        app.msgs.push(Message::user(format!("question {i}")));
        app.msgs.push(Message::assistant(format!("An answer number {i} with enough words to fill a line or two of the conversation view.")));
    }
    ask(&mut app, &cx, "deploy");
    settle(&mut app, &cx);
    let g = render_with(&mut app, &cx, 1280, 720);
    assert_no_overlap(&g, R::new(0.0, 46.0, 1280.0, 674.0), "1280x720");
    // the ask area of the transcript ends above the buttons
    let top = chips(&g).iter().map(|c| c.2.y).fold(f32::MAX, f32::min);
    for h in g.hits.iter().filter(|h| h.label == "text area") {
        assert!(h.rect.y + h.rect.h <= top, "the conversation reaches {} but the buttons start at {top}", h.rect.y + h.rect.h);
    }
    // narrower and shorter windows too
    for (w, h) in [(700u32, 520u32), (480, 600)] {
        let g = render_with(&mut app, &cx, w, h);
        assert_no_overlap(&g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), &format!("{w}x{h}"));
    }
}

#[test]
fn the_buttons_fit_the_narrow_ide_chat_pane() {
    let (mut app, cx) = plain_chat("askchip8", &[&long_answer()]);
    let root = std::env::temp_dir().join(format!("iyakki-askchip-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    app = app.with_project(ProjectScope { name: "demo".into(), root: root.clone(), key: "ide:demo".into(), features: root.join("FEATURES.md"), cluster: None });
    ask(&mut app, &cx, "deploy");
    settle(&mut app, &cx);
    for (w, h) in [(340u32, 640u32), (300, 480)] {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(9, 11, 20));
        let pane = R::new(0.0, 40.0, w as f32, h as f32 - 40.0);
        app.render(&mut g, pane, 1.0, &cx, 0);
        assert_no_overlap(&g, pane, &format!("ide {w}x{h}"));
        // the long labels are cut to the pane, never wider than it
        assert!(chips(&g).iter().all(|c| c.2.w <= w as f32), "{:?}", chips(&g));
    }
    let _ = std::fs::remove_dir_all(&root);
}
