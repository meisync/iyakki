//! The AUTH page over a fake vault and a fake clock: the codes and the bar, adding by typing, by a pasted link and by a made-up camera,
//! copying, groups, editing and deleting, a locked vault, an unset clock, the three window sizes, and that no secret or code reaches
//! the audit log or the screen after it is saved.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::*;
use crate::agent_login_scan::tests::Stage;
use crate::apps::tests::cx_with_admin_store;
use crate::camera::detect::tests::FakeMachine;
use crate::gfx::Gfx;
use crate::ui::{clipboard as cb, R};

const SECRET: &str = "JBSWY3DPEHPK3PXP";
const LINK: &str = "otpauth://totp/Acme:zed@example.com?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Acme&digits=8&algorithm=SHA256&period=60";
/// 2027-01-15 08:00:00 UTC: after the clock floor, 0 seconds into a 30 second step.
const T0: i64 = 1_800_000_000;

#[derive(Clone, Default)]
struct Shared {
    text: Rc<RefCell<Option<String>>>,
    locked: Rc<Cell<bool>>,
    fail_save: Rc<Cell<bool>>,
}

impl CodeVault for Shared {
    fn load(&self) -> Result<Book, VaultProblem> {
        if self.locked.get() {
            return Err(VaultProblem::Locked);
        }
        match self.text.borrow().as_deref() {
            None => Ok(Book::default()),
            Some(t) => Book::parse(t).map_err(VaultProblem::Other),
        }
    }
    fn save(&self, book: &Book) -> Result<(), VaultProblem> {
        if self.locked.get() {
            return Err(VaultProblem::Locked);
        }
        if self.fail_save.get() {
            return Err(VaultProblem::Other("the vault could not be written".into()));
        }
        *self.text.borrow_mut() = Some(book.to_text().map_err(VaultProblem::Other)?.to_string());
        Ok(())
    }
}

impl Shared {
    fn book(&self) -> Book {
        self.load().unwrap()
    }
    fn put(&self, entries: &[(&str, &str, &str)]) {
        let mut b = Book::default();
        for (i, a, g) in entries {
            if !b.groups.iter().any(|x| x == g) {
                b.add_group(g).unwrap();
            }
            b.add(&NewEntry::typed(i, a, SECRET, (Algo::Sha1, 6, 30), g)).unwrap();
        }
        self.save(&b).unwrap();
    }
}

struct Fx {
    app: AuthApp,
    cx: AppCx,
    vault: Shared,
    time: Rc<Cell<i64>>,
    g: Gfx,
    area: R,
    s: f32,
    tick: u64,
}

impl Fx {
    fn new(w: u32, h: u32, entries: &[(&str, &str, &str)]) -> Fx {
        cb::clear();
        let cx = cx_with_admin_store("auth");
        let vault = Shared::default();
        vault.put(entries);
        let time = Rc::new(Cell::new(T0 + 3));
        let t = time.clone();
        let app = AuthApp::with_vault(Box::new(vault.clone()), Box::new(move || t.get()));
        let mut f = Fx { app, cx, vault, time, g: Gfx::new(w, h), area: R::new(0.0, 0.0, w as f32, h as f32), s: crate::shell::scale_for(w, h), tick: 0 };
        f.draw();
        f
    }
    fn two() -> Fx {
        Fx::new(1280, 720, &[("GitHub", "alice", "Personal"), ("Slack", "alice@work", "Work")])
    }
    fn draw(&mut self) {
        self.tick += 1;
        self.g.clear(rgb(8, 10, 22));
        self.g.clear_hits();
        self.g.text_log.clear();
        self.app.render(&mut self.g, self.area, self.s, &self.cx, self.tick);
    }
    fn ev(&mut self, e: Event) -> Flow {
        let f = self.app.handle(&e, &self.cx);
        self.draw();
        f
    }
    fn key(&mut self, k: Key) -> Flow {
        self.ev(Event::Key(k))
    }
    fn typ(&mut self, t: &str) {
        for c in t.chars() {
            self.key(Key::Char(c));
        }
    }
    fn text(&self) -> String {
        self.g.text_log.iter().map(|t| t.2.clone()).collect::<Vec<_>>().join("\n")
    }
    fn hit(&self, e: &Event) -> Option<R> {
        self.g.hits.iter().find(|h| &h.ev == e).map(|h| h.rect)
    }
    fn click(&mut self, e: Event) {
        let r = self.hit(&e).unwrap_or_else(|| panic!("{e:?} is not on screen:\n{}", self.text()));
        assert!(r.w > 0.0 && r.h > 0.0);
        crate::ui::set_from_click(true);
        self.ev(e);
        crate::ui::set_from_click(false);
    }
    fn audit(&self) -> String {
        std::fs::read_to_string(self.cx.audit.path()).unwrap_or_default()
    }
    /// Whether the audit log has a line of this action with this detail.
    fn logged(&self, event: &str, detail: &str) -> bool {
        self.audit().contains(&format!("\"event\":\"{event}\",\"detail\":\"{detail}\""))
    }
    fn code(&self, secret: &str, algo: Algo, digits: u32, period: u32) -> String {
        let key = totp::base32_decode(secret).unwrap();
        totp::totp(algo, &key, self.time.get(), period, digits)
    }
    /// Run the heartbeat until `done` or a generous limit.
    fn pump(&mut self, what: &str, mut done: impl FnMut(&AuthApp) -> bool) {
        let t0 = Instant::now();
        while !done(&self.app) {
            self.app.wants_redraw();
            self.draw();
            assert!(t0.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn is_add(a: &AuthApp) -> bool {
    matches!(a.mode, Mode::Add(..))
}

#[test]
fn the_codes_of_the_chosen_group_are_drawn_in_big_grouped_digits_with_a_countdown() {
    let mut f = Fx::two();
    let t = f.text();
    assert!(t.contains("Personal") && t.contains("Work") && t.contains("GitHub") && t.contains("alice"), "{t}");
    assert!(!t.contains("Slack"), "the other group's codes are not drawn: {t}");
    let want = totp::grouped(&f.code(SECRET, Algo::Sha1, 6, 30));
    assert!(t.contains(&want), "{want} in {t}");
    assert!(t.contains("27 s"), "30 s step, 3 s in: {t}");
    assert!(!t.contains("next "), "the next code only shows at the end of the step");
    // the other group
    f.click(Event::Tab(T_GROUP + 1));
    assert!(f.text().contains("Slack") && !f.text().contains("GitHub"));
}

#[test]
fn in_the_last_five_seconds_the_next_code_is_shown_too() {
    let mut f = Fx::two();
    f.time.set(T0 + 26);
    f.draw();
    let t = f.text();
    let now = totp::grouped(&f.code(SECRET, Algo::Sha1, 6, 30));
    f.time.set(T0 + 30);
    let next = totp::grouped(&f.code(SECRET, Algo::Sha1, 6, 30));
    assert_ne!(now, next);
    assert!(t.contains(&now) && t.contains(&format!("next {next}")) && t.contains("4 s"), "{t}");
    f.time.set(T0 + 20);
    f.draw();
    assert!(!f.text().contains("next "));
}

#[test]
fn an_unset_clock_shows_no_codes_and_says_so() {
    let mut f = Fx::two();
    f.time.set(0);
    f.draw();
    let t = f.text();
    assert!(t.contains("clock") && t.contains("not set"), "{t}");
    assert!(!t.contains(&totp::grouped(&f.code(SECRET, Algo::Sha1, 6, 30))) && t.contains("------"), "{t}");
    f.key(Key::Enter);
    assert!(cb::secret_waiting().is_none(), "nothing is copied from an unset clock");
    f.time.set(T0);
    f.draw();
    assert!(!f.text().contains("clock of this computer"));
}

#[test]
fn a_locked_vault_says_how_to_open_it_and_shows_nothing_of_the_codes() {
    let mut f = Fx::two();
    f.vault.locked.set(true);
    f.app.reload();
    f.draw();
    let t = f.text();
    assert!(t.contains("vault is locked") && t.contains("sign-in password") && t.contains("Settings") && t.contains("Try again"), "{t}");
    assert!(!t.contains("GitHub") && !t.contains("Personal"), "{t}");
    // adding and scanning are refused
    f.key(Key::Char('a'));
    assert!(matches!(f.app.mode, Mode::Browse));
    f.key(Key::Char('s'));
    assert!(matches!(f.app.mode, Mode::Browse));
    // Try again while still locked: stays locked
    f.click(Event::Tab(T_RETRY));
    assert!(f.text().contains("vault is locked"));
    // unlocked: the codes are back
    f.vault.locked.set(false);
    f.click(Event::Tab(T_RETRY));
    assert!(f.text().contains("GitHub"), "{}", f.text());
    assert!(f.audit().is_empty() || !f.audit().contains(SECRET));
}

#[test]
fn a_damaged_stored_book_is_reported_and_never_overwritten() {
    let mut f = Fx::two();
    *f.vault.text.borrow_mut() = Some("garbage".into());
    f.app.reload();
    f.draw();
    assert!(f.text().contains("could not be read"), "{}", f.text());
    f.key(Key::Char('a'));
    f.key(Key::Char('n'));
    assert!(matches!(f.app.mode, Mode::Browse));
    assert_eq!(f.vault.text.borrow().as_deref(), Some("garbage"));
}

#[test]
fn the_page_draws_and_registers_clicks_at_three_window_sizes() {
    for (w, h) in [(640u32, 360u32), (1280, 720), (1920, 1080)] {
        let mut f = Fx::new(w, h, &[("GitHub", "alice", "Personal"), ("Mail", "me", "Personal"), ("Slack", "x", "Work"), ("A", "b", "Work"), ("C", "d", "Work")]);
        let t = f.text();
        assert!(t.contains("GitHub") && t.contains("Add code") && t.contains("Groups"), "{w}x{h}: {t}");
        assert!(f.hit(&Event::Tab(T_ADD)).is_some() && f.hit(&Event::Tab(T_COPY)).is_some() && f.hit(&Event::Tab(T_NEWGROUP)).is_some(), "{w}x{h}");
        for hit in &f.g.hits {
            assert!(hit.rect.x + hit.rect.w <= w as f32 + 1.0 && hit.rect.y + hit.rect.h <= f.area.h + 1.0 || hit.label == "backdrop", "{w}x{h}: {} {:?}", hit.label, hit.rect);
        }
        // the forms stay inside the window too
        for key in [Key::Char('a'), Key::Char('n')] {
            f.key(key);
            for want in ["Save", "Cancel"] {
                let r = f.g.hits.iter().find(|x| x.label == want).unwrap_or_else(|| panic!("{w}x{h}: {want}")).rect;
                assert!(r.y + r.h <= f.area.h + 1.0 && r.y >= 0.0, "{w}x{h}: {want} at {r:?}");
            }
            f.key(Key::Esc);
        }
        // the delete question
        f.key(Key::Delete);
        assert!(f.text().contains("Delete the code for") && f.hit(&Event::Key(Key::Char('y'))).is_some(), "{w}x{h}");
        f.key(Key::Esc);
        // and the camera question
        f.key(Key::Char('s'));
        f.draw();
    }
}

#[test]
fn a_code_is_added_by_typing_and_kept_in_the_vault_with_everything() {
    let mut f = Fx::two();
    f.key(Key::Char('a'));
    assert!(is_add(&f.app));
    f.typ("Example");
    f.key(Key::Enter);
    f.typ("me@example.com");
    f.key(Key::Enter);
    f.typ("jbsw y3dp ehpk 3pxp");
    f.key(Key::Ctrl('s'));
    assert!(matches!(f.app.mode, Mode::Browse), "{}", f.text());
    let b = f.vault.book();
    let e = b.entries.iter().find(|e| e.issuer == "Example").expect("saved");
    assert_eq!((e.account.as_str(), e.secret.as_str(), e.algo.as_str(), e.digits, e.period, e.group.as_str()), ("me@example.com", SECRET, "SHA1", 6, 30, "Personal"));
    assert!(f.text().contains("Example") && f.text().contains("Added"));
    // the secret is not on screen after it is saved, and the audit names only the service
    assert!(!f.text().contains(SECRET));
    let audit = f.audit();
    assert!(f.logged("auth.add", "issuer=Example"), "{audit}");
    assert!(!audit.contains(SECRET) && !audit.contains("jbsw"), "{audit}");
}

#[test]
fn a_bad_secret_keeps_the_form_open_with_a_plain_sentence() {
    let mut f = Fx::two();
    f.key(Key::Char('a'));
    f.typ("X");
    f.key(Key::Enter);
    f.key(Key::Enter);
    f.typ("not base32!");
    f.key(Key::Ctrl('s'));
    assert!(is_add(&f.app));
    assert!(f.text().contains("only the letters A to Z"), "{}", f.text());
    assert_eq!(f.vault.book().entries.len(), 2);
    f.key(Key::Esc);
    assert!(matches!(f.app.mode, Mode::Browse));
}

#[test]
fn a_link_in_the_secret_box_fills_in_everything_it_says() {
    let mut f = Fx::two();
    f.key(Key::Char('a'));
    f.key(Key::Enter);
    f.key(Key::Enter);
    f.ev(Event::Paste(LINK.into()));
    f.key(Key::Ctrl('s'));
    assert!(matches!(f.app.mode, Mode::Browse), "{}", f.text());
    let b = f.vault.book();
    let e = b.entries.iter().find(|e| e.issuer == "Acme").expect("saved");
    assert_eq!((e.account.as_str(), e.algo.as_str(), e.digits, e.period), ("zed@example.com", "SHA256", 8, 60));
    let code = f.code("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ", Algo::Sha256, 8, 60);
    assert!(f.text().contains(&totp::grouped(&code)), "{}", f.text());
}

#[test]
fn paste_link_reads_the_clipboard_and_a_bad_clipboard_is_explained() {
    let mut f = Fx::two();
    cb::copy("just some words");
    f.click(Event::Tab(T_LINK));
    assert!(matches!(f.app.mode, Mode::Browse));
    assert!(f.text().contains("no one-time-code link"), "{}", f.text());
    cb::copy(LINK);
    f.click(Event::Tab(T_LINK));
    assert!(is_add(&f.app));
    let t = f.text();
    assert!(t.contains("Acme") && t.contains("zed@example.com") && t.contains("8 digits, SHA256, 60 s"), "{t}");
    f.key(Key::Ctrl('s'));
    assert!(f.vault.book().entries.iter().any(|e| e.issuer == "Acme" && e.digits == 8));
    // a link pasted with Ctrl+V on the page opens the form as well
    f.ev(Event::Paste("otpauth://totp/Other:me?secret=JBSWY3DP".into()));
    assert!(is_add(&f.app));
    f.key(Key::Esc);
    // a broken link says why
    cb::copy("otpauth://totp/x?secret=!!!");
    f.click(Event::Tab(T_LINK));
    assert!(matches!(f.app.mode, Mode::Browse) && f.text().contains("only the letters A to Z"), "{}", f.text());
}

fn rig(tag: &str, camera: bool, st: &Stage) -> (FakeMachine, Arc<CamSource>) {
    let machine = FakeMachine::new(&format!("auth-{tag}"));
    if camera {
        machine.plug_webcam(0, "FaceTime HD Camera");
    }
    let src = CamSource::with(machine.presence(), st.opener());
    (machine, src)
}

#[test]
fn scanning_asks_first_reads_a_link_turns_the_camera_off_and_opens_the_add_form() {
    let mut f = Fx::two();
    let st = Stage::new(LINK);
    let (_m, src) = rig("scan", true, &st);
    f.app = std::mem::replace(&mut f.app, AuthApp::with_vault(Box::new(f.vault.clone()), Box::new(|| T0))).with_camera(src);
    let t = f.time.clone();
    f.app.now = Box::new(move || t.get());
    f.click(Event::Tab(T_SCAN));
    let t = f.text();
    assert!(t.contains("Turn on the camera?") && t.contains("Turn camera on"), "{t}");
    assert_eq!(st.opens.load(Ordering::SeqCst), 0, "the camera is not opened before the person says yes");
    f.click(Event::Tab(T_ALLOW));
    assert!(f.text().contains("CAMERA IS ON"), "{}", f.text());
    f.pump("the read", is_add);
    assert!(st.closed.load(Ordering::SeqCst), "the camera is off as soon as a code is read");
    let t = f.text();
    assert!(t.contains("Acme") && t.contains("zed@example.com"), "{t}");
    assert_eq!(f.vault.book().entries.len(), 2, "nothing is saved before Save");
    f.key(Key::Ctrl('s'));
    assert!(f.vault.book().entries.iter().any(|e| e.issuer == "Acme"));
    let audit = f.audit();
    assert!(f.logged("auth.add", "issuer=Acme") && !audit.contains("GEZDGNBV"), "{audit}");
}

#[test]
fn a_qr_code_that_is_not_a_link_is_ignored_and_scanning_goes_on() {
    let mut f = Fx::two();
    let st = Stage::new("https://example.test/not-a-code");
    let (_m, src) = rig("notlink", true, &st);
    f.app = std::mem::replace(&mut f.app, AuthApp::with_vault(Box::new(f.vault.clone()), Box::new(|| T0))).with_camera(src);
    f.click(Event::Tab(T_SCAN));
    f.click(Event::Tab(T_ALLOW));
    f.pump("the message", |a| matches!(&a.mode, Mode::Scan(s) if s.said().is_some()));
    assert!(matches!(&f.app.mode, Mode::Scan(s) if s.step() == Step::On), "still looking");
    let t = f.text();
    assert!(t.contains("not a one-time-code link") && !t.contains("example.test"), "{t}");
    // showing the real one finishes it
    st.show(LINK);
    f.pump("the read", is_add);
    f.key(Key::Esc);
}

#[test]
fn saying_no_to_the_camera_or_having_none_adds_nothing() {
    let mut f = Fx::two();
    let st = Stage::new(LINK);
    let (_m, src) = rig("deny", true, &st);
    f.app = std::mem::replace(&mut f.app, AuthApp::with_vault(Box::new(f.vault.clone()), Box::new(|| T0))).with_camera(src);
    f.click(Event::Tab(T_SCAN));
    f.click(Event::Tab(T_DENY));
    assert!(matches!(f.app.mode, Mode::Browse));
    assert_eq!(st.opens.load(Ordering::SeqCst), 0);
    assert!(f.text().contains("camera stays off"), "{}", f.text());
    // Esc on the question is a no too
    f.click(Event::Tab(T_SCAN));
    f.key(Key::Esc);
    assert_eq!(st.opens.load(Ordering::SeqCst), 0);
    // while it is on, Esc turns it off
    f.click(Event::Tab(T_SCAN));
    f.key(Key::Enter);
    assert!(matches!(&f.app.mode, Mode::Scan(s) if s.step() == Step::On));
    f.key(Key::Esc);
    assert!(matches!(f.app.mode, Mode::Browse));
    assert!(f.text().contains("camera is off"), "{}", f.text());
    // no camera at all
    let st2 = Stage::new(LINK);
    let (_m2, src2) = rig("nocam", false, &st2);
    f.app = std::mem::replace(&mut f.app, AuthApp::with_vault(Box::new(f.vault.clone()), Box::new(|| T0))).with_camera(src2);
    f.click(Event::Tab(T_SCAN));
    assert!(matches!(f.app.mode, Mode::Browse) && f.text().contains("no camera"), "{}", f.text());
    assert_eq!(f.vault.book().entries.len(), 2);
}

#[test]
fn leaving_the_page_while_the_camera_is_on_turns_it_off() {
    let mut f = Fx::two();
    let st = Stage::new("nothing to read");
    let (_m, src) = rig("leave", true, &st);
    f.app = std::mem::replace(&mut f.app, AuthApp::with_vault(Box::new(f.vault.clone()), Box::new(|| T0))).with_camera(src);
    f.click(Event::Tab(T_SCAN));
    f.click(Event::Tab(T_ALLOW));
    f.pump("the first frame", |a| matches!(&a.mode, Mode::Scan(s) if s.on()));
    drop(f);
    let t0 = Instant::now();
    while !st.closed.load(Ordering::SeqCst) && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(st.closed.load(Ordering::SeqCst));
}

#[test]
fn copy_puts_the_code_on_the_clipboard_as_a_secret_and_audits_only_the_service() {
    let mut f = Fx::two();
    let code = f.code(SECRET, Algo::Sha1, 6, 30);
    f.click(Event::Tab(T_COPY));
    assert!(f.text().contains("Copied."), "{}", f.text());
    assert!(cb::history().is_empty(), "a secret is not kept in the history");
    assert_eq!(cb::secret_waiting().map(|s| s.0), Some(6));
    assert_eq!(cb::paste().as_deref(), Some(code.as_str()), "the code itself, without the space");
    assert_eq!(cb::paste(), None, "once");
    let audit = f.audit();
    assert!(f.logged("auth.copy", "issuer=GitHub") && !audit.contains(&code), "{audit}");
}

#[test]
fn a_tap_on_the_code_and_the_keyboard_copy_it_too() {
    let mut f = Fx::two();
    let code = f.code(SECRET, Algo::Sha1, 6, 30);
    // the click region over the code digits
    let r = f.g.hits.iter().find(|h| h.label == "code").expect("the code is tappable").rect;
    crate::ui::set_from_click(true);
    f.ev(f.g.hits.iter().find(|h| h.label == "code").unwrap().ev.clone());
    crate::ui::set_from_click(false);
    assert!(r.w > 0.0);
    assert_eq!(cb::paste().as_deref(), Some(code.as_str()));
    f.key(Key::Enter);
    assert_eq!(cb::paste().as_deref(), Some(code.as_str()), "Enter copies the chosen code");
    f.key(Key::Char('c'));
    assert_eq!(cb::paste().as_deref(), Some(code.as_str()));
}

#[test]
fn groups_can_be_made_renamed_and_deleted_with_a_question_first() {
    let mut f = Fx::two();
    // new
    f.click(Event::Tab(T_NEWGROUP));
    f.typ("Family");
    f.key(Key::Ctrl('s'));
    assert_eq!(f.vault.book().groups, ["Personal", "Work", "Family"]);
    assert_eq!(f.app.group_name(), "Family", "the new group is shown");
    // the same name again is refused in words
    f.click(Event::Tab(T_NEWGROUP));
    f.typ("family");
    f.key(Key::Ctrl('s'));
    assert!(f.text().contains("already a group"), "{}", f.text());
    f.key(Key::Esc);
    // rename keeps the codes
    f.click(Event::Tab(T_GROUP + 1));
    f.click(Event::Tab(T_RENAME));
    for _ in 0..4 {
        f.key(Key::Backspace);
    }
    f.typ("Job");
    f.key(Key::Ctrl('s'));
    let b = f.vault.book();
    assert_eq!(b.groups, ["Personal", "Job", "Family"]);
    assert_eq!(b.entries.iter().find(|e| e.issuer == "Slack").unwrap().group, "Job");
    // delete asks first; Esc keeps it
    f.click(Event::Tab(T_DELGROUP));
    assert!(f.text().contains("Delete the group Job?") && f.text().contains("not deleted"), "{}", f.text());
    f.key(Key::Esc);
    assert_eq!(f.vault.book().groups.len(), 3);
    f.click(Event::Tab(T_DELGROUP));
    f.key(Key::Char('y'));
    let b = f.vault.book();
    assert_eq!(b.groups, ["Personal", "Family"]);
    assert_eq!(b.entries.iter().find(|e| e.issuer == "Slack").unwrap().group, "Personal", "the codes move, they are not deleted");
    assert!(f.text().contains("moved to Personal"), "{}", f.text());
    let audit = f.audit();
    assert!(audit.contains("auth.group.add") && audit.contains("auth.group.rename") && audit.contains("auth.group.delete"), "{audit}");
    // the last group has no delete button
    f.click(Event::Tab(T_GROUP + 1));
    f.click(Event::Tab(T_DELGROUP));
    f.key(Key::Char('y'));
    assert_eq!(f.vault.book().groups, ["Personal"]);
    f.draw();
    assert!(f.hit(&Event::Tab(T_DELGROUP)).is_none(), "the only group cannot be deleted");
}

#[test]
fn a_code_moves_between_groups() {
    let mut f = Fx::two();
    f.click(Event::Tab(T_MOVE));
    assert!(f.text().contains("Move GitHub"), "{}", f.text());
    f.key(Key::Right);
    f.key(Key::Ctrl('s'));
    let b = f.vault.book();
    assert_eq!(b.entries.iter().find(|e| e.issuer == "GitHub").unwrap().group, "Work");
    assert!(f.text().contains("Moved to Work"));
    assert!(f.logged("auth.move", "issuer=GitHub group=Work"));
}

#[test]
fn a_code_is_edited_and_the_secret_is_not_part_of_the_form() {
    let mut f = Fx::two();
    f.click(Event::Tab(T_EDIT));
    let t = f.text();
    assert!(t.contains("The secret stays as it is") && !t.contains(SECRET), "{t}");
    for _ in 0..6 {
        f.key(Key::Backspace);
    }
    f.typ("GitLab");
    f.key(Key::Ctrl('s'));
    let b = f.vault.book();
    let e = b.entries.iter().find(|e| e.issuer == "GitLab").expect("renamed");
    assert_eq!(e.secret, SECRET);
    assert!(f.logged("auth.edit", "issuer=GitLab"));
    // an empty name is refused
    f.click(Event::Tab(T_EDIT));
    for _ in 0..20 {
        f.key(Key::Backspace);
    }
    f.key(Key::Enter);
    for _ in 0..20 {
        f.key(Key::Backspace);
    }
    f.key(Key::Ctrl('s'));
    assert!(f.text().contains("service name or the account name"), "{}", f.text());
}

#[test]
fn deleting_a_code_asks_first_and_the_keyboard_works_the_same() {
    let mut f = Fx::two();
    f.click(Event::Tab(T_DEL));
    assert!(f.text().contains("Delete the code for GitHub (alice)?") && f.text().contains("cannot be shown again"), "{}", f.text());
    f.key(Key::Esc);
    assert_eq!(f.vault.book().entries.len(), 2, "cancelled");
    f.key(Key::Delete);
    f.key(Key::Char('x'));
    assert!(matches!(f.app.mode, Mode::DelEntry(_)), "another key does not confirm");
    f.key(Key::Char('y'));
    assert_eq!(f.vault.book().entries.len(), 1);
    assert!(f.logged("auth.delete", "issuer=GitHub") && !f.audit().contains(SECRET));
    assert!(f.text().contains("No codes in this group yet"), "{}", f.text());
}

#[test]
fn keys_choose_groups_and_codes_and_esc_steps_back() {
    let mut f = Fx::new(1280, 720, &[("A", "a", "Personal"), ("B", "b", "Personal"), ("C", "c", "Work")]);
    f.key(Key::Down);
    assert_eq!(f.app.sel, 1);
    f.key(Key::Down);
    assert_eq!(f.app.sel, 1, "stops at the last code");
    f.key(Key::Up);
    assert_eq!(f.app.sel, 0);
    f.key(Key::Left);
    assert_eq!(f.app.focus, Focus::Groups);
    f.key(Key::Down);
    assert_eq!(f.app.group_name(), "Work");
    f.key(Key::Down);
    assert_eq!(f.app.group_name(), "Work", "stops at the last group");
    f.key(Key::Enter);
    assert_eq!(f.app.focus, Focus::Codes);
    assert!(f.text().contains("C"));
    assert!(matches!(f.key(Key::Esc), Flow::Stay));
    assert_eq!(f.app.focus, Focus::Groups);
    assert!(matches!(f.key(Key::Esc), Flow::Back));
    f.key(Key::Tab);
    assert_eq!(f.app.focus, Focus::Codes);
}

#[test]
fn many_codes_scroll_and_the_chosen_one_stays_in_view() {
    let rows: Vec<(String, String)> = (0..30).map(|i| (format!("Site{i:02}"), format!("user{i}"))).collect();
    let entries: Vec<(&str, &str, &str)> = rows.iter().map(|(a, b)| (a.as_str(), b.as_str(), "Personal")).collect();
    let mut f = Fx::new(1280, 720, &entries);
    assert!(f.app.scroll.scrolls());
    for _ in 0..25 {
        f.key(Key::Down);
    }
    assert!(f.app.scroll.offset > 0.0);
    assert!(f.text().contains("Site25"), "the chosen code is drawn");
    f.key(Key::End);
    f.key(Key::Home);
    assert_eq!(f.app.scroll.offset, 0.0);
}

#[test]
fn a_failed_save_changes_nothing_and_says_so() {
    let mut f = Fx::two();
    f.vault.fail_save.set(true);
    f.key(Key::Char('a'));
    f.typ("Z");
    f.key(Key::Enter);
    f.key(Key::Enter);
    f.typ(SECRET);
    f.key(Key::Ctrl('s'));
    assert!(is_add(&f.app) && f.text().contains("could not be written"), "{}", f.text());
    assert_eq!(f.app.book.entries.len(), 2);
    f.key(Key::Esc);
    // the vault locks under the page: the next change says how to unlock
    f.vault.fail_save.set(false);
    f.vault.locked.set(true);
    f.click(Event::Tab(T_DEL));
    f.key(Key::Char('y'));
    f.draw();
    assert!(f.text().contains("vault is locked"), "{}", f.text());
}

#[test]
fn no_secret_is_ever_drawn_or_logged_in_a_whole_session() {
    let mut f = Fx::two();
    let mut seen = String::new();
    f.key(Key::Char('a'));
    f.typ("Bank");
    f.key(Key::Enter);
    f.typ("me");
    f.key(Key::Enter);
    f.typ("MFRGGZDFMZTWQ2LK");
    f.key(Key::Ctrl('s'));
    for _ in 0..3 {
        f.key(Key::Char('c'));
        seen.push_str(&f.text());
    }
    f.key(Key::Char('e'));
    seen.push_str(&f.text());
    f.key(Key::Esc);
    f.key(Key::Char('m'));
    seen.push_str(&f.text());
    f.key(Key::Esc);
    f.key(Key::Delete);
    seen.push_str(&f.text());
    f.key(Key::Esc);
    for secret in ["MFRGGZDFMZTWQ2LK", SECRET] {
        assert!(!seen.contains(secret), "{secret} was drawn");
        assert!(!f.audit().contains(secret), "{secret} was logged");
    }
    let code = f.code("MFRGGZDFMZTWQ2LK", Algo::Sha1, 6, 30);
    assert!(!f.audit().contains(&code), "a code was logged");
    // the app's own debug text hides the secret as well
    assert!(!format!("{:?}", f.app.book).contains("MFRGGZDF"));
}

#[test]
fn the_page_is_named_for_the_developer_state_and_the_hint_lists_the_keys() {
    let mut f = Fx::two();
    assert_eq!(f.app.page(), "group:personal");
    f.key(Key::Char('a'));
    assert_eq!(f.app.page(), "add");
    f.key(Key::Esc);
    f.vault.locked.set(true);
    f.app.reload();
    assert_eq!(f.app.page(), "locked");
    assert_eq!(f.app.title(), "One-time codes");
    assert!(f.app.hint().contains("Enter copy") && f.app.help().len() >= 5);
}

#[test]
fn the_page_redraws_when_the_second_changes() {
    let mut f = Fx::two();
    f.app.wants_redraw();
    assert!(!f.app.wants_redraw(), "nothing moved");
    f.time.set(f.time.get() + 1);
    assert!(f.app.wants_redraw(), "a new second: the codes and bars move");
}
