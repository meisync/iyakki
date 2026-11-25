//! The INBOX tile on a fake port (docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10): no `AppCx`, no `Services`, no session. The notes
//! and tasks are the real model store (`iyakki-inbox-model`) in a folder of its own, the calendar, the vault, the secret requests, the browser
//! questions, the device watch, the tool approvals and the audit log are in-memory fakes, the clock is the stores' `now` argument.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use iyakki_inbox_model::{Inbox, Kind, Level, Notification, Priority, Status, Task, TaskPatch, TaskSort};
use iyakki_tile_api::calendar::{Added, CalendarSource, NoCalendar, Repeat};
use iyakki_tile_api::inbox::{Answer, ConsentRequest, PendingApproval, SecretRequest};
use iyakki_uikit::gfx::{rgb, Gfx};
use iyakki_uikit::tilekit::Pixmap;
use iyakki_uikit::ui::{Event, Key, R};

use crate::app::*;
use crate::port::InboxPort;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-tile-inbox-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A calendar in UTC that keeps what was added to it.
struct FakeCalendar {
    added: Arc<Mutex<Vec<(String, i64)>>>,
}

impl CalendarSource for FakeCalendar {
    fn local(&self, unix: i64) -> iyakki_tile_api::calendar::Local {
        NoCalendar.local(unix)
    }
    fn local_to_unix(&self, y: i64, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> i64 {
        NoCalendar.local_to_unix(y, m, d, hh, mm, ss)
    }
    fn events_in(&self, _: &str, _: i64, _: i64) -> Vec<iyakki_tile_api::calendar::Occurrence> {
        vec![]
    }
    fn alarms_in(&self, _: &str, _: i64, _: i64) -> Vec<iyakki_tile_api::calendar::AlarmOccurrence> {
        vec![]
    }
    fn add_event(&self, _user: &str, title: &str, start: i64, _minutes: u32, _all_day: bool, _repeat: Repeat, _notes: &str, _by: &str, _remind: Option<u32>) -> Result<Added, String> {
        self.added.lock().unwrap().push((title.to_string(), start));
        Ok(Added { title: title.to_string(), start })
    }
    fn remove_event(&self, _: &str, _: &str) -> Result<(), String> {
        Err("no calendar".into())
    }
    fn alarms(&self, _: &str) -> Vec<iyakki_tile_api::calendar::Alarm> {
        vec![]
    }
    fn add_alarm(&self, _: &str, _: &str, _: u32, _: u32, _: u8, _: i64, _: &str) -> Result<iyakki_tile_api::calendar::Alarm, String> {
        Err("no calendar".into())
    }
    fn remove_alarm(&self, _: &str, _: &str) -> Result<(), String> {
        Err("no calendar".into())
    }
    fn set_alarm_enabled(&self, _: &str, _: &str, _: bool, _: i64) -> Result<(), String> {
        Err("no calendar".into())
    }
    fn next_alarm(&self, _: &str, _: i64) -> Option<iyakki_tile_api::calendar::AlarmOccurrence> {
        None
    }
}

struct Fake {
    admin: bool,
    inbox: Inbox,
    calendar: Arc<Mutex<Vec<(String, i64)>>>,
    requests: RefCell<Vec<SecretRequest>>,
    /// (vault name, value) of the secrets the person typed.
    secrets: RefCell<Vec<(String, String)>>,
    /// (request id, what became of it).
    outcomes: RefCell<Vec<(String, String)>>,
    consent: RefCell<Vec<ConsentRequest>>,
    trusted: RefCell<Vec<String>>,
    answered: RefCell<Vec<(String, Answer)>>,
    approvals: RefCell<Vec<PendingApproval>>,
    decided: RefCell<Vec<(String, bool)>>,
    log: RefCell<Vec<String>>,
}

impl Fake {
    fn new(tag: &str, admin: bool) -> Fake {
        let dir = temp_dir(tag);
        Fake { admin, inbox: Inbox::new(&dir.join("inbox")), calendar: Arc::new(Mutex::new(vec![])), requests: RefCell::new(vec![]), secrets: RefCell::new(vec![]), outcomes: RefCell::new(vec![]), consent: RefCell::new(vec![]), trusted: RefCell::new(vec![]), answered: RefCell::new(vec![]), approvals: RefCell::new(vec![]), decided: RefCell::new(vec![]), log: RefCell::new(vec![]) }
    }
    fn logged(&self, word: &str) -> bool {
        self.log.borrow().iter().any(|l| l.contains(word))
    }
    fn ask_secret(&self, name: &str, purpose: &str) -> String {
        let id = format!("sr{}", self.requests.borrow().len() + 1);
        self.requests.borrow_mut().push(SecretRequest { id: id.clone(), user: ME.into(), name: format!("svc.{name}"), purpose: purpose.into(), kind: "password".into(), since: 5 });
        id
    }
}

const ME: &str = "ann";

impl InboxPort for Fake {
    fn user(&self) -> String {
        ME.into()
    }
    fn is_admin(&self) -> bool {
        self.admin
    }
    fn audit(&self, action: &str, subject: &str) {
        self.log.borrow_mut().push(format!("{action} {subject}"));
    }
    fn maintain(&self, user: &str, now: i64) -> Result<(), String> {
        self.inbox.maintain(user, now)
    }
    fn take_notice(&self, user: &str) -> Option<String> {
        self.inbox.take_notice(user)
    }
    fn list(&self, user: &str) -> Vec<Notification> {
        self.inbox.list(user)
    }
    fn picture(&self, user: &str, id: &str) -> Option<Vec<u8>> {
        self.inbox.picture(user, id)
    }
    fn mark_read(&self, user: &str, id: &str, read: bool) -> Result<(), String> {
        self.inbox.mark_read(user, id, read)
    }
    fn mark_all_read(&self, user: &str) -> Result<usize, String> {
        self.inbox.mark_all_read(user)
    }
    fn flag(&self, user: &str, id: &str, flagged: bool) -> Result<(), String> {
        self.inbox.flag(user, id, flagged)
    }
    fn snooze(&self, user: &str, id: &str, until: i64) -> Result<(), String> {
        self.inbox.snooze(user, id, until)
    }
    fn level(&self, user: &str, source: &str) -> Level {
        self.inbox.level(user, source)
    }
    fn set_level(&self, user: &str, source: &str, level: Level) -> Result<(), String> {
        self.inbox.set_level(user, source, level)
    }
    fn delete(&self, user: &str, id: &str) -> Result<(), String> {
        self.inbox.delete(user, id)
    }
    fn clear_read(&self, user: &str) -> Result<usize, String> {
        self.inbox.clear_read(user)
    }
    fn tasks(&self, user: &str, sort: TaskSort) -> Vec<Task> {
        self.inbox.tasks(user, sort)
    }
    fn add_task(&self, user: &str, title: &str, notes: &str, due: Option<i64>, priority: Priority, by: &str, now: i64) -> Result<Task, String> {
        self.inbox.add_task(user, title, notes, due, priority, by, now)
    }
    fn update_task(&self, user: &str, id: &str, patch: TaskPatch, now: i64) -> Result<Task, String> {
        self.inbox.update_task(user, id, patch, now)
    }
    fn move_task(&self, user: &str, id: &str, status: Status, now: i64) -> Result<Task, String> {
        self.inbox.move_task(user, id, status, now)
    }
    fn delete_task(&self, user: &str, id: &str) -> Result<(), String> {
        self.inbox.delete_task(user, id)
    }
    fn calendar(&self) -> Box<dyn CalendarSource + '_> {
        Box::new(FakeCalendar { added: self.calendar.clone() })
    }
    fn secret_pending(&self, user: &str) -> Vec<SecretRequest> {
        self.requests.borrow().iter().filter(|r| r.user == user && !self.outcomes.borrow().iter().any(|o| o.0 == r.id)).cloned().collect()
    }
    fn secret_get(&self, user: &str, id: &str) -> Option<SecretRequest> {
        self.secret_pending(user).into_iter().find(|r| r.id == id)
    }
    fn answer_secret_request(&self, _user: &str, id: &str, name: &str, value: &str) -> Result<String, String> {
        self.secrets.borrow_mut().push((name.to_string(), value.to_string()));
        self.outcomes.borrow_mut().push((id.to_string(), format!("stored {name}")));
        Ok(name.to_string())
    }
    fn cancel_secret_request(&self, _user: &str, id: &str) -> Result<(), String> {
        self.outcomes.borrow_mut().push((id.to_string(), "cancelled".into()));
        Ok(())
    }
    fn consent_pending(&self, user: &str) -> Vec<ConsentRequest> {
        self.consent.borrow().iter().filter(|r| r.user == user).cloned().collect()
    }
    fn consent_get(&self, user: &str, id: &str) -> Option<ConsentRequest> {
        self.consent_pending(user).into_iter().find(|r| r.id == id)
    }
    fn consent_answer(&self, req: &ConsentRequest, answer: Answer) -> Result<(), String> {
        self.answered.borrow_mut().push((req.id.clone(), answer));
        self.consent.borrow_mut().retain(|r| r.id != req.id);
        Ok(())
    }
    fn devwatch_trust(&self, mac: &str) -> bool {
        self.trusted.borrow_mut().push(mac.to_string());
        true
    }
    fn approvals(&self, user: &str) -> Vec<PendingApproval> {
        self.approvals.borrow().iter().filter(|a| a.user == user).cloned().collect()
    }
    fn decide_approval(&self, _user: &str, _run: &str, call: &str, approve: bool) -> Result<PendingApproval, String> {
        let mut v = self.approvals.borrow_mut();
        let i = v.iter().position(|a| a.call_id == call).ok_or("already answered")?;
        self.decided.borrow_mut().push((call.to_string(), approve));
        Ok(v.remove(i))
    }
    fn decode_thumb(&self, _jpeg: &[u8]) -> Option<Pixmap> {
        None
    }
}

fn approval(call: &str) -> PendingApproval {
    PendingApproval { run_id: "run1".into(), user: ME.into(), prompt: "tidy the folder".into(), call_id: call.into(), tool: "write_file".into(), summary: "write notes.txt".into(), detail: "adds one line".into(), risk: "medium", since: 5 }
}

fn key(a: &mut InboxApp, p: &Fake, k: Key) {
    a.handle_with(&Event::Key(k), p);
}
fn typ(a: &mut InboxApp, p: &Fake, s: &str) {
    for c in s.chars() {
        key(a, p, Key::Char(c));
    }
}
fn draw(a: &mut InboxApp, p: &Fake, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    a.render_with(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), 1.0, p, 3);
    g
}

#[test]
fn notes_are_read_flagged_snoozed_and_opened_and_each_change_writes_its_audit_line() {
    let p = Fake::new("inbfk1", true);
    let now = iyakki_clock::now_unix();
    p.inbox.add(ME, Kind::Info, "oldest", "body", "x", None, now - 300).unwrap();
    p.inbox.add(ME, Kind::Device, "middle", "", "scan", Some(iyakki_inbox_model::Link::new("scan", "aa:bb")), now - 200).unwrap();
    p.inbox.add(ME, Kind::Reminder, "newest", "", "calendar", None, now - 100).unwrap();
    let mut a = InboxApp::with_port(&p);
    assert_eq!(a.notifications().iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), ["newest", "middle", "oldest"]);
    draw(&mut a, &p, 1280, 720);
    key(&mut a, &p, Key::Char('r'));
    assert!(a.notifications()[0].read);
    key(&mut a, &p, Key::Down);
    key(&mut a, &p, Key::Char('f'));
    assert!(a.notifications()[1].flagged);
    key(&mut a, &p, Key::Enter);
    assert_eq!(a.take_request(), Some(("scan".into(), "aa:bb".into())));
    key(&mut a, &p, Key::Char('s'));
    assert_eq!(a.notifications().len(), 2, "snoozed is hidden");
    for w in ["inbox.read", "inbox.flag", "inbox.open", "inbox.snooze"] {
        assert!(p.logged(w), "{w} missing from {:?}", p.log.borrow());
    }
    // delete asks first
    key(&mut a, &p, Key::Char('d'));
    assert!(a.dialog_open());
    key(&mut a, &p, Key::Char('n'));
    assert_eq!(a.notifications().len(), 2);
    key(&mut a, &p, Key::Char('d'));
    key(&mut a, &p, Key::Char('y'));
    assert_eq!(a.notifications().len(), 1);
    assert!(p.logged("inbox.delete"));
}

#[test]
fn a_task_is_added_through_the_form_moved_and_deleted_on_the_ports_store() {
    let p = Fake::new("inbfk2", true);
    let mut a = InboxApp::with_port(&p);
    a.handle_with(&Event::Tab(1), &p);
    assert_eq!(a.page(), Page::Tasks);
    key(&mut a, &p, Key::Char('n'));
    draw(&mut a, &p, 1280, 720);
    typ(&mut a, &p, "Pay rent");
    key(&mut a, &p, Key::Enter);
    typ(&mut a, &p, "2031-01-05");
    key(&mut a, &p, Key::Enter);
    key(&mut a, &p, Key::Right);
    key(&mut a, &p, Key::Enter);
    typ(&mut a, &p, "before the 5th");
    key(&mut a, &p, Key::Enter);
    assert!(!a.dialog_open(), "{:?}", a.status_text());
    let t = a.task_rows()[0].clone();
    assert_eq!((t.title.as_str(), t.priority, t.status, t.notes.as_str()), ("Pay rent", Priority::High, Status::Pending, "before the 5th"));
    assert!(p.logged("inbox.task.add"));
    // a bad date keeps the form open and says why
    key(&mut a, &p, Key::Char('n'));
    typ(&mut a, &p, "x");
    key(&mut a, &p, Key::Enter);
    typ(&mut a, &p, "not a date");
    key(&mut a, &p, Key::Enter);
    key(&mut a, &p, Key::Enter);
    key(&mut a, &p, Key::Enter);
    assert!(a.dialog_open(), "the form stays open for a date it cannot read");
    key(&mut a, &p, Key::Esc);
    assert_eq!(a.task_rows().len(), 1);
    // move it on, then delete it after asking
    key(&mut a, &p, Key::Enter);
    assert_eq!(a.task_rows()[0].status, Status::Doing);
    key(&mut a, &p, Key::Char('d'));
    key(&mut a, &p, Key::Char('y'));
    assert!(a.task_rows().is_empty());
}

#[test]
fn approvals_go_to_the_port_one_at_a_time_and_the_audit_line_keeps_the_reason() {
    let p = Fake::new("inbfk3", true);
    let mut a = InboxApp::with_port(&p);
    a.handle_with(&Event::Tab(2), &p);
    assert!(a.approval_rows().is_empty());
    p.approvals.borrow_mut().push(approval("c1"));
    a.handle_with(&Event::Tick, &p);
    assert_eq!(a.approval_rows().len(), 1);
    let g = draw(&mut a, &p, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Approve (y)") && g.hits.iter().any(|h| h.label == "Reject (n)"));
    key(&mut a, &p, Key::Char('y'));
    assert_eq!(p.decided.borrow().as_slice(), [("c1".to_string(), true)]);
    assert!(a.approval_rows().is_empty() && p.logged("inbox.approve write_file"));
    // reject asks for an optional reason and puts it in the audit line
    p.approvals.borrow_mut().push(approval("c2"));
    a.handle_with(&Event::Tick, &p);
    key(&mut a, &p, Key::Char('n'));
    assert!(a.dialog_open());
    typ(&mut a, &p, "not that folder");
    key(&mut a, &p, Key::Enter);
    assert!(a.approval_rows().is_empty());
    assert_eq!(p.decided.borrow().last(), Some(&("c2".to_string(), false)));
    assert!(p.log.borrow().iter().any(|l| l.starts_with("inbox.reject") && l.contains("not that folder")));
    // one answered elsewhere is reported, not repeated
    p.approvals.borrow_mut().push(approval("c3"));
    a.handle_with(&Event::Tick, &p);
    p.approvals.borrow_mut().clear();
    key(&mut a, &p, Key::Char('y'));
    assert!(a.status_text().unwrap_or("").contains("already answered"), "{:?}", a.status_text());
}

#[test]
fn a_secret_request_is_typed_by_the_person_goes_to_the_vault_and_is_never_logged() {
    let p = Fake::new("inbfk4", true);
    let mut a = InboxApp::with_port(&p);
    a.handle_with(&Event::Tab(2), &p);
    let ids: Vec<String> = ["nas", "key"].iter().map(|n| p.ask_secret(n, "SFTP password for the NAS")).collect();
    a.handle_with(&Event::Tick, &p);
    assert_eq!(a.approval_rows().len(), 2);
    assert!(a.approval_rows().iter().all(|r| r.tool == "vault_request" && r.risk == "low"));
    key(&mut a, &p, Key::Char('y'));
    assert!(a.dialog_open());
    key(&mut a, &p, Key::Tab);
    typ(&mut a, &p, "Sup3r-Secret!");
    key(&mut a, &p, Key::Ctrl('s'));
    assert!(!a.dialog_open(), "{:?}", a.status_text());
    assert_eq!(p.secrets.borrow().as_slice(), [("svc.nas".to_string(), "Sup3r-Secret!".to_string())]);
    assert_eq!(p.outcomes.borrow().iter().find(|o| o.0 == ids[0]).map(|o| o.1.as_str()), Some("stored svc.nas"));
    assert!(p.logged("vault.user.set svc.nas") && !p.log.borrow().iter().any(|l| l.contains("Sup3r-Secret!")));
    // n cancels the other one at once
    key(&mut a, &p, Key::Char('n'));
    assert!(a.approval_rows().is_empty());
    assert_eq!(p.outcomes.borrow().iter().find(|o| o.0 == ids[1]).map(|o| o.1.as_str()), Some("cancelled"));
}

#[test]
fn only_an_administrator_trusts_new_devices_and_every_page_fits_the_sizes() {
    let p = Fake::new("inbfk5", false);
    let now = iyakki_clock::now_unix();
    p.inbox.add(ME, Kind::Device, "New device", "aa:bb:cc:dd:ee:01", "scan", None, now).unwrap();
    let mut a = InboxApp::with_port(&p);
    for (w, h) in [(1280u32, 720u32), (800, 600), (480, 400)] {
        for page in 0..3 {
            a.handle_with(&Event::Tab(page), &p);
            draw(&mut a, &p, w, h);
        }
    }
    a.handle_with(&Event::Tab(0), &p);
    assert!(!p.logged("inbox.trust_devices"), "nothing trusted by looking");
    assert_eq!(a.page(), Page::Notifications);
}

#[test]
fn a_note_is_put_in_the_calendar_through_the_ports_calendar_and_the_audit_line_names_it() {
    let p = Fake::new("inbfk6", true);
    let now = iyakki_clock::now_unix();
    p.inbox.add(ME, Kind::Info, "dentist", "bring the card", "calendar", None, now).unwrap();
    let mut a = InboxApp::with_port(&p);
    draw(&mut a, &p, 1280, 720);
    key(&mut a, &p, Key::Char('c'));
    assert!(a.dialog_open());
    // the form starts with the note's title; the date is tomorrow at 09:00 in the port's zone
    key(&mut a, &p, Key::Enter);
    typ(&mut a, &p, "");
    key(&mut a, &p, Key::Enter);
    key(&mut a, &p, Key::Enter);
    assert!(!a.dialog_open(), "{:?}", a.status_text());
    let added = p.calendar.lock().unwrap().clone();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].0, "dentist");
    assert!(added[0].1 > now, "tomorrow, after now");
    assert!(p.logged("inbox.calendar dentist"), "{:?}", p.log.borrow());
}

#[test]
fn a_question_about_the_browser_is_an_approval_row_answered_by_a_key_through_the_port() {
    use iyakki_tile_api::inbox::{ConsentChoice, ConsentKind};
    let p = Fake::new("inbfk7", true);
    p.consent.borrow_mut().push(ConsentRequest {
        id: "q1".into(),
        user: ME.into(),
        kind: ConsentKind::Look,
        domain: "example.com".into(),
        title: "Look at example.com?".into(),
        text: String::new(),
        detail: String::new(),
        choices: vec![ConsentChoice { answer: Answer::Allow, label: "Allow".into() }, ConsentChoice { answer: Answer::Deny, label: "Deny".into() }],
        since: 5,
        payload: String::new(),
        quiet: false,
    });
    let mut a = InboxApp::with_port(&p);
    a.handle_with(&Event::Tab(2), &p);
    assert_eq!(a.approval_rows().len(), 1);
    assert!(a.approval_rows()[0].tool == "browser_consent" && a.approval_rows()[0].summary.contains("example.com"));
    key(&mut a, &p, Key::Char('y'));
    assert!(a.dialog_open());
    draw(&mut a, &p, 1280, 720);
    key(&mut a, &p, Key::Char('1'));
    assert_eq!(p.answered.borrow().as_slice(), [("q1".to_string(), Answer::Allow)]);
    assert!(a.approval_rows().is_empty());
}
