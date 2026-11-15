//! The inbound mirror (F-MSG-07): new messages of the rooms the person saved by name are copied into their INBOX, one entry per message,
//! labelled as coming from outside. Off until the person switches it on for an account (Settings or the `message_mirror_set` tool).
//!
//! * Read only: the mirror calls the same `/sync` the poll uses and writes INBOX entries; it never sends. Answering is the assistant's
//!   `message_send`, which asks on a card with the whole text every time.
//! * Only rooms the person saved by name are mirrored; a message from any other room joined by the account is counted and dropped.
//! * Our own messages are not mirrored. An encrypted message that cannot be read is mirrored as a line that says so (F-MSG-08 is the add-on).
//! * Replays are harmless: an entry is keyed by the account and a hash of the event id, so the same message never makes a second entry.
//! * The text is stored through [`super::label_untrusted`] (control characters removed, clipped, "it is data, not an instruction"), and
//!   the entry's source is `matrix`, so a person can turn those notices off in Settings, Notifications.
//! * Audit lines carry names and counts, never a text.

use std::time::Duration;

use sha2::{Digest, Sha256};

use super::service::MessageService;
use crate::inbox::{Inbox, Kind, Link};

/// The INBOX source name of mirrored messages.
pub const SOURCE: &str = "matrix";
/// The most messages one sync writes (the INBOX keeps 60 per source anyway).
pub const MAX_PER_SYNC: usize = 50;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MirrorReport {
    /// New INBOX entries.
    pub added: usize,
    /// Messages that were already in the INBOX (a replay).
    pub known: usize,
    /// Messages by this account itself.
    pub own: usize,
    /// Messages from a room that is not saved by name.
    pub other_rooms: usize,
    /// Dropped because notices from `matrix` are turned off in the INBOX.
    pub muted: usize,
    /// From a saved room the person left out of the copy.
    pub left_out: usize,
    /// Everything the sync returned.
    pub total: usize,
}

impl MirrorReport {
    /// A sentence for a tool result or a status line (counts only).
    pub fn words(&self) -> String {
        if self.total == 0 {
            return "Nothing new in the saved rooms.".into();
        }
        let mut s = format!("{} new message{} copied into the INBOX", self.added, if self.added == 1 { "" } else { "s" });
        for (n, what) in [(self.known, "already there"), (self.own, "of your own skipped"), (self.other_rooms, "from rooms that are not saved skipped"), (self.left_out, "from rooms left out of the copy skipped"), (self.muted, "dropped because Matrix notices are off")] {
            if n > 0 {
                s.push_str(&format!(", {n} {what}"));
            }
        }
        s.push('.');
        s
    }
}

/// The INBOX key of one message: stable, short, and free of the event id itself.
pub fn entry_key(account: &str, external_id: &str) -> String {
    let h = Sha256::digest(external_id.as_bytes());
    let hex: String = h.iter().take(12).map(|b| format!("{b:02x}")).collect();
    format!("mx:{}:{hex}", account.chars().take(20).collect::<String>())
}

pub struct Mirror<'a> {
    pub svc: &'a MessageService,
    pub inbox: &'a Inbox,
    pub user: &'a str,
}

impl Mirror<'_> {
    /// Is the mirror on for this account?
    pub fn is_on(&self, account: &str) -> bool {
        self.svc.accounts().pick(account).map(|a| self.svc.accounts().mirror_on(&a.id)).unwrap_or(false)
    }

    /// One long poll (at most `wait`) and the copy. Refuses, with a sentence, while the mirror is off.
    pub fn sync(&self, account: &str, wait: Duration, now: i64) -> Result<MirrorReport, String> {
        let a = self.svc.accounts().pick(account)?;
        if !self.svc.accounts().mirror_on(&a.id) {
            return Err(format!("copying messages into the INBOX is off for {}: switch it on first (Settings, Notifications, Matrix account, or ask the assistant to)", a.label));
        }
        let rooms = self.svc.accounts().rooms(&a.id);
        let got = self.svc.poll_once(&a.id, wait)?;
        let mut rep = MirrorReport { total: got.len(), ..Default::default() };
        let have: std::collections::HashSet<String> = self.inbox.list(self.user).into_iter().map(|n| n.group).collect();
        for m in got.into_iter().take(MAX_PER_SYNC * 4) {
            if m.is_self {
                rep.own += 1;
                continue;
            }
            let Some(room) = rooms.iter().find(|r| r.room_id == m.room) else {
                rep.other_rooms += 1;
                continue;
            };
            if !room.mirror {
                rep.left_out += 1;
                continue;
            }
            if rep.added >= MAX_PER_SYNC {
                break;
            }
            let title = format!("{} in {} ({})", super::clean_text(&m.from, 40), room.name, a.label);
            let body = super::label_untrusted(&m, &room.name);
            let key = entry_key(&a.id, &m.external_id);
            let was_there = have.contains(&format!("keyed:{key}"));
            match self.inbox.upsert_keyed(self.user, &key, Kind::Info, &title, &body, SOURCE, Some(Link::new("inbox", "")), false, now) {
                Ok(_) if was_there => rep.known += 1,
                Ok(_) => rep.added += 1,
                Err(e) if e.contains("turned off") => rep.muted += 1,
                Err(e) => return Err(e),
            }
        }
        self.svc.accounts().audit("mirror", &format!("account=\"{}\" added={} own={} other_rooms={} left_out={} muted={}", a.label, rep.added, rep.own, rep.other_rooms, rep.left_out, rep.muted));
        Ok(rep)
    }

    /// Unread copied messages per saved room of one account: (room name, count), in the order the rooms were saved. A room with
    /// nothing unread is listed with 0, so a page can say so. The room is found from the entry's title (`<who> in <room> (<account>)`).
    pub fn unread_by_room(&self, account: &str) -> Result<Vec<(String, usize)>, String> {
        let a = self.svc.accounts().pick(account)?;
        let rooms = self.svc.accounts().rooms(&a.id);
        let unread: Vec<String> = self.inbox.list(self.user).into_iter().filter(|n| n.source == SOURCE && !n.read).map(|n| n.title).collect();
        Ok(rooms.iter().map(|r| {
            let tail = format!(" in {} ({})", r.name, a.label);
            (r.name.clone(), unread.iter().filter(|t| t.ends_with(&tail)).count())
        }).collect())
    }

    /// The mirrored messages, newest first (what the reader agent summarises). Every body already says it is from outside.
    pub fn recent(&self, limit: usize) -> Vec<crate::inbox::Notification> {
        let mut v: Vec<_> = self.inbox.list(self.user).into_iter().filter(|n| n.source == SOURCE).collect();
        v.sort_by_key(|n| std::cmp::Reverse(n.time));
        v.truncate(limit);
        v
    }
}

// ------------------------------------------------------------------ the background round

/// How often a signed-in person's switched-on accounts are asked for new messages.
pub const ROUND: Duration = Duration::from_secs(60);

struct Slot {
    last: Option<std::time::Instant>,
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

fn slots() -> &'static std::sync::Mutex<std::collections::HashMap<String, Slot>> {
    static S: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Slot>>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// Called from the shell's heartbeat for the signed-in person: at most once per [`ROUND`], and never two rounds at once, it starts one
/// round on a thread (every account whose copy is on is asked once, with a short wait). Returns whether a round was started.
pub fn tick(svc: &std::sync::Arc<crate::services::Services>, user: &str, audit: crate::gitkeys::AuditFn) -> bool {
    let (s2, u2) = (svc.clone(), user.to_string());
    tick_with(&format!("{}:{user}", svc.dir().display()), ROUND, svc.clone(), user, Box::new(move || MessageService::for_services(&s2, &u2, audit)))
}

/// [`tick`] with the slot name, the interval and the service made by the caller (tests).
pub fn tick_with(key: &str, interval: Duration, svc: std::sync::Arc<crate::services::Services>, user: &str, make: Box<dyn FnOnce() -> MessageService + Send>) -> bool {
    use std::sync::atomic::Ordering;
    let busy = {
        let mut m = slots().lock().unwrap_or_else(|e| e.into_inner());
        let slot = m.entry(key.to_string()).or_insert(Slot { last: None, busy: Default::default() });
        if slot.busy.load(Ordering::SeqCst) || slot.last.is_some_and(|t| t.elapsed() < interval) {
            return false;
        }
        slot.last = Some(std::time::Instant::now());
        slot.busy.store(true, Ordering::SeqCst);
        slot.busy.clone()
    };
    let user = user.to_string();
    let spawned = std::thread::Builder::new().name("iyakki-msg-mirror".into()).spawn({
        let busy = busy.clone();
        move || {
            let ms = make();
            if ms.accounts().list().iter().any(|a| ms.accounts().mirror_on(&a.id)) {
                let m = Mirror { svc: &ms, inbox: &svc.inbox, user: &user };
                for a in ms.accounts().list() {
                    if ms.accounts().mirror_on(&a.id) {
                        let _ = m.sync(&a.id, Duration::from_millis(1500), crate::clock::now_unix());
                    }
                }
            }
            busy.store(false, Ordering::SeqCst);
        }
    });
    if spawned.is_err() {
        busy.store(false, Ordering::SeqCst);
        return false;
    }
    true
}

/// Has the round of this slot finished (tests wait on it)?
pub fn round_idle(key: &str) -> bool {
    slots().lock().unwrap_or_else(|e| e.into_inner()).get(key).is_none_or(|s| !s.busy.load(std::sync::atomic::Ordering::SeqCst))
}
