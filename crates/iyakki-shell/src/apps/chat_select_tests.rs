//! Tests for selecting text in the chat transcript.

use super::tests::{render_with, transcript_chat};
use super::*;
use crate::apps::tests::cx_with_backend;
use crate::apps::App;

// ---- selecting text in the transcript

/// Press (and release) at a character of a selectable line, as the shell would deliver it.
fn press_at(app: &mut ChatApp, cx: &AppCx, line: usize, ch: usize, clicks: u8, shift: bool) {
    let (x, y) = app.view.point_of(line, ch, false).expect("that line is on screen");
    crate::ui::set_click_count(clicks);
    crate::ui::set_shift_held(shift);
    app.handle(&Event::AreaAt(chat_view::AREA, x, y), cx);
    crate::ui::set_click_count(1);
    crate::ui::set_shift_held(false);
}

fn drag_to(app: &mut ChatApp, cx: &AppCx, line: usize, ch: usize) {
    let (x, y) = app.view.point_of(line, ch, false).expect("that line is on screen");
    crate::ui::set_dragging(true);
    app.handle(&Event::AreaAt(chat_view::AREA, x, y), cx);
    crate::ui::set_dragging(false);
}

fn line_of(app: &ChatApp, start: &str) -> usize {
    app.view.line_texts().iter().position(|t| t.starts_with(start)).unwrap_or_else(|| panic!("no line starting with {start:?} in {:?}", app.view.line_texts()))
}

#[test]
fn dragging_across_two_blocks_selects_text_and_ctrl_c_copies_exactly_that() {
    crate::ui::clipboard::clear();
    let (mut app, cx) = transcript_chat("chatsel1");
    render_with(&mut app, &cx, 1100, 1100);
    let texts = app.view.line_texts();
    let answer = line_of(&app, "I'll look at the folder");
    assert!(answer >= 1, "the question is wrapped over lines before the first answer: {texts:?}");
    // from inside the question to inside the answer, two blocks
    press_at(&mut app, &cx, 0, 6, 1, false);
    assert!(!app.view.has_selection(), "a press alone selects nothing");
    drag_to(&mut app, &cx, answer, 3);
    app.handle(&Event::MouseUp(0, 0), &cx);
    let mut expected: String = texts[0].chars().skip(6).collect();
    for t in &texts[1..answer] {
        expected.push('\n');
        expected.push_str(t);
    }
    expected.push_str("\n\nI'l");
    assert_eq!(app.view.selected_text().as_deref(), Some(expected.as_str()));
    // nothing was copied by selecting; Ctrl+C copies exactly the selection, as plain text
    assert!(crate::ui::clipboard::latest().is_none());
    app.handle(&Event::Key(Key::Ctrl('c')), &cx);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some(expected.as_str()));
    assert_eq!(crate::ui::clipboard::take_host_copy().as_deref(), Some(expected.as_str()), "and it goes to the system clipboard");
    // the selection is drawn (a translucent accent over the text)
    let plain = render_with(&mut app, &cx, 1100, 1100);
    app.view.clear_selection();
    let clear = render_with(&mut app, &cx, 1100, 1100);
    assert_ne!(plain.rgba(), clear.rgba(), "the highlight is painted");
}

#[test]
fn double_click_selects_a_word_triple_click_a_paragraph_and_shift_click_extends() {
    crate::ui::clipboard::clear();
    let (mut app, cx) = transcript_chat("chatsel2");
    render_with(&mut app, &cx, 1100, 1100);
    let l = line_of(&app, "I'll look at the folder");
    let at = app.view.line_texts()[l].find("folder").unwrap() + 2;
    press_at(&mut app, &cx, l, at, 2, false);
    assert_eq!(app.view.selected_text().as_deref(), Some("folder"));
    press_at(&mut app, &cx, l, at, 3, false);
    assert_eq!(app.view.selected_text().as_deref(), Some("I'll look at the folder first."));
    // a plain press, then Shift+press further on
    press_at(&mut app, &cx, l, 5, 1, false);
    press_at(&mut app, &cx, l, 9, 1, true);
    assert_eq!(app.view.selected_text().as_deref(), Some("look"));
    // Shift+click on another line grows it across lines
    let b = line_of(&app, "Want me to draft");
    press_at(&mut app, &cx, b, 4, 1, true);
    let t = app.view.selected_text().unwrap();
    assert!(t.starts_with("look at the folder first.") && t.ends_with("Want"), "{t:?}");
}

#[test]
fn a_click_no_longer_copies_it_clears_the_selection_and_ctrl_c_without_one_copies_nothing() {
    crate::ui::clipboard::clear();
    let (mut app, cx) = transcript_chat("chatsel3");
    render_with(&mut app, &cx, 1100, 1100);
    let l = line_of(&app, "I'll look at the folder");
    press_at(&mut app, &cx, l, 2, 1, false);
    app.handle(&Event::MouseUp(0, 0), &cx);
    assert!(crate::ui::clipboard::latest().is_none() && crate::ui::clipboard::take_host_copy().is_none(), "a click copies nothing");
    press_at(&mut app, &cx, l, 2, 2, false);
    assert!(app.view.has_selection());
    press_at(&mut app, &cx, l, 12, 1, false);
    assert!(!app.view.has_selection(), "a plain click clears the selection");
    // Ctrl+C with nothing selected and an empty box does what it did: nothing
    app.handle(&Event::Key(Key::Ctrl('c')), &cx);
    assert!(crate::ui::clipboard::latest().is_none());
    // but the input box still copies its own selection first
    app.input.set("typed text");
    app.input.select_all();
    press_at(&mut app, &cx, l, 2, 2, false);
    app.handle(&Event::Key(Key::Ctrl('c')), &cx);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("typed text"));
}

#[test]
fn ctrl_a_selects_the_whole_transcript_when_the_box_is_empty_and_the_box_otherwise() {
    crate::ui::clipboard::clear();
    let (mut app, cx) = transcript_chat("chatsel4");
    render_with(&mut app, &cx, 1100, 1100);
    app.handle(&Event::Key(Key::Ctrl('a')), &cx);
    let all = app.view.selected_text().unwrap();
    assert!(all.starts_with("Which of my projects") && all.contains("run_command") && all.ends_with("Want me to draft the plan?"), "{all:?}");
    app.view.clear_selection();
    app.input.set("abc");
    app.handle(&Event::Key(Key::Ctrl('a')), &cx);
    assert!(!app.view.has_selection(), "with text in the box Ctrl+A selects the box");
    assert!(app.input.is_all_selected());
}

#[test]
fn the_copy_button_of_a_hovered_message_or_code_block_copies_that_block() {
    crate::ui::clipboard::clear();
    let (mut app, cx) = transcript_chat("chatsel5");
    render_with(&mut app, &cx, 1100, 1100);
    // hover the code block: two buttons, the message's and the block's
    let code = line_of(&app, "cargo test -p iyakki-shell");
    let (hx, hy) = app.view.point_of(code, 3, true).unwrap();
    let mut g = Gfx::new(1100, 1100);
    g.clear(rgb(9, 11, 20));
    g.mouse = (hx as i32, hy as i32);
    app.render(&mut g, R::new(0.0, 46.0, 1100.0, 1054.0), 1.0, &cx, 0);
    let copies: Vec<Event> = g.hits.iter().filter(|h| h.label == "copy").map(|h| h.ev.clone()).collect();
    assert_eq!(copies.len(), 2, "the whole answer and its code block: {copies:?}");
    // the later one is the code block's
    app.handle(&copies[1], &cx);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("cargo test -p iyakki-shell --lib"));
    app.handle(&copies[0], &cx);
    let msg = crate::ui::clipboard::latest().unwrap();
    assert!(msg.starts_with("Two things stand out:") && msg.contains("```") && msg.ends_with("Want me to draft the plan?"), "{msg:?}");
    // the user's own box has one too
    let (ux, uy) = app.view.point_of(0, 3, true).unwrap();
    g.clear(rgb(9, 11, 20));
    g.clear_hits();
    g.mouse = (ux as i32, uy as i32);
    app.render(&mut g, R::new(0.0, 46.0, 1100.0, 1054.0), 1.0, &cx, 0);
    let ev = g.hits.iter().find(|h| h.label == "copy").expect("a button on the question").ev.clone();
    app.handle(&ev, &cx);
    assert!(crate::ui::clipboard::latest().unwrap().starts_with("Which of my projects need attention?"));
    // nothing hovered: no buttons
    g.clear_hits();
    g.mouse = (0, 0);
    app.render(&mut g, R::new(0.0, 46.0, 1100.0, 1054.0), 1.0, &cx, 0);
    assert!(!g.hits.iter().any(|h| h.label == "copy"));
}

#[test]
fn the_chat_adds_copy_message_and_copy_code_block_to_the_context_menu() {
    use crate::apps::App;
    crate::ui::clipboard::clear();
    let (mut app, cx) = transcript_chat("chatsel6");
    render_with(&mut app, &cx, 1100, 1100);
    let code = line_of(&app, "cargo test -p iyakki-shell");
    let (x, y) = app.view.point_of(code, 3, true).unwrap();
    let items = app.context_items(x as i32, y as i32);
    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, vec!["copy", "copymsg", "copycode", "selall"]);
    assert!(!items[0].enabled, "Copy needs a selection");
    assert!(matches!(app.context_pick("copycode", &cx), Flow::Stay));
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("cargo test -p iyakki-shell --lib"));
    // with a selection, Copy is usable
    press_at(&mut app, &cx, code, 0, 2, false);
    assert!(app.context_items(x as i32, y as i32)[0].enabled);
    // away from the transcript there is nothing of the chat's to offer
    assert!(app.context_items(5, 5).is_empty());
}

#[test]
fn dragging_past_the_top_or_bottom_edge_scrolls_while_the_button_is_held() {
    crate::ui::clipboard::clear();
    let (cx, _d) = cx_with_backend("chatsel7", iyakki_agent::chat::Unconfigured);
    let mut app = ChatApp::new();
    for i in 0..40 {
        app.msgs.push(Message::user(format!("question number {i}")));
        app.msgs.push(Message::assistant(format!("answer number {i} with some words in it")));
    }
    render_with(&mut app, &cx, 1100, 500);
    let last = app.view.line_texts().len() - 1;
    press_at(&mut app, &cx, last, 3, 1, false);
    // the pointer goes above the transcript's top edge
    crate::ui::set_dragging(true);
    app.handle(&Event::AreaAt(chat_view::AREA, 100.0, -30.0), &cx);
    crate::ui::set_dragging(false);
    let before = app.scroll_px;
    for _ in 0..5 {
        app.handle(&Event::Tick, &cx);
    }
    assert!(app.scroll_px > before, "the transcript scrolled up: {} -> {}", before, app.scroll_px);
    let t = app.view.selected_text().unwrap();
    assert!(t.lines().count() > 3, "the selection grew with it: {t:?}");
    // releasing the button stops it
    app.handle(&Event::MouseUp(0, 0), &cx);
    let stop = app.scroll_px;
    app.handle(&Event::Tick, &cx);
    assert_eq!(app.scroll_px, stop);
}

#[test]
fn a_real_mouse_drag_through_the_shell_selects_chat_text_and_ctrl_c_copies_it() {
    use crate::shell::tests::{login, shell_with_users};
    crate::ui::clipboard::clear();
    let (mut sh, _) = shell_with_users("chatmouse");
    login(&mut sh, "alice", "alicepass1");
    let (app, _cx) = transcript_chat("chatmouse2");
    sh.show_app_for_test(Box::new(app), iyakki_sec::policy::App::Chat);
    let mut g = crate::gfx::Gfx::new(1280, 900);
    sh.render(&mut g);
    let area = sh.hits().iter().find(|h| h.label == "text area").expect("the transcript area").rect;
    // press inside the first message, drag down into the answer, release
    let (x0, y0) = ((area.x + 70.0) as i32, (area.y + 30.0) as i32);
    sh.handle(&Event::MouseMove(x0, y0));
    sh.handle(&Event::MouseDown(x0, y0));
    for step in 1..=8 {
        let (x, y) = (x0 + step * 20, y0 + step * 25);
        sh.handle(&Event::MouseMove(x, y));
        sh.render(&mut g);
    }
    sh.handle(&Event::MouseUp(x0 + 160, y0 + 200));
    sh.handle(&Event::Key(Key::Ctrl('c')));
    let copied = crate::ui::clipboard::latest();
    assert!(copied.as_deref().is_some_and(|t| t.chars().count() > 20), "the dragged text was copied: {copied:?}");
}
