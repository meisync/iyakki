//! INBOX: what wants your attention, in three pages. **Notifications** (reminders that rang, runs that finished, new
//! devices, downloads, warnings: read, flag, snooze, add to the calendar), **Tasks** (a small to-do list the assistants can
//! add to: pending, doing, done) and **Approvals** (tool calls that agent runs are waiting to do: approve or reject each one
//! on its own, from whichever app started the run). Opened from the bell at the top of every screen or with F9.
//! Approvals are never bundled: there is no "approve all" for tool calls. (The one exception is the person's own choice for the
//! assistant in their browser: "Approve all actions on <site> for this session", asked as a question of its own; see
//! `browser_consent.rs` and docs/BROWSER-AGENT.md.)

use iyakki_agent::calendar::{parse_local, Repeat};
use iyakki_agent::clock::Zone;
use iyakki_agent::inbox::{Kind, Notification, Priority, Status, Task, TaskPatch, TaskSort};
use iyakki_agent::runs::PendingApproval;

use super::browser_consent::{row_summary, ConsentDialog};
use super::widgets::{render_confirm, Form, FormEvent};
use super::{blink, App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(255, 120, 90);
const AMBER: [u8; 4] = rgb(255, 196, 94);
const BLUE: [u8; 4] = rgb(110, 190, 255);
const SNOOZE_SECS: i64 = 3600;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Notifications,
    Tasks,
    Approvals,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    All,
    Open,
    Done,
}

impl Filter {
    fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Open => "open",
            Filter::Done => "done",
        }
    }
    fn next(self) -> Filter {
        match self {
            Filter::All => Filter::Open,
            Filter::Open => Filter::Done,
            Filter::Done => Filter::All,
        }
    }
}

enum Mode {
    Browse,
    /// New task (`None`) or editing the task with this id.
    Task(Form, Option<String>),
    /// Put something in the calendar: the form, and what it was made from (title, notes).
    Calendar(Form, String),
    /// Rejecting an approval: an optional reason.
    Reject(Form, PendingApproval),
    Detail(PendingApproval),
    Confirm(String, Confirmed),
    /// The assistant asked for a secret: the private form, and the request it answers.
    Secret(Form, String),
    /// A question about the assistant in the person's browser (look at a site, act on it, one action, a hand-over).
    Browser(ConsentDialog),
}

#[derive(Clone)]
enum Confirmed {
    Note(String),
    Task(String),
    ClearRead,
}

pub struct InboxApp {
    page: Page,
    notes: Vec<Notification>,
    tasks: Vec<Task>,
    approvals: Vec<PendingApproval>,
    sel: [usize; 3],
    filter: Filter,
    mode: Mode,
    status: Option<(String, bool)>,
    request: Option<(String, String)>,
    zone: Zone,
    snoozed: usize,
    /// Selecting and copying the text of an approval's details.
    pane: crate::textsel::Pane,
}

fn local_text(zone: &Zone, t: i64) -> String {
    let l = zone.local(t);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", l.year, l.month, l.day, l.hour, l.minute)
}

fn ago(now: i64, t: i64) -> String {
    let d = (now - t).max(0);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86_399 => format!("{} h ago", d / 3600),
        _ => format!("{} d ago", d / 86_400),
    }
}

impl InboxApp {
    pub fn new(cx: &AppCx) -> InboxApp {
        let mut a = InboxApp { page: Page::Notifications, notes: vec![], tasks: vec![], approvals: vec![], sel: [0; 3], filter: Filter::All, mode: Mode::Browse, status: None, request: None, zone: Zone::system(), snoozed: 0, pane: crate::textsel::Pane::new(61_400) };
        a.reload(cx);
        a
    }

    pub fn page(&self) -> Page {
        self.page
    }
    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }
    pub fn notifications(&self) -> &[Notification] {
        &self.notes
    }
    pub fn task_rows(&self) -> &[Task] {
        &self.tasks
    }
    pub fn approval_rows(&self) -> &[PendingApproval] {
        &self.approvals
    }
    pub fn selected(&self) -> usize {
        self.sel[self.page as usize]
    }
    pub fn filter(&self) -> Filter {
        self.filter
    }
    pub fn dialog_open(&self) -> bool {
        !matches!(self.mode, Mode::Browse)
    }

    fn ok(&mut self, m: impl Into<String>) {
        self.status = Some((m.into(), true));
    }
    fn fail(&mut self, m: impl Into<String>) {
        self.status = Some((m.into(), false));
    }

    /// Read everything again (after an action, and about twice a second).
    fn reload(&mut self, cx: &AppCx) {
        let (u, now) = (cx.session.user.as_str(), iyakki_agent::clock::now_unix());
        let all = cx.services.inbox.list(u);
        self.snoozed = all.iter().filter(|n| n.is_snoozed(now)).count();
        self.notes = all.into_iter().filter(|n| !n.is_snoozed(now)).collect();
        // grouped Pending, Doing, Done, Dropped; soonest due first inside a group
        let group = |s: Status| match s {
            Status::Pending => 0,
            Status::Doing => 1,
            Status::Done => 2,
            Status::Dropped => 3,
        };
        let mut t = cx.services.inbox.tasks(u, TaskSort::Due);
        t.retain(|t| match self.filter {
            Filter::All => true,
            Filter::Open => t.status.is_open(),
            Filter::Done => !t.status.is_open(),
        });
        t.sort_by_key(|t| group(t.status));
        self.tasks = t;
        self.approvals = cx.services.approvals(u);
        // secrets the assistant asked for show as approvals of their own kind: the person types the value, not the assistant
        let mut asked: Vec<PendingApproval> = vec![];
        for r in cx.services.secret_requests.pending(u) {
            let summary = format!("Enter a {} for “{}”. It goes straight into your vault as {} and the assistant never sees it.", r.kind.replace('_', " "), r.purpose, r.name);
            asked.push(PendingApproval { run_id: "vault".into(), user: u.to_string(), prompt: format!("needs a secret: {}", r.purpose), call_id: r.id, tool: "vault_request".into(), summary, detail: String::new(), risk: "low", since: r.since });
        }
        // questions about the assistant in the person's browser (the wait of a running tool ends when they answer)
        for r in cx.services.browser.consent.pending(u) {
            asked.push(PendingApproval { run_id: "browser".into(), user: u.to_string(), prompt: r.title.clone(), call_id: r.id.clone(), tool: "browser_consent".into(), summary: row_summary(&r), detail: r.detail.clone(), risk: "medium", since: r.since });
        }
        asked.append(&mut self.approvals);
        self.approvals = asked;
        for (i, n) in [self.notes.len(), self.tasks.len(), self.approvals.len()].into_iter().enumerate() {
            self.sel[i] = self.sel[i].min(n.saturating_sub(1));
        }
    }

    fn note(&self) -> Option<&Notification> {
        self.notes.get(self.sel[0])
    }
    fn task(&self) -> Option<&Task> {
        self.tasks.get(self.sel[1])
    }
    fn approval(&self) -> Option<&PendingApproval> {
        self.approvals.get(self.sel[2])
    }

    fn tomorrow_9(&self) -> (String, String) {
        let l = self.zone.local(iyakki_agent::clock::now_unix() + 86_400);
        (format!("{:04}-{:02}-{:02}", l.year, l.month, l.day), "09:00".into())
    }

    fn task_form(&self, t: Option<&Task>) -> Form {
        let due = t.and_then(|t| t.due).map(|d| local_text(&self.zone, d)).unwrap_or_default();
        let pr = t.map_or(1, |t| match t.priority {
            Priority::Low => 0,
            Priority::Normal => 1,
            Priority::High => 2,
        });
        Form::new(if t.is_some() { "Edit task" } else { "New task" })
            .text("Title", t.map_or("", |t| t.title.as_str()), "what needs doing?")
            .text("Due", &due, "YYYY-MM-DD or YYYY-MM-DD HH:MM, empty for none")
            .choice("Priority", &["low", "normal", "high"], pr)
            .text("Notes", t.map_or("", |t| t.notes.as_str()), "optional")
    }

    fn calendar_form(&self, title: &str, when: Option<i64>) -> Form {
        let (d, tm) = match when {
            Some(w) => {
                let t = local_text(&self.zone, w);
                (t[..10].to_string(), t[11..].to_string())
            }
            None => self.tomorrow_9(),
        };
        Form::new("Add to the calendar").text("Title", title, "the event's name").text("Date", &d, "YYYY-MM-DD").text("Starts at", &tm, "HH:MM")
    }

    fn parse_due(&self, s: &str) -> Result<Option<i64>, String> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(None);
        }
        let (t, all_day) = parse_local(&self.zone, s)?;
        // a date alone means the end of that day, so a task due today is not late all day
        Ok(Some(if all_day { t + 86_399 } else { t }))
    }

    // ------------------------------------------------------------ actions

    fn open_link(&mut self, cx: &AppCx) {
        let Some(n) = self.note().cloned() else { return };
        let _ = cx.services.inbox.mark_read(&cx.session.user, &n.id, true);
        cx.audit.record(&cx.session.user, "inbox.open", &format!("{}: {}", n.kind.name(), n.title));
        match &n.link {
            Some(l) if !l.app.is_empty() && l.app != "inbox" => self.request = Some((l.app.clone(), l.id.clone())),
            _ => self.ok("This note has nothing to open."),
        }
        self.reload(cx);
    }

    fn note_action(&mut self, cx: &AppCx, k: char) {
        let (u, now) = (cx.session.user.clone(), iyakki_agent::clock::now_unix());
        if k == 'A' {
            match cx.services.inbox.mark_all_read(&u) {
                Ok(n) => {
                    cx.audit.record(&u, "inbox.read_all", &n.to_string());
                    self.ok(format!("Marked {n} as read."));
                }
                Err(e) => self.fail(e),
            }
            self.reload(cx);
            return;
        }
        if k == 'X' {
            self.mode = Mode::Confirm("Remove every notification you have read (flagged ones stay)?".into(), Confirmed::ClearRead);
            return;
        }
        let Some(n) = self.note().cloned() else { return };
        let r = match k {
            'r' => cx.services.inbox.mark_read(&u, &n.id, !n.read).map(|_| cx.audit.record(&u, "inbox.read", &n.title)),
            'f' => cx.services.inbox.flag(&u, &n.id, !n.flagged).map(|_| cx.audit.record(&u, if n.flagged { "inbox.unflag" } else { "inbox.flag" }, &n.title)),
            's' => cx.services.inbox.snooze(&u, &n.id, now + SNOOZE_SECS).map(|_| {
                cx.audit.record(&u, "inbox.snooze", &n.title);
                self.ok("Snoozed for an hour.");
            }),
            'd' => {
                self.mode = Mode::Confirm(format!("Delete “{}”?", n.title), Confirmed::Note(n.id.clone()));
                return;
            }
            'c' => {
                // a reminder already is a calendar item: open it
                if n.kind == Kind::Reminder {
                    let _ = cx.services.inbox.mark_read(&u, &n.id, true);
                    self.request = Some(("calendar".into(), n.link.as_ref().map(|l| l.id.clone()).unwrap_or_default()));
                } else {
                    self.mode = Mode::Calendar(self.calendar_form(&n.title, None), n.body.clone());
                }
                return;
            }
            _ => return,
        };
        if let Err(e) = r {
            self.fail(e);
        }
        self.reload(cx);
    }

    fn task_action(&mut self, cx: &AppCx, ev: &Event) {
        let (u, now) = (cx.session.user.clone(), iyakki_agent::clock::now_unix());
        let key = match ev {
            Event::Key(k) => k.clone(),
            _ => return,
        };
        if key == Key::Char('n') {
            self.mode = Mode::Task(self.task_form(None), None);
            return;
        }
        if key == Key::Char('f') {
            self.filter = self.filter.next();
            self.sel[1] = 0;
            self.reload(cx);
            return;
        }
        let Some(t) = self.task().cloned() else { return };
        let moved = |to: Status| cx.services.inbox.move_task(&u, &t.id, to, now);
        let r: Result<String, String> = match key {
            Key::Enter | Key::Right if t.status.is_open() => moved(t.status.next()).map(|n| format!("“{}” is now {}.", n.title, n.status.name())),
            Key::Left => moved(if t.status == Status::Dropped { Status::Pending } else { t.status.prev() }).map(|n| format!("“{}” is now {}.", n.title, n.status.name())),
            Key::Char('x') => moved(Status::Dropped).map(|n| format!("Dropped “{}”.", n.title)),
            Key::Char('p') => cx.services.inbox.update_task(&u, &t.id, TaskPatch { priority: Some(t.priority.cycle()), ..Default::default() }, now).map(|n| format!("Priority of “{}”: {}.", n.title, n.priority.name())),
            Key::Char('e') => {
                self.mode = Mode::Task(self.task_form(Some(&t)), Some(t.id.clone()));
                return;
            }
            Key::Char('d') => {
                self.mode = Mode::Confirm(format!("Delete the task “{}”?", t.title), Confirmed::Task(t.id.clone()));
                return;
            }
            Key::Char('c') => {
                self.mode = Mode::Calendar(self.calendar_form(&t.title, t.due.filter(|d| *d > now)), t.notes.clone());
                return;
            }
            _ => return,
        };
        match r {
            Ok(m) => {
                cx.audit.record(&u, "inbox.task", &format!("{}: {}", t.title, m));
                self.ok(m);
            }
            Err(e) => self.fail(e),
        }
        self.reload(cx);
        // the row moves to another group when its state changes: keep the selection on the same task
        if let Some(i) = self.tasks.iter().position(|x| x.id == t.id) {
            self.sel[1] = i;
        }
    }

    fn submit_task(&mut self, cx: &AppCx, f: &Form, edit: &Option<String>) -> Result<(), String> {
        let (u, now) = (cx.session.user.as_str(), iyakki_agent::clock::now_unix());
        let due = self.parse_due(&f.value(1))?;
        let pr = [Priority::Low, Priority::Normal, Priority::High][f.choice_idx(2).min(2)];
        match edit {
            Some(id) => {
                cx.services.inbox.update_task(u, id, TaskPatch { title: Some(f.value(0)), notes: Some(f.value(3)), due: Some(due), priority: Some(pr), status: None }, now)?;
                cx.audit.record(u, "inbox.task.edit", &f.value(0));
            }
            None => {
                cx.services.inbox.add_task(u, &f.value(0), &f.value(3), due, pr, "user", now)?;
                cx.audit.record(u, "inbox.task.add", &f.value(0));
            }
        }
        Ok(())
    }

    fn submit_calendar(&mut self, cx: &AppCx, f: &Form, notes: &str) -> Result<(), String> {
        let (start, _) = parse_local(&self.zone, &format!("{} {}", f.value(1), f.value(2)))?;
        let e = cx.services.calendar.add_event(&cx.session.user, &f.value(0), start, 60, false, Repeat::None, notes, "user", Some(10))?;
        cx.audit.record(&cx.session.user, "inbox.calendar", &e.title);
        Ok(())
    }

    fn is_browser(a: &PendingApproval) -> bool {
        a.run_id == "browser" && a.tool == "browser_consent"
    }

    /// Open the dialog for a browser question.
    fn open_browser(&mut self, cx: &AppCx, a: &PendingApproval) {
        match cx.services.browser.consent.get(&cx.session.user, &a.call_id) {
            Some(r) => self.mode = Mode::Browser(ConsentDialog::new(r)),
            None => {
                self.fail("That question is not waiting any more.");
                self.reload(cx);
            }
        }
    }

    fn is_secret(a: &PendingApproval) -> bool {
        a.run_id == "vault" && a.tool == "vault_request"
    }

    /// Open the private form for a secret request.
    fn open_secret(&mut self, cx: &AppCx, a: &PendingApproval) {
        let name = cx.services.secret_requests.get(&cx.session.user, &a.call_id).map(|r| r.name).unwrap_or_else(|| "svc.secret".into());
        self.mode = Mode::Secret(super::widgets::secret_form(&format!("Enter secret: {}", a.prompt.trim_start_matches("needs a secret: ")), &name, None), a.call_id.clone());
    }

    fn submit_secret(&mut self, cx: &AppCx) {
        let Mode::Secret(form, id) = &mut self.mode else { return };
        let (name, value) = (form.value(0), form.raw(1));
        match cx.services.answer_secret_request(&cx.session.user, id, &name, &value) {
            Ok(stored) => {
                // the log records that a secret with this name was stored, never the value
                cx.audit.record(&cx.session.user, "vault.user.set", &stored);
                self.mode = Mode::Browse;
                self.ok(format!("Stored as uvault:{stored}."));
                self.reload(cx);
            }
            Err(e) => form.error = Some(e),
        }
    }

    fn decide(&mut self, cx: &AppCx, a: &PendingApproval, approve: bool, reason: &str) {
        if Self::is_browser(a) {
            if approve {
                self.open_browser(cx, a);
            } else if let Some(r) = cx.services.browser.consent.get(&cx.session.user, &a.call_id) {
                // "n" says no: Deny (or Cancel for a hand-over)
                let d = ConsentDialog::new(r);
                match d.answer_of(&Key::Esc).map(|ans| d.answer(cx, ans)) {
                    Some(Ok(())) => self.ok("Said no; the assistant was told."),
                    Some(Err(e)) => self.fail(e),
                    None => self.fail("Open the question (Enter) and choose."),
                }
                self.reload(cx);
            }
            return;
        }
        if Self::is_secret(a) {
            if !approve {
                match cx.services.cancel_secret_request(&cx.session.user, &a.call_id) {
                    Ok(()) => {
                        cx.audit.record(&cx.session.user, "inbox.reject", &format!("vault_request: {}", a.prompt.chars().take(120).collect::<String>()));
                        self.ok("Cancelled; the assistant was told you chose not to.");
                    }
                    Err(e) => self.fail(e),
                }
                self.reload(cx);
            } else {
                self.open_secret(cx, a);
            }
            return;
        }
        let u = &cx.session.user;
        match cx.services.decide_approval(u, &a.run_id, &a.call_id, approve) {
            Ok(_) => {
                let what = format!("{}: {}", a.tool, a.summary.chars().take(160).collect::<String>());
                let detail = if approve || reason.is_empty() { what } else { format!("{what} (reason: {})", reason.chars().take(200).collect::<String>()) };
                cx.audit.record(u, if approve { "inbox.approve" } else { "inbox.reject" }, &detail);
                self.ok(if approve { format!("Approved {}.", a.tool) } else { format!("Rejected {}.", a.tool) });
            }
            Err(e) => self.fail(e),
        }
        self.reload(cx);
    }
}

impl App for InboxApp {
    fn title(&self) -> &'static str {
        "Inbox"
    }
    fn icon(&self) -> Icon {
        Icon::Bell
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        match self.page {
            Page::Notifications => "Tab/1-3 pages · Enter open · r read · f flag · c calendar · s snooze · d delete · A all read · X clear read".into(),
            Page::Tasks => "Tab/1-3 pages · n new · Enter/→ next · ← back · e edit · p priority · c calendar · x drop · d delete · f filter".into(),
            Page::Approvals => "Tab/1-3 pages · y approve · n reject · Enter details (each one on its own)".into(),
        }
    }
    fn help(&self) -> Vec<String> {
        let mut v = vec!["Tab or 1 2 3 · Notifications, Tasks, Approvals".to_string(), "↑ ↓ · choose a row".to_string(), "F9 or the bell · open or close the inbox from anywhere".to_string()];
        v.extend(self.hint().split(" · ").skip(1).map(str::to_string));
        v
    }

    fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }

    fn context_items(&mut self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        if matches!(self.mode, Mode::Detail(_)) {
            self.pane.context_items(x, y)
        } else {
            Vec::new()
        }
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if matches!(ev, Event::Tick) {
            // new things arrive while it is open (not while a form is being filled in)
            if matches!(self.mode, Mode::Browse) {
                self.reload(cx);
            }
            return Flow::Stay;
        }
        match &mut self.mode {
            Mode::Task(f, _) | Mode::Calendar(f, _) | Mode::Reject(f, _) => {
                match f.handle_event(ev) {
                    FormEvent::Cancel => self.mode = Mode::Browse,
                    FormEvent::Submit => {
                        let mode = std::mem::replace(&mut self.mode, Mode::Browse);
                        match mode {
                            Mode::Task(form, edit) => match self.submit_task(cx, &form, &edit) {
                                Ok(()) => self.ok(if edit.is_some() { "Task saved." } else { "Task added." }),
                                Err(e) => {
                                    let mut form = form;
                                    form.error = Some(e);
                                    self.mode = Mode::Task(form, edit);
                                }
                            },
                            Mode::Calendar(form, notes) => match self.submit_calendar(cx, &form, &notes) {
                                Ok(()) => self.ok("Added to the calendar."),
                                Err(e) => {
                                    let mut form = form;
                                    form.error = Some(e);
                                    self.mode = Mode::Calendar(form, notes);
                                }
                            },
                            Mode::Reject(form, a) => self.decide(cx, &a, false, &form.value(0)),
                            _ => {}
                        }
                        self.reload(cx);
                    }
                    _ => {}
                }
                return Flow::Stay;
            }
            Mode::Secret(f, _) => {
                match f.handle_event(ev) {
                    FormEvent::Cancel => self.mode = Mode::Browse,
                    FormEvent::Submit => self.submit_secret(cx),
                    _ => {}
                }
                return Flow::Stay;
            }
            Mode::Browser(d) => {
                if let Event::Key(k) = ev {
                    if let Some(ans) = d.answer_of(k) {
                        let d = d.clone();
                        match d.answer(cx, ans) {
                            Ok(()) => self.ok("Answered; the assistant carries on."),
                            Err(e) => self.fail(e),
                        }
                        self.mode = Mode::Browse;
                        self.reload(cx);
                    } else if matches!(k, Key::Enter) {
                        self.mode = Mode::Browse;
                    }
                }
                return Flow::Stay;
            }
            Mode::Detail(_) => {
                if self.pane.handle(ev) {
                    return Flow::Stay;
                }
                if matches!(ev, Event::Key(Key::Ctrl('a'))) {
                    self.pane.select_all();
                } else if matches!(ev, Event::Key(Key::Esc | Key::Enter | Key::Char('q'))) {
                    self.mode = Mode::Browse;
                    self.pane.clear();
                }
                return Flow::Stay;
            }
            Mode::Confirm(_, what) => {
                let what = what.clone();
                if matches!(ev, Event::Key(Key::Char('y'))) {
                    self.mode = Mode::Browse;
                    let u = cx.session.user.clone();
                    let r = match what {
                        Confirmed::Note(id) => cx.services.inbox.delete(&u, &id).map(|_| "Deleted.".to_string()),
                        Confirmed::Task(id) => cx.services.inbox.delete_task(&u, &id).map(|_| "Task deleted.".to_string()),
                        Confirmed::ClearRead => cx.services.inbox.clear_read(&u).map(|n| format!("Removed {n} read notification{}.", if n == 1 { "" } else { "s" })),
                    };
                    match r {
                        Ok(m) => {
                            cx.audit.record(&u, "inbox.delete", &m);
                            self.ok(m);
                        }
                        Err(e) => self.fail(e),
                    }
                    self.reload(cx);
                } else if matches!(ev, Event::Key(_)) {
                    self.mode = Mode::Browse;
                }
                return Flow::Stay;
            }
            Mode::Browse => {}
        }
        match ev {
            Event::Tab(n @ 0..=2) => {
                self.page = [Page::Notifications, Page::Tasks, Page::Approvals][*n];
                self.reload(cx);
            }
            Event::Row(i) => {
                let len = [self.notes.len(), self.tasks.len(), self.approvals.len()][self.page as usize];
                self.sel[self.page as usize] = (*i).min(len.saturating_sub(1));
            }
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Tab) => {
                self.page = [Page::Tasks, Page::Approvals, Page::Notifications][self.page as usize];
                self.reload(cx);
            }
            Event::Key(Key::BackTab) => {
                self.page = [Page::Approvals, Page::Notifications, Page::Tasks][self.page as usize];
                self.reload(cx);
            }
            Event::Key(Key::Char(c @ '1'..='3')) => {
                self.page = [Page::Notifications, Page::Tasks, Page::Approvals][*c as usize - '1' as usize];
                self.reload(cx);
            }
            Event::Key(Key::Up) => self.sel[self.page as usize] = self.sel[self.page as usize].saturating_sub(1),
            Event::Key(Key::Down) => {
                let len = [self.notes.len(), self.tasks.len(), self.approvals.len()][self.page as usize];
                self.sel[self.page as usize] = (self.sel[self.page as usize] + 1).min(len.saturating_sub(1));
            }
            Event::Key(k) => match self.page {
                Page::Notifications => match k {
                    Key::Enter => self.open_link(cx),
                    Key::Char(c @ ('r' | 'f' | 'c' | 's' | 'd' | 'A' | 'X')) => self.note_action(cx, *c),
                    _ => {}
                },
                Page::Tasks => self.task_action(cx, ev),
                Page::Approvals => match k {
                    Key::Char('y') => {
                        if let Some(a) = self.approval().cloned() {
                            self.decide(cx, &a, true, "");
                        }
                    }
                    Key::Char('n') => {
                        if let Some(a) = self.approval().cloned().filter(|a| Self::is_secret(a) || Self::is_browser(a)) {
                            self.decide(cx, &a, false, "");
                        } else if let Some(a) = self.approval().cloned() {
                            self.mode = Mode::Reject(Form::new(&format!("Reject {}", a.tool)).text("Reason", "", "optional: why not?"), a);
                        }
                    }
                    Key::Enter => {
                        if let Some(a) = self.approval().cloned() {
                            if Self::is_browser(&a) {
                                self.open_browser(cx, &a);
                            } else if Self::is_secret(&a) {
                                self.open_secret(cx, &a);
                            } else {
                                self.mode = Mode::Detail(a);
                            }
                        }
                    }
                    _ => {}
                },
            },
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let now = iyakki_agent::clock::now_unix();
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 10.0 * s;
        let counts = [self.notes.iter().filter(|n| !n.read).count(), self.tasks.iter().filter(|t| t.status.is_open()).count(), self.approvals.len()];
        let pages = [("Notifications", Page::Notifications), ("Tasks", Page::Tasks), ("Approvals", Page::Approvals)];
        let items: Vec<(String, Event)> = pages.iter().enumerate().map(|(i, (label, _))| (if counts[i] > 0 { format!("{label} ({})", counts[i]) } else { label.to_string() }, Event::Tab(i))).collect();
        let cur = pages.iter().position(|(_, p)| self.page == *p).unwrap_or(0);
        crate::fx::tab_strip(g, x, y, s, w, &items, cur, if cur == 2 && counts[2] > 0 { ERR } else { ACCENT });
        y += 50.0 * s;
        if let Some((m, ok)) = &self.status {
            crate::fx::status_line(g, x, y - 8.0 * s, w, s, m, *ok);
            y += 18.0 * s;
        }
        let body = R::new(x, y, w, area.y + area.h - y - 66.0 * s);
        let actions = R::new(x, area.y + area.h - 56.0 * s, w, 44.0 * s);
        match self.page {
            Page::Notifications => self.draw_notes(g, body, actions, s, now),
            Page::Tasks => self.draw_tasks(g, body, actions, s, now),
            Page::Approvals => self.draw_approvals(g, body, actions, s, now),
        }
        let _ = cx;
        match &self.mode {
            Mode::Task(f, _) | Mode::Calendar(f, _) | Mode::Reject(f, _) => f.render(g, area, s, ACCENT, blink(tick)),
            Mode::Secret(f, _) => f.render(g, area, s, ACCENT, blink(tick)),
            Mode::Browser(d) => d.render(g, area, s),
            Mode::Confirm(text, _) => render_confirm(g, area, s, text, "Yes"),
            Mode::Detail(a) => draw_detail(g, area, s, a, now, &mut self.pane),
            Mode::Browse => {}
        }
    }
}

fn kind_color(k: Kind) -> [u8; 4] {
    match k {
        Kind::Error => ERR,
        Kind::Warning => AMBER,
        Kind::Reminder => BLUE,
        Kind::Agent => rgb(167, 116, 255),
        Kind::Device => rgb(255, 150, 110),
        Kind::Download => OK,
        Kind::System | Kind::Info => MUTED,
    }
}

/// The first row to draw so that the selected row is visible.
fn first_visible(sel: usize, fit: usize) -> usize {
    sel.saturating_sub(fit.saturating_sub(1))
}

fn action_row(g: &mut Gfx, r: R, s: f32, items: &[(&str, [u8; 4], Key)]) {
    let mut bx = r.x;
    for (label, col, key) in items {
        let bw = g.text_width(13.5 * s, label, Face::Bold) + 13.5 * s * 1.7;
        if bx + bw > r.x + r.w {
            break;
        }
        bx += g.button(bx, r.y, 13.5 * s, label, *col, Event::Key(key.clone())) + 8.0 * s;
    }
}

impl InboxApp {
    fn draw_notes(&mut self, g: &mut Gfx, body: R, actions: R, s: f32, now: i64) {
        if self.notes.is_empty() {
            crate::fx::empty_state(g, body, s, Icon::Bell, "Nothing new", "Reminders, finished runs, new devices and warnings will show up here.", ACCENT);
        }
        let rh = 62.0 * s;
        let fit = ((body.h / (rh + 6.0 * s)) as usize).max(1);
        let first = first_visible(self.sel[0], fit);
        for (row, (i, n)) in self.notes.iter().enumerate().skip(first).take(fit).enumerate() {
            let r = R::new(body.x, body.y + row as f32 * (rh + 6.0 * s), body.w, rh);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == self.sel[0], g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), &n.id);
            g.rect(r.x, r.y + 8.0 * s, 4.0 * s, r.h - 16.0 * s, kind_color(n.kind));
            let face = if n.read { Face::Sans } else { Face::Bold };
            let star = if n.flagged { "★ " } else { "" };
            let tw = r.w - 200.0 * s;
            let t = g.fit(15.5 * s, &format!("{star}{}", n.title), face, tw);
            g.text(r.x + 16.0 * s, r.y + 8.0 * s, 15.5 * s, if n.read { MUTED } else { TEXT }, &t, face);
            let first_line = n.body.lines().next().unwrap_or("");
            let b = g.fit(13.0 * s, first_line, Face::Sans, tw);
            g.text(r.x + 16.0 * s, r.y + 34.0 * s, 13.0 * s, DIM, &b, Face::Sans);
            let meta = format!("{} · {}", n.kind.name(), ago(now, n.time));
            let mw = g.text_width(12.5 * s, &meta, Face::Sans);
            g.text(r.x + r.w - mw - 14.0 * s, r.y + 10.0 * s, 12.5 * s, kind_color(n.kind), &meta, Face::Sans);
            if !n.source.is_empty() {
                let sw = g.text_width(12.0 * s, &n.source, Face::Sans);
                g.text(r.x + r.w - sw - 14.0 * s, r.y + 36.0 * s, 12.0 * s, DIM, &n.source, Face::Sans);
            }
        }
        if self.snoozed > 0 {
            g.text(body.x + body.w - 160.0 * s, actions.y - 18.0 * s, 12.0 * s, DIM, &format!("{} snoozed", self.snoozed), Face::Sans);
        }
        action_row(g, actions, s, &[("Open", ACCENT, Key::Enter), ("Read (r)", MUTED, Key::Char('r')), ("Flag (f)", AMBER, Key::Char('f')), ("Calendar (c)", BLUE, Key::Char('c')), ("Snooze (s)", MUTED, Key::Char('s')), ("Delete (d)", ERR, Key::Char('d')), ("All read (A)", MUTED, Key::Char('A')), ("Clear read (X)", MUTED, Key::Char('X'))]);
    }

    fn draw_tasks(&mut self, g: &mut Gfx, body: R, actions: R, s: f32, now: i64) {
        let head = format!("Showing: {} (f changes it)", self.filter.label());
        g.text(body.x, body.y - 4.0 * s, 12.5 * s, DIM, &head, Face::Sans);
        let body = R::new(body.x, body.y + 16.0 * s, body.w, body.h - 16.0 * s);
        if self.tasks.is_empty() {
            crate::fx::empty_state(g, body, s, Icon::Bell, "No tasks here", "Press n to add one; assistants can add to-dos for you too.", ACCENT);
        }
        let rh = 50.0 * s;
        let hh = 24.0 * s;
        // lay the rows out with a heading before each group, then show the window that holds the selection
        let mut lines: Vec<(Option<usize>, Status)> = Vec::new();
        let mut last = None;
        for (i, t) in self.tasks.iter().enumerate() {
            if last != Some(t.status) {
                lines.push((None, t.status));
                last = Some(t.status);
            }
            lines.push((Some(i), t.status));
        }
        let height = |l: &(Option<usize>, Status)| if l.0.is_none() { hh } else { rh + 4.0 * s };
        let sel_line = lines.iter().position(|l| l.0 == Some(self.sel[1])).unwrap_or(0);
        let mut start = 0;
        while start < sel_line && lines[start..=sel_line].iter().map(height).sum::<f32>() > body.h {
            start += 1;
        }
        let mut y = body.y;
        for l in &lines[start.min(lines.len())..] {
            if y + height(l) > body.y + body.h + 1.0 {
                break;
            }
            match l.0 {
                None => {
                    let n = self.tasks.iter().filter(|t| t.status == l.1).count();
                    g.text(body.x + 4.0 * s, y + 2.0 * s, 13.0 * s, MUTED, &format!("{} ({n})", l.1.name().to_uppercase()), Face::Bold);
                }
                Some(i) => {
                    let t = &self.tasks[i];
                    let r = R::new(body.x, y, body.w, rh);
                    crate::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == self.sel[1], g.hovered(r), ACCENT);
                    g.hit(r, Event::Row(i), &t.id);
                    let pc = match t.priority {
                        Priority::High => ERR,
                        Priority::Normal => BLUE,
                        Priority::Low => DIM,
                    };
                    g.rect(r.x, r.y + 8.0 * s, 4.0 * s, r.h - 16.0 * s, pc);
                    let done = !t.status.is_open();
                    let title = g.fit(15.0 * s, &t.title, Face::Bold, r.w - 260.0 * s);
                    g.text(r.x + 16.0 * s, r.y + 6.0 * s, 15.0 * s, if done { DIM } else { TEXT }, &title, Face::Bold);
                    let who = if t.created_by == "user" { String::new() } else { format!(" · from {}", t.created_by) };
                    let sub = g.fit(12.5 * s, &format!("{} priority{}{}", t.priority.name(), who, if t.notes.is_empty() { String::new() } else { format!(" · {}", t.notes.lines().next().unwrap_or("")) }), Face::Sans, r.w - 260.0 * s);
                    g.text(r.x + 16.0 * s, r.y + 28.0 * s, 12.5 * s, MUTED, &sub, Face::Sans);
                    if let Some(d) = t.due {
                        let late = t.is_overdue(now);
                        let txt = format!("{}due {}", if late { "OVERDUE · " } else { "" }, local_text(&self.zone, d));
                        let tw = g.text_width(12.5 * s, &txt, Face::Bold);
                        g.text(r.x + r.w - tw - 14.0 * s, r.y + 16.0 * s, 12.5 * s, if late { ERR } else { MUTED }, &txt, Face::Bold);
                    }
                }
            }
            y += height(l);
        }
        action_row(g, actions, s, &[("+ New (n)", ACCENT, Key::Char('n')), ("◀ Back", MUTED, Key::Left), ("Next ▶", OK, Key::Right), ("Edit (e)", MUTED, Key::Char('e')), ("Priority (p)", AMBER, Key::Char('p')), ("Calendar (c)", BLUE, Key::Char('c')), ("Drop (x)", MUTED, Key::Char('x')), ("Delete (d)", ERR, Key::Char('d')), ("Filter (f)", MUTED, Key::Char('f'))]);
    }

    fn draw_approvals(&mut self, g: &mut Gfx, body: R, actions: R, s: f32, now: i64) {
        if self.approvals.is_empty() {
            crate::fx::empty_state(g, body, s, Icon::Bell, "Nothing is waiting for your answer", "When an assistant wants to do something that needs your say-so, it shows up here.", ACCENT);
        }
        let rh = 92.0 * s;
        let fit = ((body.h / (rh + 6.0 * s)) as usize).max(1);
        let first = first_visible(self.sel[2], fit);
        for (row, (i, a)) in self.approvals.iter().enumerate().skip(first).take(fit).enumerate() {
            let r = R::new(body.x, body.y + row as f32 * (rh + 6.0 * s), body.w, rh);
            crate::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == self.sel[2], g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), &a.call_id);
            let (rc, rl) = match a.risk {
                "high" => (ERR, "HIGH RISK"),
                "medium" => (AMBER, "MEDIUM RISK"),
                _ => (OK, "LOW RISK"),
            };
            g.rect(r.x, r.y + 8.0 * s, 4.0 * s, r.h - 16.0 * s, rc);
            let tw = r.w - 200.0 * s;
            let title = if Self::is_secret(a) { format!("{}'s assistant needs a secret from you", a.user) } else { format!("{} wants to run: {}", a.user, a.tool) };
            let t = g.fit(15.5 * s, &title, Face::Bold, tw);
            g.text(r.x + 16.0 * s, r.y + 8.0 * s, 15.5 * s, TEXT, &t, Face::Bold);
            let sm = g.fit(13.5 * s, &a.summary.replace('\n', " "), Face::Sans, tw);
            g.text(r.x + 16.0 * s, r.y + 33.0 * s, 13.5 * s, TEXT, &sm, Face::Sans);
            let why = g.fit(12.5 * s, &format!("asked: {}", a.prompt.replace('\n', " ")), Face::Sans, tw);
            g.text(r.x + 16.0 * s, r.y + 58.0 * s, 12.5 * s, MUTED, &why, Face::Sans);
            let rw = g.text_width(12.5 * s, rl, Face::Bold);
            g.text(r.x + r.w - rw - 14.0 * s, r.y + 10.0 * s, 12.5 * s, rc, rl, Face::Bold);
            let age = format!("waiting {}", ago(now, a.since).replace(" ago", ""));
            let aw = g.text_width(12.5 * s, &age, Face::Sans);
            g.text(r.x + r.w - aw - 14.0 * s, r.y + 36.0 * s, 12.5 * s, MUTED, &age, Face::Sans);
        }
        if self.approval().is_some_and(Self::is_secret) {
            action_row(g, actions, s, &[("Enter secret (y)", OK, Key::Char('y')), ("Not now (n)", ERR, Key::Char('n'))]);
        } else {
            action_row(g, actions, s, &[("Approve (y)", OK, Key::Char('y')), ("Reject (n)", ERR, Key::Char('n')), ("Details (Enter)", MUTED, Key::Enter)]);
        }
    }
}

fn draw_detail(g: &mut Gfx, area: R, s: f32, a: &PendingApproval, now: i64, pane: &mut crate::textsel::Pane) {
    let p = crate::fx::modal(g, area, s, (area.w - 80.0 * s).min(820.0 * s), area.h - 80.0 * s, ACCENT, false);
    let (bx, by, bw, bh) = (p.x, p.y, p.w, p.h);
    g.text(bx + 24.0 * s, by + 20.0 * s, 19.0 * s, TEXT, &format!("{} — {} risk", a.tool, a.risk), Face::Bold);
    let mut text = format!("Run {} of {}, waiting {}.\nAsked: {}\n\nWill do: {}", a.run_id, a.user, ago(now, a.since).replace(" ago", ""), a.prompt, a.summary);
    if !a.detail.is_empty() {
        text.push_str(&format!("\n\nWhat changes:\n{}", a.detail));
    }
    let lh = g.line_height(14.0 * s, Face::Mono);
    let mut y = by + 58.0 * s;
    pane.begin(R::new(bx + 24.0 * s, y, bw - 48.0 * s, by + bh - 60.0 * s - y));
    'outer: for (n, para) in text.lines().enumerate() {
        for l in g.wrap(14.0 * s, para, Face::Mono, bw - 48.0 * s) {
            if y + lh > by + bh - 60.0 * s {
                break 'outer;
            }
            g.text(bx + 24.0 * s, y, 14.0 * s, TEXT, &l, Face::Mono);
            pane.line(g, bx + 24.0 * s, y, lh, 14.0 * s, Face::Mono, &l, 0, n as u32);
            y += lh;
        }
    }
    pane.end(g, ACCENT);
    g.button(bx + 24.0 * s, by + bh - 50.0 * s, 14.0 * s, "Close (Esc)", MUTED, Event::Key(Key::Esc));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
    use iyakki_agent::inbox::Link;
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
        std::fs::read_to_string(cx.audit.path()).unwrap_or_default()
    }

    #[test]
    fn notifications_are_read_flagged_snoozed_deleted_cleared_and_opened() {
        let cx = admin_cx("inb1");
        let now = iyakki_agent::clock::now_unix();
        let ib = &cx.services.inbox;
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
        assert_eq!(cx.services.inbox.counts("root", now).unread, 1, "snoozed is not counted");
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
        cx.services.notify("root", Kind::Download, "Model downloaded", "qwen is ready", None).unwrap();
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
        let ev = cx.services.calendar.events("root");
        assert_eq!(ev.len(), 1);
        assert_eq!((ev[0].title.as_str(), ev[0].notes.as_str(), ev[0].by.as_str()), ("Model downloaded", "qwen is ready", "user"));
        let zone = Zone::system();
        let (l, today) = (zone.local(ev[0].start), zone.local(iyakki_agent::clock::now_unix() + 86_400));
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
        assert!(t.due.is_some_and(|d| d > iyakki_agent::clock::now_unix()), "a date alone means the end of that day");
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
        assert_eq!(cx.services.calendar.events("root").len(), 1, "{:?}", a.status_text());
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
    impl iyakki_agent::chat::ChatBackend for Script {
        fn complete(&self, _: &[iyakki_agent::chat::Message]) -> Result<String, String> {
            Err("unused".into())
        }
        fn complete_tools(&self, _: &[iyakki_agent::chat::Message], _: &[iyakki_agent::chat::ToolSpec]) -> Result<iyakki_agent::chat::ModelTurn, String> {
            if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Ok(iyakki_agent::chat::ModelTurn { text: String::new(), calls: vec![iyakki_agent::chat::ToolCall { id: "c1".into(), name: "write_file".into(), args: serde_json::json!({}) }] })
            } else {
                Ok(iyakki_agent::chat::ModelTurn { text: "finished".into(), calls: vec![] })
            }
        }
    }
    struct WriteTools;
    impl iyakki_agent::agent::Tools for WriteTools {
        fn defs(&self) -> Vec<iyakki_agent::agent::ToolDef> {
            vec![iyakki_agent::agent::ToolDef { spec: iyakki_agent::chat::ToolSpec { name: "write_file".into(), description: "w".into(), schema: serde_json::json!({"type": "object"}) }, risk: iyakki_agent::agent::Risk::Ask, untrusted: false }]
        }
        fn call(&self, _: &str, _: &serde_json::Value) -> Result<String, String> {
            Ok("written".into())
        }
        fn describe(&self, _: &str, _: &serde_json::Value) -> String {
            "write 3 characters to /home/root/n.txt".into()
        }
    }

    fn waiting_run(cx: &AppCx) -> iyakki_agent::runs::RunSession {
        let mut run = iyakki_agent::runs::RunSession::start(Some(cx.services.runs.clone()), "root", "save my note", Arc::new(Script(Default::default())), Arc::new(WriteTools), vec![], Default::default());
        for _ in 0..400 {
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
        for _ in 0..400 {
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
        run.decide(&p.call_id, iyakki_agent::agent::Verdict::Deny);
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
        cx.services.notify("root", Kind::Info, "one", "", None).unwrap();
        cx.services.notify("root", Kind::Info, "two", "", None).unwrap();
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
        let ids: Vec<String> = ["nas", "key"].iter().map(|n| cx.services.secret_requests.create("root", n, "SFTP password for the NAS", "password", 5).unwrap()).collect();
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
        assert_eq!(cx.services.user_get("root", "svc.nas").unwrap().as_str(), "Sup3r-Secret!");
        assert_eq!(cx.services.secret_requests.wait(&ids[0], std::time::Duration::from_millis(20)), Some(iyakki_agent::secretreq::Outcome::Stored("svc.nas".into())));
        let log = audit(&cx);
        assert!(log.contains("vault.user.set") && log.contains("svc.nas") && !log.contains("Sup3r-Secret!"), "{log}");
        assert_eq!(a.approval_rows().len(), 1);
        // n says no at once: the assistant is told it was cancelled
        key(&mut a, &cx, Key::Char('n'));
        assert!(a.approval_rows().is_empty() && !a.dialog_open());
        assert_eq!(cx.services.secret_requests.wait(&ids[1], std::time::Duration::from_millis(20)), Some(iyakki_agent::secretreq::Outcome::Cancelled));
        assert!(cx.services.user_get("root", "svc.key").is_err());
        // a locked vault is explained in the form and nothing is lost
        let cx2 = { let mut c = cx_with_admin_store("inbsec2"); c.services = services_in(&temp_dir("inbsec2-svc"), Arc::new(Offline)); c };
        let id = cx2.services.secret_requests.create("root", "x", "y", "token", 1).unwrap();
        let mut b = InboxApp::new(&cx2);
        b.handle(&Event::Tab(2), &cx2);
        key(&mut b, &cx2, Key::Char('y'));
        key(&mut b, &cx2, Key::Tab);
        typ(&mut b, &cx2, "abc12345");
        key(&mut b, &cx2, Key::Ctrl('s'));
        assert!(b.dialog_open() && cx2.services.secret_requests.get("root", &id).is_some(), "still waiting");
    }

    #[test]
    fn questions_about_the_assistant_in_the_browser_are_answered_in_the_approvals_page() {
        use iyakki_agent::browserconsent::{Answer, ConsentKind};
        let cx = admin_cx("inbbrowser");
        let mut a = InboxApp::new(&cx);
        a.handle(&Event::Tab(2), &cx);
        let c = &cx.services.browser.consent;
        let look = c.create("root", ConsentKind::Look, "gemini.google.com", "", "", 1).unwrap();
        let act = c.create("root", ConsentKind::Interact, "gemini.google.com", &iyakki_agent::browserconsent::willingness_text("gemini.google.com", None), "click button \"Send\"", 2).unwrap();
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
}
