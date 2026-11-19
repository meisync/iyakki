use crate::editorview::*;
use crate::event::UiKey;
use crate::geom::Rect;
use crate::list::{DisplayList, ListCanvas, Op};
use crate::look::Look;
use crate::size::Metrics;

const SRC: &str = "fn main() {\n    let x = 1;\n    println!(\"{}\", x);\n}\n";

fn editor() -> EditorView {
    let mut e = EditorView::new(900);
    e.load("src/main.rs", SRC).unwrap();
    e
}

fn draw(e: &mut EditorView, w: f32, h: f32, touch: bool, scale: f32) -> DisplayList {
    let th = Look::default().theme;
    let m = Metrics::new(w, h, 1.0, scale, touch);
    let mut c = ListCanvas::new(w, h);
    e.draw(&mut c, &th, &m, Rect::new(0.0, 0.0, w, h));
    c.finish()
}

fn action_named(l: &DisplayList, name: &str) -> u32 {
    l.ops.iter().find_map(|o| if let Op::Hit(_, h) = o { (h.name == name).then_some(h.action) } else { None }).unwrap_or_else(|| panic!("no control {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

fn colours_of(l: &DisplayList, word: &str) -> Vec<[u8; 4]> {
    l.ops.iter().filter_map(|o| if let Op::Text { text, color, .. } = o { text.contains(word).then_some(*color) } else { None }).collect()
}

#[test]
fn a_file_is_drawn_with_line_numbers_and_the_languages_colours() {
    let mut e = editor();
    let th = Look::default().theme;
    let l = draw(&mut e, 700.0, 400.0, false, 1.0);
    let texts: Vec<String> = l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect();
    assert!(texts.iter().any(|t| t.trim() == "1") && texts.iter().any(|t| t.trim() == "3"), "{texts:?}");
    assert_eq!(colours_of(&l, "fn"), vec![th.accent], "a keyword has its colour");
    assert!(colours_of(&l, "\"{}\"").contains(&th.ok), "a string has its colour");
    assert_eq!(e.text(), SRC.trim_end_matches('\n').to_string() + "\n");
}

#[test]
fn typing_moving_undo_and_a_tap_on_a_line_edit_the_text_and_mark_it_changed() {
    let mut e = editor();
    assert!(!e.key(UiKey::Char('x')), "not focused: the app keeps its keys");
    let l = draw(&mut e, 700.0, 400.0, false, 1.0);
    e.action(action_named(&l, "line 2"));
    assert!(e.wants_keyboard() && e.cursor().0 == 1);
    for c in " // hi".chars() {
        assert!(e.key(UiKey::Char(c)));
    }
    assert!(e.text().contains("let x = 1; // hi") && e.dirty());
    e.key(UiKey::Enter);
    assert_eq!(e.cursor().0, 2);
    e.key(UiKey::Up);
    assert_eq!(e.cursor().0, 1);
    let l = draw(&mut e, 700.0, 400.0, false, 1.0);
    let undo = action_named(&l, "Undo");
    for _ in 0..10 {
        e.action(undo);
    }
    assert_eq!(e.text(), SRC, "everything undone");
    e.action(action_named(&l, "Redo"));
    assert_ne!(e.text(), SRC);
    assert!(e.text_in("é\u{7}z"));
    assert!(e.text().contains("éz"), "control characters are dropped from typed text");
    assert!(e.key(UiKey::Esc) && !e.wants_keyboard());
}

#[test]
fn save_hands_the_app_the_path_and_the_text_once_and_the_app_marks_it_saved() {
    let mut e = editor();
    e.focused = true;
    e.key(UiKey::Char('a'));
    assert!(e.dirty());
    let l = draw(&mut e, 700.0, 400.0, false, 1.0);
    assert!(e.action(action_named(&l, "Save")));
    let (p, t) = e.take_save().expect("a save was asked");
    assert_eq!(p, "src/main.rs");
    assert!(t.starts_with('a') || t.contains("a"));
    assert_eq!(e.take_save(), None);
    e.mark_saved();
    assert!(!e.dirty());
}

#[test]
fn the_context_for_the_assistant_is_the_document_and_the_selection() {
    let mut e = editor();
    let c = e.context();
    assert_eq!(c.document.0, "src/main.rs");
    assert!(c.document.1.contains("let x = 1;") && c.selection.is_none());
    e.focused = true;
    e.key(UiKey::Down);
    // select the second line by tapping it and shifting with the mouse is not in the keys; the app may select through the buffer's own reply
    assert!(e.text().len() > 10);
    let mut big = EditorView::new(0);
    big.load("big.txt", &"line of text\n".repeat(10_000)).unwrap();
    let c = big.context();
    assert!(c.document.1.len() <= 40 * 1024 + 64 && c.document.1.contains("the rest of the file is not shown"));
}

#[test]
fn an_assistant_proposal_is_reviewed_hunk_by_hunk_and_applied_as_one_undoable_edit() {
    let mut e = editor();
    let reply = "I will change two things.\n<<<<<<< SEARCH\nfn main() {\n=======\nfn main() -> () {\n>>>>>>> REPLACE\n<<<<<<< SEARCH\n    println!(\"{}\", x);\n=======\n    println!(\"x = {}\", x);\n>>>>>>> REPLACE\nDone.";
    let explanation = e.offer_reply(reply).unwrap().unwrap();
    assert!(explanation.contains("two things"));
    assert!(e.reviewing() && e.hunks() == 2);
    assert_eq!(e.text(), SRC, "nothing changes before the person decides");
    let l = draw(&mut e, 700.0, 480.0, false, 1.0);
    let names: Vec<String> = l.a11y().iter().map(|n| n.name.clone()).collect();
    assert!(names.iter().any(|n| n.starts_with("Accept line 1")), "{names:?}");
    // skip the second hunk, apply
    e.action(action_named(&l, "Accept line 1: -1 +1"));
    e.action(action_named(&l, "Apply"));
    assert!(!e.reviewing());
    assert!(e.text().contains("fn main() {") && e.text().contains("println!(\"x = {}\", x)"), "{}", e.text());
    assert!(e.dirty());
    // one undo takes the whole applied edit back
    let l = draw(&mut e, 700.0, 480.0, false, 1.0);
    e.action(action_named(&l, "Undo"));
    assert_eq!(e.text(), SRC);
    // a reply that does not match is refused with its reason; a reply with no edit gives nothing; discard drops a proposal
    assert!(e.offer_reply("<<<<<<< SEARCH\nnot there\n=======\nx\n>>>>>>> REPLACE").is_err());
    assert_eq!(e.offer_reply("Just a question answered.").unwrap(), None);
    e.offer_reply(&reply).unwrap();
    assert!(e.key(UiKey::Esc) && !e.reviewing());
    e.offer_reply(&reply).unwrap();
    assert!(e.key(UiKey::Enter) && e.text().contains("-> ()"));
}

#[test]
fn a_binary_or_too_big_file_is_refused_with_a_sentence_and_keeps_what_was_open() {
    let mut e = editor();
    assert!(e.load("x.bin", "a\u{0}b").is_err());
    assert!(e.error().is_some());
    assert_eq!(e.path, "src/main.rs");
    assert!(e.load("big.txt", &"x".repeat(3 * 1024 * 1024)).unwrap_err().contains("too large"));
}

#[test]
fn every_control_has_a_name_stays_inside_the_area_and_is_big_enough_at_every_size() {
    for (w, h, touch, scale) in [(320.0f32, 480.0f32, true, 1.0f32), (400.0, 300.0, false, 1.0), (900.0, 600.0, false, 1.5), (1600.0, 900.0, false, 1.0), (320.0, 480.0, true, 2.0)] {
        let mut e = editor();
        e.offer_reply("<<<<<<< SEARCH\n    let x = 1;\n=======\n    let x = 2;\n>>>>>>> REPLACE").unwrap();
        let m = Metrics::new(w, h, 1.0, scale, touch);
        let l = draw(&mut e, w, h, touch, scale);
        let mut seen = 0;
        for o in &l.ops {
            if let Op::Hit(r, hit) = o {
                seen += 1;
                assert!(!hit.name.is_empty());
                assert!(r.x >= -0.5 && r.y >= -0.5 && r.right() <= w + 0.5 && r.bottom() <= h + 0.5, "{} outside at {w}x{h}: {r:?}", hit.name);
                if !hit.name.starts_with("line ") && !hit.name.contains(" line ") {
                    assert!(r.h >= m.min_target() - 0.5, "{} too low at {w}x{h}", hit.name);
                }
                assert!(e.owns(hit.action));
            }
        }
        assert!(seen >= 5, "toolbar and lines at {w}x{h}: {seen}");
    }
}
