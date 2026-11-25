use super::*;
use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper};
use iyakki_ui_sdk::event::{Gesture, Phase};
use iyakki_ui_sdk::host::Host;
use iyakki_ui_sdk::list::{DisplayList, Op};
use iyakki_ui_sdk::look::Look;
use iyakki_ui_sdk::ports::{MemStorage, PortError};
use iyakki_ui_sdk::terminal::{MemTerminal, Opened, Reply, TermError};
use std::cell::RefCell;
use std::rc::Rc;

/// The in-memory terminal, shared with the test so it can see what ran.
#[derive(Clone)]
struct Shell(Rc<RefCell<MemTerminal>>);

impl TerminalPort for Shell {
    fn open(&mut self) -> Result<Opened, TermError> {
        self.0.borrow_mut().open()
    }
    fn run(&mut self, line: &str) -> Result<Reply, TermError> {
        self.0.borrow_mut().run(line)
    }
    fn decide(&mut self, call: &str, approve: bool) -> Result<Reply, TermError> {
        self.0.borrow_mut().decide(call, approve)
    }
    fn close(&mut self) {
        self.0.borrow_mut().close()
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

fn host_of(app: CliApp, w: u32, h: u32) -> Host<CliApp> {
    Host::new(app, Look::default(), Arrangement::single(w, h), Box::new(MemDock::knowing(&[ID])), Box::new(MemWallpaper::default()))
}

fn host(w: u32, h: u32) -> (Host<CliApp>, Shell) {
    let sh = Shell(Rc::new(RefCell::new(MemTerminal::new())));
    let mut hs = host_of(CliApp::new(Box::new(sh.clone())), w, h);
    hs.event(&UiEvent::Tick);
    (hs, sh)
}

fn center_of(l: &DisplayList, name: &str) -> (f32, f32) {
    l.ops.iter().find_map(|o| if let Op::Hit(r, hit) = o { (hit.name == name).then_some((r.x + r.w / 2.0, r.y + r.h / 2.0)) } else { None }).unwrap_or_else(|| panic!("no control named {name}: {:?}", l.a11y().iter().map(|n| n.name.clone()).collect::<Vec<_>>()))
}

fn tap(h: &mut Host<CliApp>, name: &str) {
    let l = h.record();
    let (x, y) = center_of(&l, name);
    h.mouse(x, y, Phase::Down, 0);
    h.mouse(x, y, Phase::Up, 60);
}

fn type_line(h: &mut Host<CliApp>, s: &str) {
    h.event(&UiEvent::Text(s.into()));
    h.event(&UiEvent::Key(UiKey::Enter));
}

#[test]
fn a_read_only_line_runs_a_changing_line_waits_for_allow_and_a_no_runs_nothing() {
    let (mut h, sh) = host(800, 600);
    assert!(h.app.wants_keyboard(), "the field has the focus: the host shows the keyboard");
    type_line(&mut h, "echo hello");
    assert!(h.app.terminal().transcript().contains(&"hello".to_string()));
    type_line(&mut h, "mkdir notes");
    assert!(h.app.terminal().pending_question().is_some());
    assert!(!sh.0.borrow().ran.iter().any(|l| l.contains("mkdir")), "nothing ran before the answer");
    tap(&mut h, "Deny");
    assert!(h.app.terminal().transcript().contains(&"Not run: you said no.".to_string()));
    type_line(&mut h, "mkdir notes");
    tap(&mut h, "Allow");
    assert!(sh.0.borrow().ran.iter().any(|l| l.contains("mkdir notes")));
}

#[test]
fn esc_goes_to_the_view_first_and_leaves_the_app_only_when_the_view_has_no_use_for_it() {
    let (mut h, sh) = host(800, 600);
    // Esc in the field gives the focus back; the next Esc leaves and closes the session
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Stay);
    assert!(!h.app.wants_keyboard());
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Back);
    assert!(!sh.0.borrow().ran.iter().any(|l| l == "x"));
    // a question that waits takes Esc as a no
    let (mut h, _) = host(800, 600);
    type_line(&mut h, "write a.txt hi");
    assert_eq!(h.event(&UiEvent::Key(UiKey::Esc)), Flow::Stay);
    assert!(h.app.terminal().pending_question().is_none());
}

#[test]
fn a_host_without_the_whole_session_says_so_in_words_and_the_line_still_works() {
    struct LineOnly(MemTerminal);
    impl TerminalPort for LineOnly {
        fn open(&mut self) -> Result<Opened, TermError> {
            self.0.open()
        }
        fn run(&mut self, line: &str) -> Result<Reply, TermError> {
            self.0.run(line)
        }
        fn decide(&mut self, call: &str, approve: bool) -> Result<Reply, TermError> {
            self.0.decide(call, approve)
        }
        fn close(&mut self) {
            self.0.close()
        }
    }
    let mut h = host_of(CliApp::new(Box::new(LineOnly(MemTerminal::new()))), 800, 600);
    h.event(&UiEvent::Tick);
    tap(&mut h, "Full");
    let l = h.record();
    assert!(l.texts().iter().any(|t| t.contains("not available")), "{:?}", l.texts());
    type_line(&mut h, "echo still here");
    assert!(h.app.terminal().transcript().contains(&"still here".to_string()));
    let (mut off, _) = (host_of(CliApp::new(Box::new(MemTerminal::unavailable())), 800, 600), ());
    off.event(&UiEvent::Tick);
    assert!(off.record().texts().iter().any(|t| t.contains("not available")));
}

#[test]
fn a_pinch_and_the_two_buttons_change_the_text_size_and_the_size_is_kept_and_read_back() {
    let kept = Kept::default();
    let sh = Shell(Rc::new(RefCell::new(MemTerminal::new())));
    let mut h = host_of(CliApp::new(Box::new(sh)).with_storage(Box::new(kept.clone())), 800, 600);
    h.event(&UiEvent::Tick);
    tap(&mut h, "A+");
    assert_eq!(h.app.zoom(), 1150);
    assert_eq!(kept.0.borrow().map.get(ZOOM_KEY).map(String::as_str), Some("1150"));
    h.event(&UiEvent::Gesture(Gesture::Pinch { scale: 2.0, x: 1.0, y: 1.0 }));
    assert_eq!(h.app.zoom(), 2300);
    assert_eq!(kept.0.borrow().map.get(ZOOM_KEY).map(String::as_str), Some("1150"), "kept when the pinch ends");
    h.event(&UiEvent::Gesture(Gesture::PinchEnd { scale: 2.0 }));
    assert_eq!(kept.0.borrow().map.get(ZOOM_KEY).map(String::as_str), Some("2300"));
    let sh2 = Shell(Rc::new(RefCell::new(MemTerminal::new())));
    assert_eq!(CliApp::new(Box::new(sh2)).with_storage(Box::new(kept)).zoom(), 2300);
}

#[test]
fn a_bigger_text_size_draws_bigger_text_and_every_size_stays_inside_the_pane() {
    for (w, hh) in [(360u32, 740u32), (800, 480), (1280, 720)] {
        let (mut h, _) = host(w, hh);
        type_line(&mut h, "echo hello");
        let size_of = |l: &DisplayList| l.ops.iter().find_map(|o| if let Op::Text { size, text, .. } = o { (text == "A+").then_some(*size) } else { None }).expect("A+ is drawn");
        let l = h.record();
        let names: Vec<String> = l.a11y().iter().map(|n| n.name.clone()).collect();
        for want in ["A-", "A+", "Full", "Run"] {
            assert!(names.iter().any(|n| n == want), "{w}x{hh}: no {want} in {names:?}");
        }
        assert!(l.a11y().iter().all(|n| !n.name.is_empty()));
        let small = size_of(&l);
        h.event(&UiEvent::Gesture(Gesture::Pinch { scale: 2.0, x: 1.0, y: 1.0 }));
        let l = h.record();
        assert!(size_of(&l) > small, "{w}x{hh}");
        for o in &l.ops {
            if let Op::Hit(r, _) = o {
                assert!(r.x >= -1.0 && r.right() <= w as f32 + 1.0, "{w}x{hh}: a control sticks out: {r:?}");
            }
        }
    }
}

#[test]
fn a_pod_item_from_another_app_asks_for_a_shell_in_that_pod_and_anything_else_is_ignored() {
    use iyakki_ui_sdk::terminal::PodSession;
    let item = PodSession { context: "dev".into(), namespace: "shop".into(), pod: "web-1".into(), container: "app".into(), ..Default::default() }.to_item();
    let mut app = CliApp::new(Box::new(MemTerminal::new()));
    app.open_item("sandbox:x");
    app.open_item("pod:too\nfew");
    assert!(app.terminal().pending_question().is_none() && !app.terminal().in_screen(), "not a pod item: nothing is asked");
    app.open_item(&item);
    let q = app.terminal().pending_question().expect("the card is waiting");
    assert!(q.summary.contains("pod web-1") && q.summary.contains("namespace shop"), "{}", q.summary);
    assert!(app.terminal().in_screen() && app.terminal().screen_text().is_none(), "asked, nothing running before the yes");
}
