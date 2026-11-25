//! INBOX: what wants your attention, in three pages. **Notifications** (reminders that rang, runs that finished, new
//! devices, downloads, warnings: read, flag, snooze, add to the calendar), **Tasks** (a small to-do list the assistants can
//! add to: pending, doing, done) and **Approvals** (tool calls that agent runs are waiting to do: approve or reject each one
//! on its own, from whichever app started the run). Opened from the bell at the top of every screen or with F9.
//! Approvals are never bundled: there is no "approve all" for tool calls. (The one exception is the person's own choice for the
//! assistant in their browser: "Approve all actions on <site> for this session", asked as a question of its own; see
//! `browser_consent.rs` and docs/BROWSER-AGENT.md.)

use iyakki_tile_api::calendar::{parse_local, Repeat};
use iyakki_clock::Zone;
use iyakki_inbox_model::{Kind, Notification, Priority, Status, Task, TaskPatch, TaskSort};
use iyakki_tile_api::inbox::PendingApproval;

use crate::consent::{row_summary, ConsentDialog};
use crate::port::InboxPort;
use iyakki_uikit::widgets::{render_confirm, Form, FormEvent};
use iyakki_uikit::blink::blink;
use iyakki_uikit::gfx::{rgb, Face, Gfx, Icon};
use iyakki_uikit::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

/// What a screen of the tile answers to an event: stay, or go back to the launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Stay,
    Back,
}

pub const ACCENT: [u8; 4] = rgb(255, 120, 90);
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
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Open => "open",
            Filter::Done => "done",
        }
    }
    pub fn next(self) -> Filter {
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
    /// The notes inside a group: the group's id and the chosen line (newest first).
    Group(String, usize),
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
    pane: iyakki_uikit::textsel::Pane,
    /// The small pictures of the notes that have one (a motion event's thumbnail), decoded once; `None`: it could not be read.
    pub thumbs: std::collections::BTreeMap<String, Option<tiny_skia::Pixmap>>,
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
    /// The tile over any port (a fake in tests).
    pub fn with_port(port: &dyn InboxPort) -> InboxApp {
        // group an older file and drop what is past its keeping time; say so once
        let _ = port.maintain(&port.user(), iyakki_clock::now_unix());
        let notice = port.take_notice(&port.user());
        let mut a = InboxApp { page: Page::Notifications, notes: vec![], tasks: vec![], approvals: vec![], sel: [0; 3], filter: Filter::All, mode: Mode::Browse, status: None, request: None, zone: Zone::system(), snoozed: 0, pane: iyakki_uikit::textsel::Pane::new(61_400), thumbs: Default::default() };
        if let Some(m) = notice {
            a.ok(m);
        }
        a.reload(port);
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
    /// The id of the group that is open, if one is.
    pub fn open_group(&self) -> Option<&str> {
        match &self.mode {
            Mode::Group(id, _) => Some(id),
            _ => None,
        }
    }
    pub fn dialog_open(&self) -> bool {
        !matches!(self.mode, Mode::Browse)
    }

    pub fn ok(&mut self, m: impl Into<String>) {
        self.status = Some((m.into(), true));
    }
    pub fn fail(&mut self, m: impl Into<String>) {
        self.status = Some((m.into(), false));
    }

    /// Read everything again (after an action, and about twice a second).
    pub fn reload(&mut self, port: &dyn InboxPort) {
        let user = port.user();
        let (u, now) = (user.as_str(), iyakki_clock::now_unix());
        let all = port.list(u);
        self.snoozed = all.iter().filter(|n| n.is_snoozed(now)).count();
        self.notes = all.into_iter().filter(|n| !n.is_snoozed(now)).collect();
        let ids: std::collections::BTreeSet<&str> = self.notes.iter().filter(|n| n.has_picture).map(|n| n.id.as_str()).collect();
        self.thumbs.retain(|k, _| ids.contains(k.as_str()));
        for n in self.notes.iter().filter(|n| n.has_picture) {
            if !self.thumbs.contains_key(&n.id) {
                let pm = port.picture(u, &n.id).and_then(|b| port.decode_thumb(&b));
                self.thumbs.insert(n.id.clone(), pm);
            }
        }
        // grouped Pending, Doing, Done, Dropped; soonest due first inside a group
        let group = |s: Status| match s {
            Status::Pending => 0,
            Status::Doing => 1,
            Status::Done => 2,
            Status::Dropped => 3,
        };
        let mut t = port.tasks(u, TaskSort::Due);
        t.retain(|t| match self.filter {
            Filter::All => true,
            Filter::Open => t.status.is_open(),
            Filter::Done => !t.status.is_open(),
        });
        t.sort_by_key(|t| group(t.status));
        self.tasks = t;
        self.approvals = port.approvals(u);
        // secrets the assistant asked for show as approvals of their own kind: the person types the value, not the assistant
        let mut asked: Vec<PendingApproval> = vec![];
        for r in port.secret_pending(u) {
            let summary = format!("Enter a {} for “{}”. It goes straight into your vault as {} and the assistant never sees it.", r.kind.replace('_', " "), r.purpose, r.name);
            asked.push(PendingApproval { run_id: "vault".into(), user: u.to_string(), prompt: format!("needs a secret: {}", r.purpose), call_id: r.id, tool: "vault_request".into(), summary, detail: String::new(), risk: "low", since: r.since });
        }
        // questions about the assistant in the person's browser (the wait of a running tool ends when they answer)
        for r in port.consent_pending(u) {
            asked.push(PendingApproval { run_id: "browser".into(), user: u.to_string(), prompt: r.title.clone(), call_id: r.id.clone(), tool: "browser_consent".into(), summary: row_summary(&r), detail: r.detail.clone(), risk: "medium", since: r.since });
        }
        asked.append(&mut self.approvals);
        self.approvals = asked;
        for (i, n) in [self.notes.len(), self.tasks.len(), self.approvals.len()].into_iter().enumerate() {
            self.sel[i] = self.sel[i].min(n.saturating_sub(1));
        }
    }

    pub fn note(&self) -> Option<&Notification> {
        self.notes.get(self.sel[0])
    }
    pub fn task(&self) -> Option<&Task> {
        self.tasks.get(self.sel[1])
    }
    pub fn approval(&self) -> Option<&PendingApproval> {
        self.approvals.get(self.sel[2])
    }

    pub fn tomorrow_9(&self) -> (String, String) {
        let l = self.zone.local(iyakki_clock::now_unix() + 86_400);
        (format!("{:04}-{:02}-{:02}", l.year, l.month, l.day), "09:00".into())
    }

    pub fn task_form(&self, t: Option<&Task>) -> Form {
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

    pub fn calendar_form(&self, title: &str, when: Option<i64>) -> Form {
        let (d, tm) = match when {
            Some(w) => {
                let t = local_text(&self.zone, w);
                (t[..10].to_string(), t[11..].to_string())
            }
            None => self.tomorrow_9(),
        };
        Form::new("Add to the calendar").text("Title", title, "the event's name").text("Date", &d, "YYYY-MM-DD").text("Starts at", &tm, "HH:MM")
    }

    pub fn parse_due(&self, port: &dyn InboxPort, s: &str) -> Result<Option<i64>, String> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(None);
        }
        let (t, all_day) = parse_local(port.calendar().as_ref(), s)?;
        // a date alone means the end of that day, so a task due today is not late all day
        Ok(Some(if all_day { t + 86_399 } else { t }))
    }

    // ------------------------------------------------------------ actions

    pub fn open_link(&mut self, port: &dyn InboxPort) {
        let Some(n) = self.note().cloned() else { return };
        if n.is_group() {
            // a group opens its list; looking at it counts as reading it
            let _ = port.mark_read(&port.user(), &n.id, true);
            port.audit("inbox.open", &format!("{}: {}", n.kind.name(), n.title));
            self.mode = Mode::Group(n.id, 0);
            self.reload(port);
            return;
        }
        let _ = port.mark_read(&port.user(), &n.id, true);
        port.audit("inbox.open", &format!("{}: {}", n.kind.name(), n.title));
        match &n.link {
            Some(l) if !l.app.is_empty() && l.app != "inbox" => self.request = Some((l.app.clone(), l.id.clone())),
            _ => self.ok("This note has nothing to open."),
        }
        self.reload(port);
    }

    pub fn note_action(&mut self, port: &dyn InboxPort, k: char) {
        let (u, now) = (port.user(), iyakki_clock::now_unix());
        if k == 'A' {
            match port.mark_all_read(&u) {
                Ok(n) => {
                    port.audit("inbox.read_all", &n.to_string());
                    self.ok(format!("Marked {n} as read."));
                }
                Err(e) => self.fail(e),
            }
            self.reload(port);
            return;
        }
        if k == 'X' {
            self.mode = Mode::Confirm("Remove every notification you have read (flagged ones stay)?".into(), Confirmed::ClearRead);
            return;
        }
        let Some(n) = self.note().cloned() else { return };
        let r = match k {
            'm' => return self.mute_source(port, &n),
            't' => return self.trust_devices(port, &n),
            'i' => port.mark_read(&u, &n.id, true).map(|_| {
                port.audit("inbox.ignore", &n.title);
                self.ok("Ignored: marked as read, nothing trusted.");
            }),
            'g' => {
                let what = if n.is_group() { format!("Clear this group: {} notifications (“{}”)?", n.count.max(n.items.len() as u32), n.title) } else { format!("Delete “{}”?", n.title) };
                self.mode = Mode::Confirm(what, Confirmed::Note(n.id.clone()));
                return;
            }
            'r' => port.mark_read(&u, &n.id, !n.read).map(|_| port.audit("inbox.read", &n.title)),
            'f' => port.flag(&u, &n.id, !n.flagged).map(|_| port.audit(if n.flagged { "inbox.unflag" } else { "inbox.flag" }, &n.title)),
            's' => port.snooze(&u, &n.id, now + SNOOZE_SECS).map(|_| {
                port.audit("inbox.snooze", &n.title);
                self.ok("Snoozed for an hour.");
            }),
            'd' => {
                self.mode = Mode::Confirm(format!("Delete “{}”?", n.title), Confirmed::Note(n.id.clone()));
                return;
            }
            'c' => {
                // a reminder already is a calendar item: open it
                if n.kind == Kind::Reminder {
                    let _ = port.mark_read(&u, &n.id, true);
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
        self.reload(port);
    }

    /// Show, then Quiet (panel only), then Off, then Show again, for the source of this note.
    pub fn mute_source(&mut self, port: &dyn InboxPort, n: &Notification) {
        let u = &port.user();
        if n.source.is_empty() {
            return self.fail("This note has no source to mute.");
        }
        let next = port.level(u, &n.source).next();
        match port.set_level(u, &n.source, next) {
            Ok(()) => {
                port.audit("inbox.level", &format!("{} {}", n.source, next.name()));
                self.ok(format!("{}: {}. Press m again to change it, or use Settings \u{2192} Notifications.", n.source, next.label()));
            }
            Err(e) => self.fail(e),
        }
        self.reload(port);
    }

    /// Trust the devices of a new-devices note (they stop being new) and mark it read.
    pub fn trust_devices(&mut self, port: &dyn InboxPort, n: &Notification) {
        let u = &port.user();
        if n.kind != Kind::Device {
            return;
        }
        if !port.is_admin() {
            return self.fail("Only an administrator can trust devices.");
        }
        let macs: Vec<&str> = if n.items.is_empty() { n.link.iter().map(|l| l.id.as_str()).collect() } else { n.items.iter().map(|i| i.id.as_str()).collect() };
        let trusted = macs.iter().filter(|m| !m.is_empty() && port.devwatch_trust(m)).count();
        let _ = port.mark_read(u, &n.id, true);
        port.audit("inbox.trust_devices", &trusted.to_string());
        self.ok(format!("Trusted {trusted} device{}.", if trusted == 1 { "" } else { "s" }));
        self.reload(port);
    }

    pub fn task_action(&mut self, port: &dyn InboxPort, ev: &Event) {
        let (u, now) = (port.user(), iyakki_clock::now_unix());
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
            self.reload(port);
            return;
        }
        let Some(t) = self.task().cloned() else { return };
        let moved = |to: Status| port.move_task(&u, &t.id, to, now);
        let r: Result<String, String> = match key {
            Key::Enter | Key::Right if t.status.is_open() => moved(t.status.next()).map(|n| format!("“{}” is now {}.", n.title, n.status.name())),
            Key::Left => moved(if t.status == Status::Dropped { Status::Pending } else { t.status.prev() }).map(|n| format!("“{}” is now {}.", n.title, n.status.name())),
            Key::Char('x') => moved(Status::Dropped).map(|n| format!("Dropped “{}”.", n.title)),
            Key::Char('p') => port.update_task(&u, &t.id, TaskPatch { priority: Some(t.priority.cycle()), ..Default::default() }, now).map(|n| format!("Priority of “{}”: {}.", n.title, n.priority.name())),
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
                port.audit("inbox.task", &format!("{}: {}", t.title, m));
                self.ok(m);
            }
            Err(e) => self.fail(e),
        }
        self.reload(port);
        // the row moves to another group when its state changes: keep the selection on the same task
        if let Some(i) = self.tasks.iter().position(|x| x.id == t.id) {
            self.sel[1] = i;
        }
    }

    pub fn submit_task(&mut self, port: &dyn InboxPort, f: &Form, edit: &Option<String>) -> Result<(), String> {
        let user = port.user();
        let (u, now) = (user.as_str(), iyakki_clock::now_unix());
        let due = self.parse_due(port, &f.value(1))?;
        let pr = [Priority::Low, Priority::Normal, Priority::High][f.choice_idx(2).min(2)];
        match edit {
            Some(id) => {
                port.update_task(u, id, TaskPatch { title: Some(f.value(0)), notes: Some(f.value(3)), due: Some(due), priority: Some(pr), status: None }, now)?;
                port.audit("inbox.task.edit", &f.value(0));
            }
            None => {
                port.add_task(u, &f.value(0), &f.value(3), due, pr, "user", now)?;
                port.audit("inbox.task.add", &f.value(0));
            }
        }
        Ok(())
    }

    pub fn submit_calendar(&mut self, port: &dyn InboxPort, f: &Form, notes: &str) -> Result<(), String> {
        let (start, _) = parse_local(port.calendar().as_ref(), &format!("{} {}", f.value(1), f.value(2)))?;
        let e = port.calendar().add_event(&port.user(), &f.value(0), start, 60, false, Repeat::None, notes, "user", Some(10))?;
        port.audit("inbox.calendar", &e.title);
        Ok(())
    }

    pub fn is_browser(a: &PendingApproval) -> bool {
        a.run_id == "browser" && a.tool == "browser_consent"
    }

    /// Open the dialog for a browser question.
    pub fn open_browser(&mut self, port: &dyn InboxPort, a: &PendingApproval) {
        match port.consent_get(&port.user(), &a.call_id) {
            Some(r) => self.mode = Mode::Browser(ConsentDialog::new(r)),
            None => {
                self.fail("That question is not waiting any more.");
                self.reload(port);
            }
        }
    }

    pub fn is_secret(a: &PendingApproval) -> bool {
        a.run_id == "vault" && a.tool == "vault_request"
    }

    /// Open the private form for a secret request.
    pub fn open_secret(&mut self, port: &dyn InboxPort, a: &PendingApproval) {
        let name = port.secret_get(&port.user(), &a.call_id).map(|r| r.name).unwrap_or_else(|| "svc.secret".into());
        self.mode = Mode::Secret(iyakki_uikit::widgets::secret_form(&format!("Enter secret: {}", a.prompt.trim_start_matches("needs a secret: ")), &name, None), a.call_id.clone());
    }

    pub fn submit_secret(&mut self, port: &dyn InboxPort) {
        let Mode::Secret(form, id) = &mut self.mode else { return };
        let (name, value) = (form.value(0), form.raw(1));
        match port.answer_secret_request(&port.user(), id, &name, &value) {
            Ok(stored) => {
                // the log records that a secret with this name was stored, never the value
                port.audit("vault.user.set", &stored);
                self.mode = Mode::Browse;
                self.ok(format!("Stored as uvault:{stored}."));
                self.reload(port);
            }
            Err(e) => form.error = Some(e),
        }
    }

    pub fn decide(&mut self, port: &dyn InboxPort, a: &PendingApproval, approve: bool, reason: &str) {
        if Self::is_browser(a) {
            if approve {
                self.open_browser(port, a);
            } else if let Some(r) = port.consent_get(&port.user(), &a.call_id) {
                // "n" says no: Deny (or Cancel for a hand-over)
                let d = ConsentDialog::new(r);
                match d.answer_of(&Key::Esc).map(|ans| port.consent_answer(&d.req, ans)) {
                    Some(Ok(())) => self.ok("Said no; the assistant was told."),
                    Some(Err(e)) => self.fail(e),
                    None => self.fail("Open the question (Enter) and choose."),
                }
                self.reload(port);
            }
            return;
        }
        if Self::is_secret(a) {
            if !approve {
                match port.cancel_secret_request(&port.user(), &a.call_id) {
                    Ok(()) => {
                        port.audit("inbox.reject", &format!("vault_request: {}", a.prompt.chars().take(120).collect::<String>()));
                        self.ok("Cancelled; the assistant was told you chose not to.");
                    }
                    Err(e) => self.fail(e),
                }
                self.reload(port);
            } else {
                self.open_secret(port, a);
            }
            return;
        }
        let u = &port.user();
        match port.decide_approval(u, &a.run_id, &a.call_id, approve) {
            Ok(_) => {
                let what = format!("{}: {}", a.tool, a.summary.chars().take(160).collect::<String>());
                let detail = if approve || reason.is_empty() { what } else { format!("{what} (reason: {})", reason.chars().take(200).collect::<String>()) };
                port.audit(if approve { "inbox.approve" } else { "inbox.reject" }, &detail);
                self.ok(if approve { format!("Approved {}.", a.tool) } else { format!("Rejected {}.", a.tool) });
            }
            Err(e) => self.fail(e),
        }
        self.reload(port);
    }
}

impl InboxApp {
    pub fn dialog_title(&self) -> Option<String> {
        match &self.mode {
            Mode::Browse | Mode::Group(..) => None,
            Mode::Task(f, _) | Mode::Calendar(f, _) | Mode::Reject(f, _) | Mode::Secret(f, _) => Some(f.title.clone()),
            Mode::Detail(_) => Some("Approval details".into()),
            Mode::Confirm(..) => Some("Are you sure?".into()),
            Mode::Browser(_) => Some("The assistant asks about your browser".into()),
        }
    }

    pub fn title(&self) -> &'static str {
        "Inbox"
    }
    pub fn icon(&self) -> Icon {
        Icon::Bell
    }
    pub fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    pub fn hint(&self) -> String {
        match self.page {
            Page::Notifications => "Tab/1-3 pages · Enter open (a group shows its list) · r read · f flag · c calendar · s snooze · d delete · g clear group · m mute source · A all read · X clear read".into(),
            Page::Tasks => "Tab/1-3 pages · n new · Enter/→ next · ← back · e edit · p priority · c calendar · x drop · d delete · f filter".into(),
            Page::Approvals => "Tab/1-3 pages · y approve · n reject · Enter details (each one on its own)".into(),
        }
    }
    pub fn help(&self) -> Vec<String> {
        let mut v = vec!["Tab or 1 2 3 · Notifications, Tasks, Approvals".to_string(), "↑ ↓ · choose a row".to_string(), "F9 or the bell · open or close the inbox from anywhere".to_string()];
        v.extend(self.hint().split(" · ").skip(1).map(str::to_string));
        v
    }

    pub fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }

    pub fn context_items(&mut self, x: i32, y: i32) -> Vec<iyakki_uikit::ctxmenu::Item> {
        if matches!(self.mode, Mode::Detail(_)) {
            self.pane.context_items(x, y)
        } else {
            Vec::new()
        }
    }
}

impl InboxApp {
    pub fn handle_with(&mut self, ev: &Event, port: &dyn InboxPort) -> Flow {
        if matches!(ev, Event::Tick) {
            // new things arrive while it is open (not while a form is being filled in)
            if matches!(self.mode, Mode::Browse) {
                self.reload(port);
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
                            Mode::Task(form, edit) => match self.submit_task(port, &form, &edit) {
                                Ok(()) => self.ok(if edit.is_some() { "Task saved." } else { "Task added." }),
                                Err(e) => {
                                    let mut form = form;
                                    form.error = Some(e);
                                    self.mode = Mode::Task(form, edit);
                                }
                            },
                            Mode::Calendar(form, notes) => match self.submit_calendar(port, &form, &notes) {
                                Ok(()) => self.ok("Added to the calendar."),
                                Err(e) => {
                                    let mut form = form;
                                    form.error = Some(e);
                                    self.mode = Mode::Calendar(form, notes);
                                }
                            },
                            Mode::Reject(form, a) => self.decide(port, &a, false, &form.value(0)),
                            _ => {}
                        }
                        self.reload(port);
                    }
                    _ => {}
                }
                return Flow::Stay;
            }
            Mode::Secret(f, _) => {
                match f.handle_event(ev) {
                    FormEvent::Cancel => self.mode = Mode::Browse,
                    FormEvent::Submit => self.submit_secret(port),
                    _ => {}
                }
                return Flow::Stay;
            }
            Mode::Browser(d) => {
                if let Event::Key(k) = ev {
                    if let Some(ans) = d.answer_of(k) {
                        let d = d.clone();
                        match port.consent_answer(&d.req, ans) {
                            Ok(()) => self.ok("Answered; the assistant carries on."),
                            Err(e) => self.fail(e),
                        }
                        self.mode = Mode::Browse;
                        self.reload(port);
                    } else if matches!(k, Key::Enter) {
                        self.mode = Mode::Browse;
                    }
                }
                return Flow::Stay;
            }
            Mode::Group(id, sel) => {
                let id = id.clone();
                let note = port.list(&port.user()).into_iter().find(|n| n.id == id);
                let Some(n) = note else {
                    self.mode = Mode::Browse;
                    return Flow::Stay;
                };
                let len = n.items.len();
                match ev {
                    Event::Row(i) => *sel = (*i).min(len.saturating_sub(1)),
                    Event::Key(Key::Up) => *sel = sel.saturating_sub(1),
                    Event::Key(Key::Down) => *sel = (*sel + 1).min(len.saturating_sub(1)),
                    Event::Key(Key::Esc | Key::Left | Key::Char('q')) => self.mode = Mode::Browse,
                    Event::Key(Key::Enter) => {
                        // newest first on screen
                        let item = len.checked_sub(1 + *sel).and_then(|i| n.items.get(i)).cloned();
                        if let (Some(l), Some(it)) = (n.link.as_ref().filter(|l| !l.app.is_empty() && l.app != "inbox"), item) {
                            self.request = Some((l.app.clone(), it.id));
                            self.mode = Mode::Browse;
                        }
                    }
                    Event::Key(Key::Char('t')) => self.trust_devices(port, &n),
                    Event::Key(Key::Char('i')) => {
                        let _ = port.mark_read(&port.user(), &n.id, true);
                        port.audit("inbox.ignore", &n.title);
                        self.mode = Mode::Browse;
                        self.ok("Ignored: marked as read, nothing trusted.");
                        self.reload(port);
                    }
                    Event::Key(Key::Char('g')) => self.mode = Mode::Confirm(format!("Clear this group: {} notifications (“{}”)?", n.count.max(len as u32), n.title), Confirmed::Note(n.id.clone())),
                    Event::Key(Key::Char('m')) => {
                        self.mute_source(port, &n);
                    }
                    _ => {}
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
                    let u = port.user();
                    let r = match what {
                        Confirmed::Note(id) => port.delete(&u, &id).map(|_| "Deleted.".to_string()),
                        Confirmed::Task(id) => port.delete_task(&u, &id).map(|_| "Task deleted.".to_string()),
                        Confirmed::ClearRead => port.clear_read(&u).map(|n| format!("Removed {n} read notification{}.", if n == 1 { "" } else { "s" })),
                    };
                    match r {
                        Ok(m) => {
                            port.audit("inbox.delete", &m);
                            self.ok(m);
                        }
                        Err(e) => self.fail(e),
                    }
                    self.reload(port);
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
                self.reload(port);
            }
            Event::Row(i) => {
                let len = [self.notes.len(), self.tasks.len(), self.approvals.len()][self.page as usize];
                self.sel[self.page as usize] = (*i).min(len.saturating_sub(1));
            }
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Tab) => {
                self.page = [Page::Tasks, Page::Approvals, Page::Notifications][self.page as usize];
                self.reload(port);
            }
            Event::Key(Key::BackTab) => {
                self.page = [Page::Approvals, Page::Notifications, Page::Tasks][self.page as usize];
                self.reload(port);
            }
            Event::Key(Key::Char(c @ '1'..='3')) => {
                self.page = [Page::Notifications, Page::Tasks, Page::Approvals][*c as usize - '1' as usize];
                self.reload(port);
            }
            Event::Key(Key::Up) => self.sel[self.page as usize] = self.sel[self.page as usize].saturating_sub(1),
            Event::Key(Key::Down) => {
                let len = [self.notes.len(), self.tasks.len(), self.approvals.len()][self.page as usize];
                self.sel[self.page as usize] = (self.sel[self.page as usize] + 1).min(len.saturating_sub(1));
            }
            Event::Key(k) => match self.page {
                Page::Notifications => match k {
                    Key::Enter => self.open_link(port),
                    Key::Right | Key::Char(' ') if self.note().is_some_and(Notification::is_group) => self.open_link(port),
                    Key::Char(c @ ('r' | 'f' | 'c' | 's' | 'd' | 'A' | 'X' | 'm' | 't' | 'i' | 'g')) => self.note_action(port, *c),
                    _ => {}
                },
                Page::Tasks => self.task_action(port, ev),
                Page::Approvals => match k {
                    Key::Char('y') => {
                        if let Some(a) = self.approval().cloned() {
                            self.decide(port, &a, true, "");
                        }
                    }
                    Key::Char('n') => {
                        if let Some(a) = self.approval().cloned().filter(|a| Self::is_secret(a) || Self::is_browser(a)) {
                            self.decide(port, &a, false, "");
                        } else if let Some(a) = self.approval().cloned() {
                            self.mode = Mode::Reject(Form::new(&format!("Reject {}", a.tool)).text("Reason", "", "optional: why not?"), a);
                        }
                    }
                    Key::Enter => {
                        if let Some(a) = self.approval().cloned() {
                            if Self::is_browser(&a) {
                                self.open_browser(port, &a);
                            } else if Self::is_secret(&a) {
                                self.open_secret(port, &a);
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

    pub fn render_with(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn InboxPort, tick: u64) {
        let now = iyakki_clock::now_unix();
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 10.0 * s;
        let counts = [self.notes.iter().filter(|n| !n.read).count(), self.tasks.iter().filter(|t| t.status.is_open()).count(), self.approvals.len()];
        let pages = [("Notifications", Page::Notifications), ("Tasks", Page::Tasks), ("Approvals", Page::Approvals)];
        let items: Vec<(String, Event)> = pages.iter().enumerate().map(|(i, (label, _))| (if counts[i] > 0 { format!("{label} ({})", counts[i]) } else { label.to_string() }, Event::Tab(i))).collect();
        let cur = pages.iter().position(|(_, p)| self.page == *p).unwrap_or(0);
        iyakki_uikit::fx::tab_strip(g, x, y, s, w, &items, cur, if cur == 2 && counts[2] > 0 { ERR } else { ACCENT });
        y += 50.0 * s;
        if let Some((m, ok)) = &self.status {
            iyakki_uikit::fx::status_line(g, x, y - 8.0 * s, w, s, m, *ok);
            y += 18.0 * s;
        }
        let body = R::new(x, y, w, area.y + area.h - y - 88.0 * s);
        let actions = R::new(x, area.y + area.h - 78.0 * s, w, 66.0 * s);
        match self.page {
            Page::Notifications => self.draw_notes(g, body, actions, s, now),
            Page::Tasks => self.draw_tasks(g, body, actions, s, now),
            Page::Approvals => self.draw_approvals(g, body, actions, s, now),
        }
        match &self.mode {
            Mode::Task(f, _) | Mode::Calendar(f, _) | Mode::Reject(f, _) => f.render(g, area, s, ACCENT, blink(tick)),
            Mode::Secret(f, _) => f.render(g, area, s, ACCENT, blink(tick)),
            Mode::Browser(d) => d.render(g, area, s),
            Mode::Confirm(text, _) => render_confirm(g, area, s, text, "Yes"),
            Mode::Detail(a) => draw_detail(g, area, s, a, now, &mut self.pane),
            Mode::Group(id, sel) => {
                let note = port.list(&port.user()).into_iter().find(|n| n.id == *id);
                if let Some(n) = note {
                    draw_group(g, area, s, &n, *sel, now, port.is_admin());
                }
            }
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

/// Buttons in a row; when they do not fit they go on a second line (the area is two lines high), and what still does not fit
/// is left out (every one has a key).
fn action_row(g: &mut Gfx, r: R, s: f32, items: &[(&str, [u8; 4], Key)]) {
    let line = 13.5 * s * 1.7 + 6.0 * s;
    // where each button goes; None when it does not fit
    let place = |g: &mut Gfx, strip: bool| -> Vec<Option<(f32, f32, String)>> {
        let (mut bx, mut by) = (r.x, r.y);
        items
            .iter()
            .map(|(label, _, _)| {
                let text = if strip { label.split(" (").next().unwrap_or(label).to_string() } else { label.to_string() };
                let bw = g.text_width(13.5 * s, &text, Face::Bold) + 13.5 * s * 1.7;
                if bx + bw > r.x + r.w {
                    bx = r.x;
                    by += line;
                }
                if bx + bw > r.x + r.w || by + line > r.y + r.h + 4.0 * s {
                    return None;
                }
                let at = (bx, by, text);
                bx += bw + 8.0 * s;
                Some(at)
            })
            .collect()
    };
    // full labels (with their key) when everything fits, else the short labels: every key still works
    let mut spots = place(g, false);
    if spots.iter().any(Option::is_none) {
        spots = place(g, true);
    }
    for ((_, col, key), spot) in items.iter().zip(spots) {
        if let Some((x, y, text)) = spot {
            g.button(x, y, 13.5 * s, &text, *col, Event::Key(key.clone()));
        }
    }
}

impl InboxApp {
    pub fn draw_notes(&mut self, g: &mut Gfx, body: R, actions: R, s: f32, now: i64) {
        if self.notes.is_empty() {
            iyakki_uikit::fx::empty_state(g, body, s, Icon::Bell, "Nothing new", "Reminders, finished runs, new devices and warnings will show up here.", ACCENT);
        }
        let rh = 62.0 * s;
        let fit = ((body.h / (rh + 6.0 * s)) as usize).max(1);
        let first = first_visible(self.sel[0], fit);
        for (row, (i, n)) in self.notes.iter().enumerate().skip(first).take(fit).enumerate() {
            let r = R::new(body.x, body.y + row as f32 * (rh + 6.0 * s), body.w, rh);
            iyakki_uikit::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == self.sel[0], g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), &n.id);
            g.rect(r.x, r.y + 8.0 * s, 4.0 * s, r.h - 16.0 * s, kind_color(n.kind));
            let face = if n.read { Face::Sans } else { Face::Bold };
            let star = if n.flagged { "★ " } else { "" };
            // a note with a picture shows it small at the left, and its words start after it
            let pic = if n.has_picture { 58.0 * s } else { 0.0 };
            if let Some(Some(pm)) = self.thumbs.get(&n.id) {
                let (bw, bh) = (50.0 * s, r.h - 12.0 * s);
                let k = (bw / pm.width() as f32).min(bh / pm.height() as f32);
                let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
                g.pm_mut().draw_pixmap(0, 0, pm.as_ref(), &paint, tiny_skia::Transform::from_translate(r.x + 14.0 * s, r.y + 6.0 * s).pre_scale(k, k), None);
            }
            let tw = r.w - 200.0 * s - pic;
            let t = g.fit(15.5 * s, &format!("{star}{}", n.title), face, tw);
            g.text(r.x + 16.0 * s + pic, r.y + 8.0 * s, 15.5 * s, if n.read { MUTED } else { TEXT }, &t, face);
            // a group shows what its newest note says, and how many there are
            let first_line = match n.items.last().filter(|_| n.is_group()) {
                Some(it) => format!("latest: {}", it.text),
                None => n.body.lines().next().unwrap_or("").to_string(),
            };
            let b = g.fit(13.0 * s, &first_line, Face::Sans, tw);
            g.text(r.x + 16.0 * s + pic, r.y + 34.0 * s, 13.0 * s, DIM, &b, Face::Sans);
            let meta = if n.is_group() { format!("{} · \u{25b8} {} · last {}", n.kind.name(), n.count.max(n.items.len() as u32), ago(now, n.time)) } else { format!("{} · {}", n.kind.name(), ago(now, n.time)) };
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
        action_row(g, actions, s, &[("Open", ACCENT, Key::Enter), ("Read (r)", MUTED, Key::Char('r')), ("Flag (f)", AMBER, Key::Char('f')), ("Calendar (c)", BLUE, Key::Char('c')), ("Snooze (s)", MUTED, Key::Char('s')), ("Delete (d)", ERR, Key::Char('d')), ("Clear group (g)", ERR, Key::Char('g')), ("Mute source (m)", AMBER, Key::Char('m')), ("All read (A)", MUTED, Key::Char('A')), ("Clear read (X)", MUTED, Key::Char('X'))]);
    }

    pub fn draw_tasks(&mut self, g: &mut Gfx, body: R, actions: R, s: f32, now: i64) {
        let head = format!("Showing: {} (f changes it)", self.filter.label());
        g.text(body.x, body.y - 4.0 * s, 12.5 * s, DIM, &head, Face::Sans);
        let body = R::new(body.x, body.y + 16.0 * s, body.w, body.h - 16.0 * s);
        if self.tasks.is_empty() {
            iyakki_uikit::fx::empty_state(g, body, s, Icon::Bell, "No tasks here", "Press n to add one; assistants can add to-dos for you too.", ACCENT);
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
                    iyakki_uikit::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == self.sel[1], g.hovered(r), ACCENT);
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

    pub fn draw_approvals(&mut self, g: &mut Gfx, body: R, actions: R, s: f32, now: i64) {
        if self.approvals.is_empty() {
            iyakki_uikit::fx::empty_state(g, body, s, Icon::Bell, "Nothing is waiting for your answer", "When an assistant wants to do something that needs your say-so, it shows up here.", ACCENT);
        }
        let rh = 92.0 * s;
        let fit = ((body.h / (rh + 6.0 * s)) as usize).max(1);
        let first = first_visible(self.sel[2], fit);
        for (row, (i, a)) in self.approvals.iter().enumerate().skip(first).take(fit).enumerate() {
            let r = R::new(body.x, body.y + row as f32 * (rh + 6.0 * s), body.w, rh);
            iyakki_uikit::fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == self.sel[2], g.hovered(r), ACCENT);
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

/// The notes inside a group, newest first, with the actions that make sense for it.
fn draw_group(g: &mut Gfx, area: R, s: f32, n: &Notification, sel: usize, now: i64, admin: bool) {
    let p = iyakki_uikit::fx::modal(g, area, s, (area.w - 80.0 * s).min(820.0 * s), area.h - 80.0 * s, ACCENT, false);
    let head = g.fit(19.0 * s, &n.title, Face::Bold, p.w - 48.0 * s);
    g.text(p.x + 24.0 * s, p.y + 18.0 * s, 19.0 * s, TEXT, &head, Face::Bold);
    let more = (n.count as usize).saturating_sub(n.items.len());
    let sub = format!("{} · {} note{}{}", n.source, n.items.len(), if n.items.len() == 1 { "" } else { "s" }, if more > 0 { format!(" shown, {more} older ones not kept") } else { String::new() });
    g.text(p.x + 24.0 * s, p.y + 46.0 * s, 12.5 * s, DIM, &sub, Face::Sans);
    let top = p.y + 72.0 * s;
    let foot = 2.0 * (13.5 * s * 1.7 + 6.0 * s) + 12.0 * s;
    let rh = 26.0 * s;
    let fit = (((p.y + p.h - foot - top) / rh) as usize).max(1);
    let len = n.items.len();
    let sel = sel.min(len.saturating_sub(1));
    let first = first_visible(sel, fit);
    for (row, i) in (first..len).take(fit).enumerate() {
        let it = &n.items[len - 1 - i];
        let r = R::new(p.x + 20.0 * s, top + row as f32 * rh, p.w - 40.0 * s, rh - 2.0 * s);
        iyakki_uikit::fx::row_at(g, r.x, r.y, r.w, r.h, 6.0 * s, i == sel, g.hovered(r), ACCENT);
        g.hit(r, Event::Row(i), &format!("{}:{}", n.id, i));
        let age = ago(now, it.time);
        let aw = g.text_width(12.5 * s, &age, Face::Sans);
        g.text(r.x + r.w - aw - 10.0 * s, r.y + 5.0 * s, 12.5 * s, DIM, &age, Face::Sans);
        let t = g.fit(13.5 * s, &it.text, Face::Sans, r.w - aw - 36.0 * s);
        g.text(r.x + 12.0 * s, r.y + 4.0 * s, 13.5 * s, TEXT, &t, Face::Sans);
    }
    let mut items: Vec<(&str, [u8; 4], Key)> = vec![("Open (Enter)", ACCENT, Key::Enter)];
    if n.kind == Kind::Device && admin {
        items.push(("Trust all (t)", OK, Key::Char('t')));
    }
    if n.kind == Kind::Device {
        items.push(("Ignore (i)", MUTED, Key::Char('i')));
    }
    items.extend([("Clear group (g)", ERR, Key::Char('g')), ("Mute source (m)", AMBER, Key::Char('m')), ("Close (Esc)", MUTED, Key::Esc)]);
    action_row(g, R::new(p.x + 24.0 * s, p.y + p.h - foot + 4.0 * s, p.w - 48.0 * s, foot - 8.0 * s), s, &items);
}

fn draw_detail(g: &mut Gfx, area: R, s: f32, a: &PendingApproval, now: i64, pane: &mut iyakki_uikit::textsel::Pane) {
    let p = iyakki_uikit::fx::modal(g, area, s, (area.w - 80.0 * s).min(820.0 * s), area.h - 80.0 * s, ACCENT, false);
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
