//! Settings -> Encrypted home: the page is there for everyone with a password (not guests), it says in words what is and is not protected,
//! the keys open masked forms, a wrong sign-in password makes nothing, and a request the assistant left opens the form. Nothing here touches
//! a kernel: the only paths taken stop at the password check or at a damaged stand-in file.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
use iyakki_agent::enchome::form::{self as ef, Request};
use iyakki_agent::enchome::{EncHome, State};

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    cx
}

fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

fn type_in(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    for c in s.chars() {
        a.handle(&Event::Key(Key::Char(c)), cx);
    }
}

fn draw_text(a: &mut SettingsApp, cx: &AppCx) -> String {
    let mut g = Gfx::new(1100, 700);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(1100, 700);
    crate::access::start_frame();
    a.render(&mut g, R::new(0.0, 46.0 * s, 1100.0, 700.0 - 46.0 * s), s, cx, 0);
    g.text_log.iter().map(|(_, _, t)| t.clone()).collect::<Vec<_>>().join(" ")
}

#[test]
fn the_page_is_in_the_list_for_everyone_with_a_password_and_found_by_its_words() {
    let cx = admin("ench-tab");
    let a = SettingsApp::new(&cx);
    assert!(a.tabs().contains(&Tab::EncHome));
    assert_eq!(Tab::EncHome.group(), CATEGORIES[1]);
    assert!(Tab::ORDER.contains(&Tab::EncHome));
    for w in ["encrypt", "recovery key", "stolen", "move"] {
        assert!(Tab::EncHome.keywords().contains(w), "{w}");
    }
    let mut user = cx_with_admin_store("ench-user");
    user.session.role = iyakki_sec::policy::Role::User;
    user.session.policy = iyakki_sec::policy::Role::User.template();
    assert!(SettingsApp::new(&user).tabs().contains(&Tab::EncHome));
    let mut guest = cx_with_admin_store("ench-guest");
    guest.session.role = iyakki_sec::policy::Role::Guest;
    guest.session.policy = iyakki_sec::policy::Role::Guest.template();
    assert!(!SettingsApp::new(&guest).tabs().contains(&Tab::EncHome), "a guest has no password to encrypt with");
}

#[test]
fn the_page_says_what_is_and_is_not_protected_and_offers_make_and_move() {
    let cx = admin("ench-text");
    let mut a = SettingsApp::new(&cx).on(Tab::EncHome);
    let t = draw_text(&mut a, &cx);
    for w in ["Not encrypted", "Not protected", "recovery key is shown once", "stay readable"] {
        assert!(t.contains(w), "missing: {w}\n{t}");
    }
    let labels: Vec<&str> = a.ench_buttons().iter().map(|b| b.0).collect();
    assert_eq!(labels, vec!["Make my home encrypted", "Move my existing files"]);
}

#[test]
fn a_wrong_sign_in_password_makes_nothing_and_keeps_the_form_open_with_the_reason() {
    let cx = admin("ench-wrong");
    let dir = cx.services.dir().to_path_buf();
    let mut a = SettingsApp::new(&cx).on(Tab::EncHome);
    press(&mut a, &cx, 'c');
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::EncHome, _))), "the masked form opens");
    type_in(&mut a, &cx, "not-the-password");
    a.handle(&Event::Key(Key::Enter), &cx);
    // the check runs on the settings job now: wait for it, the form then comes back with the reason
    let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while a.is_busy() && std::time::Instant::now() < end {
        std::thread::sleep(std::time::Duration::from_millis(10));
        a.handle(&Event::Tick, &cx);
    }
    match &a.modal {
        Some(Modal::Form(FormKind::EncHome, f)) => assert!(f.error.as_deref().unwrap_or("").contains("not your sign-in password")),
        _ => panic!("the form must stay open"),
    }
    assert!(!dir.join("home-enc").exists() || std::fs::read_dir(dir.join("home-enc")).unwrap().next().is_none(), "nothing was made");
    // Esc leaves; the password never reached the status line or the audit log
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(a.modal.is_none());
    assert!(!a.status.as_ref().is_some_and(|(m, _)| m.contains("not-the-password")));
}

#[test]
fn a_request_from_the_assistant_opens_the_form_on_this_page() {
    let cx = admin("ench-req");
    let mut a = SettingsApp::new(&cx);
    ef::leave_request(cx.services.dir(), &cx.session.user, Request::Create);
    for _ in 0..20 {
        a.handle(&Event::Tick, &cx);
    }
    assert_eq!(a.tab(), Tab::EncHome);
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::EncHome, _))));
    assert_eq!(ef::take_request(cx.services.dir(), &cx.session.user), None, "taken once");
    // asking for something that makes no sense now only says so
    let mut b = SettingsApp::new(&cx).on(Tab::EncHome);
    b.ench_start(&cx, ef::Purpose::Unlock);
    assert!(b.modal.is_none() && b.status.as_ref().is_some_and(|(m, ok)| !ok && m.contains("make one first")), "{:?}", b.status);
}

#[test]
fn a_damaged_container_is_reported_and_offers_nothing_dangerous() {
    let cx = admin("ench-damaged");
    let d = cx.services.dir().join("home-enc");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("root.ivol"), vec![0u8; 20000]).unwrap();
    let mut a = SettingsApp::new(&cx).on(Tab::EncHome);
    let t = draw_text(&mut a, &cx);
    assert!(t.contains("Damaged"), "{t}");
    assert!(a.ench_buttons().is_empty());
    for c in ['c', 'm', 'u', 'l', 'k', 'a', 'x'] {
        press(&mut a, &cx, c);
        assert!(a.modal.is_none(), "{c}");
    }
}

// ---- the slow steps run on the settings job, not on the screen's thread ----

use iyakki_storage::dm::fake::FakeKernel;
use iyakki_storage::dm::{DmCmd, DmKernel};
use std::time::{Duration, Instant};

/// The fake kernel with a pause in the two slow calls (making the file system, mounting): a stand-in for Argon2id and the copy.
struct SlowKernel {
    inner: FakeKernel,
    pause: Duration,
}

impl DmKernel for SlowKernel {
    fn dm_ioctl(&self, cmd: DmCmd, buf: &mut Vec<u8>) -> std::io::Result<()> {
        self.inner.dm_ioctl(cmd, buf)
    }
    fn loop_attach(&self, file: &std::path::Path) -> std::io::Result<String> {
        self.inner.loop_attach(file)
    }
    fn loop_detach(&self, dev: &str) -> std::io::Result<()> {
        self.inner.loop_detach(dev)
    }
    fn make_fs(&self, dev: &str, label: &str) -> std::io::Result<()> {
        std::thread::sleep(self.pause);
        self.inner.make_fs(dev, label)
    }
    fn mount(&self, dev: &str, target: &std::path::Path) -> std::io::Result<()> {
        std::thread::sleep(self.pause);
        self.inner.mount(dev, target)
    }
    fn umount(&self, target: &std::path::Path) -> std::io::Result<()> {
        self.inner.umount(target)
    }
    fn is_mounted(&self, target: &std::path::Path) -> bool {
        self.inner.is_mounted(target)
    }
}

/// A page on a fake machine whose slow calls take `pause`; the account `root` has a home folder with one file.
fn slow_page(tag: &str, pause: Duration) -> (AppCx, SettingsApp) {
    page_with_home(tag, pause, false)
}

/// The same, with a file in the home folder: only "Move my existing files" works on a home that holds files (Make needs it empty).
fn page_with_home(tag: &str, pause: Duration, with_file: bool) -> (AppCx, SettingsApp) {
    let cx = admin(tag);
    let root = temp_dir(&format!("{tag}-machine"));
    let (home, fs) = (root.join("home"), root.join("fs"));
    std::fs::create_dir_all(home.join("root")).unwrap();
    std::fs::create_dir_all(&fs).unwrap();
    if with_file {
        std::fs::write(home.join("root").join("note.txt"), b"hello").unwrap();
    }
    let kernel: Arc<dyn DmKernel> = Arc::new(SlowKernel { inner: FakeKernel::new(&fs), pause });
    let data = cx.services.dir().to_path_buf();
    let mut a = SettingsApp::new(&cx).on(Tab::EncHome);
    a.ench.env = Some(Arc::new(move || iyakki_agent::enchome::testkit::fake_env(&data, &home, kernel.clone())));
    (cx, a)
}

fn wait_idle(a: &mut SettingsApp, cx: &AppCx) {
    let end = Instant::now() + Duration::from_secs(30);
    while a.is_busy() {
        assert!(Instant::now() < end, "the job never ended");
        std::thread::sleep(Duration::from_millis(10));
        a.handle(&Event::Tick, cx);
    }
}

fn recovery_text(a: &SettingsApp) -> String {
    match &a.ench.flow.flow {
        ef::Flow::ShowRecovery { text, .. } => text.to_string(),
        _ => panic!("no recovery key showing"),
    }
}

#[test]
fn make_runs_in_the_background_and_the_page_keeps_drawing_and_refuses_a_second_job() {
    let (cx, mut a) = slow_page("ench-bg-make", Duration::from_millis(2500));
    press(&mut a, &cx, 'c');
    type_in(&mut a, &cx, "rootpass12");
    let t0 = Instant::now();
    a.handle(&Event::Key(Key::Enter), &cx);
    assert!(t0.elapsed() < Duration::from_millis(1200), "Enter must return at once, took {:?}", t0.elapsed());
    assert!(a.is_busy() && a.modal.is_none());
    assert!(a.ench_buttons().is_empty(), "no button while it works");
    // the render loop is not blocked: frames keep coming in far less time than the slow step takes
    let mut frames = 0;
    let mut text = String::new();
    while a.is_busy() && frames < 1000 {
        let f0 = Instant::now();
        text = draw_text(&mut a, &cx);
        a.handle(&Event::Tick, &cx);
        assert!(f0.elapsed() < Duration::from_millis(1200), "a frame took {:?}", f0.elapsed());
        frames += 1;
        if frames == 2 {
            assert!(text.contains("Working on your encrypted home"), "{text}");
            // a second job cannot start while one runs
            press(&mut a, &cx, 'c');
            assert!(a.status.as_ref().is_some_and(|(m, ok)| !ok && m.contains("still running")), "{:?}", a.status);
            assert!(a.modal.is_none());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(frames >= 3, "the page drew only {frames} frames while it worked");
    wait_idle(&mut a, &cx);
    // the recovery key is on screen once, the home exists and is open, and nothing leaked
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::EncHome, _))));
    let key = recovery_text(&a);
    // the home exists and stays sealed until the recovery key is typed back (that is what "saved" means)
    assert_eq!(EncHome::for_status(cx.services.dir()).status(&cx.session.user).state, State::Locked);
    let after = draw_text(&mut a, &cx);
    // the recovery key is on this page on purpose (shown once); the password never is
    assert!(!after.contains("rootpass12"));
    assert!(!a.status.as_ref().is_some_and(|(m, _)| m.contains("rootpass12") || m.contains(&key)));
}

#[test]
fn a_wrong_password_in_the_background_gives_the_same_plain_answer_and_keeps_the_form() {
    let (cx, mut a) = slow_page("ench-bg-wrong", Duration::from_millis(50));
    press(&mut a, &cx, 'c');
    type_in(&mut a, &cx, "not-the-password");
    a.handle(&Event::Key(Key::Enter), &cx);
    wait_idle(&mut a, &cx);
    match &a.modal {
        Some(Modal::Form(FormKind::EncHome, f)) => {
            let e = f.error.clone().unwrap_or_default();
            assert!(e.contains("not your sign-in password"), "{e}");
            assert!(!e.contains("not-the-password"));
        }
        _ => panic!("the form must come back with the reason"),
    }
    assert_eq!(EncHome::for_status(cx.services.dir()).status(&cx.session.user).state, State::NotSetUp);
    // the form is usable again afterwards
    type_in(&mut a, &cx, "rootpass12");
    a.handle(&Event::Key(Key::Enter), &cx);
    wait_idle(&mut a, &cx);
    assert!(matches!(a.ench.flow.flow, ef::Flow::ShowRecovery { .. }));
}

#[test]
fn leaving_the_page_does_not_stop_the_job_and_the_result_brings_the_person_back() {
    let (cx, mut a) = slow_page("ench-bg-leave", Duration::from_millis(300));
    press(&mut a, &cx, 'c');
    type_in(&mut a, &cx, "rootpass12");
    a.handle(&Event::Key(Key::Enter), &cx);
    assert!(a.is_busy());
    let other = a.tabs().iter().position(|t| *t != Tab::EncHome).unwrap();
    a.tab = other;
    wait_idle(&mut a, &cx);
    assert_eq!(a.tab(), Tab::EncHome, "the recovery key is shown on its own page");
    assert!(matches!(a.modal, Some(Modal::Form(FormKind::EncHome, _))));
}

#[test]
fn open_with_the_password_runs_in_the_background_and_reports_in_plain_words() {
    let (cx, mut a) = slow_page("ench-bg-open", Duration::from_millis(100));
    press(&mut a, &cx, 'c');
    type_in(&mut a, &cx, "rootpass12");
    a.handle(&Event::Key(Key::Enter), &cx);
    wait_idle(&mut a, &cx);
    let key = recovery_text(&a);
    type_in(&mut a, &cx, &key);
    a.handle(&Event::Key(Key::Enter), &cx);
    wait_idle(&mut a, &cx);
    assert!(a.modal.is_none());
    draw_text(&mut a, &cx);
    // lock through the same fake machine the page uses (the Lock button itself goes to the real kernel path)
    let env = a.ench.env.clone().expect("the test machine");
    env().lock(&cx.session.user).unwrap();
    assert_eq!(EncHome::for_status(cx.services.dir()).status(&cx.session.user).state, State::Locked);
    // the buttons follow the state when a frame is drawn, as they do on a real screen
    draw_text(&mut a, &cx);
    // the right password opens it (the fake clock stands still, so a wrong try first would keep the growing wait for this one)
    draw_text(&mut a, &cx);
    press(&mut a, &cx, 'u');
    type_in(&mut a, &cx, "rootpass12");
    a.handle(&Event::Key(Key::Enter), &cx);
    wait_idle(&mut a, &cx);
    assert!(a.modal.is_none(), "the form closes when the right password opens the home");
    // the unlock happened on the test's fake machine: ask it (the page's own status view reads the real kernel path)
    let shown = draw_text(&mut a, &cx);
    assert_eq!(env().status(&cx.session.user).state, State::Open, "{:?}", a.status);
    assert!(a.status.as_ref().is_some_and(|(m, ok)| *ok && m.contains("is open")), "{:?} {shown}", a.status);
    // lock again, then a wrong password: the form comes back with one plain sentence that names nothing typed
    env().lock(&cx.session.user).unwrap();
    draw_text(&mut a, &cx);
    press(&mut a, &cx, 'u');
    type_in(&mut a, &cx, "wrong-password");
    a.handle(&Event::Key(Key::Enter), &cx);
    wait_idle(&mut a, &cx);
    let wrong = match &a.modal {
        Some(Modal::Form(FormKind::EncHome, f)) => f.error.clone().unwrap_or_default(),
        _ => panic!("the form must come back; status {:?}", a.status),
    };
    assert!(!wrong.is_empty() && !wrong.contains("wrong-password"));
}

#[test]
fn a_move_shows_its_progress_while_it_copies_and_stays_responsive() {
    let (cx, mut a) = page_with_home("ench-bg-move", Duration::from_millis(400), true);
    press(&mut a, &cx, 'm');
    type_in(&mut a, &cx, "rootpass12");
    a.handle(&Event::Key(Key::Enter), &cx);
    wait_idle(&mut a, &cx);
    assert!(matches!(a.modal, Some(Modal::Confirm(ConfirmKind::EncMove, _))), "{:?} {:?}", a.status, a.ench.flow.notice);
    let t0 = Instant::now();
    a.handle(&Event::Key(Key::Char('y')), &cx);
    assert!(t0.elapsed() < Duration::from_millis(300), "pressing Move must return at once");
    assert!(a.is_busy());
    let mut seen = vec![];
    while a.is_busy() {
        let before = a.ench.progress.lock().unwrap().clone();
        let t = draw_text(&mut a, &cx);
        let p = a.ench.progress.lock().unwrap().clone();
        assert!(t.contains("Moving your files into the encrypted home"), "{t}");
        // the step can change between reading it and drawing: only compare when it stood still
        if !p.is_empty() && p == before {
            assert!(t.contains(&p), "the page shows the step: {p}");
        }
        if !p.is_empty() && !seen.contains(&p) {
            seen.push(p);
        }
        a.handle(&Event::Tick, &cx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(seen.len() >= 2, "progress steps seen: {seen:?}");
    wait_idle(&mut a, &cx);
    let _ = recovery_text(&a);
    assert!(a.ench.flow.notice.as_deref().unwrap_or("").contains("copied, checked one by one"));
}
