//! INBOX in the shell: the tile is the crate `iyakki-tile-inbox` (alpha.9 P5). This file gives it the shell's context: on every call it builds the
//! real port (`inbox_port::CxPort`: the member stores, the vault's secret requests, the browser questions, the device watch, the approvals, the
//! audit log) and forwards the `App` methods. The tile's own tests run in the crate against a fake port; the tests here run it over the real
//! services. `Deref` shows the tile to the tests.

use std::ops::{Deref, DerefMut};

use super::inbox_port::CxPort;
use super::{App, AppCx, Flow as ShellFlow};
use crate::gfx::Gfx;
use crate::ui::{Event, R};

pub use iyakki_tile_inbox::app::*;

pub struct InboxApp(iyakki_tile_inbox::InboxApp);

impl Deref for InboxApp {
    type Target = iyakki_tile_inbox::InboxApp;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for InboxApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl InboxApp {
    pub fn new(cx: &AppCx) -> InboxApp {
        InboxApp(iyakki_tile_inbox::InboxApp::with_port(&CxPort(cx)))
    }

    /// The page showing (an inherent method, so it wins over the shell's `App::page` text).
    pub fn page(&self) -> Page {
        self.0.page()
    }
}

impl App for InboxApp {
    fn dialog_title(&self) -> Option<String> {
        self.0.dialog_title()
    }
    fn title(&self) -> &'static str {
        self.0.title()
    }
    fn icon(&self) -> crate::gfx::Icon {
        self.0.icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.0.accent()
    }
    fn hint(&self) -> String {
        self.0.hint()
    }
    fn help(&self) -> Vec<String> {
        self.0.help()
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.0.take_request()
    }
    fn context_items(&mut self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        self.0.context_items(x, y)
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> ShellFlow {
        match self.0.handle_with(ev, &CxPort(cx)) {
            iyakki_tile_inbox::Flow::Stay => ShellFlow::Stay,
            iyakki_tile_inbox::Flow::Back => ShellFlow::Back,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.0.render_with(g, area, s, &CxPort(cx), tick);
    }
}

#[cfg(test)]
mod tests {
    use iyakki_agent::facets_h::RealServices as _;
    use super::*;
    use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
    use iyakki_shared::inbox::{Kind, Link, Priority, Status};
    use iyakki_tile_inbox::app::Page;
    use crate::ui::Key;
    use crate::gfx::rgb;
    use crate::apps::Flow;
    use iyakki_clock::Zone;
    use std::sync::Arc;

    fn admin_cx(tag: &str) -> AppCx {
        let mut cx = cx_with_admin_store(tag);
        let d = temp_dir(&format!("{tag}-svc"));
        cx.services = services_in(&d, Arc::new(Offline));
        cx.services.on_login("root", "rootpass12", true);
        cx
    }

    fn key(a: &mut InboxApp, cx: &AppCx, k: Key) {
        a.handle(&Event::Key(k), cx);
    }
    fn typ(a: &mut InboxApp, cx: &AppCx, s: &str) {
        for c in s.chars() {
            key(a, cx, Key::Char(c));
        }
    }
    fn draw(a: &mut InboxApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(9, 11, 20));
        a.render(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), 1.0, cx, 3);
        g
    }
    fn click(a: &mut InboxApp, cx: &AppCx, g: &Gfx, label: &str) {
        let h = g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("no control `{label}` in {:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>()));
        a.handle(&h.ev.clone(), cx);
    }
    fn audit(cx: &AppCx) -> String {
        std::fs::read_to_string(cx.audit.path_buf()).unwrap_or_default()
    }

    #[test]
    fn notifications_are_read_flagged_snoozed_deleted_cleared_and_opened() {
        let cx = admin_cx("inb1");
        let now = iyakki_clock::now_unix();
        let ib = &cx.inbox();
        ib.add("root", Kind::Info, "oldest", "body a", "x", None, now - 300).unwrap();
        ib.add("root", Kind::Device, "middle", "", "scan", Some(Link::new("scan", "aa:bb")), now - 200).unwrap();
        ib.add("root", Kind::Reminder, "newest", "", "calendar", Some(Link::new("calendar", "ev1")), now - 100).unwrap();
        let mut a = InboxApp::new(&cx);
        assert_eq!(a.notifications().iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), vec!["newest", "middle", "oldest"]);
        draw(&mut a, &cx, 1280, 720);
        // read / unread toggles
        key(&mut a, &cx, Key::Char('r'));
        assert!(a.notifications()[0].read);
        key(&mut a, &cx, Key::Char('r'));
        assert!(!a.notifications()[0].read);
        // flag the middle one
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Char('f'));
        assert!(a.notifications()[1].flagged);
        // Enter opens the linked app and marks it read
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.take_request(), Some(("scan".into(), "aa:bb".into())));
        assert!(a.notifications()[1].read);
        assert_eq!(a.take_request(), None);
        // a reminder opens in the calendar when asked to add it
        key(&mut a, &cx, Key::Up);
        key(&mut a, &cx, Key::Char('c'));
        assert_eq!(a.take_request().map(|r| r.0), Some("calendar".into()));
        // snooze hides it
        key(&mut a, &cx, Key::Char('s'));
        assert_eq!(a.notifications().len(), 2);
        assert_eq!(cx.inbox().counts("root", now).unread, 1, "snoozed is not counted");
        // delete asks first
        key(&mut a, &cx, Key::Char('d'));
        assert!(a.dialog_open());
        key(&mut a, &cx, Key::Char('n'));
        assert_eq!(a.notifications().len(), 2);
        key(&mut a, &cx, Key::Char('d'));
        key(&mut a, &cx, Key::Char('y'));
        assert_eq!(a.notifications().len(), 1);
        // mark all read, then clear read keeps flagged ones (the flagged one was deleted above? it is the second row)
        key(&mut a, &cx, Key::Char('A'));
        assert!(a.notifications().iter().all(|n| n.read));
        key(&mut a, &cx, Key::Char('X'));
        key(&mut a, &cx, Key::Char('y'));
        let left: Vec<_> = a.notifications().iter().map(|n| n.flagged).collect();
        assert!(left.iter().all(|f| *f), "only flagged notes survive Clear read: {left:?}");
        let log = audit(&cx);
        for w in ["inbox.read", "inbox.flag", "inbox.open", "inbox.snooze", "inbox.delete", "inbox.read_all"] {
            assert!(log.contains(w), "{w} missing from {log}");
        }
        for (w, h) in [(1280u32, 720u32), (800, 600), (480, 400)] {
            draw(&mut a, &cx, w, h);
        }
    }

    #[test]
    fn a_notification_becomes_a_calendar_event_at_the_chosen_time() {
        let cx = admin_cx("inb2");
        cx.notify("root", Kind::Download, "Model downloaded", "qwen is ready", None).unwrap();
        let mut a = InboxApp::new(&cx);
        key(&mut a, &cx, Key::Char('c'));
        assert!(a.dialog_open());
        draw(&mut a, &cx, 1280, 720);
        // title is prefilled, the date is tomorrow, the time 09:00; change the time
        key(&mut a, &cx, Key::Tab);
        key(&mut a, &cx, Key::Tab);
        for _ in 0..5 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "14:30");
        key(&mut a, &cx, Key::Enter);
        assert!(!a.dialog_open(), "{:?}", a.status_text());
        let ev = cx.calendar().events("root");
        assert_eq!(ev.len(), 1);
        assert_eq!((ev[0].title.as_str(), ev[0].notes.as_str(), ev[0].by.as_str()), ("Model downloaded", "qwen is ready", "user"));
        let zone = Zone::system();
        let (l, today) = (zone.local(ev[0].start), zone.local(iyakki_clock::now_unix() + 86_400));
        assert_eq!((l.year, l.month, l.day, l.hour, l.minute), (today.year, today.month, today.day, 14, 30));
        assert!(audit(&cx).contains("inbox.calendar"));
        // a bad time stays in the form with the reason
        key(&mut a, &cx, Key::Char('c'));
        key(&mut a, &cx, Key::Tab);
        key(&mut a, &cx, Key::Tab);
        for _ in 0..5 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "late");
        key(&mut a, &cx, Key::Enter);
        assert!(a.dialog_open());
        key(&mut a, &cx, Key::Esc);
        assert!(!a.dialog_open());
    }

    #[test]
    fn tasks_are_added_moved_through_their_states_edited_filtered_and_deleted() {
        let cx = admin_cx("inb3");
        let mut a = InboxApp::new(&cx);
        a.handle(&Event::Tab(1), &cx);
        assert_eq!(a.page(), Page::Tasks);
        key(&mut a, &cx, Key::Char('n'));
        draw(&mut a, &cx, 1280, 720);
        typ(&mut a, &cx, "Pay rent");
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "2031-01-05");
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Right); // priority: high
        key(&mut a, &cx, Key::Enter);
        typ(&mut a, &cx, "before the 5th");
        key(&mut a, &cx, Key::Enter);
        assert!(!a.dialog_open(), "{:?}", a.status_text());
        let t = a.task_rows()[0].clone();
        assert_eq!((t.title.as_str(), t.priority, t.status, t.created_by.as_str(), t.notes.as_str()), ("Pay rent", Priority::High, Status::Pending, "user", "before the 5th"));
        assert!(t.due.is_some_and(|d| d > iyakki_clock::now_unix()), "a date alone means the end of that day");
        // a second task
        key(&mut a, &cx, Key::Char('n'));
        typ(&mut a, &cx, "Water plants");
        for _ in 0..4 {
            key(&mut a, &cx, Key::Enter);
        }
        assert_eq!(a.task_rows().len(), 2);
        // move the selected one: pending → doing → done, and back
        let id = a.task_rows()[a.selected()].id.clone();
        let status = |a: &InboxApp| a.task_rows().iter().find(|t| t.id == id).unwrap().status;
        key(&mut a, &cx, Key::Enter);
        assert_eq!(status(&a), Status::Doing);
        key(&mut a, &cx, Key::Right);
        assert_eq!(status(&a), Status::Done);
        key(&mut a, &cx, Key::Right);
        assert_eq!(status(&a), Status::Done, "done stays done");
        key(&mut a, &cx, Key::Left);
        assert_eq!(status(&a), Status::Doing);
        draw(&mut a, &cx, 1280, 720);
        // rows are grouped pending, doing, done
        let order: Vec<Status> = a.task_rows().iter().map(|t| t.status).collect();
        let mut sorted = order.clone();
        sorted.sort_by_key(|s| match s {
            Status::Pending => 0,
            Status::Doing => 1,
            Status::Done => 2,
            Status::Dropped => 3,
        });
        assert_eq!(order, sorted);
        // priority cycles, edit changes the title
        let before = a.task().unwrap().priority;
        key(&mut a, &cx, Key::Char('p'));
        assert_eq!(a.task().unwrap().priority, before.cycle());
        key(&mut a, &cx, Key::Char('e'));
        typ(&mut a, &cx, " now");
        key(&mut a, &cx, Key::Char('s').clone());
        key(&mut a, &cx, Key::Esc);
        assert!(!a.dialog_open());
        // filter: open / done / all
        key(&mut a, &cx, Key::Char('f'));
        assert_eq!(a.filter(), Filter::Open);
        assert!(a.task_rows().iter().all(|t| t.status.is_open()));
        key(&mut a, &cx, Key::Char('f'));
        assert_eq!(a.filter(), Filter::Done);
        assert!(a.task_rows().iter().all(|t| !t.status.is_open()));
        key(&mut a, &cx, Key::Char('f'));
        assert_eq!(a.filter(), Filter::All);
        // to the calendar from a task
        key(&mut a, &cx, Key::Char('c'));
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Enter);
        assert_eq!(cx.calendar().events("root").len(), 1, "{:?}", a.status_text());
        // drop, delete (asks first)
        key(&mut a, &cx, Key::Char('x'));
        assert!(a.task_rows().iter().any(|t| t.status == Status::Dropped));
        let n = a.task_rows().len();
        key(&mut a, &cx, Key::Char('d'));
        key(&mut a, &cx, Key::Char('y'));
        assert_eq!(a.task_rows().len(), n - 1);
        assert!(audit(&cx).contains("inbox.task"));
    }

    // a model that asks for one risky tool, then answers
    struct Script(std::sync::atomic::AtomicUsize);
    impl iyakki_shared::chat::ChatBackend for Script {
        fn complete(&self, _: &[iyakki_shared::chat::Message]) -> Result<String, String> {
            Err("unused".into())
        }
        fn complete_tools(&self, _: &[iyakki_shared::chat::Message], _: &[iyakki_shared::chat::ToolSpec]) -> Result<iyakki_shared::chat::ModelTurn, String> {
            if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Ok(iyakki_shared::chat::ModelTurn { text: String::new(), calls: vec![iyakki_shared::chat::ToolCall { id: "c1".into(), name: "write_file".into(), args: serde_json::json!({}) }] })
            } else {
                Ok(iyakki_shared::chat::ModelTurn { text: "finished".into(), calls: vec![] })
            }
        }
    }
    struct WriteTools;
    impl iyakki_shared::agent::Tools for WriteTools {
        fn defs(&self) -> Vec<iyakki_shared::agent::ToolDef> {
            vec![iyakki_shared::agent::ToolDef { spec: iyakki_shared::chat::ToolSpec { name: "write_file".into(), description: "w".into(), schema: serde_json::json!({"type": "object"}) }, risk: iyakki_shared::agent::Risk::Ask, untrusted: false }]
        }
        fn call(&self, _: &str, _: &serde_json::Value) -> Result<String, String> {
            Ok("written".into())
        }
        fn describe(&self, _: &str, _: &serde_json::Value) -> String {
            "write 3 characters to /home/root/n.txt".into()
        }
    }

    fn waiting_run(cx: &AppCx) -> iyakki_agent::runs::RunSession {
        let mut run = iyakki_agent::runs::RunSession::start(Some(cx.services.real().runs.clone()), "root", "save my note", Arc::new(Script(Default::default())), Arc::new(WriteTools), vec![], Default::default());
        let __wall = std::time::Instant::now();
        while __wall.elapsed() < std::time::Duration::from_secs(30) {
            run.poll();
            if !cx.services.approvals("root").is_empty() {
                return run;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("no approval appeared");
    }

    fn finish(run: &mut iyakki_agent::runs::RunSession) -> Vec<iyakki_agent::agent::RunEvent> {
        let mut seen = vec![];
        let __wall = std::time::Instant::now();
        while __wall.elapsed() < std::time::Duration::from_secs(30) {
            seen.extend(run.poll());
            if run.is_done() {
                return seen;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("run did not finish");
    }

    #[test]
    fn approvals_are_approved_or_rejected_one_at_a_time_and_audited() {
        use iyakki_agent::agent::RunEvent;
        let cx = admin_cx("inb4");
        let mut a = InboxApp::new(&cx);
        a.handle(&Event::Tab(2), &cx);
        draw(&mut a, &cx, 1280, 720);
        assert!(a.approval_rows().is_empty());
        // approve with y
        let mut run = waiting_run(&cx);
        a.handle(&Event::Tick, &cx);
        assert_eq!(a.approval_rows().len(), 1);
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Approve (y)") && g.hits.iter().any(|h| h.label == "Reject (n)"));
        // details first
        key(&mut a, &cx, Key::Enter);
        assert!(a.dialog_open());
        draw(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Esc);
        assert!(!a.dialog_open());
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.approval_rows().is_empty());
        let seen = finish(&mut run);
        assert!(seen.iter().any(|e| matches!(e, RunEvent::ToolResult { ok: true, text, .. } if text == "written")));
        assert!(seen.iter().any(|e| matches!(e, RunEvent::Final { text } if text == "finished")));
        // reject with a reason, by clicking
        let mut run = waiting_run(&cx);
        a.handle(&Event::Tick, &cx);
        let g = draw(&mut a, &cx, 1280, 720);
        click(&mut a, &cx, &g, "Reject (n)");
        assert!(a.dialog_open(), "rejecting asks for an optional reason");
        draw(&mut a, &cx, 1280, 720);
        typ(&mut a, &cx, "not that folder");
        key(&mut a, &cx, Key::Enter);
        assert!(!a.dialog_open() && a.approval_rows().is_empty());
        let seen = finish(&mut run);
        assert!(seen.iter().any(|e| matches!(e, RunEvent::Decision { approved: false, .. })));
        assert!(!seen.iter().any(|e| matches!(e, RunEvent::ToolResult { ok: true, .. })), "the tool must not run");
        let log = audit(&cx);
        assert!(log.contains("inbox.approve") && log.contains("inbox.reject") && log.contains("not that folder") && log.contains("write_file"), "{log}");
        // a request that was answered elsewhere is reported, not repeated
        let mut run = waiting_run(&cx);
        a.handle(&Event::Tick, &cx);
        let p = a.approval_rows()[0].clone();
        run.decide(&p.call_id, iyakki_shared::agent::Verdict::Deny);
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.status_text().unwrap_or("").contains("already answered"), "{:?}", a.status_text());
        finish(&mut run);
    }

    #[test]
    fn pages_switch_with_tab_numbers_and_clicks_and_esc_leaves() {
        let cx = admin_cx("inb5");
        let mut a = InboxApp::new(&cx);
        assert_eq!(a.page(), Page::Notifications);
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.page(), Page::Tasks);
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.page(), Page::Approvals);
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.page(), Page::Notifications);
        key(&mut a, &cx, Key::Char('3'));
        assert_eq!(a.page(), Page::Approvals);
        key(&mut a, &cx, Key::Char('2'));
        assert_eq!(a.page(), Page::Tasks);
        let g = draw(&mut a, &cx, 1280, 720);
        click(&mut a, &cx, &g, "Notifications");
        assert_eq!(a.page(), Page::Notifications);
        // clicking a row selects it; the action buttons act
        cx.notify("root", Kind::Info, "one", "", None).unwrap();
        cx.notify("root", Kind::Info, "two", "", None).unwrap();
        a.handle(&Event::Tick, &cx);
        let g = draw(&mut a, &cx, 1280, 720);
        let id = a.notifications()[1].id.clone();
        click(&mut a, &cx, &g, &id);
        assert_eq!(a.selected(), 1);
        click(&mut a, &cx, &g, "Flag (f)");
        assert!(a.notifications()[1].flagged);
        assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
    }

    #[test]
    fn a_secret_the_assistant_asks_for_is_typed_by_the_person_into_the_vault_and_never_logged() {
        let cx = admin_cx("inbsec");
        let mut a = InboxApp::new(&cx);
        a.handle(&Event::Tab(2), &cx);
        let ids: Vec<String> = ["nas", "key"].iter().map(|n| cx.secret_requests().create("root", n, "SFTP password for the NAS", "password", 5).unwrap()).collect();
        a.handle(&Event::Tick, &cx);
        assert_eq!(a.approval_rows().len(), 2);
        assert!(a.approval_rows().iter().all(|r| r.tool == "vault_request" && r.risk == "low"));
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Enter secret (y)"), "the special item has its own action");
        // y opens the masked form with the suggested name; a wrong name keeps it open and stores nothing
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.dialog_open());
        draw(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Tab);
        typ(&mut a, &cx, "Sup3r-Secret!");
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(!g.hits.is_empty());
        key(&mut a, &cx, Key::Ctrl('s'));
        assert!(!a.dialog_open(), "{:?}", a.status_text());
        assert_eq!(cx.vault().user_get("root", "svc.nas").unwrap().as_str(), "Sup3r-Secret!");
        assert_eq!(cx.secret_requests().wait(&ids[0], std::time::Duration::from_millis(20)), Some(iyakki_agent::secretreq::Outcome::Stored("svc.nas".into())));
        let log = audit(&cx);
        assert!(log.contains("vault.user.set") && log.contains("svc.nas") && !log.contains("Sup3r-Secret!"), "{log}");
        assert_eq!(a.approval_rows().len(), 1);
        // n says no at once: the assistant is told it was cancelled
        key(&mut a, &cx, Key::Char('n'));
        assert!(a.approval_rows().is_empty() && !a.dialog_open());
        assert_eq!(cx.secret_requests().wait(&ids[1], std::time::Duration::from_millis(20)), Some(iyakki_agent::secretreq::Outcome::Cancelled));
        assert!(cx.vault().user_get("root", "svc.key").is_err());
        // a locked vault is explained in the form and nothing is lost
        let cx2 = { let mut c = cx_with_admin_store("inbsec2"); c.services = services_in(&temp_dir("inbsec2-svc"), Arc::new(Offline)); c };
        let id = cx2.services.secret_requests().create("root", "x", "y", "token", 1).unwrap();
        let mut b = InboxApp::new(&cx2);
        b.handle(&Event::Tab(2), &cx2);
        key(&mut b, &cx2, Key::Char('y'));
        key(&mut b, &cx2, Key::Tab);
        typ(&mut b, &cx2, "abc12345");
        key(&mut b, &cx2, Key::Ctrl('s'));
        assert!(b.dialog_open() && cx2.services.secret_requests().get("root", &id).is_some(), "still waiting");
    }

    #[test]
    fn questions_about_the_assistant_in_the_browser_are_answered_in_the_approvals_page() {
        use iyakki_shared::browserconsent::{Answer, ConsentKind};
        let cx = admin_cx("inbbrowser");
        let mut a = InboxApp::new(&cx);
        a.handle(&Event::Tab(2), &cx);
        let c = &cx.browser_consent();
        let look = c.create("root", ConsentKind::Look, "gemini.google.com", "", "", 1).unwrap();
        let act = c.create("root", ConsentKind::Interact, "gemini.google.com", &iyakki_shared::browserconsent::willingness_text("gemini.google.com", None), "click button \"Send\"", 2).unwrap();
        a.handle(&Event::Tick, &cx);
        let rows: Vec<_> = a.approval_rows().iter().filter(|r| r.tool == "browser_consent").collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].prompt, "Let the assistant look at gemini.google.com in your browser?");
        assert!(rows[1].summary.starts_with("Are you willing to let the assistant act on your behalf on gemini.google.com?"), "{}", rows[1].summary);
        // Enter opens the question; the numbered choices answer it
        key(&mut a, &cx, Key::Enter);
        assert!(a.dialog_open());
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "1  Allow") && g.hits.iter().any(|h| h.label == "3  Deny"));
        key(&mut a, &cx, Key::Char('1'));
        assert!(!a.dialog_open());
        assert_eq!(c.wait(&look, std::time::Duration::from_millis(20)), Some(Answer::Allow));
        // the willingness question: three choices, exactly
        assert_eq!(a.approval_rows().iter().filter(|r| r.tool == "browser_consent").count(), 1);
        key(&mut a, &cx, Key::Enter);
        let g = draw(&mut a, &cx, 1280, 720);
        let numbered: Vec<_> = g.hits.iter().filter(|h| h.label.starts_with(|ch: char| ch.is_ascii_digit())).map(|h| h.label.clone()).collect();
        assert_eq!(numbered, vec!["1  Approve all actions on gemini.google.com for this session", "2  Approve one by one", "3  Deny"]);
        key(&mut a, &cx, Key::Esc);
        assert_eq!(c.wait(&act, std::time::Duration::from_millis(20)), Some(Answer::Deny), "Esc is no");
        let log = audit(&cx);
        assert!(log.contains("browser.agent.answer") && log.contains("look allowed") && log.contains("interaction denied"), "{log}");
        // n answers no without opening anything
        let step = c.create("root", ConsentKind::Step, "gemini.google.com", "t", "", 3).unwrap();
        a.handle(&Event::Tick, &cx);
        key(&mut a, &cx, Key::Char('n'));
        assert_eq!(c.wait(&step, std::time::Duration::from_millis(20)), Some(Answer::Deny));
    }

    const NEWDEV: &str = "New device on your network";

    fn nb(ip: &str, mac: &str) -> iyakki_shared::devwatch::Neighbor {
        iyakki_shared::devwatch::Neighbor { ip: ip.into(), mac: mac.into(), iface: "eth0".into() }
    }

    #[test]
    fn a_crowd_of_new_devices_is_one_collapsed_row_that_opens_a_list_and_trusts_or_ignores_them() {
        let cx = admin_cx("inb_grp");
        let now = iyakki_clock::now_unix();
        let dw = &cx.devwatch();
        dw.observe(&[nb("10.0.0.1", "aa:aa:aa:aa:aa:01")], 1);
        let mut seen = vec![nb("10.0.0.1", "aa:aa:aa:aa:aa:01")];
        for i in 2..42 {
            seen.push(nb(&format!("10.0.0.{i}"), &format!("bb:bb:bb:bb:bb:{i:02x}")));
        }
        for d in dw.observe(&seen, 2) {
            cx.notify("root", Kind::Device, NEWDEV, &format!("{} ({}) appeared.", d.ip, d.mac), Some(Link::new("scan", &d.mac))).unwrap();
        }
        assert_eq!(dw.untrusted_count(), 40);
        let mut a = InboxApp::new(&cx);
        assert_eq!(a.notifications().len(), 1, "forty devices, one row");
        assert_eq!(a.notifications()[0].title, "40 new devices on your network");
        // collapsed rows draw at every size, with the count and the newest line
        for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
            let g = draw(&mut a, &cx, w, h);
            assert!(g.hits.iter().any(|x| x.label == "Clear group (g)" || x.label == "Mute source (m)" || x.label.len() > 4), "{w}x{h}");
        }
        // Enter opens the list instead of leaving the inbox; the group counts as looked at
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.open_group(), Some(a.notifications()[0].id.as_str()));
        assert_eq!(a.take_request(), None);
        for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
            let g = draw(&mut a, &cx, w, h);
            assert!(g.hits.iter().any(|x| matches!(x.ev, Event::Key(Key::Esc))), "a Close button at {w}x{h}");
            assert!(g.hits.iter().filter(|x| matches!(x.ev, Event::Row(_))).count() >= 1);
        }
        // a click on a row selects it; Enter opens the device in NET SCAN
        let g = draw(&mut a, &cx, 1280, 720);
        let row = g.hits.iter().find(|x| matches!(x.ev, Event::Row(1))).expect("a second line").ev.clone();
        a.handle(&row, &cx);
        key(&mut a, &cx, Key::Enter);
        let (app, id) = a.take_request().expect("opens NET SCAN");
        assert_eq!(app, "scan");
        assert!(id.starts_with("bb:bb:bb:bb:bb:"), "{id}");
        assert!(a.open_group().is_none());
        // Trust all (t) from the list
        key(&mut a, &cx, Key::Right);
        assert!(a.open_group().is_some());
        key(&mut a, &cx, Key::Char('t'));
        assert_eq!(dw.untrusted_count(), 0);
        assert!(a.status_text().unwrap_or("").contains("Trusted 40"), "{:?}", a.status_text());
        assert!(audit(&cx).contains("inbox.trust_devices"));
        // Ignore (i) only marks it read
        cx.notify("root", Kind::Device, NEWDEV, "10.0.0.99 (cc) appeared.", Some(Link::new("scan", "cc:cc:cc:cc:cc:99"))).unwrap();
        cx.notify("root", Kind::Device, NEWDEV, "10.0.0.98 (cd) appeared.", Some(Link::new("scan", "cc:cc:cc:cc:cc:98"))).unwrap();
        a.handle(&Event::Key(Key::Esc), &cx);
        a.handle(&Event::Tick, &cx);
        assert_eq!(a.notifications().len(), 2, "looked-at groups stay apart from new arrivals: {:?}", a.notifications().iter().map(|n| &n.title).collect::<Vec<_>>());
        key(&mut a, &cx, Key::Char('i'));
        assert!(a.notifications()[0].read && !a.notifications()[0].items.is_empty());
        assert!(audit(&cx).contains("inbox.ignore"));
        let _ = now;
    }

    #[test]
    fn keys_clear_a_group_and_mute_a_source_in_three_steps_and_the_choice_is_kept_per_person() {
        let cx = admin_cx("inb_mute");
        let ib = &cx.inbox();
        let now = iyakki_clock::now_unix();
        for i in 0..5 {
            ib.add("root", Kind::Agent, &format!("Job \u{201c}Setup {i}\u{201d} finished"), "ok", "runs", Some(Link::new("runs", "jobs")), now).unwrap();
        }
        ib.add("root", Kind::Info, "Hello", "", "system", None, now - 10).unwrap();
        let mut a = InboxApp::new(&cx);
        assert_eq!(a.notifications().len(), 2);
        assert_eq!(a.notifications()[0].title, "5 setup jobs finished");
        // m: Show -> Quiet -> Off -> Show
        key(&mut a, &cx, Key::Char('m'));
        assert_eq!(ib.level("root", "runs").name(), "quiet");
        assert!(a.status_text().unwrap_or("").contains("Quiet"), "{:?}", a.status_text());
        assert_eq!(ib.counts("root", now).other, 1, "the quiet source no longer shows as a dot");
        key(&mut a, &cx, Key::Char('m'));
        assert_eq!(ib.level("root", "runs").name(), "off");
        key(&mut a, &cx, Key::Char('m'));
        assert_eq!(ib.level("root", "runs").name(), "show");
        assert_eq!(ib.level("bob", "runs").name(), "show", "per person");
        // g asks, then clears the whole group but not the other note
        key(&mut a, &cx, Key::Char('g'));
        assert!(a.dialog_open());
        key(&mut a, &cx, Key::Char('n'));
        assert_eq!(a.notifications().len(), 2);
        key(&mut a, &cx, Key::Char('g'));
        key(&mut a, &cx, Key::Char('y'));
        assert_eq!(a.notifications().iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), vec!["Hello"]);
        // every key has a button, at every size
        for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
            let g = draw(&mut a, &cx, w, h);
            assert!(g.hits.iter().any(|x| matches!(x.ev, Event::Key(Key::Char('m')))), "Mute at {w}x{h}");
        }
        let log = audit(&cx);
        assert!(log.contains("inbox.level") && log.contains("inbox.delete"), "{log}");
    }

    #[test]
    fn an_old_inbox_file_is_grouped_when_the_panel_opens_and_the_panel_says_so_once() {
        let d = temp_dir("inb_old");
        let mut cx = cx_with_admin_store("inb_old");
        cx.services = services_in(&d, Arc::new(Offline));
        cx.services.on_login("root", "rootpass12", true);
        let now = iyakki_clock::now_unix();
        let mut notes = Vec::new();
        for i in 0..79 {
            notes.push(serde_json::json!({"id": format!("d{i}"), "time": now - 100 + i, "kind": "device", "title": NEWDEV, "body": format!("10.0.0.{i} (m{i}) appeared."), "source": "scan", "link": {"app": "scan", "id": format!("m{i}")}, "read": false, "flagged": false, "snoozed_until": 0}));
        }
        for i in 0..41 {
            notes.push(serde_json::json!({"id": format!("j{i}"), "time": now - 200 + i, "kind": "agent", "title": format!("Job \u{201c}Install Python 3.12 ({i}/12)\u{201d} finished"), "body": "", "source": "runs", "read": false, "flagged": false, "snoozed_until": 0}));
        }
        notes.push(serde_json::json!({"id": "old", "time": now - 40 * 86_400, "kind": "info", "title": "Ancient", "body": "", "source": "system", "read": true, "flagged": false, "snoozed_until": 0}));
        notes.push(serde_json::json!({"id": "rem", "time": now - 50, "kind": "reminder", "title": "Reminder: Standup", "body": "", "source": "calendar", "read": false, "flagged": false, "snoozed_until": 0}));
        let dir = d.join("svc").join("inbox");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("root.json"), serde_json::to_vec(&serde_json::json!({"notifications": notes, "tasks": []})).unwrap()).unwrap();
        assert_eq!(cx.inbox().list("root").len(), 79 + 41 + 2);
        let mut a = InboxApp::new(&cx);
        assert_eq!(a.notifications().len(), 3, "{:?}", a.notifications().iter().map(|n| &n.title).collect::<Vec<_>>());
        let said = a.status_text().expect("said once").to_string();
        assert!(said.contains("tidied") && said.contains("122") && said.contains("Nothing else was removed"), "{said}");
        assert!(draw(&mut a, &cx, 800, 480).hits.len() > 3);
        // a second panel does not repeat it
        let b = InboxApp::new(&cx);
        assert!(b.status_text().is_none());
        assert_eq!(cx.inbox().counts("root", now).attention, 1, "only the reminder needs the person");
    }
    #[test]
    fn a_note_with_a_picture_shows_it_small_beside_its_words_and_one_without_is_unchanged() {
        let cx = admin_cx("inb-pic");
        let now = iyakki_clock::now_unix();
        let ib = &cx.inbox();
        let plain = ib.add("root", Kind::Info, "plain note", "no picture", "x", None, now - 200).unwrap();
        let with = ib.add("root", Kind::Info, "Motion at the door", "front", "cameras", Some(Link::new("cameras", "cam1")), now - 100).unwrap();
        // a real JPEG: a 64x48 red picture through the camera tile's own encoder
        let rgba: Vec<u8> = (0..64 * 48).flat_map(|_| [220u8, 20, 20, 255]).collect();
        let jpeg = crate::camera::jpeg::encode_rgba(64, 48, &rgba, 85);
        ib.attach_picture("root", &with.id, &jpeg).unwrap();
        let mut a = InboxApp::new(&cx);
        assert_eq!(a.thumbs.len(), 1, "only the note with a picture is decoded");
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(a.thumbs.get(&with.id).is_some_and(Option::is_some), "the picture decoded");
        // the red of the picture is on the screen (the row of the note that has it)
        let r = g.hits.iter().find(|h| h.label == with.id).unwrap().rect;
        let reds = (0..40).filter(|dy| { let y = (r.y + 8.0 + *dy as f32 * 1.0) as u32; let x = (r.x + 20.0) as u32; let p = &g.rgba()[((y * g.width() + x) * 4) as usize..][..4]; p[0] > 150 && p[1] < 90 }).count();
        assert!(reds > 10, "{reds} red pixels down the left of the row");
        let none = g.hits.iter().find(|h| h.label == plain.id).unwrap().rect;
        let p = &g.rgba()[(((none.y + 12.0) as u32 * g.width() + (none.x + 20.0) as u32) * 4) as usize..][..4];
        assert!(!(p[0] > 150 && p[1] < 90), "a note without a picture has none");
        // deleting the note drops its picture from memory at the next reload
        key(&mut a, &cx, Key::Char('d'));
        let _ = draw(&mut a, &cx, 1280, 720);
        assert!(a.thumbs.len() <= 1);
    }
}
