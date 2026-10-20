//! The message box of the chat: three lines tall when empty, growing to its most and then scrolling inside, a long message
//! (20,000 characters) kept in full with a notice with the counts when it does not fit, and a layout that holds at the four
//! window sizes in the full window and in the narrow AI IDE pane.

use super::*;
use crate::apps::tests::cx_with_backend;
use crate::apps::App;
use std::sync::{Arc, Mutex};

/// A model that records what it is asked and answers once.
struct Spy(Arc<Mutex<Vec<Vec<Message>>>>);
impl ChatBackend for Spy {
    fn complete(&self, h: &[Message]) -> Result<String, String> {
        self.0.lock().unwrap().push(h.to_vec());
        Ok("Read it.".into())
    }
}

fn rig(tag: &str, _replies: &[&str]) -> (AppCx, (), Arc<Mutex<Vec<Vec<Message>>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (cx, _d) = cx_with_backend(tag, Spy(seen.clone()));
    (cx, (), seen)
}


fn overlap(a: R, b: R) -> bool {
    a.x < b.x + b.w - 0.5 && b.x < a.x + a.w - 0.5 && a.y < b.y + b.h - 0.5 && b.y < a.y + a.h - 0.5
}

fn draw(app: &mut ChatApp, cx: &AppCx, w: u32, h: u32, ide: bool, mouse: Option<(i32, i32)>) -> (Gfx, R) {
    let s = crate::shell::scale_for(w, h);
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    if let Some(m) = mouse {
        g.mouse = m;
    }
    // the AI IDE's chat pane is a column on the right
    let area = if ide { R::new(w as f32 * 0.68, 46.0 * s, w as f32 * 0.32, h as f32 - 46.0 * s) } else { R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s) };
    app.render(&mut g, area, s, cx, 1);
    (g, area)
}

fn rect(g: &Gfx, label: &str) -> Option<R> {
    g.hits.iter().find(|h| h.label == label).map(|h| h.rect)
}

fn project_chat(root: &std::path::Path) -> ChatApp {
    std::fs::create_dir_all(root).unwrap();
    ChatApp::new().with_project(ProjectScope { name: "demo".into(), root: root.to_path_buf(), key: "ide:demo".into(), features: root.join("FEATURES.md"), cluster: None })
}

/// Empty: three lines (two at 800x480 at least); long text: more lines up to the most; the box, Send and the mic never overlap
/// each other or anything else, are inside the window, and the transcript keeps its room above.
fn geometry(ide: bool) {
    let (cx, _home, _) = rig(if ide { "boxgeo-ide" } else { "boxgeo" }, &[]);
    let root = std::env::temp_dir().join(format!("iyakki-boxgeo-{}-{ide}", std::process::id()));
    let mut app = if ide { project_chat(&root) } else { ChatApp::new() };
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let s = crate::shell::scale_for(w, h);
        let what = format!("{} at {w}x{h}", if ide { "IDE chat" } else { "chat" });
        app.input.set("");
        let (g, area) = draw(&mut app, &cx, w, h, ide, None);
        let empty = rect(&g, "message field").unwrap_or_else(|| panic!("{what}: no message box"));
        let rows = app.input_rows();
        assert!(rows == 3 || (rows == 2 && h <= 480), "{what}: {rows} lines when empty");
        let lh = g.line_height(17.0 * s, Face::Sans);
        assert!(empty.h >= rows as f32 * lh + 2.0 * 9.0 * s - 0.5, "{what}: the box is {} high for {rows} lines", empty.h);
        let send = rect(&g, "Send").unwrap();
        assert!(send.y >= empty.y && send.y + send.h <= empty.y + empty.h + 1.0 && send.x >= empty.x + empty.w, "{what}: Send is level with the box, beside it");
        assert!(send.x + send.w <= area.x + area.w + 0.5 && send.y + send.h <= h as f32, "{what}: Send is in view");
        // a long message: the box grows to its most (ten lines in the pane, twelve in the full window) and scrolls
        app.input.set(&"a line of the pasted log\n".repeat(80));
        let (g, area) = draw(&mut app, &cx, w, h, ide, None);
        let tall = rect(&g, "message field").unwrap();
        let most = if ide { 10 } else { 12 };
        assert!(tall.h > empty.h && app.input_rows() <= most && app.input.lay.len() > app.input_rows(), "{what}: {} lines of {}", app.input_rows(), app.input.lay.len());
        assert!(tall.y >= area.y + 0.3 * area.h, "{what}: the conversation keeps its room above ({} of {})", tall.y - area.y, area.h);
        assert!(tall.y + tall.h <= h as f32, "{what}: the box is inside the window");
        let send = rect(&g, "Send").unwrap();
        assert!(send.y >= tall.y && send.y + send.h <= tall.y + tall.h + 1.0 && send.y + send.h <= h as f32, "{what}: Send stays beside the tall box");
        for hit in &g.hits {
            if matches!(hit.label.as_str(), "message field" | "clipboard" | "backdrop" | "text area") || hit.rect.w > 0.9 * area.w || hit.rect.w <= 0.0 {
                continue;
            }
            assert!(!overlap(hit.rect, tall), "{what}: {:?} overlaps the message box {:?}", (&hit.label, hit.rect), tall);
        }
        // the transcript ends above the box
        for t in g.hits.iter().filter(|h| h.label == "text area") {
            assert!(t.rect.y + t.rect.h <= tall.y + 0.5, "{what}: the conversation reaches {} but the box starts at {}", t.rect.y + t.rect.h, tall.y);
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_message_box_fits_at_the_four_window_sizes_in_the_full_chat() {
    geometry(false);
}

#[test]
fn the_message_box_fits_at_the_four_window_sizes_in_the_ide_chat() {
    geometry(true);
}

#[test]
fn a_message_of_twenty_thousand_characters_is_kept_in_full_and_sent_whole_and_one_more_shows_the_notice() {
    let (cx, _home, spy) = rig("boxlong", &["Read it."]);
    let mut app = ChatApp::new();
    // the old limit was 512: a long error log pasted in is all there
    let log: String = (0..400).map(|i| format!("error[E{i:04}]: something failed in module {i}\n")).collect();
    assert!(log.chars().count() > 15_000 && log.chars().count() < 20_000);
    app.handle(&Event::Paste(log.clone()), &cx);
    assert_eq!(app.input_text(), log, "pasted whole, line breaks kept");
    assert!(app.input_notice().is_none(), "well under the limit: no notice");
    app.input.set(&"w".repeat(19_000));
    assert!(app.input_notice().is_some_and(|n| !n.1 && n.0 == "19,000 of 20,000 characters"), "near the limit: only the count shows");
    app.input.set(&log);
    // typing goes in too
    for c in "tail".chars() {
        app.handle(&Event::Key(Key::Char(c)), &cx);
    }
    assert!(app.input_text().ends_with("tail"));
    // a message at the limit is accepted exactly, one more is refused with the counts
    app.input.set(&"w".repeat(multiline::CHAT_MAX));
    assert_eq!(app.input_text().chars().count(), 20_000);
    app.handle(&Event::Key(Key::Char('x')), &cx);
    let (note, bad) = app.input_notice().expect("the notice");
    assert!(bad && note.contains("20,000 of 20,001"), "{note}");
    app.handle(&Event::Paste("y".repeat(1500)), &cx);
    assert!(app.input_notice().unwrap().0.contains("20,000 of 21,500"), "{:?}", app.input_notice());
    assert_eq!(app.input_text().chars().count(), 20_000, "nothing is dropped from what was there");
    // the whole message reaches the model, not a cut of it
    app.input.set(&log);
    app.handle(&Event::Key(Key::Enter), &cx);
    assert!(app.is_waiting());
    assert!(app.input_text().is_empty() && app.input_notice().is_none(), "sent: the box is empty and the notice gone");
    let t0 = std::time::Instant::now();
    while app.is_waiting() && t0.elapsed().as_secs() < 20 {
        app.handle(&Event::Tick, &cx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let seen = spy.lock().unwrap();
    let last = seen.last().expect("the model was asked");
    assert_eq!(last.last().unwrap().content, log.trim(), "the model got all {} characters", log.trim().chars().count());
    assert!(app.messages().iter().any(|m| m.role == Role::User && m.content == log.trim()), "and the chat keeps it whole");
}

#[test]
fn the_message_box_grows_from_three_lines_to_its_most_then_scrolls_with_the_caret_in_view() {
    let (cx, _home, _) = rig("boxgrow", &[]);
    let mut app = ChatApp::new();
    let (w, h) = (1280u32, 720u32);
    let rows_for = |app: &mut ChatApp, n: usize| {
        app.input.set(&"x\n".repeat(n.saturating_sub(1)));
        let _ = draw(app, &cx, w, h, false, None);
        app.input_rows()
    };
    assert_eq!((rows_for(&mut app, 1), rows_for(&mut app, 3), rows_for(&mut app, 6), rows_for(&mut app, 12), rows_for(&mut app, 50)), (3, 3, 6, 12, 12));
    let lines: String = (0..60).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    app.input.set(&lines);
    let _ = draw(&mut app, &cx, w, h, false, None);
    let visible = |a: &ChatApp| {
        let l = a.input.line_of(a.input.cur);
        l >= a.input.top && l < a.input.top + a.input_rows()
    };
    assert!(app.input.top > 0 && visible(&app), "the caret at the end is in view");
    for _ in 0..40 {
        app.handle(&Event::Key(Key::Up), &cx);
    }
    let _ = draw(&mut app, &cx, w, h, false, None);
    assert!(visible(&app) && app.input.top < 20, "moving up scrolls to the caret (top {})", app.input.top);
    app.handle(&Event::Key(Key::Char('Z')), &cx);
    assert!(app.input_text().contains('Z') && visible(&app));
    // Page keys on the box move the caret by a page; Home and End stay in the line
    app.handle(&Event::Key(Key::Nav { nav: crate::ui::Nav::PageDown, shift: false, ctrl: false }), &cx);
    let _ = draw(&mut app, &cx, w, h, false, None);
    assert!(visible(&app));
    // the wheel over the box scrolls its lines and not the conversation
    let first = draw(&mut app, &cx, w, h, false, None).0;
    let r = rect(&first, "message field").unwrap();
    let mouse = Some(((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32));
    let _ = draw(&mut app, &cx, w, h, false, mouse);
    let (top, scroll) = (app.input.top, app.scroll_px);
    app.handle(&Event::Scroll(1), &cx);
    assert!(app.input.top < top, "wheel up moved the lines ({top} to {})", app.input.top);
    assert_eq!(app.scroll_px, scroll, "the conversation did not move");
    // away from the box the wheel is the conversation's again
    let _ = draw(&mut app, &cx, w, h, false, Some((5, 5)));
    app.handle(&Event::Scroll(1), &cx);
    assert!(app.scroll_px > scroll);
    // a press puts the caret, Ctrl+M makes a line, Enter sends
    app.input.set("one");
    app.handle(&Event::Key(Key::Ctrl('m')), &cx);
    assert_eq!(app.input_text(), "one\n");
    app.handle(&Event::Key(Key::Ctrl('j')), &cx);
    assert_eq!(app.input_text(), "one\n\n", "Ctrl+J too in the full window");
}
