//! The INBOX: each person's notifications and tasks, one JSON file per user. Notifications are what the system and the
//! assistants want the person to know (a reminder rang, a run finished, a new device appeared); tasks are a small
//! to-do list the person and the assistants share. Approvals are not stored here: they are read live from the runs
//! that are waiting (see [`crate::runs::RunStore::pending_approvals`]).
//!
//! Everything is capped (counts and sizes), written atomically with owner-only permissions, and validated: a user name
//! that could climb out of the directory is refused.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The most entries kept per person (a group of similar notes is one entry).
pub const MAX_NOTIFICATIONS: usize = 200;
/// The most entries kept from one source.
pub const MAX_PER_SOURCE: usize = 60;
/// The most detail lines kept inside one group.
pub const MAX_ITEMS: usize = 100;
/// Flow failures with the same flow and the same error within this long are one entry with a counter.
pub const FAILURE_WINDOW: i64 = 3600;
const DAY: i64 = 86_400;
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
    /// What similar notes are collected under (empty: a note of its own). See [`auto_group`].
    #[serde(default)]
    pub group: String,
    /// How many notes this entry stands for (1 for a single note).
    #[serde(default = "one")]
    pub count: u32,
    /// The title once there are several, `{n}` standing for the count ("{n} new devices on your network").
    #[serde(default)]
    pub plural: String,
    /// The notes inside a group, oldest first (the newest [`MAX_ITEMS`]).
    #[serde(default)]
    pub items: Vec<Item>,
}

fn one() -> u32 {
    1
}

/// One note inside a group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub time: i64,
    pub text: String,
    /// What it points at in the linked app (a device's hardware address, a run's id).
    #[serde(default)]
    pub id: String,
}

impl Notification {
    pub fn is_snoozed(&self, now: i64) -> bool {
        self.snoozed_until > now
    }
    /// Several notes collected in one entry.
    pub fn is_group(&self) -> bool {
        self.count > 1 || self.items.len() > 1
    }
    /// Something that needs the person (counted in the bell's number): warnings, errors, reminders that rang and assistants
    /// that are waiting for them. Information, finished jobs, downloads and new devices are not.
    pub fn needs_attention(&self) -> bool {
        match self.kind {
            Kind::Warning | Kind::Error | Kind::Reminder => true,
            Kind::Agent => self.title.contains("needs you") || self.title.starts_with("Approval needed"),
            _ => false,
        }
    }
}

/// How long a note is kept (flagged ones are kept until the person removes them).
pub fn retention_secs(kind: Kind, source: &str) -> i64 {
    match kind {
        Kind::Info | Kind::Device | Kind::Download | Kind::System => 7 * DAY,
        Kind::Agent if source == "runs" => 14 * DAY,
        Kind::Agent | Kind::Reminder => 14 * DAY,
        Kind::Warning | Kind::Error => 30 * DAY,
    }
}

/// Which notes are collected into one entry: `(key, plural title, text of the item, merge into a read entry within)`.
/// Everything else stays a note of its own.
pub fn auto_group(kind: Kind, source: &str, title: &str, body: &str) -> Option<(String, String, i64)> {
    if kind == Kind::Device && source == "scan" && title == "New device on your network" {
        return Some(("scan:new-devices".into(), "{n} new devices on your network".into(), 0));
    }
    if kind == Kind::Agent && source == "runs" {
        if title.starts_with("Job \u{201c}") && title.ends_with("finished") {
            return Some(("runs:jobs-finished".into(), "{n} setup jobs finished".into(), 0));
        }
        if title.starts_with("Finished: ") {
            return Some(("runs:runs-finished".into(), "{n} assistant runs finished".into(), 0));
        }
    }
    if source == "flows" && matches!(kind, Kind::Error | Kind::Warning) {
        let head: String = body.chars().take(80).collect();
        return Some((format!("flows:{kind:?}:{title}:{head}"), format!("{title} (x{{n}})"), FAILURE_WINDOW));
    }
    None
}

/// How much of a source's notes the person wants to see. Chosen per person in Settings -> Notifications.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Normal: counted in the bell by kind.
    Show,
    /// Kept in the inbox, but never in the bell (no number, no dot, no sound).
    Quiet,
    /// Not kept at all.
    Off,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Show => "show",
            Level::Quiet => "quiet",
            Level::Off => "off",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Level::Show => "Show",
            Level::Quiet => "Quiet (panel only)",
            Level::Off => "Off",
        }
    }
    pub fn next(self) -> Level {
        match self {
            Level::Show => Level::Quiet,
            Level::Quiet => Level::Off,
            Level::Off => Level::Show,
        }
    }
}

/// The sources that raise notes, for the Settings page (any other source that has left a note is added to the list).
pub const SOURCES: [(&str, &str); 9] = [
    ("scan", "New devices on the network"),
    ("runs", "Finished assistant runs and jobs"),
    ("flows", "Flows"),
    ("calendar", "Reminders and alarms"),
    ("media", "Downloads"),
    ("tasks", "Tasks"),
    ("assistant", "What assistants leave for you"),
    ("settings", "Warnings about the system"),
    ("system", "Other system notes"),
];

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
    /// Every unread entry that is not snoozed.
    pub unread: usize,
    /// Unread entries that need the person (the bell's number): see [`Notification::needs_attention`]. Quiet sources excluded.
    pub attention: usize,
    /// Unread entries of the other kinds (the bell's dot). Quiet sources excluded.
    pub other: usize,
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
    /// 0: a file from before notes were grouped.
    #[serde(default)]
    version: u32,
    /// Per source: show, quiet or off.
    #[serde(default)]
    levels: BTreeMap<String, Level>,
    /// Said once in the panel (what the tidy-up on first open did).
    #[serde(default)]
    notice: String,
    /// Fields a newer release wrote: kept and written back.
    #[serde(flatten)]
    extra: iyakki_schema::Extra,
}

/// 2 = notes grouped (the layout since alpha.1's first tidy-up). The field is called `version` here (older than `iyakki-schema`),
/// and the same rules apply: a newer file is never written, a damaged one is moved aside, unknown fields are kept.
const VERSION: u32 = 2;

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

/// Drop what is past its retention, then the oldest read entries of a source over its cap, then the oldest over the hard cap.
/// Flagged entries stay. Returns how many went.
fn prune(d: &mut Data, now: i64) -> usize {
    let before = d.notifications.len();
    d.notifications.retain(|n| n.flagged || now - n.time <= retention_secs(n.kind, &n.source));
    let sources: Vec<String> = d.notifications.iter().map(|n| n.source.clone()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    for src in sources {
        while d.notifications.iter().filter(|n| n.source == src).count() > MAX_PER_SOURCE {
            match oldest_droppable(&d.notifications, |n| n.source == src) {
                Some(i) => {
                    d.notifications.remove(i);
                }
                None => break,
            }
        }
    }
    while d.notifications.len() > MAX_NOTIFICATIONS {
        let i = oldest_droppable(&d.notifications, |_| true).or_else(|| d.notifications.iter().enumerate().min_by_key(|(_, x)| x.time).map(|(i, _)| i)).unwrap_or(0);
        d.notifications.remove(i);
    }
    before - d.notifications.len()
}

/// The oldest read unflagged entry, else the oldest unflagged one, among those that match.
fn oldest_droppable(v: &[Notification], pick: impl Fn(&Notification) -> bool) -> Option<usize> {
    v.iter().enumerate().filter(|(_, x)| pick(x) && x.read && !x.flagged).min_by_key(|(_, x)| x.time).or_else(|| v.iter().enumerate().filter(|(_, x)| pick(x) && !x.flagged).min_by_key(|(_, x)| x.time)).map(|(i, _)| i)
}

/// Collect the single notes of an older file that [`auto_group`] would have collected (read and unread ones apart).
fn regroup(d: &mut Data) {
    let old = std::mem::take(&mut d.notifications);
    let mut out: Vec<Notification> = Vec::new();
    let mut at: BTreeMap<(String, String, bool), usize> = BTreeMap::new();
    let mut old = old;
    old.sort_by_key(|n| n.time);
    for mut n in old {
        let rule = if n.group.is_empty() && !n.flagged && n.snoozed_until == 0 { auto_group(n.kind, &n.source, &n.title, &n.body) } else { None };
        let Some((key, plural, _)) = rule else {
            out.push(n);
            continue;
        };
        let text = if n.kind == Kind::Device || n.kind == Kind::Error || n.kind == Kind::Warning { if n.body.is_empty() { n.title.clone() } else { n.body.clone() } } else { n.title.clone() };
        let item = Item { time: n.time, text: clean(&text, 300, false), id: n.link.as_ref().map(|l| l.id.clone()).unwrap_or_default() };
        match at.get(&(key.clone(), n.source.clone(), n.read)) {
            Some(&i) => {
                let e = &mut out[i];
                e.count = e.count.saturating_add(1);
                e.time = n.time;
                e.body = n.body;
                e.link = n.link;
                e.title = e.plural.replace("{n}", &e.count.to_string());
                e.items.push(item);
                if e.items.len() > MAX_ITEMS {
                    e.items.remove(0);
                }
            }
            None => {
                n.group = key.clone();
                n.plural = plural;
                n.items = vec![item];
                at.insert((key, n.source.clone(), n.read), out.len());
                out.push(n);
            }
        }
    }
    d.notifications = out;
}

impl Inbox {
    pub fn new(dir: &std::path::Path) -> Inbox {
        Inbox { dir: dir.into(), lock: Mutex::new(()), agent_log: Mutex::new(HashMap::new()) }
    }

    fn path(&self, user: &str) -> Result<PathBuf, String> {
        if iyakki_sec::policy::valid_username(user) { Ok(self.dir.join(format!("{user}.json"))) } else { Err("invalid user".into()) }
    }

    fn load(&self, user: &str) -> Result<Data, String> {
        let path = self.path(user)?;
        match std::fs::read(&path) {
            Ok(b) => match serde_json::from_slice::<Data>(&b) {
                Ok(d) => Ok(d),
                Err(e) => {
                    // never lose it, never block the inbox: keep the damaged file next to the new one, say so in the audit log
                    let moved = iyakki_schema::move_aside(&path, iyakki_schema::now_secs());
                    iyakki_schema::report(format!("inbox {}: damaged ({e}); {} and started an empty inbox", path.display(), moved.map(|m| format!("moved to {}", m.display())).unwrap_or_else(|| "could not be moved aside".into())));
                    Ok(Data::default())
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Data::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn save(&self, user: &str, d: &Data) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.path(user)?;
        if let Some(n) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).and_then(|v| v.get("version").and_then(|n| n.as_u64())).filter(|n| *n > VERSION as u64) {
            return Err(format!("The inbox file was written by a newer IYAKKI (version {n}; this one reads up to {VERSION}). It is kept as it is; update IYAKKI first."));
        }
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
        let (body, source) = (clean(body, MAX_BODY, true), clean(source, 40, false));
        let link = link.map(|l| Link { app: clean(&l.app, 24, false), id: clean(&l.id, 64, false) });
        let grouping = auto_group(kind, &source, &title, &body);
        let item_text = if kind == Kind::Device || kind == Kind::Error || kind == Kind::Warning { if body.is_empty() { title.clone() } else { body.clone() } } else { title.clone() };
        let item = Item { time: now, text: clean(&item_text, 300, false), id: link.as_ref().map(|l| l.id.clone()).unwrap_or_default() };
        let mut n = Notification {
            id: new_id(),
            time: now,
            kind,
            title,
            body,
            source,
            link,
            read: false,
            flagged: false,
            snoozed_until: 0,
            group: String::new(),
            count: 1,
            plural: String::new(),
            items: Vec::new(),
        };
        if let Some((key, plural, _)) = &grouping {
            n.group = key.clone();
            n.plural = plural.clone();
            n.items = vec![item.clone()];
        }
        self.edit(user, |d| {
            if d.levels.get(&n.source) == Some(&Level::Off) {
                return Err(format!("notifications from {} are turned off", if n.source.is_empty() { "this source" } else { &n.source }));
            }
            if let Some((key, _, window)) = &grouping {
                // one more of the same: into the entry that is still open (unread, or read a moment ago for repeated failures)
                if let Some(e) = d.notifications.iter_mut().filter(|e| e.group == *key && e.source == n.source && !e.flagged).filter(|e| !e.read || now - e.time < *window).max_by_key(|e| e.time) {
                    e.count = e.count.saturating_add(1);
                    e.time = now;
                    e.body = n.body.clone();
                    e.link = n.link.clone();
                    e.title = e.plural.replace("{n}", &e.count.to_string());
                    e.items.push(item.clone());
                    if e.items.len() > MAX_ITEMS {
                        e.items.remove(0);
                    }
                    n = e.clone();
                    prune(d, now);
                    return Ok(());
                }
            }
            d.notifications.push(n.clone());
            prune(d, now);
            Ok(())
        })?;
        Ok(n)
    }

    /// Tidy a person's file: group the notes of an older file, drop what is past its retention. Saves only when something
    /// changed; the first time on an older file it leaves a notice for the panel to say once ([`Inbox::take_notice`]).
    pub fn maintain(&self, user: &str, now: i64) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut d = self.load(user)?;
        let before = d.notifications.len();
        let migrating = d.version < VERSION;
        if migrating {
            regroup(&mut d);
        }
        let grouped_to = d.notifications.len();
        let pruned = prune(&mut d, now);
        if !migrating && pruned == 0 {
            return Ok(());
        }
        if migrating && (grouped_to < before || pruned > 0) {
            let mut m = format!("Your inbox was tidied: {before} notifications are now {} entries, similar ones grouped (open one to see them all).", d.notifications.len());
            if pruned > 0 {
                m.push_str(&format!(" {pruned} old {} past {} keeping time {} removed (information 7 days, jobs 14, errors 30).", if pruned == 1 { "one" } else { "ones" }, if pruned == 1 { "its" } else { "their" }, if pruned == 1 { "was" } else { "were" }));
            }
            m.push_str(" Nothing else was removed.");
            d.notice = m;
        }
        if migrating {
            // the file as the older release left it, kept once
            if let Ok(p) = self.path(user) {
                let b = iyakki_schema::sibling(&p, &format!("pre-v{VERSION}"));
                if p.exists() && !b.exists() && std::fs::copy(&p, &b).is_ok() {
                    iyakki_schema::report(format!("inbox {}: upgraded to version {VERSION} (old file kept as {})", p.display(), b.display()));
                }
            }
        }
        d.version = VERSION;
        self.save(user, &d)
    }

    /// What the tidy-up did, once (then it is forgotten).
    pub fn take_notice(&self, user: &str) -> Option<String> {
        self.edit(user, |d| Ok(std::mem::take(&mut d.notice))).ok().filter(|m| !m.is_empty())
    }

    /// Delete every entry of a group key or source: "Clear this group". Flagged entries stay. Returns how many went.
    pub fn clear_source(&self, user: &str, source: &str) -> Result<usize, String> {
        self.edit(user, |d| {
            let before = d.notifications.len();
            d.notifications.retain(|n| n.source != source || n.flagged);
            Ok(before - d.notifications.len())
        })
    }

    pub fn level(&self, user: &str, source: &str) -> Level {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load(user).ok().and_then(|d| d.levels.get(source).copied()).unwrap_or(Level::Show)
    }

    pub fn levels(&self, user: &str) -> BTreeMap<String, Level> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.load(user).map(|d| d.levels).unwrap_or_default()
    }

    /// Show, quiet or off for one source (per person; `Show` forgets the choice).
    pub fn set_level(&self, user: &str, source: &str, level: Level) -> Result<(), String> {
        let source = clean(source, 40, false);
        if source.is_empty() {
            return Err("a source needs a name".into());
        }
        self.edit(user, |d| {
            if level == Level::Show {
                d.levels.remove(&source);
            } else {
                d.levels.insert(source, level);
            }
            Ok(())
        })
    }

    /// The sources a person can choose for: the known ones and any other that has left a note.
    pub fn sources(&self, user: &str) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = SOURCES.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        for n in self.list(user) {
            if !n.source.is_empty() && !v.iter().any(|(s, _)| *s == n.source) {
                v.push((n.source.clone(), format!("Notes from {}", n.source)));
            }
        }
        v
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
        let live = |n: &&Notification| !n.read && !n.is_snoozed(now);
        let shown = |n: &&Notification| d.levels.get(&n.source).copied().unwrap_or(Level::Show) == Level::Show;
        Counts {
            unread: d.notifications.iter().filter(live).count(),
            attention: d.notifications.iter().filter(live).filter(shown).filter(|n| n.needs_attention()).count(),
            other: d.notifications.iter().filter(live).filter(shown).filter(|n| !n.needs_attention()).count(),
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
        assert_eq!(s.counts("amy", 1000), Counts { unread: 1, attention: 1, other: 0, flagged: 1, open_tasks: 0, overdue: 0 });
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

    const NEWDEV: &str = "New device on your network";

    #[test]
    fn new_devices_collapse_into_one_entry_with_the_list_inside() {
        let s = store("g1");
        for i in 0..79 {
            s.add("amy", Kind::Device, NEWDEV, &format!("10.0.0.{i} (aa:bb:cc:00:00:{i:02x}) appeared."), "scan", Some(Link::new("scan", &format!("aa:bb:cc:00:00:{i:02x}"))), 1000 + i).unwrap();
        }
        let l = s.list("amy");
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].count, l[0].title.as_str(), l[0].items.len()), (79, "79 new devices on your network", 79));
        assert_eq!(l[0].items[3].id, "aa:bb:cc:00:00:03");
        // looked at: the next device starts a new entry (only what is new since the last look)
        s.mark_read("amy", &l[0].id, true).unwrap();
        s.add("amy", Kind::Device, NEWDEV, "10.0.0.200 (x) appeared.", "scan", None, 5000).unwrap();
        let l = s.list("amy");
        assert_eq!((l.len(), l[0].count, l[0].title.as_str()), (2, 1, NEWDEV));
        // a different source with the same title is not collapsed into it
        s.add("amy", Kind::Device, NEWDEV, "b", "other", None, 5001).unwrap();
        assert_eq!(s.list("amy").len(), 3);
    }

    #[test]
    fn finished_jobs_are_one_group_and_flow_failures_count_repeats_inside_a_window() {
        let s = store("g2");
        for i in 0..41 {
            s.add("amy", Kind::Agent, &format!("Job \u{201c}Install {i}\u{201d} finished"), "done", "runs", Some(Link::new("runs", "jobs")), 100 + i).unwrap();
        }
        let l = s.list("amy");
        assert_eq!((l.len(), l[0].count, l[0].title.as_str()), (1, 41, "41 setup jobs finished"));
        // one flow, one error: a counter and the last time, even after it was read
        let f = |t| s.add("amy", Kind::Error, "Health check failed", "https://example.com/health is not answering", "flows", Some(Link::new("flows", "f1")), t).unwrap();
        f(1000);
        let n = f(1100);
        assert_eq!((n.count, n.title.as_str(), n.time), (2, "Health check failed (x2)", 1100));
        s.mark_read("amy", &n.id, true).unwrap();
        let n2 = f(1200);
        assert_eq!((n2.id.as_str(), n2.count, n2.read), (n.id.as_str(), 3, true), "inside the window it only counts");
        // after the window, or for another error, it is a new entry
        let n3 = f(1200 + FAILURE_WINDOW + 5);
        assert_ne!(n3.id, n.id);
        s.add("amy", Kind::Error, "Health check failed", "a different error", "flows", None, 1300).unwrap();
        assert_eq!(s.list("amy").iter().filter(|x| x.source == "flows").count(), 3);
        // unrelated notes stay single
        s.add("amy", Kind::Info, "Hello", "", "x", None, 1).unwrap();
        s.add("amy", Kind::Info, "Hello", "", "x", None, 2).unwrap();
        assert_eq!(s.list("amy").iter().filter(|x| x.title == "Hello").count(), 2);
    }

    #[test]
    fn the_bell_counts_only_what_needs_the_person() {
        let s = store("g3");
        s.add("amy", Kind::Device, NEWDEV, "a", "scan", None, 10).unwrap();
        s.add("amy", Kind::Info, "fyi", "", "x", None, 10).unwrap();
        s.add("amy", Kind::Download, "got it", "", "media", None, 10).unwrap();
        s.add("amy", Kind::Agent, "Job \u{201c}x\u{201d} finished", "", "runs", None, 10).unwrap();
        let c = s.counts("amy", 20);
        assert_eq!((c.attention, c.other, c.unread), (0, 4 - 0, 4), "routine items never add to the number: {c:?}");
        s.add("amy", Kind::Error, "Store Writer failed", "", "flows", None, 11).unwrap();
        s.add("amy", Kind::Warning, "Vault locked", "", "settings", None, 12).unwrap();
        s.add("amy", Kind::Reminder, "Reminder: x", "", "calendar", None, 13).unwrap();
        s.add("amy", Kind::Agent, "The assistant needs you", "", "assistant", None, 14).unwrap();
        assert_eq!(s.counts("amy", 20).attention, 4);
        // 130 routine items still make no number
        let t = store("g3b");
        for i in 0..130 {
            t.add("amy", Kind::Info, &format!("i{i}"), "", &format!("s{}", i % 5), None, 10 + i).unwrap();
        }
        let c = t.counts("amy", 200);
        assert_eq!(c.attention, 0);
        assert!(c.other > 0);
    }

    #[test]
    fn levels_hide_from_the_bell_or_drop_a_source_per_person_and_persist() {
        let s = store("g4");
        s.set_level("amy", "scan", Level::Quiet).unwrap();
        s.set_level("amy", "flows", Level::Off).unwrap();
        s.add("amy", Kind::Error, "boom", "", "flows", None, 10).unwrap_err();
        s.add("amy", Kind::Warning, "quiet warn", "", "scan", None, 10).unwrap();
        s.add("amy", Kind::Warning, "loud warn", "", "settings", None, 10).unwrap();
        let c = s.counts("amy", 20);
        assert_eq!((c.attention, c.other, c.unread), (1, 0, 2), "a quiet source is in the panel only");
        // bob is not affected, and a restart keeps it
        assert_eq!(s.level("bob", "scan"), Level::Show);
        s.add("bob", Kind::Error, "boom", "", "flows", None, 10).unwrap();
        let again = Inbox::new(&s.dir);
        assert_eq!((again.level("amy", "scan"), again.level("amy", "flows")), (Level::Quiet, Level::Off));
        assert_eq!(again.levels("amy").len(), 2);
        again.set_level("amy", "scan", Level::Show).unwrap();
        assert_eq!(again.levels("amy").len(), 1);
        assert_eq!((Level::Show.next(), Level::Quiet.next(), Level::Off.next()), (Level::Quiet, Level::Off, Level::Show));
        assert!(again.sources("amy").iter().any(|(k, _)| k == "scan"));
    }

    #[test]
    fn retention_drops_old_entries_by_kind_per_source_and_in_total_but_keeps_flagged() {
        let s = store("g5");
        let day = 86_400;
        let now = 100 * day;
        let old_info = s.add("amy", Kind::Info, "old info", "", "x", None, now - 8 * day).unwrap();
        s.add("amy", Kind::Info, "fresh info", "", "x", None, now - 6 * day).unwrap();
        s.add("amy", Kind::Agent, "Done: a", "", "runs", None, now - 15 * day).unwrap();
        s.add("amy", Kind::Agent, "Done: b", "", "runs", None, now - 13 * day).unwrap();
        s.add("amy", Kind::Error, "error 29 d", "", "flows", None, now - 29 * day).unwrap();
        s.add("amy", Kind::Error, "error 31 d", "", "flows", None, now - 31 * day).unwrap();
        let keep = s.add("amy", Kind::Info, "flagged old", "", "x", None, now - 90 * day).unwrap();
        s.flag("amy", &keep.id, true).unwrap();
        s.maintain("amy", now).unwrap();
        let mut t: Vec<String> = s.list("amy").into_iter().map(|n| n.title).collect();
        t.sort();
        assert_eq!(t, vec!["Done: b", "error 29 d", "flagged old", "fresh info"]);
        assert!(!s.list("amy").iter().any(|n| n.id == old_info.id));
        // per source: the oldest go first
        let u = store("g5b");
        for i in 0..MAX_PER_SOURCE + 10 {
            u.add("amy", Kind::Info, &format!("n{i}"), "", "one", None, now + i as i64).unwrap();
        }
        let l = u.list("amy");
        assert_eq!(l.len(), MAX_PER_SOURCE);
        assert!(l.iter().all(|n| n.title != "n0") && l.iter().any(|n| n.title == format!("n{}", MAX_PER_SOURCE + 9)));
    }

    #[test]
    fn an_older_file_loads_is_grouped_once_and_says_so_without_losing_anything_else() {
        let s = store("g6");
        let now = 100 * 86_400;
        // the shape found on a real machine: single notes, all unread
        let mut notes = Vec::new();
        let mut push = |kind: &str, source: &str, title: &str, body: &str, time: i64| {
            notes.push(serde_json::json!({"id": format!("id{}", notes.len()), "time": time, "kind": kind, "title": title, "body": body, "source": source, "link": {"app": source, "id": "z"}, "read": false, "flagged": false, "snoozed_until": 0}));
        };
        for i in 0..79 {
            push("device", "scan", NEWDEV, &format!("10.0.0.{i} (m{i}) appeared."), now - 3600 + i);
        }
        for i in 0..41 {
            push("agent", "runs", &format!("Job \u{201c}Install Python 3.12 ({i}/12)\u{201d} finished"), "ok", now - 7200 + i);
        }
        for _ in 0..4 {
            push("error", "flows", "Health flow failed", "https://example.com/health is not answering", now - 600);
        }
        push("error", "flows", "Store Writer failed", "disk", now - 500);
        push("error", "flows", "Store Writer failed", "disk", now - 400);
        push("reminder", "calendar", "Reminder: Standup", "", now - 300);
        push("info", "system", "Welcome", "", now - 20 * 86_400);
        std::fs::create_dir_all(&s.dir).unwrap();
        std::fs::write(s.dir.join("amy.json"), serde_json::to_vec(&serde_json::json!({"notifications": notes, "tasks": []})).unwrap()).unwrap();
        // it loads as it is, and the bell already counts by kind
        assert_eq!(s.list("amy").len(), 79 + 41 + 4 + 2 + 1 + 1);
        assert_eq!(s.counts("amy", now).attention, 4 + 2 + 1);
        s.maintain("amy", now).unwrap();
        let l = s.list("amy");
        let titles: Vec<&str> = l.iter().map(|n| n.title.as_str()).collect();
        assert!(titles.contains(&"79 new devices on your network") && titles.contains(&"41 setup jobs finished"), "{titles:?}");
        assert!(titles.contains(&"Health flow failed (x4)") && titles.contains(&"Store Writer failed (x2)"), "{titles:?}");
        assert_eq!(l.len(), 5, "the 20-day-old info note was past its 7 days: {titles:?}");
        assert_eq!(l.iter().find(|n| n.count == 79).unwrap().items.len(), 79);
        let notice = s.take_notice("amy").expect("said once");
        assert!(notice.contains("128") && notice.contains("tidied") && notice.contains("1 old one past its"), "{notice}");
        assert!(s.take_notice("amy").is_none());
        // a second run changes nothing
        s.maintain("amy", now).unwrap();
        assert_eq!(s.list("amy").len(), 5);
        assert!(s.take_notice("amy").is_none());
        // old files with none of the new fields still load, and a flagged or snoozed note is left alone
        let f = store("g6b");
        std::fs::create_dir_all(&f.dir).unwrap();
        std::fs::write(f.dir.join("amy.json"), r#"{"notifications":[{"id":"a","time":5,"kind":"device","title":"New device on your network","source":"scan","flagged":true},{"id":"b","time":6,"kind":"device","title":"New device on your network","source":"scan"}],"tasks":[]}"#).unwrap();
        f.maintain("amy", 100).unwrap();
        assert_eq!(f.list("amy").len(), 2);
        assert!(f.take_notice("amy").is_none(), "nothing changed, nothing to say");
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
            s.add("amy", Kind::Info, &format!("n{i}"), "", &format!("s{}", i % 10), None, 10 + i as i64).unwrap();
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
        assert_eq!(s.counts("amy", 400), Counts { open_tasks: 3, overdue: 1, ..Default::default() });
        assert_eq!(s.counts("amy", 600).overdue, 2);
        // pending → doing → done, and back
        let t = s.move_task("amy", &c.id, Status::Pending.next(), 40).unwrap();
        assert_eq!((t.status, t.updated), (Status::Doing, 40));
        assert_eq!(s.move_task("amy", &c.id, Status::Doing.next(), 41).unwrap().status, Status::Done);
        assert_eq!(Status::Done.prev(), Status::Doing);
        assert_eq!(Status::Pending.prev(), Status::Pending);
        assert_eq!(s.counts("amy", 600), Counts { open_tasks: 2, overdue: 1, ..Default::default() }, "done tasks are not open or overdue");
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
