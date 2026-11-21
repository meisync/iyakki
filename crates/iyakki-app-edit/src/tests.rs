use super::*;
use iyakki_ui_sdk::assistant::{SharedAssistant, Turn};
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Gesture, Phase};
use iyakki_ui_sdk::files::{Ask, Change, DirListing, FilesError, MemFiles};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;
use iyakki_ui_sdk::ports::{MemStorage, PortError};
use std::cell::RefCell;
use std::rc::Rc;

/// The in-memory home, shared with the test so it can see what was written.
#[derive(Clone)]
struct Home(Rc<RefCell<MemFiles>>);

impl FilesPort for Home {
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

#[derive(Clone, Default)]
struct Kept(Rc<RefCell<MemStorage>>);

impl StoragePort for Kept {
    fn get(&mut self, key: &str) -> Option<String> {
        self.0.borrow_mut().get(key)
    }
    fn set(&mut self, key: &str, value: &str) -> Result<(), PortError> {
        self.0.borrow_mut().set(key, value)
    }
    fn remove(&mut self, key: &str) -> Result<(), PortError> {
        self.0.borrow_mut().remove(key)
    }
    fn keys(&mut self) -> Vec<String> {
        self.0.borrow_mut().keys()
    }
}

fn home() -> Home {
    let mut f = MemFiles::new();
    f.put("notes/todo.md", "# Todo\nbuy milk\n");
    f.put("readme.txt", "hello\n");
    Home(Rc::new(RefCell::new(f)))
}

fn host_of(app: EditApp, w: u32, h: u32) -> Host<EditApp> {
    Host::new(app, Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}

fn host(w: u32, h: u32) -> (Host<EditApp>, Home) {
    let hm = home();
    (host_of(EditApp::new(Box::new(hm.clone())), w, h), hm)
}

fn center_of(l: &DisplayList, name: &str) -> (f32, f32) {
    l.ops.iter().find_map(|o| if let Op::Hit(r, hit) = o { (hit.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

fn tap(h: &mut Host<EditApp>, name: &str) {
    let l = h.record();
    let (x, y) = center_of(&l, name);
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}

fn pinch(h: &mut Host<EditApp>, s: f32) {
    h.event(&UiEvent::Gesture(Gesture::Pinch { scale: s, x: 50.0, y: 50.0 }));
}

fn open_todo(h: &mut Host<EditApp>) {
    h.event(&UiEvent::Tick);
    tap(h, "notes/");
    tap(h, "todo.md");
    assert_eq!(h.app.pane(), Pane::Editor, "choosing a file opens the editor");
}

#[test]
fn choosing_a_file_opens_it_in_the_editor_and_esc_goes_back_to_the_list_and_out() {
    let (mut h, _) = host(800, 600);
    assert_eq!(h.app.pane(), Pane::Files);
    open_todo(&mut h);
    assert_eq!(h.app.editor().text(), "# Todo\nbuy milk\n");
    assert_eq!(h.app.editor().path, "notes/todo.md");
    assert!(!h.app.editor().dirty());
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Stay);
    assert_eq!(h.app.pane(), Pane::Files, "Esc in the editor goes to the list");
    // with a file open Esc in the list returns to it
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Stay);
    assert_eq!(h.app.pane(), Pane::Editor);
    // a fresh app with no file open leaves on Esc
    let (mut fresh, _) = host(800, 600);
    assert_eq!(fresh.event(&UiEvent::Key(UiKey::Esc)), Flow::Back);
}

#[test]
fn typing_marks_the_file_changed_and_save_asks_first_and_writes_only_after_a_yes() {
    let (mut h, hm) = host(800, 600);
    open_todo(&mut h);
    tap(&mut h, "line 2");
    assert!(h.app.wants_keyboard(), "a field of the editor has the focus: the host shows the keyboard");
    h.event(&UiEvent::Text(" now".into()));
    assert!(h.app.editor().dirty());
    assert_eq!(h.app.dock_status().unwrap().badge, Some(1));
    tap(&mut h, "Save");
    assert_eq!(h.app.pane(), Pane::Files, "the question is on screen in the list");
    assert!(h.app.files().pending_question().is_some());
    assert_eq!(hm.0.borrow().text_of("notes/todo.md").as_deref(), Some("# Todo\nbuy milk\n"), "nothing is written before the yes");
    // a no keeps the text unsaved and returns to the editor
    h.event(&UiEvent::Key(UiKey::Char('n')));
    assert_eq!(h.app.pane(), Pane::Editor);
    assert!(h.app.editor().dirty());
    assert_eq!(hm.0.borrow().text_of("notes/todo.md").as_deref(), Some("# Todo\nbuy milk\n"));
    // saving again and allowing writes the file and clears the mark
    tap(&mut h, "Save");
    h.event(&UiEvent::Key(UiKey::Char('y')));
    assert_eq!(h.app.pane(), Pane::Editor);
    assert!(!h.app.editor().dirty());
    assert_eq!(hm.0.borrow().text_of("notes/todo.md").unwrap(), h.app.editor().text());
    assert!(h.app.editor().text().contains(" now"));
    assert_eq!(h.app.dock_status().unwrap().badge, None);
}

#[test]
fn a_port_with_no_files_is_said_in_words_and_a_text_the_editor_refuses_keeps_the_list() {
    let hm = Home(Rc::new(RefCell::new(MemFiles::unavailable())));
    let mut h = host_of(EditApp::new(Box::new(hm)), 800, 600);
    h.event(&UiEvent::Tick);
    let l = h.record();
    assert!(l.texts().iter().any(|t| t.contains("not available")), "{:?}", l.texts());
    assert_eq!(h.app.pane(), Pane::Files);
    // a text the editor refuses (binary) is said and the pane stays
    h.app.open("x.bin", "a\0b");
    assert_eq!(h.app.pane(), Pane::Files);
    assert!(!h.app.note().is_empty());
}

#[test]
fn the_assistant_is_given_the_document_and_a_reply_with_changes_becomes_a_proposal_to_review() {
    let (port, script) = SharedAssistant::new(vec![Turn::say("Here it is.\n<<<<<<< SEARCH\nbuy milk\n=======\nbuy oat milk\n>>>>>>> REPLACE\n")]);
    let app = EditApp::new(Box::new(home())).with_assistant(Box::new(port));
    let mut h = host_of(app, 1000, 700);
    open_todo(&mut h);
    h.event(&UiEvent::Tick);
    h.app.chat.as_mut().unwrap().focused = true;
    h.event(&UiEvent::Text("use oat milk".into()));
    h.event(&UiEvent::Key(UiKey::Enter));
    for _ in 0..6 {
        h.event(&UiEvent::Tick);
    }
    assert!(script.borrow().seen.iter().any(|s| s.contains("todo.md")), "the model saw the document: {:?}", script.borrow().seen);
    assert!(h.app.editor().reviewing(), "the reply became a proposal: {}", h.app.note());
    assert_eq!(h.app.editor().hunks(), 1);
    assert_eq!(h.app.editor().text(), "# Todo\nbuy milk\n", "nothing is applied by itself");
    // the person applies it
    tap(&mut h, "Apply");
    assert_eq!(h.app.editor().text(), "# Todo\nbuy oat milk\n");
}

#[test]
fn a_pinch_changes_the_text_size_from_the_size_it_began_at_and_stays_inside_the_limits() {
    let (mut h, _) = host(800, 600);
    assert_eq!(h.app.zoom(), ZOOM_START);
    pinch(&mut h, 1.2);
    assert_eq!(h.app.zoom(), 1200);
    pinch(&mut h, 1.5);
    assert_eq!(h.app.zoom(), 1500, "the scale counts from where the pinch began, not from the last report");
    h.event(&UiEvent::Gesture(Gesture::PinchEnd { scale: 1.5 }));
    pinch(&mut h, 0.5);
    assert_eq!(h.app.zoom(), 750, "a new pinch counts from the size the last one ended at");
    pinch(&mut h, 0.01);
    assert_eq!(h.app.zoom(), ZOOM_MIN);
    h.event(&UiEvent::Gesture(Gesture::PinchEnd { scale: 0.01 }));
    pinch(&mut h, 50.0);
    assert_eq!(h.app.zoom(), ZOOM_MAX);
    // junk scales change nothing
    h.event(&UiEvent::Gesture(Gesture::PinchEnd { scale: 1.0 }));
    pinch(&mut h, f32::NAN);
    pinch(&mut h, -1.0);
    assert_eq!(h.app.zoom(), ZOOM_MAX);
}

#[test]
fn the_text_size_buttons_do_the_same_and_the_size_is_kept_and_read_back() {
    let kept = Kept::default();
    let app = EditApp::new(Box::new(home())).with_storage(Box::new(kept.clone()));
    let mut h = host_of(app, 800, 600);
    tap(&mut h, "A+");
    assert_eq!(h.app.zoom(), ZOOM_START + ZOOM_STEP);
    assert_eq!(kept.0.borrow().map.get(ZOOM_KEY).map(String::as_str), Some("1150"));
    tap(&mut h, "A-");
    tap(&mut h, "A-");
    assert_eq!(h.app.zoom(), ZOOM_START - ZOOM_STEP);
    // a pinch is kept when it ends, not while it goes on
    pinch(&mut h, 2.0);
    assert_eq!(kept.0.borrow().map.get(ZOOM_KEY).map(String::as_str), Some("850"));
    h.event(&UiEvent::Gesture(Gesture::PinchEnd { scale: 2.0 }));
    assert_eq!(kept.0.borrow().map.get(ZOOM_KEY).map(String::as_str), Some("1700"));
    // the next start reads it, and a value that is not a number or is out of range is held inside the limits
    let again = EditApp::new(Box::new(home())).with_storage(Box::new(kept.clone()));
    assert_eq!(again.zoom(), 1700);
    kept.0.borrow_mut().map.insert(ZOOM_KEY.into(), "99999".into());
    assert_eq!(EditApp::new(Box::new(home())).with_storage(Box::new(kept.clone())).zoom(), ZOOM_MAX);
    kept.0.borrow_mut().map.insert(ZOOM_KEY.into(), "big".into());
    assert_eq!(EditApp::new(Box::new(home())).with_storage(Box::new(kept)).zoom(), ZOOM_START);
}

#[test]
fn a_bigger_text_size_draws_bigger_text() {
    let (mut h, _) = host(800, 600);
    open_todo(&mut h);
    let size_of = |l: &DisplayList| l.ops.iter().find_map(|o| if let Op::Text { size, text, .. } = o { text.contains("Todo").then_some(*size) } else { None }).expect("the first line is drawn");
    let small = size_of(&h.record());
    pinch(&mut h, 2.0);
    let big = size_of(&h.record());
    assert!(big > small * 1.5, "{small} -> {big}");
}

#[test]
fn every_size_draws_inside_the_pane_with_named_controls() {
    for (w, hh) in [(360u32, 740u32), (800, 480), (1280, 720)] {
        let (port, _) = SharedAssistant::new(vec![]);
        let mut h = host_of(EditApp::new(Box::new(home())).with_assistant(Box::new(port)), w, hh);
        open_todo(&mut h);
        let l = h.record();
        let names: Vec<String> = l.a11y().iter().map(|n| n.name.clone()).collect();
        for want in ["Files", "Assistant", "A-", "A+", "Save", "Undo"] {
            assert!(names.iter().any(|n| n == want), "{w}x{hh}: no {want} in {names:?}");
        }
        assert!(l.a11y().iter().all(|n| !n.name.is_empty()), "every control has a name");
        pinch(&mut h, 2.0);
        let l = h.record();
        for o in &l.ops {
            if let Op::Hit(r, _) = o {
                assert!(r.x >= -1.0 && r.right() <= w as f32 + 1.0, "{w}x{hh}: a control sticks out: {r:?}");
            }
        }
        // the assistant takes the pane on a phone, and sits beside the editor where there is room
        tap(&mut h, "Assistant");
        let _ = h.record();
    }
}

#[test]
fn the_assistant_button_is_not_there_without_an_assistant_and_no_event_breaks_the_app_without_a_file() {
    let (mut h, _) = host(360, 740);
    let l = h.record();
    assert!(!l.a11y().iter().any(|n| n.name == "Assistant"));
    for ev in [UiEvent::Action(A_ASSIST), UiEvent::Key(UiKey::Enter), UiEvent::Text("x".into()), UiEvent::Wheel(3), UiEvent::Key(UiKey::Backspace), UiEvent::Tick] {
        h.event(&ev);
        h.record();
    }
    assert_eq!(h.app.pane(), Pane::Files);
}
