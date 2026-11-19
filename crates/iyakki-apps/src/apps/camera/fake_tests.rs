//! The CAMERA tile driven through a fake [`CameraPort`]: a made-up camera, a scratch folder, a recorded audit list and a switch for the account's
//! write permission; no `Services`, no session.

use std::cell::{Cell, RefCell};

use super::*;
use crate::apps::tests::temp_dir;
use crate::camera::detect::tests::FakeMachine;
use crate::camera::fake::FakeOpener;

struct FakePort {
    may_write: Cell<bool>,
    audit: RefCell<Vec<(String, String)>>,
    machine: FakeMachine,
    opener: Arc<FakeOpener>,
}

impl CameraPort for FakePort {
    fn user(&self) -> String {
        "amy".into()
    }
    fn allows_write(&self, _dir: &str) -> bool {
        self.may_write.get()
    }
    fn camera_source(&self) -> (Presence, Arc<dyn Opener>) {
        (self.machine.presence(), self.opener.clone())
    }
    fn failure_reworder(&self) -> Arc<dyn Fn(&str) -> String> {
        Arc::new(|t| format!("[core] {t}"))
    }
    fn audit(&self, action: &str, subject: &str) {
        self.audit.borrow_mut().push((action.to_string(), subject.to_string()));
    }
}

fn setup(tag: &str) -> (FakePort, CameraApp, PathBuf) {
    let dir = temp_dir(&format!("camfake-{tag}"));
    let machine = FakeMachine::new(&format!("fake-{tag}"));
    machine.plug_webcam(0, "HD 4MP WEBCAM");
    let port = FakePort { may_write: Cell::new(true), audit: RefCell::new(Vec::new()), machine, opener: Arc::new(FakeOpener::default()) };
    let (presence, opener) = port.camera_source();
    let app = CameraApp::with_parts(presence, opener, dir.join("Pictures"), dir.join("Videos"));
    (port, app, dir)
}

fn pump(app: &mut CameraApp, port: &FakePort) {
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        app.handle_with(&Event::Tick, port);
        if app.live().is_some_and(|l| l.preview().is_some() && l.latest().is_some()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("no live picture; status {:?}, error {:?}", app.status_text(), app.error_text());
}

#[test]
fn new_asks_the_port_for_the_person_and_the_camera() {
    let (port, _, _) = setup("new");
    assert_eq!(port.user(), "amy");
    let app = CameraApp::new(&port);
    assert!(app.status_text().is_none());
}

#[test]
fn a_photo_is_saved_and_audited_through_the_port() {
    let (port, mut app, dir) = setup("photo");
    pump(&mut app, &port);
    app.handle_with(&Event::Key(Key::Char(' ')), &port);
    let n = std::fs::read_dir(dir.join("Pictures")).map(|r| r.count()).unwrap_or(0);
    assert_eq!(n, 1, "{:?}", app.status_text());
    let log = port.audit.borrow().clone();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].0, "camera.photo");
    assert!(log[0].1.contains("Pictures") && log[0].1.ends_with(".jpg"), "{log:?}");
}

#[test]
fn a_port_that_refuses_writing_saves_nothing_says_so_and_audits_nothing() {
    let (port, mut app, dir) = setup("deny");
    pump(&mut app, &port);
    port.may_write.set(false);
    app.handle_with(&Event::Key(Key::Char(' ')), &port);
    assert!(!dir.join("Pictures").exists());
    assert!(app.status_text().unwrap().contains("may not save"));
    assert!(port.audit.borrow().is_empty());
}

#[test]
fn a_failure_is_worded_by_what_the_port_gives_and_a_good_message_is_left_alone() {
    let (port, mut app, _dir) = setup("reword");
    app.handle_with(&Event::Tick, &port);
    app.say("Timer cancelled.", true);
    assert_eq!(app.status_text(), Some("Timer cancelled."));
    app.say("write failed", false);
    assert_eq!(app.status_text(), Some("[core] write failed"));
}
