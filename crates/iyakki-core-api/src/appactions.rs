//! What an app's event handler may do on the person's machine (docs/sdk/events.md, F-SDK-201): leave a note in the inbox, start a flow,
//! ask for a chat tool to run. Each goes through the core, which checks the capability the app holds, logs by names, and, for a tool, asks the
//! person: `request_tool` never runs anything. It puts the request in the person's waiting list ([`ActionBook`]) with one line of what it
//! would do; the person answers on a card (the chat tool `app_actions_decide` asks, and says which app wants which tool), and only then the
//! tool runs, through the same tool stack the chat uses and its own risk rules. An app cannot approve its own request.
//!
//! * `notify` needs the capability `notify` (a built-in app holds it; an installed app is asked at install).
//! * `start_flow` needs `flows.start`: a flow by its name, one that is published; a flow that changes something asks inside as always.
//! * `request_tool` needs `tools.request` (a hard ask: the card at the person's answer is the approval), at most [`MAX_WAITING`] waiting
//!   requests per person and [`MAX_PER_APP`] per app, each kept [`KEEP_MS`], arguments up to [`MAX_ARGS`] bytes.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;

pub const APP_ACTIONS_FACET_VERSION: u32 = 1;
pub const MAX_WAITING: usize = 24;
pub const MAX_PER_APP: usize = 6;
pub const KEEP_MS: u64 = 24 * 3600 * 1000;
pub const MAX_ARGS: usize = 4096;
pub const MAX_NOTE: usize = 300;
pub const NO_ACTIONS: &str = "this core does not let apps act on events";

/// One request waiting for the person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingAction {
    pub id: String,
    pub app: String,
    pub tool: String,
    /// The arguments as the app gave them (JSON text); shown to the person on the card, never logged.
    pub args: String,
    pub at_ms: u64,
}

impl PendingAction {
    /// One line for the card: which app wants which tool. The arguments are on the card too, but not in this line.
    pub fn summary(&self) -> String {
        format!("{} asks to run {}", self.app, self.tool)
    }
}

pub trait AppActions: Send + Sync {
    fn notify(&self, _user: &str, _app: &str, _text: &str) -> Result<(), String> {
        Err(NO_ACTIONS.into())
    }
    /// Start a published flow by its name; returns what to tell the app.
    fn start_flow(&self, _user: &str, _app: &str, _name: &str) -> Result<String, String> {
        Err(NO_ACTIONS.into())
    }
    /// Put a tool request in the person's waiting list; returns its id. Nothing runs.
    fn request_tool(&self, _user: &str, _app: &str, _tool: &str, _args: &str) -> Result<String, String> {
        Err(NO_ACTIONS.into())
    }
    fn pending(&self, _user: &str) -> Vec<PendingAction> {
        Vec::new()
    }
    /// The person answered: the request leaves the list (the caller runs the tool only for an approval).
    fn take(&self, _user: &str, _id: &str) -> Option<PendingAction> {
        None
    }
    /// The audit lines of the actions (names and numbers only).
    fn audit(&self) -> Vec<String> {
        Vec::new()
    }
}

#[derive(Default)]
pub struct NoAppActions;
impl AppActions for NoAppActions {}

/// The waiting list: bounded, per person, with ids that are not guessable by order alone.
#[derive(Default)]
pub struct ActionBook {
    inner: Mutex<Inner>,
}

#[derive(Default, Serialize, Deserialize)]
struct Inner {
    next: u64,
    by_user: BTreeMap<String, Vec<PendingAction>>,
}

impl ActionBook {
    pub fn new() -> ActionBook {
        ActionBook::default()
    }

    /// The list as text, to keep it across a restart (requests older than [`KEEP_MS`] are dropped when it is read back).
    pub fn export(&self) -> String {
        serde_json::to_string(&*self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)).unwrap_or_default()
    }

    /// Read a list back; text that does not parse gives an empty list, and requests older than [`KEEP_MS`] are dropped.
    pub fn import(text: &str, now: u64) -> ActionBook {
        let mut inner: Inner = serde_json::from_str(text).unwrap_or_default();
        for l in inner.by_user.values_mut() {
            l.retain(|p| now.saturating_sub(p.at_ms) <= KEEP_MS);
        }
        inner.by_user.retain(|_, l| !l.is_empty());
        ActionBook { inner: Mutex::new(inner) }
    }

    pub fn ask(&self, user: &str, app: &str, tool: &str, args: &str, now: u64) -> Result<String, String> {
        if tool.is_empty() || tool.len() > 64 || !tool.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err("a tool is called by its name (letters, digits, _ and -)".into());
        }
        if args.len() > MAX_ARGS {
            return Err(format!("the arguments are at most {MAX_ARGS} bytes"));
        }
        if !args.is_empty() && serde_json::from_str::<serde_json::Value>(args).is_err() {
            return Err("the arguments are JSON text".into());
        }
        let mut g = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.next += 1;
        let id = format!("act{}", g.next);
        let list = g.by_user.entry(user.to_string()).or_default();
        list.retain(|p| now.saturating_sub(p.at_ms) <= KEEP_MS);
        if list.iter().filter(|p| p.app == app).count() >= MAX_PER_APP {
            return Err(format!("{app} already has {MAX_PER_APP} requests waiting: the person has to answer first"));
        }
        if list.len() >= MAX_WAITING {
            return Err("too many requests are waiting for the person".into());
        }
        list.push(PendingAction { id: id.clone(), app: app.into(), tool: tool.into(), args: args.into(), at_ms: now });
        Ok(id)
    }

    pub fn pending(&self, user: &str, now: u64) -> Vec<PendingAction> {
        let g = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.by_user.get(user).map(|l| l.iter().filter(|p| now.saturating_sub(p.at_ms) <= KEEP_MS).cloned().collect()).unwrap_or_default()
    }

    pub fn take(&self, user: &str, id: &str) -> Option<PendingAction> {
        let mut g = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let l = g.by_user.get_mut(user)?;
        let i = l.iter().position(|p| p.id == id)?;
        Some(l.remove(i))
    }
}

/// A note for the inbox: plain text, cut, no control characters.
pub fn clean_note(text: &str) -> Result<String, String> {
    let t: String = text.chars().filter(|c| !c.is_control()).take(MAX_NOTE).collect();
    if t.trim().is_empty() { Err("a note needs some text".into()) } else { Ok(t) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_waits_is_listed_and_leaves_once() {
        let b = ActionBook::new();
        let id = b.ask("pat", "notes", "backup_status", r#"{"x":1}"#, 10).unwrap();
        assert_eq!(b.pending("pat", 11).len(), 1);
        assert!(b.pending("sam", 11).is_empty(), "somebody else's list");
        assert_eq!(b.pending("pat", 11)[0].summary(), "notes asks to run backup_status");
        assert!(b.take("sam", &id).is_none());
        assert_eq!(b.take("pat", &id).unwrap().tool, "backup_status");
        assert!(b.take("pat", &id).is_none());
    }

    #[test]
    fn the_list_is_bounded_per_app_and_per_person_and_old_requests_lapse() {
        let b = ActionBook::new();
        for _ in 0..MAX_PER_APP {
            b.ask("pat", "a", "t", "", 1).unwrap();
        }
        assert!(b.ask("pat", "a", "t", "", 1).unwrap_err().contains("already has"));
        for i in 0..(MAX_WAITING - MAX_PER_APP) {
            let _ = b.ask("pat", &format!("app{i}"), "t", "", 1);
        }
        assert!(b.ask("pat", "z", "t", "", 2 + KEEP_MS).is_ok(), "the old ones lapsed");
        assert_eq!(b.pending("pat", 2 + KEEP_MS).len(), 1);
    }

    #[test]
    fn the_list_survives_a_restart_and_the_old_ones_lapse_and_a_broken_file_is_empty() {
        let b = ActionBook::new();
        let old = b.ask("pat", "a", "t1", "", 1).unwrap();
        let new = b.ask("pat", "a", "t2", "{}", KEEP_MS).unwrap();
        let text = b.export();
        let again = ActionBook::import(&text, KEEP_MS + 5);
        let p = again.pending("pat", KEEP_MS + 5);
        assert_eq!(p.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), [new.as_str()], "{old} lapsed");
        let id = again.ask("pat", "a", "t3", "", KEEP_MS + 5).unwrap();
        assert_ne!(id, new, "ids are not reused");
        assert!(ActionBook::import("{not json", 1).pending("pat", 1).is_empty());
    }

    #[test]
    fn a_bad_tool_name_or_arguments_are_refused() {
        let b = ActionBook::new();
        assert!(b.ask("pat", "a", "rm -rf", "", 1).is_err());
        assert!(b.ask("pat", "a", "t", "{not json", 1).is_err());
        assert!(b.ask("pat", "a", "t", &"x".repeat(MAX_ARGS + 1), 1).is_err());
        assert!(clean_note("   ").is_err());
        assert_eq!(clean_note("a\nb").unwrap(), "ab");
    }

    #[test]
    fn a_core_without_actions_refuses_in_a_sentence() {
        assert_eq!(NoAppActions.notify("p", "a", "x").unwrap_err(), NO_ACTIONS);
        assert!(NoAppActions.pending("p").is_empty());
    }
}
