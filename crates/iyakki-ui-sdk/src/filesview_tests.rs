use crate::event::UiKey;
use crate::files::*;
use crate::filesview::*;
use crate::geom::Rect;
use crate::list::{DisplayList, ListCanvas, Op};
use crate::look::Look;
use crate::size::Metrics;

fn home() -> MemFiles {
    let mut f = MemFiles::new();
    f.put("readme.txt", "hello\nworld");
    f.put("Docs/plan.md", "# Plan\n- one");
    f.put("Docs/deep/x.txt", "x");
    f.put(".secret", "no");
    f.put("zeta.txt", "z");
    f
}

fn view() -> FilesView {
    let mut v = FilesView::new(Box::new(home()), 700);
    v.tick();
    v
}

fn draw(v: &mut FilesView, w: f32, h: f32, touch: bool, scale: f32) -> DisplayList {
    let th = Look::default().theme;
    let m = Metrics::new(w, h, 1.0, scale, touch);
    let mut c = ListCanvas::new(w, h);
    v.draw(&mut c, &th, &m, Rect::new(0.0, 0.0, w, h));
    c.finish()
}

fn action_named(l: &DisplayList, name: &str) -> u32 {
    l.ops.iter().find_map(|o| if let Op::Hit(_, h) = o { (h.name == name).then_some(h.action) } else { None }).unwrap_or_else(|| panic!("no control {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

fn texts(l: &DisplayList) -> Vec<String> {
    l.ops.iter().filter_map(|o| if let Op::Text { text, .. } = o { Some(text.clone()) } else { None }).collect()
}

#[test]
fn the_home_folder_opens_on_the_first_tick_folders_first_without_hidden_names() {
    let v = view();
    assert_eq!(v.names(), ["Docs/", "readme.txt", "zeta.txt"]);
    assert_eq!(v.dir, "");
    assert!(v.error().is_none());
    // a second tick does not open it again (the selection stays)
    let mut v = v;
    v.key(UiKey::Down);
    v.tick();
    assert_eq!(v.selected, 1);
}

#[test]
fn a_tap_on_a_folder_goes_in_up_comes_back_to_the_folder_it_left_and_a_file_shows_its_text() {
    let mut v = view();
    let l = draw(&mut v, 400.0, 480.0, false, 1.0);
    let docs = action_named(&l, "Docs/");
    assert!(v.owns(docs));
    assert!(v.action(docs));
    assert_eq!((v.dir.as_str(), v.names()), ("Docs", vec!["deep/".to_string(), "plan.md".to_string()]));
    let l = draw(&mut v, 400.0, 480.0, false, 1.0);
    assert!(texts(&l).iter().any(|t| t.contains("Home / Docs")), "{:?}", texts(&l));
    v.action(action_named(&l, "plan.md"));
    assert_eq!(v.preview().map(|p| p.0.as_str()), Some("Docs/plan.md"));
    assert_eq!(v.take_opened(), Some(("Docs/plan.md".into(), "# Plan\n- one".into())));
    assert_eq!(v.take_opened(), None, "once");
    // on a narrow pane the text replaces the list and has a Close button
    let l = draw(&mut v, 400.0, 480.0, false, 1.0);
    assert!(texts(&l).iter().any(|t| t == "# Plan") && !texts(&l).iter().any(|t| t == "deep/"), "{:?}", texts(&l));
    v.action(action_named(&l, "Close"));
    assert!(v.preview().is_none());
    // Up goes to the home and selects the folder it came from
    let l = draw(&mut v, 400.0, 480.0, false, 1.0);
    v.action(action_named(&l, "Up"));
    assert_eq!((v.dir.as_str(), v.selected_path().as_deref()), ("", Some("Docs")));
}

#[test]
fn on_a_wide_pane_the_list_and_the_text_are_side_by_side() {
    let mut v = view();
    v.key(UiKey::Down);
    v.key(UiKey::Enter);
    assert_eq!(v.preview().map(|p| p.1.as_str()), Some("hello\nworld"));
    let l = draw(&mut v, 900.0, 480.0, false, 1.0);
    let t = texts(&l);
    assert!(t.iter().any(|x| x == "readme.txt") && t.iter().any(|x| x == "hello") && t.iter().any(|x| x == "world"), "{t:?}");
}

#[test]
fn the_keys_move_open_go_up_and_close() {
    let mut v = view();
    assert!(v.key(UiKey::Down) && v.key(UiKey::Down));
    assert_eq!(v.selected_path().as_deref(), Some("zeta.txt"));
    v.key(UiKey::Down);
    assert_eq!(v.selected, 2, "stays on the last");
    v.key(UiKey::Up);
    v.key(UiKey::Up);
    v.key(UiKey::Right);
    assert_eq!(v.dir, "Docs");
    v.key(UiKey::Left);
    assert_eq!(v.dir, "");
    v.key(UiKey::Down);
    v.key(UiKey::Enter);
    assert!(v.preview().is_some());
    assert!(v.key(UiKey::Esc));
    assert!(v.preview().is_none());
    assert!(!v.key(UiKey::Esc), "nothing to close: the app may use Esc");
    assert!(!v.key(UiKey::Char('x')));
}

#[test]
fn a_long_folder_scrolls_with_the_wheel_and_the_keys_and_draws_only_the_rows_that_fit() {
    let mut f = MemFiles::new();
    for i in 0..120 {
        f.put(&format!("f{i:03}.txt"), "");
    }
    let mut v = FilesView::new(Box::new(f), 0);
    v.tick();
    let l = draw(&mut v, 400.0, 300.0, false, 1.0);
    let rows = l.ops.iter().filter(|o| matches!(o, Op::Hit(_, h) if h.name.ends_with(".txt"))).count();
    assert!((3..30).contains(&rows), "{rows}");
    v.wheel(-5);
    let l = draw(&mut v, 400.0, 300.0, false, 1.0);
    assert!(texts(&l).iter().all(|t| t != "f000.txt"), "scrolled off the top");
    for _ in 0..70 {
        v.key(UiKey::Down);
    }
    let l = draw(&mut v, 400.0, 300.0, false, 1.0);
    assert!(texts(&l).iter().any(|t| t == "f070.txt"), "the selection is on screen: {:?}", texts(&l));
    v.wheel(10_000);
    v.wheel(-10_000);
    let l = draw(&mut v, 400.0, 300.0, false, 1.0);
    assert!(texts(&l).iter().any(|t| t == "f119.txt"), "the end is reachable");
}

#[test]
fn a_refusal_is_said_and_a_missing_browser_is_said_once_with_nothing_drawn_outside() {
    let mut v = FilesView::new(Box::new(MemFiles::unavailable()), 0);
    v.tick();
    v.tick();
    assert_eq!(v.error(), Some("The file browser is not available here."));
    let l = draw(&mut v, 400.0, 300.0, false, 1.0);
    assert!(texts(&l).iter().any(|t| t.contains("not available")));
    // a file that is too big or not a file: the person is told, nothing is previewed
    let mut f = MemFiles::new();
    f.put("big.txt", &"x".repeat(MAX_PREVIEW + 1));
    let mut v = FilesView::new(Box::new(f), 0);
    v.tick();
    v.key(UiKey::Enter);
    assert!(v.preview().is_none() && v.error().is_some_and(|e| e.contains("over 256 KB")), "{:?}", v.error());
}

#[test]
fn every_control_has_a_name_stays_inside_the_area_and_is_big_enough_at_every_size() {
    for (w, h, touch, scale) in [(320.0f32, 480.0f32, true, 1.0f32), (400.0, 300.0, false, 1.0), (900.0, 600.0, false, 1.5), (1600.0, 900.0, false, 1.0), (320.0, 480.0, true, 2.0)] {
        let mut v = view();
        v.key(UiKey::Down);
        v.key(UiKey::Enter);
        let m = Metrics::new(w, h, 1.0, scale, touch);
        let l = draw(&mut v, w, h, touch, scale);
        let mut seen = 0;
        for o in &l.ops {
            if let Op::Hit(r, hit) = o {
                seen += 1;
                assert!(!hit.name.is_empty());
                assert!(r.x >= -0.5 && r.y >= -0.5 && r.right() <= w + 0.5 && r.bottom() <= h + 0.5, "{} outside at {w}x{h}: {r:?}", hit.name);
                assert!(r.h >= m.min_target() - 0.5, "{} too low at {w}x{h}: {}", hit.name, r.h);
                assert!(v.owns(hit.action), "{}", hit.name);
            }
        }
        assert!(seen >= 3);
    }
}

// ---- changing files (F-SDK-101): every change is a question the person answers

use std::cell::RefCell;
use std::rc::Rc;

struct Shared(Rc<RefCell<MemFiles>>);

impl FilesPort for Shared {
    fn list(&mut self, dir: &str) -> Result<DirListing, FilesError> {
        self.0.borrow_mut().list(dir)
    }
    fn read_text(&mut self, path: &str, max: usize) -> Result<String, FilesError> {
        self.0.borrow_mut().read_text(path, max)
    }
    fn change(&mut self, c: &Change) -> Result<Ask, FilesError> {
        self.0.borrow_mut().change(c)
    }
    fn decide(&mut self, call: &str, approve: bool) -> Result<String, FilesError> {
        self.0.borrow_mut().decide(call, approve)
    }
}

fn writable() -> (FilesView, Rc<RefCell<MemFiles>>) {
    let h = Rc::new(RefCell::new(home()));
    let mut v = FilesView::new(Box::new(Shared(h.clone())), 700);
    v.tick();
    (v, h)
}

fn press(v: &mut FilesView, name: &str) {
    let l = draw(v, 700.0, 520.0, false, 1.0);
    let a = action_named(&l, name);
    assert!(v.action(a), "{name}");
}

#[test]
fn a_new_folder_asks_for_a_name_then_the_core_asks_the_person_and_only_allow_makes_it() {
    let (mut v, h) = writable();
    press(&mut v, "New folder");
    assert!(v.wants_keyboard());
    assert!(v.text("Pictures 2026"));
    press(&mut v, "OK");
    let q = v.pending_question().expect("the core asks").clone();
    assert!(q.summary.contains("Make the folder `Pictures 2026`"), "{}", q.summary);
    assert!(h.borrow_mut().list("").unwrap().entries.iter().all(|e| e.name != "Pictures 2026"), "nothing is made before the answer");
    let l = draw(&mut v, 700.0, 520.0, false, 1.0);
    action_named(&l, "Deny");
    press(&mut v, "Allow");
    assert!(v.pending_question().is_none());
    assert_eq!(v.note(), Some("Made the folder `Pictures 2026`."));
    assert!(v.names().contains(&"Pictures 2026/".to_string()), "the folder is read again: {:?}", v.names());
}

#[test]
fn deny_and_the_keys_stop_a_change_and_one_question_waits_at_a_time() {
    let (mut v, h) = writable();
    v.key(UiKey::Down);
    press(&mut v, "Delete");
    assert!(v.pending_question().unwrap().summary.contains("Move `readme.txt` to the trash"));
    // the other buttons do nothing while the question waits, and an answer key is taken first
    press(&mut v, "New folder");
    assert!(!v.wants_keyboard());
    assert!(!v.save("x.txt", "t"), "a second question is refused");
    assert!(v.key(UiKey::Char('n')));
    assert_eq!(v.note(), Some("Not changed: you said no."));
    assert!(h.borrow().trashed.is_empty() && v.names().contains(&"readme.txt".to_string()));
    press(&mut v, "Delete");
    assert!(v.key(UiKey::Char('y')));
    assert_eq!(h.borrow().trashed, ["readme.txt"]);
    assert!(!v.names().contains(&"readme.txt".to_string()));
    press(&mut v, "Delete");
    assert!(v.key(UiKey::Esc), "Esc denies");
    assert_eq!(h.borrow().trashed.len(), 1);
}

#[test]
fn rename_starts_from_the_name_and_a_save_goes_through_the_same_question_and_keeps_the_old_text_on_a_no() {
    let (mut v, h) = writable();
    v.key(UiKey::Down);
    press(&mut v, "Rename");
    for _ in 0.."readme.txt".len() {
        v.key(UiKey::Backspace);
    }
    v.text("README.md");
    assert!(v.key(UiKey::Enter));
    assert!(v.pending_question().unwrap().summary.contains("Rename `readme.txt` to `README.md`"));
    v.key(UiKey::Char('y'));
    assert!(v.names().contains(&"README.md".to_string()) && !v.names().contains(&"readme.txt".to_string()));
    assert_eq!(h.borrow().text_of("README.md").as_deref(), Some("hello\nworld"), "the text came along");
    // the app's Save
    assert!(v.save("README.md", "new words"));
    assert!(v.pending_question().unwrap().summary.contains("Save `README.md` (9 bytes)"));
    assert_eq!(h.borrow().text_of("README.md").as_deref(), Some("hello\nworld"), "not saved before the answer");
    v.key(UiKey::Char('n'));
    assert_eq!(h.borrow().text_of("README.md").as_deref(), Some("hello\nworld"));
    assert!(v.save("README.md", "new words"));
    v.key(UiKey::Char('y'));
    assert_eq!(h.borrow().text_of("README.md").as_deref(), Some("new words"));
}

#[test]
fn a_refused_change_or_name_is_said_and_a_host_that_changes_nothing_says_so() {
    let (mut v, _h) = writable();
    press(&mut v, "New folder");
    v.text("a/b");
    press(&mut v, "OK");
    assert!(v.pending_question().is_none() && v.error().is_some_and(|e| e.contains("not allowed")), "{:?}", v.error());
    press(&mut v, "New folder");
    v.text(".hidden");
    v.key(UiKey::Enter);
    assert!(v.pending_question().is_none());
    // Esc cancels the name field and an empty name asks nothing
    press(&mut v, "New folder");
    assert!(v.key(UiKey::Esc) && !v.wants_keyboard());
    press(&mut v, "New folder");
    v.key(UiKey::Enter);
    assert!(v.pending_question().is_none());
    // the unavailable port
    let mut off = FilesView::new(Box::new(MemFiles::unavailable()), 0);
    off.tick();
    off.begin_new_folder();
    off.text("x");
    off.key(UiKey::Enter);
    assert!(off.pending_question().is_none());
    assert_eq!(off.error(), Some("The file browser is not available here."));
}

#[test]
fn the_change_controls_are_named_inside_the_area_and_big_enough_at_every_size_with_a_question_and_a_name_field_open() {
    for (w, h, touch, scale) in [(320.0f32, 480.0f32, true, 1.0f32), (400.0, 480.0, false, 1.0), (900.0, 600.0, false, 1.5), (1600.0, 900.0, false, 1.0)] {
        let (mut v, _) = writable();
        v.key(UiKey::Down);
        v.delete_selected();
        let m = Metrics::new(w, h, 1.0, scale, touch);
        let l = draw(&mut v, w, h, touch, scale);
        for name in ["New folder", "Rename", "Delete", "Allow", "Deny"] {
            action_named(&l, name);
        }
        for o in &l.ops {
            if let Op::Hit(r, hit) = o {
                assert!(r.x >= -0.5 && r.y >= -0.5 && r.right() <= w + 0.5 && r.bottom() <= h + 0.5, "{} outside at {w}x{h}: {r:?}", hit.name);
                if hit.role == crate::canvas::Role::Button {
                    assert!(r.h >= m.min_target() - 0.5, "{} too low at {w}x{h}", hit.name);
                }
            }
        }
        v.decide(false);
        v.begin_new_folder();
        let l = draw(&mut v, w, h, touch, scale);
        for name in ["OK", "Cancel", "Name of the new folder"] {
            action_named(&l, name);
        }
    }
}
