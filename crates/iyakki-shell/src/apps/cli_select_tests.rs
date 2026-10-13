//! Selecting and copying the output of a scrolling log (`ScrollLog`, used by the CLI terminal), and the context menu's entries for it.

use super::*;
use crate::apps::tests::cx_with_backend;
use crate::apps::LOG_AREA;

const AREA: R = R { x: 10.0, y: 10.0, w: 380.0, h: 200.0 };

fn log_with(lines: &[&str]) -> (ScrollLog, Gfx) {
    let mut log = ScrollLog::default();
    for l in lines {
        log.push(*l, [0, 0, 0, 0]);
    }
    let mut g = Gfx::new(400, 220);
    g.clear(rgb(0, 0, 0));
    log.render(&mut g, AREA, 14.0, Face::Mono);
    (log, g)
}

fn press(log: &mut ScrollLog, x: f32, y: f32, clicks: u8) {
    crate::ui::set_click_count(clicks);
    log.handle(&Event::AreaAt(LOG_AREA, x, y));
    crate::ui::set_click_count(1);
}

fn drag(log: &mut ScrollLog, x: f32, y: f32) {
    crate::ui::set_dragging(true);
    log.handle(&Event::AreaAt(LOG_AREA, x, y));
    crate::ui::set_dragging(false);
}

#[test]
fn press_and_drag_select_text_across_lines_and_ctrl_c_copies_it() {
    crate::ui::clipboard::clear();
    let (mut log, _g) = log_with(&["alpha beta", "gamma delta", "epsilon"]);
    let g = Gfx::new(400, 220);
    let lh = g.line_height(14.0, Face::Mono);
    press(&mut log, 0.0, lh * 0.5, 1);
    assert!(!log.pane.has_selection());
    drag(&mut log, 1000.0, lh * 1.5);
    assert_eq!(log.pane.selected_text().as_deref(), Some("alpha beta\ngamma delta"));
    assert!(log.handle(&Event::Key(Key::Ctrl('c'))));
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some("alpha beta\ngamma delta"));
    // painted over the text
    let mut a = Gfx::new(400, 220);
    a.clear(rgb(0, 0, 0));
    log.render(&mut a, AREA, 14.0, Face::Mono);
    log.pane.clear();
    let mut b = Gfx::new(400, 220);
    b.clear(rgb(0, 0, 0));
    log.render(&mut b, AREA, 14.0, Face::Mono);
    assert_ne!(a.rgba(), b.rgba());
    // with nothing selected Ctrl+C is not used by the log
    assert!(!log.handle(&Event::Key(Key::Ctrl('c'))));
}

#[test]
fn double_click_selects_a_word_and_triple_click_a_whole_entry_even_when_it_wraps() {
    crate::ui::clipboard::clear();
    let mut g = Gfx::new(400, 220);
    let adv = g.text_width(14.0, "0", Face::Mono);
    let lh = g.line_height(14.0, Face::Mono);
    // the second entry is long enough to wrap over several lines in a 380 px wide area
    let long = "word ".repeat(30);
    let (mut log, _) = log_with(&["first line", long.trim_end(), "last"]);
    press(&mut log, adv * 7.5, lh * 0.5, 2);
    assert_eq!(log.pane.selected_text().as_deref(), Some("line"));
    press(&mut log, 5.0, lh * 1.5, 3);
    let t = log.pane.selected_text().unwrap();
    assert!(t.lines().count() >= 2 && t.replace('\n', " ").trim() == long.trim_end(), "the whole wrapped entry, and only it: {t:?}");
}

#[test]
fn a_selection_survives_scrolling_and_a_drag_past_the_edge_scrolls_the_log() {
    crate::ui::clipboard::clear();
    let lines: Vec<String> = (0..60).map(|i| format!("line {i}")).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let (mut log, _) = log_with(&refs);
    let mut g = Gfx::new(400, 220);
    let lh = g.line_height(14.0, Face::Mono);
    // the newest lines are shown; select the last visible one
    press(&mut log, 0.0, AREA.h - lh * 0.5, 1);
    drag(&mut log, 1000.0, AREA.h - lh * 0.5);
    let sel = log.pane.selected_text().unwrap();
    assert!(sel.starts_with("line 5") && sel.len() < 10, "{sel:?}");
    log.scroll_by(5);
    g.clear(rgb(0, 0, 0));
    log.render(&mut g, AREA, 14.0, Face::Mono);
    assert_eq!(log.pane.selected_text().as_deref(), Some(sel.as_str()), "the same text stays selected after scrolling");
    // dragging above the top edge: the log scrolls on each heartbeat while the button is held
    let before = log.scroll;
    drag(&mut log, 100.0, -40.0);
    log.on_tick();
    log.on_tick();
    assert!(log.scroll > before);
    log.handle(&Event::MouseUp(0, 0));
    let stop = log.scroll;
    log.on_tick();
    assert_eq!(log.scroll, stop, "releasing stops it");
}

#[test]
fn the_terminal_selects_all_with_an_empty_input_and_offers_copy_and_select_all_in_the_menu() {
    use crate::apps::App;
    crate::ui::clipboard::clear();
    let (cx, _d) = cx_with_backend("clisel", iyakki_agent::chat::Unconfigured);
    let mut app = CliApp::new(&cx);
    for c in "whoami".chars() {
        app.handle(&Event::Key(Key::Char(c)), &cx);
    }
    app.handle(&Event::Key(Key::Enter), &cx);
    let mut g = Gfx::new(900, 600);
    g.clear(rgb(0, 0, 0));
    app.render(&mut g, R::new(0.0, 80.0, 900.0, 520.0), 1.0, &cx, 0);
    let area = g.hits.iter().find(|h| h.label == "text area").expect("the output takes presses").rect;
    // the menu over the output
    let items = app.context_items(area.x as i32 + 20, area.y as i32 + 10);
    assert_eq!(items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec!["copy", "selall"]);
    assert!(!items[0].enabled && items[1].enabled);
    // Ctrl+A with an empty input selects the output; Ctrl+C copies it
    app.handle(&Event::Key(Key::Ctrl('a')), &cx);
    app.handle(&Event::Key(Key::Ctrl('c')), &cx);
    let copied = crate::ui::clipboard::latest().unwrap();
    assert!(copied.contains("whoami") && copied.contains("tester"), "{copied:?}");
    // with text in the input Ctrl+A is the input's
    app.log.pane.clear();
    app.handle(&Event::Key(Key::Char('x')), &cx);
    app.handle(&Event::Key(Key::Ctrl('a')), &cx);
    assert!(!app.log.pane.has_selection() && app.input.is_all_selected());
}
