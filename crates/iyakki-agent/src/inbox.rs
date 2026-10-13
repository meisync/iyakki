//! The INBOX: each person's notifications and tasks, one JSON file per user. Notifications are what the system and the
//! assistants want the person to know (a reminder rang, a run finished, a new device appeared); tasks are a small
//! to-do list the person and the assistants share. Approvals are not stored here: they are read live from the runs
//! that are waiting (see [`crate::runs::RunStore::pending_approvals`]).
//!
//! Everything is capped (counts and sizes), written atomically with owner-only permissions, and validated: a user name
//! that could climb out of the directory is refused.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const MAX_NOTIFICATIONS: usize = 300;
pub const MAX_TASKS: usize = 300;
pub const MAX_TITLE: usize = 160;
pub const MAX_BODY: usize = 2000;
/// How many notes and tasks the assistants may leave per person in an hour.
pub const AGENT_PER_HOUR: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Info,
    Warning,
    Error,
    Reminder,
    Agent,
    System,
    Device,
    Download,
}

impl Kind {
    pub const ALL: [Kind; 8] = [Kind::Info, Kind::Warning, Kind::Error, Kind::Reminder, Kind::Agent, Kind::System, Kind::Device, Kind::Download];
    pub fn name(self) -> &'static str {
        match self {
            Kind::Info => "info",
            Kind::Warning => "warning",
            Kind::Error => "error",
            Kind::Reminder => "reminder",
            Kind::Agent => "agent",
            Kind::System => "system",
            Kind::Device => "device",
            Kind::Download => "download",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == s.trim().to_lowercase())
    }
}

/// Where a notification points: an app (`calendar`, `runs`, `scan`, …) and optionally an item in it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub app: String,
    #[serde(default)]
    pub id: String,
}

impl Link {
    pub fn new(app: &str, id: &str) -> Link {
        Link { app: app.into(), id: id.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub time: i64,
    pub kind: Kind,
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// Who or what raised it: `calendar`, `runs`, an assistant's name, …
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub link: Option<Link>,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub flagged: bool,
    /// Hidden (and not counted) until this instant.
    #[serde(default)]
    pub snoozed_until: i64,
}

impl Notification {
    pub fn is_snoozed(&self, now: i64) -> bool {
        self.snoozed_until > now
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    Normal,
    High,
}

impl Priority {
    pub fn name(self) -> &'static str {
        match self {
            Priority::Low => "low",
            Priority::Normal => "normal",
            Priority::High => "high",
        }
    }
    pub fn parse(s: &str) -> Option<Priority> {
        match s.trim().to_lowercase().as_str() {
            "low" => Some(Priority::Low),
            "normal" | "" => Some(Priority::Normal),
            "high" => Some(Priority::High),
            _ => None,
        }
    }
    /// Low → normal → high → low.
    pub fn cycle(self) -> Priority {
        match self {
            Priority::Low => Priority::Normal,
            Priority::Normal => Priority::High,
            Priority::High => Priority::Low,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pending,
    Doing,
    Done,
    Dropped,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Doing => "doing",
            Status::Done => "done",
            Status::Dropped => "dropped",
        }
    }
    pub fn parse(s: &str) -> Option<Status> {
        match s.trim().to_lowercase().as_str() {
            "pending" | "todo" => Some(Status::Pending),
            "doing" | "started" | "in_progress" => Some(Status::Doing),
            "done" => Some(Status::Done),
            "dropped" => Some(Status::Dropped),
            _ => None,
        }
    }
    /// pending → doing → done (done and dropped stay).
    pub fn next(self) -> Status {
        match self {
            Status::Pending => Status::Doing,
            Status::Doing | Status::Done => Status::Done,
            Status::Dropped => Status::Dropped,
        }
    }
    pub fn prev(self) -> Status {
        match self {
            Status::Done => Status::Doing,
            Status::Doing | Status::Pending => Status::Pending,
            Status::Dropped => Status::Dropped,
        }
    }
    pub fn is_open(self) -> bool {
        matches!(self, Status::Pending | Status::Doing)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub due: Option<i64>,
    pub priority: Priority,
    pub status: Status,
    /// `user`, or the name of the assistant that added it.
    pub created_by: String,
    pub created: i64,
    pub updated: i64,
}

impl Task {
    pub fn is_overdue(&self, now: i64) -> bool {
        self.status.is_open() && self.due.is_some_and(|d| d < now)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskSort {
    /// Soonest due first; tasks without a date last.
    Due,
    /// High first.
    Priority,
    /// Newest first.
    Created,
}

/// Changes to a task; `None` leaves a field alone.
#[derive(Clone, Debug, Default)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub notes: Option<String>,
    /// `Some(None)` clears the date.
    pub due: Option<Option<i64>>,
    pub priority: Option<Priority>,
    pub status: Option<Status>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub unread: usize,
    pub flagged: usize,
    pub open_tasks: usize,
    pub overdue: usize,
}

#[derive(Serialize, Deserialize, Default)]
struct Data {
    #[serde(default)]
    notifications: Vec<Notification>,
    #[serde(default)]
    tasks: Vec<Task>,
}

pub struct Inbox {
    dir: PathBuf,
    lock: Mutex<()>,
    /// When the assistants last left something, per person (for the hourly limit).
    agent_log: Mutex<HashMap<String, VecDeque<i64>>>,
}

fn new_id() -> String {
    let mut r = [0u8; 5];
    let _ = iyakki_sec::random_fill(&mut r);
    r.iter().map(|b| format!("{b:02x}")).collect()
}

fn clean(s: &str, max: usize, keep_newlines: bool) -> String {
    let t: String = s.chars().map(|c| if c.is_control() && !(keep_newlines && c == '\n') { ' ' } else { c }).collect();
    let t = t.trim();
    if t.chars().count() > max {
        let head: String = t.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    } else {
        t.to_string()
    }
}

impl Inbox {
    pub fn new(dir: &std::path::Path) -> Inbox {
        Inbox { dir: dir.into(), lock: Mutex::new(()), agent_log: Mutex::new(HashMap::new()) }
    }

    fn path(&self, user: &str) -> Result<PathBuf, String> {
        if iyakki_sec::policy::valid_username(user) { Ok(self.dir.join(format!("{user}.json"))) } else { Err("invalid user".into()) }
    }

    fn load(&self, user: &str) -> Result<Data, String> {
        match std::fs::read(self.path(user)?) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("the inbox file is damaged: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Data::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn save(&self, user: &str, d: &Data) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.path(user)?;
        let tmp = path.with_extension("tmp");
        {
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
            f.write_all(&serde_json::to_vec_pretty(d).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    /// Load, change, save — one at a time.
    fn edit<T>(&self, user: &str, f: impl FnOnce(&mut Data) -> Result<T, String>) -> Result<T, String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = self.load(user)?;
        let r = f(&mut d)?;
        self.save(user, &d)?;
        Ok(r)
    }

    // ------------------------------------------------------------ notifications

    pub fn add(&self, user: &str, kind: Kind, title: &str, body: &str, source: &str, link: Option<Link>, now: i64) -> Result<Notification, String> {
        let title = clean(title, MAX_TITLE, false);
        if title.is_empty() {
            return Err("a notification needs a title".into());
        }
        let n = Notification {
            id: new_id(),
            time: now,
            kind,
            title,
            body: clean(body, MAX_BODY, true),
            source: clean(source, 40, false),
            link: link.map(|l| Link { app: clean(&l.app, 24, false), id: clean(&l.id, 64, false) }),
            read: false,
            flagged: false,
            snoozed_until: 0,
        };
        self.edit(user, |d| {
            d.notifications.push(n.clone());
            // over the cap: the oldest read, unflagged ones go first, then the oldest of all
            while d.notifications.len() > MAX_NOTIFICATIONS {
                let drop = d
                    .notifications
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| x.read && !x.flagged)
                    .min_by_key(|(_, x)| x.time)
                    .or_else(|| d.notifications.iter().enumerate().filter(|(_, x)| !x.flagged).min_by_key(|(_, x)| x.time))
                    .or_else(|| d.notifications.iter().enumerate().min_by_key(|(_, x)| x.time))
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                d.notifications.remove(drop);
            }
            Ok(())
        })?;
        Ok(n)
    }

    /// Newest first (snoozed ones included; see [`Notification::is_snoozed`]).
    pub fn list(&self, user: &str) -> Vec<Notification> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = self.load(user).map(|d| d.notifications).unwrap_or_default();
        v.sort_by(|a, b| b.time.cmp(&a.time).then(b.id.cmp(&a.id)));
        v
    }

    fn with_note(&self, user: &str, id: &str, f: impl FnOnce(&mut Notification)) -> Result<(), String> {
        self.edit(user, |d| {
            let n = d.notifications.iter_mut().find(|n| n.id == id).ok_or("no such notification")?;
            f(n);
            Ok(())
        })
    }

    pub fn mark_read(&self, user: &str, id: &str, read: bool) -> Result<(), String> {
        self.with_note(user, id, |n| n.read = read)
    }

    pub fn mark_all_read(&self, user: &str) -> Result<usize, String> {
        self.edit(user, |d| {
            let mut c = 0;
            for n in d.notifications.iter_mut().filter(|n| !n.read) {
                n.read = true;
                c += 1;
            }
            Ok(c)
        })
    }

    pub fn flag(&self, user: &str, id: &str, flagged: bool) -> Result<(), String> {
        self.with_note(user, id, |n| n.flagged = flagged)
    }

    /// Hide it until `until`; it comes back unread.
    pub fn snooze(&self, user: &str, id: &str, until: i64) -> Result<(), String> {
        self.with_note(user, id, |n| {
            n.snoozed_until = until;
            n.read = false;
        })
    }

    pub fn delete(&self, user: &str, id: &str) -> Result<(), String> {
        self.edit(user, |d| {
            let before = d.notifications.len();
            d.notifications.retain(|n| n.id != id);
            if d.notifications.len() == before { Err("no such notification".into()) } else { Ok(()) }
        })
    }

    /// Remove everything that has been read, except what is flagged. Returns how many went.
    pub fn clear_read(&self, user: &str) -> Result<usize, String> {
        self.edit(user, |d| {
            let before = d.notifications.len();
            d.notifications.retain(|n| !n.read || n.flagged);
            Ok(before - d.notifications.len())
        })
    }

    // ------------------------------------------------------------ tasks

    #[allow(clippy::too_many_arguments)]
    pub fn add_task(&self, user: &str, title: &str, notes: &str, due: Option<i64>, priority: Priority, by: &str, now: i64) -> Result<Task, String> {
        let title = clean(title, MAX_TITLE, false);
        if title.is_empty() {
            return Err("a task needs a title".into());
        }
        if due.is_some_and(|d| !(0..4_102_444_800).contains(&d)) {
            return Err("that date is out of range (1970 to 2100)".into());
        }
        let t = Task { id: new_id(), title, notes: clean(notes, MAX_BODY, true), due, priority, status: Status::Pending, created_by: clean(by, 40, false), created: now, updated: now };
        self.edit(user, |d| {
            if d.tasks.len() >= MAX_TASKS {
                // make room by forgetting the oldest finished task
                match d.tasks.iter().enumerate().filter(|(_, t)| !t.status.is_open()).min_by_key(|(_, t)| t.updated).map(|(i, _)| i) {
                    Some(i) => {
                        d.tasks.remove(i);
                    }
                    None => return Err(format!("the task list is full ({MAX_TASKS} open tasks); finish or delete some")),
                }
            }
            d.tasks.push(t.clone());
            Ok(())
        })?;
        Ok(t)
    }

    pub fn tasks(&self, user: &str, sort: TaskSort) -> Vec<Task> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut v = self.load(user).map(|d| d.tasks).unwrap_or_default();
        match sort {
            TaskSort::Due => v.sort_by(|a, b| a.due.unwrap_or(i64::MAX).cmp(&b.due.unwrap_or(i64::MAX)).then(b.priority.cmp(&a.priority)).then(a.created.cmp(&b.created)).then(a.id.cmp(&b.id))),
            TaskSort::Priority => v.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.due.unwrap_or(i64::MAX).cmp(&b.due.unwrap_or(i64::MAX))).then(a.created.cmp(&b.created)).then(a.id.cmp(&b.id))),
            TaskSort::Created => v.sort_by(|a, b| b.created.cmp(&a.created).then(b.id.cmp(&a.id))),
        }
        v
    }

    pub fn update_task(&self, user: &str, id: &str, p: TaskPatch, now: i64) -> Result<Task, String> {
        self.edit(user, |d| {
            let t = d.tasks.iter_mut().find(|t| t.id == id).ok_or("no such task")?;
            if let Some(title) = &p.title {
                let c = clean(title, MAX_TITLE, false);
                if c.is_empty() {
                    return Err("a task needs a title".into());
                }
                t.title = c;
            }
            if let Some(n) = &p.notes {
                t.notes = clean(n, MAX_BODY, true);
            }
            if let Some(due) = p.due {
                if due.is_some_and(|d| !(0..4_102_444_800).contains(&d)) {
                    return Err("that date is out of range (1970 to 2100)".into());
                }
                t.due = due;
            }
            if let Some(pr) = p.priority {
                t.priority = pr;
            }
            if let Some(s) = p.status {
                t.status = s;
            }
            t.updated = now;
            Ok(t.clone())
        })
    }

    pub fn move_task(&self, user: &str, id: &str, status: Status, now: i64) -> Result<Task, String> {
        self.update_task(user, id, TaskPatch { status: Some(status), ..Default::default() }, now)
    }

    pub fn delete_task(&self, user: &str, id: &str) -> Result<(), String> {
        self.edit(user, |d| {
            let before = d.tasks.len();
            d.tasks.retain(|t| t.id != id);
            if d.tasks.len() == before { Err("no such task".into()) } else { Ok(()) }
        })
    }

    // ------------------------------------------------------------ counts

    pub fn counts(&self, user: &str, now: i64) -> Counts {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(d) = self.load(user) else { return Counts::default() };
        Counts {
            unread: d.notifications.iter().filter(|n| !n.read && !n.is_snoozed(now)).count(),
            flagged: d.notifications.iter().filter(|n| n.flagged).count(),
            open_tasks: d.tasks.iter().filter(|t| t.status.is_open()).count(),
            overdue: d.tasks.iter().filter(|t| t.is_overdue(now)).count(),
        }
    }

    /// The assistants may leave a limited number of notes and tasks per hour; true when there is room (and uses it).
    pub fn agent_allowed(&self, user: &str, now: i64) -> bool {
        let mut g = self.agent_log.lock().unwrap_or_else(|e| e.into_inner());
        let q = g.entry(user.to_string()).or_default();
        while q.front().is_some_and(|t| now - t >= 3600) {
            q.pop_front();
        }
        if q.len() >= AGENT_PER_HOUR {
            return false;
        }
        q.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str) -> Inbox {
        let d = std::env::temp_dir().join(format!("iyakki-inbox-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        Inbox::new(&d)
    }

    #[test]
    fn notifications_round_trip_flag_snooze_and_clear() {
        let s = store("a");
        let a = s.add("amy", Kind::Info, "  First\n", "body", "test", None, 100).unwrap();
        let b = s.add("amy", Kind::Reminder, "Second", "", "calendar", Some(Link::new("calendar", "abc")), 200).unwrap();
        s.add("bob", Kind::Info, "Bob's", "", "x", None, 300).unwrap();
        let l = s.list("amy");
        assert_eq!(l.iter().map(|n| n.title.as_str()).collect::<Vec<_>>(), vec!["Second", "First"], "newest first, only amy's");
        assert_eq!(l[0].link, Some(Link::new("calendar", "abc")));
        assert_eq!(s.counts("amy", 1000).unread, 2);
        s.mark_read("amy", &a.id, true).unwrap();
        s.flag("amy", &a.id, true).unwrap();
        assert_eq!(s.counts("amy", 1000), Counts { unread: 1, flagged: 1, open_tasks: 0, overdue: 0 });
        // snoozed ones are not counted until the time passes, then they are unread again
        s.snooze("amy", &b.id, 5000).unwrap();
        assert_eq!(s.counts("amy", 1000).unread, 0);
        assert_eq!(s.counts("amy", 5000).unread, 1);
        assert!(s.list("amy")[0].is_snoozed(1000));
        // clear_read keeps flagged ones
        s.mark_read("amy", &b.id, true).unwrap();
        assert_eq!(s.clear_read("amy").unwrap(), 1);
        assert_eq!(s.list("amy").len(), 1);
        s.flag("amy", &a.id, false).unwrap();
        assert_eq!(s.clear_read("amy").unwrap(), 1);
        assert!(s.list("amy").is_empty());
        let c = s.add("amy", Kind::Error, "x", "", "", None, 1).unwrap();
        assert_eq!(s.mark_all_read("amy").unwrap(), 1);
        s.delete("amy", &c.id).unwrap();
        assert!(s.delete("amy", &c.id).is_err() && s.mark_read("amy", "nope", true).is_err());
        assert!(s.add("amy", Kind::Info, "   ", "", "", None, 1).is_err());
        // it survives a restart and the file is private
        s.add("amy", Kind::System, "kept", "", "", None, 5).unwrap();
        let again = Inbox::new(&s.dir);
        assert_eq!(again.list("amy").len(), 1);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(s.dir.join("amy.json")).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn caps_sizes_and_path_tricks() {
        let s = store("b");
        let long = "x".repeat(10_000);
        let n = s.add("amy", Kind::Info, &long, &long, &long, None, 1).unwrap();
        assert!(n.title.chars().count() <= MAX_TITLE && n.body.chars().count() <= MAX_BODY && n.source.chars().count() <= 40);
        // the oldest read notification goes first when the list is full; flagged ones are kept
        let keep = s.add("amy", Kind::Info, "flagged", "", "", None, 0).unwrap();
        s.flag("amy", &keep.id, true).unwrap();
        s.mark_read("amy", &n.id, true).unwrap();
        for i in 0..MAX_NOTIFICATIONS + 5 {
            s.add("amy", Kind::Info, &format!("n{i}"), "", "", None, 10 + i as i64).unwrap();
        }
        let l = s.list("amy");
        assert_eq!(l.len(), MAX_NOTIFICATIONS);
        assert!(l.iter().any(|x| x.id == keep.id), "flagged survives");
        assert!(!l.iter().any(|x| x.id == n.id), "the read one went first");
        for bad in ["../x", "a/b", ""] {
            assert!(s.add(bad, Kind::Info, "t", "", "", None, 1).is_err(), "{bad}");
            assert!(s.list(bad).is_empty());
        }
        // control characters are neutralised
        let n = s.add("amy", Kind::Info, "a\u{7}b\nc", "l1\nl2\u{1b}", "", None, 1).unwrap();
        assert_eq!((n.title.as_str(), n.body.as_str()), ("a b c", "l1\nl2"));
    }

    #[test]
    fn tasks_move_sort_update_and_count_overdue() {
        let s = store("c");
        let a = s.add_task("amy", "Pay rent", "", Some(500), Priority::Normal, "user", 10).unwrap();
        let b = s.add_task("amy", "Call mum", "soon", None, Priority::High, "helper", 20).unwrap();
        let c = s.add_task("amy", "Water plants", "", Some(300), Priority::Low, "user", 30).unwrap();
        assert_eq!(b.created_by, "helper");
        let ids = |v: Vec<Task>| v.into_iter().map(|t| t.title).collect::<Vec<_>>();
        assert_eq!(ids(s.tasks("amy", TaskSort::Due)), vec!["Water plants", "Pay rent", "Call mum"]);
        assert_eq!(ids(s.tasks("amy", TaskSort::Priority)), vec!["Call mum", "Pay rent", "Water plants"]);
        assert_eq!(ids(s.tasks("amy", TaskSort::Created)), vec!["Water plants", "Call mum", "Pay rent"]);
        assert_eq!(s.counts("amy", 400), Counts { unread: 0, flagged: 0, open_tasks: 3, overdue: 1 });
        assert_eq!(s.counts("amy", 600).overdue, 2);
        // pending → doing → done, and back
        let t = s.move_task("amy", &c.id, Status::Pending.next(), 40).unwrap();
        assert_eq!((t.status, t.updated), (Status::Doing, 40));
        assert_eq!(s.move_task("amy", &c.id, Status::Doing.next(), 41).unwrap().status, Status::Done);
        assert_eq!(Status::Done.prev(), Status::Doing);
        assert_eq!(Status::Pending.prev(), Status::Pending);
        assert_eq!(s.counts("amy", 600), Counts { unread: 0, flagged: 0, open_tasks: 2, overdue: 1 }, "done tasks are not open or overdue");
        // edit
        let t = s.update_task("amy", &a.id, TaskPatch { title: Some("Pay the rent".into()), due: Some(None), priority: Some(Priority::High), ..Default::default() }, 50).unwrap();
        assert_eq!((t.title.as_str(), t.due, t.priority), ("Pay the rent", None, Priority::High));
        assert!(s.update_task("amy", &a.id, TaskPatch { title: Some(" ".into()), ..Default::default() }, 51).is_err());
        assert!(s.update_task("amy", "nope", TaskPatch::default(), 51).is_err());
        assert_eq!(s.counts("amy", 600).overdue, 0);
        s.delete_task("amy", &b.id).unwrap();
        assert!(s.delete_task("amy", &b.id).is_err());
        assert_eq!(s.tasks("amy", TaskSort::Due).len(), 2);
        assert!(s.add_task("amy", "", "", None, Priority::Low, "user", 1).is_err());
        assert!(s.add_task("amy", "x", "", Some(-5), Priority::Low, "user", 1).is_err());
        assert_eq!((Priority::parse("HIGH"), Priority::parse("x"), Status::parse("todo"), Kind::parse("Device")), (Some(Priority::High), None, Some(Status::Pending), Some(Kind::Device)));
        assert_eq!(Priority::Low.cycle().cycle().cycle(), Priority::Low);
    }

    #[test]
    fn the_task_list_is_capped_but_finished_tasks_make_room() {
        let s = store("d");
        for i in 0..MAX_TASKS {
            s.add_task("amy", &format!("t{i}"), "", None, Priority::Normal, "user", i as i64).unwrap();
        }
        assert!(s.add_task("amy", "one more", "", None, Priority::Normal, "user", 999).unwrap_err().contains("full"));
        let first = s.tasks("amy", TaskSort::Created).pop().unwrap();
        s.move_task("amy", &first.id, Status::Done, 1000).unwrap();
        s.add_task("amy", "one more", "", None, Priority::Normal, "user", 1001).unwrap();
        let all = s.tasks("amy", TaskSort::Created);
        assert_eq!(all.len(), MAX_TASKS);
        assert!(!all.iter().any(|t| t.id == first.id));
    }

    #[test]
    fn assistants_are_rate_limited_per_hour() {
        let s = store("e");
        for _ in 0..AGENT_PER_HOUR {
            assert!(s.agent_allowed("amy", 1000));
        }
        assert!(!s.agent_allowed("amy", 1001), "over the hourly limit");
        assert!(s.agent_allowed("bob", 1001), "per person");
        assert!(s.agent_allowed("amy", 1000 + 3600), "an hour later there is room again");
    }
}
