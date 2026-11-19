//! Telling the person when internet sharing ended without their asking (F-HOTSPOT-14): the administrator's policy no longer allows it,
//! the longest session ended, the cable or the uplink went away, or the idle switch-off came. The service only writes `status.json`;
//! the shell reads it every few seconds and leaves one note in the inbox of the person who started the session and of the signed-in
//! administrator. A stop the person asked for says nothing (its note is empty). Each ended session is told once per person.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use super::{control, State, Status};
use crate::inbox::Kind;

/// What to put in the inbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub kind: Kind,
    pub title: String,
    pub body: String,
}

/// The note for a status, or `None` while sharing runs, when the person stopped it, or before anything happened.
pub fn note_for(s: &Status) -> Option<Note> {
    if s.state == State::On || s.request_id == 0 || s.note.trim().is_empty() {
        return None;
    }
    let note: String = s.note.chars().take(300).collect();
    Some(match s.state {
        State::Failed => Note { kind: Kind::Warning, title: "Internet sharing stopped".into(), body: format!("{note} Devices that used it have lost their internet. Open Settings -> Share internet to start it again.") },
        _ => Note { kind: Kind::Info, title: "Internet sharing switched off".into(), body: note },
    })
}

static ON: Mutex<Vec<std::path::PathBuf>> = Mutex::new(Vec::new());

/// Is internet sharing running on this machine (data folder), as of the last look (for the badge in the shell's bar)?
pub fn is_on(data: &Path) -> bool {
    ON.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p == data)
}

fn set_on(data: &Path, on: bool) {
    let mut g = ON.lock().unwrap_or_else(|e| e.into_inner());
    g.retain(|p| p != data);
    if on {
        g.push(data.to_path_buf());
    }
}

static TOLD: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// Has this person already been told about the session that `request_id` started? Marks it told.
fn first_time(user: &str, request_id: u64) -> bool {
    let mut g = TOLD.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    if m.get(user) == Some(&request_id) {
        return false;
    }
    m.insert(user.to_string(), request_id);
    true
}

/// Should this signed-in person hear about it? The one who started the session, and any administrator.
pub fn is_for(user: &str, admin: bool, starter: &str) -> bool {
    admin || (!starter.is_empty() && user == starter)
}

/// Look at the service's status and leave the note, once per session and person. Returns whether a note was left.
pub fn tick(svc: &crate::services::Services, data: &Path, user: &str, admin: bool) -> bool {
    let status = control::read_status(data);
    set_on(data, status.state == State::On);
    let Some(n) = note_for(&status) else { return false };
    if !is_for(user, admin, &control::read_request(data).by) || !first_time(user, status.request_id) {
        return false;
    }
    svc.notify(user, n.kind, &n.title, &n.body, Some(crate::inbox::Link::new("settings", ""))).is_ok()
}

/// Start over (tests of other crates use it too).
#[doc(hidden)]
pub fn forget_for_tests() {
    *TOLD.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(state: State, note: &str, id: u64) -> Status {
        Status { state, note: note.into(), request_id: id, ..Status::default() }
    }

    #[test]
    fn only_an_unasked_end_with_a_reason_makes_a_note() {
        assert!(note_for(&st(State::On, "", 3)).is_none(), "running");
        assert!(note_for(&st(State::Off, "", 3)).is_none(), "the person stopped it");
        assert!(note_for(&st(State::Failed, "Sharing stopped: the administrator allows at most 4 h per session.", 0)).is_none(), "no session yet");
        let f = note_for(&st(State::Failed, "The cable was unplugged.", 4)).unwrap();
        assert_eq!(f.kind, Kind::Warning);
        assert!(f.body.contains("cable was unplugged") && f.body.contains("lost their internet") && f.body.contains("Settings"), "{}", f.body);
        let o = note_for(&st(State::Off, "It switched itself off after 2 h with nobody using it.", 5)).unwrap();
        assert_eq!((o.kind, o.title.as_str()), (Kind::Info, "Internet sharing switched off"));
        let long = note_for(&st(State::Failed, &"x".repeat(1000), 6)).unwrap();
        assert!(long.body.chars().count() < 450);
    }

    #[test]
    fn the_starter_and_administrators_hear_about_it_and_each_session_is_told_once_per_person() {
        assert!(is_for("amy", false, "amy") && is_for("root", true, "amy") && !is_for("bob", false, "amy") && !is_for("bob", false, ""));
        forget_for_tests();
        assert!(first_time("notice-amy", 7));
        assert!(!first_time("notice-amy", 7));
        assert!(first_time("notice-root", 7), "another person is told separately");
        assert!(first_time("notice-amy", 8), "a later session is new");
    }
}
