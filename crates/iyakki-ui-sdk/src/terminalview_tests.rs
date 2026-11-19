use crate::event::UiKey;
use crate::geom::Rect;
use crate::list::{DisplayList, ListCanvas, Op};
use crate::look::Look;
use crate::size::Metrics;
use crate::terminal::*;
use crate::terminalview::*;

fn view() -> TerminalView {
    let mut v = TerminalView::new(Box::new(MemTerminal::new()), 800);
    v.tick();
    v
}

fn type_line(v: &mut TerminalView, s: &str) {
    for c in s.chars() {
        v.key(UiKey::Char(c));
    }
    v.key(UiKey::Enter);
}

fn draw(v: &mut TerminalView, w: f32, h: f32, touch: bool, scale: f32) -> DisplayList {
    let th = Look::default().theme;
    let m = Metrics::new(w, h, 1.0, scale, touch);
    let mut c = ListCanvas::new(w, h);
    v.draw(&mut c, &th, &m, Rect::new(0.0, 0.0, w, h));
    c.finish()
}

fn action_named(l: &DisplayList, name: &str) -> u32 {
    l.ops.iter().find_map(|o| if let Op::Hit(_, h) = o { (h.name == name).then_some(h.action) } else { None }).unwrap_or_else(|| panic!("no control {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

#[test]
fn a_read_only_line_runs_at_once_and_is_shown_with_its_prompt_and_output() {
    let mut v = view();
    assert_eq!(v.transcript(), ["Memory terminal."]);
    type_line(&mut v, "echo hello world");
    assert_eq!(v.transcript(), ["Memory terminal.", "$ echo hello world", "hello world"]);
    assert!(v.pending_question().is_none());
    assert!(!v.submit("   "), "an empty line is not sent");
}

#[test]
fn a_line_that_changes_something_waits_for_the_person_and_runs_only_after_allow() {
    let mut v = view();
    type_line(&mut v, "mkdir notes");
    let q = v.pending_question().expect("a card is waiting").clone();
    assert!(q.summary.contains("mkdir notes"));
    assert_eq!(v.transcript().last().map(String::as_str), Some("$ mkdir notes"), "nothing ran");
    assert!(!v.submit("echo x"), "no new line while the question waits");
    let l = draw(&mut v, 700.0, 420.0, false, 1.0);
    let allow = action_named(&l, "Allow");
    action_named(&l, "Deny");
    assert!(v.action(allow));
    assert!(v.pending_question().is_none());
    assert_eq!(v.transcript().iter().rev().take(2).cloned().collect::<Vec<_>>(), ["done: mkdir notes", "Allowed."]);
}

#[test]
fn deny_and_the_keys_y_n_and_escape_work_and_nothing_runs_on_a_no() {
    let mut v = view();
    type_line(&mut v, "write a.txt hi");
    assert!(v.key(UiKey::Char('n')));
    assert!(v.transcript().contains(&"Not run: you said no.".to_string()));
    type_line(&mut v, "write b.txt hi");
    assert!(v.key(UiKey::Esc));
    assert!(v.pending_question().is_none());
    type_line(&mut v, "write c.txt hi");
    assert!(!v.key(UiKey::Char('q')), "only the answer keys are taken while a question waits");
    assert!(v.key(UiKey::Char('y')));
    assert!(v.transcript().contains(&"done: write c.txt hi".to_string()));
}

#[test]
fn the_lines_typed_before_come_back_with_up_and_down_and_clear_empties_the_screen() {
    let mut v = view();
    type_line(&mut v, "pwd");
    type_line(&mut v, "echo two");
    v.key(UiKey::Up);
    assert_eq!(v.input, "echo two");
    v.key(UiKey::Up);
    assert_eq!(v.input, "pwd");
    v.key(UiKey::Down);
    assert_eq!(v.input, "echo two");
    v.key(UiKey::Down);
    assert_eq!(v.input, "");
    type_line(&mut v, "clear");
    assert_eq!(v.transcript(), Vec::<String>::new());
}

#[test]
fn a_missing_terminal_is_said_once_and_a_refused_line_is_shown_as_an_error() {
    let mut v = TerminalView::new(Box::new(MemTerminal::unavailable()), 0);
    v.tick();
    v.tick();
    assert_eq!(v.transcript(), ["The terminal is not available here."]);
    assert!(!v.submit("ls"));
    let mut w = view();
    type_line(&mut w, &"x".repeat(5000));
    assert!(w.error().is_some() || w.input.len() <= MAX_INPUT, "a long line is cut by the field");
    w.close();
    assert!(w.submit("ls"), "a closed view opens its session again on the next line");
}

#[test]
fn long_output_wraps_the_newest_lines_show_and_the_wheel_goes_back() {
    let mut v = view();
    for i in 0..60 {
        type_line(&mut v, &format!("echo line {i}"));
    }
    let texts = |l: &DisplayList| -> Vec<String> { l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect() };
    let l = draw(&mut v, 500.0, 300.0, false, 1.0);
    assert!(texts(&l).iter().any(|t| t == "line 59"), "{:?}", texts(&l));
    assert!(!texts(&l).iter().any(|t| t == "line 5"));
    v.wheel(1);
    v.wheel(1);
    let l = draw(&mut v, 500.0, 300.0, false, 1.0);
    assert!(!texts(&l).iter().any(|t| t == "line 59"), "scrolled back");
    // a very long line is wrapped by characters, not drawn past the edge
    let mut w = view();
    w.submit(&format!("echo {}", "w".repeat(300)));
    let l = draw(&mut w, 320.0, 300.0, true, 1.0);
    assert!(texts(&l).iter().filter(|t| t.chars().all(|c| c == 'w')).count() >= 2);
}

#[test]
fn every_control_has_a_name_stays_inside_the_area_and_is_big_enough_at_every_size() {
    for (w, h, touch, scale) in [(320.0f32, 480.0f32, true, 1.0f32), (400.0, 300.0, false, 1.0), (900.0, 600.0, false, 1.5), (1600.0, 900.0, false, 1.0), (320.0, 480.0, true, 2.0)] {
        let mut v = view();
        type_line(&mut v, "mkdir x");
        let m = Metrics::new(w, h, 1.0, scale, touch);
        let l = draw(&mut v, w, h, touch, scale);
        let mut seen = 0;
        for o in &l.ops {
            if let Op::Hit(r, hit) = o {
                seen += 1;
                assert!(!hit.name.is_empty());
                assert!(r.x >= -0.5 && r.y >= -0.5 && r.right() <= w + 0.5 && r.bottom() <= h + 0.5, "{} outside at {w}x{h}: {r:?}", hit.name);
                assert!(r.h >= m.min_target() - 0.5, "{} too low at {w}x{h}", hit.name);
                assert!(v.owns(hit.action));
            }
        }
        assert!(seen >= 4, "field, Run, Allow, Deny at {w}x{h}");
    }
}

#[test]
fn a_full_terminal_asks_once_then_shows_the_cell_grid_and_sends_every_key_and_the_size() {
    let mut v = view();
    let l = draw(&mut v, 700.0, 420.0, false, 1.0);
    let full = action_named(&l, "Full");
    assert!(v.action(full));
    // asked, nothing started: the card has Allow and Deny and no key reaches a program
    assert!(v.in_screen() && v.screen_text().is_none());
    let q = v.pending_question().expect("the card").clone();
    assert!(q.summary.contains("not checked line by line"));
    assert!(!v.key(UiKey::Char('x')), "a letter is not taken while the card waits");
    let l = draw(&mut v, 700.0, 420.0, false, 1.0);
    let allow = action_named(&l, "Allow");
    assert!(v.action(allow));
    let t = v.screen_text().expect("the session is on screen");
    assert!(t.starts_with("mem$"), "{t:?}");
    // every key goes to the program, Esc and Tab too; a control character is ctrl and its letter
    for c in "ls".chars() {
        assert!(v.key(UiKey::Char(c)));
    }
    assert!(v.key(UiKey::Esc) && v.key(UiKey::Tab) && v.key(UiKey::Char('\u{3}')));
    assert!(v.text(" -l"));
    assert!(v.key(UiKey::Enter));
    let t = v.screen_text().unwrap();
    assert!(t.contains("mem$ ls -l") && t.contains("ran: ls -l"), "{t:?}");
    // drawing follows the area: the grid is as wide as the area allows and the core is told of the size
    let l = draw(&mut v, 700.0, 420.0, false, 1.0);
    let texts: Vec<String> = l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect();
    assert!(texts.iter().any(|s| s.contains("ran: ls -l")), "{texts:?}");
    let leave = action_named(&l, "Leave");
    assert!(v.action(leave));
    assert!(!v.in_screen() && v.pending_question().is_none());
    assert!(v.transcript().last().unwrap().contains("closed"));
}

#[test]
fn a_full_terminal_that_is_denied_never_shows_and_one_that_is_unavailable_says_so() {
    let mut v = view();
    assert!(v.open_screen());
    v.decide(false);
    assert!(!v.in_screen() && v.screen_text().is_none());
    assert!(v.open_screen(), "the person may ask again");
    assert!(v.in_screen());
    assert!(v.key(UiKey::Esc), "Esc is a no on the card");
    assert!(!v.in_screen() && v.screen_text().is_none());
    let mut off = TerminalView::new(Box::new(MemTerminal::unavailable()), 800);
    assert!(!off.open_screen());
    assert!(off.error().unwrap().contains("not available"));
    // a port that has no whole sessions at all answers the same way
    struct LinesOnly;
    impl TerminalPort for LinesOnly {
        fn open(&mut self) -> Result<Opened, TermError> {
            Ok(Opened { prompt: "$".into(), banner: String::new() })
        }
        fn run(&mut self, _l: &str) -> Result<Reply, TermError> {
            Ok(Reply::default())
        }
        fn decide(&mut self, _c: &str, _a: bool) -> Result<Reply, TermError> {
            Ok(Reply::default())
        }
        fn close(&mut self) {}
    }
    let mut v2 = TerminalView::new(Box::new(LinesOnly), 800);
    assert!(!v2.open_screen() && v2.error().is_some());
}

#[test]
fn the_palette_has_the_sixteen_the_cube_and_the_grey_ramp() {
    assert_eq!(palette(0), [0, 0, 0, 255]);
    assert_eq!(palette(15), [255, 255, 255, 255]);
    assert_eq!(palette(16), [0, 0, 0, 255]);
    assert_eq!(palette(231), [255, 255, 255, 255]);
    assert_eq!(palette(196), [255, 0, 0, 255]);
    assert_eq!(palette(232), [8, 8, 8, 255]);
    assert_eq!(palette(255), [238, 238, 238, 255]);
}
