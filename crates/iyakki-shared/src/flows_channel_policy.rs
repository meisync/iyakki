//! Quiet hours and escalation for the outside messages of flows (F-MSG-04), as plain logic on a clock that is passed in.
//!
//! * **Quiet hours.** In the person's quiet hours a message is held and sent when they end; an error (`level: error`) goes through.
//!   The INBOX note is always made at once. Held messages wait in the file, so a restart does not lose them.
//! * **Escalation.** A message that needs the person is watched: when nobody acknowledged it within the set time, one more message
//!   goes to a second room. Acknowledging the flow stops the watch.
//!
//! Routing reuses `notifyhub::rules` (the same quiet-hours rule as the cloud hub). The file is `<flows data>/channel-policy.json`;
//! a damaged one is moved aside and the defaults (nothing held, nothing escalated) apply.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::notifyhub_rules::{Quiet, Route, Rules};
use crate::notifyhub_sink::Severity;

const VERSION: u32 = 1;
/// Held messages kept at most (the oldest go first).
pub const MAX_HELD: usize = 50;
/// Watched messages kept at most.
pub const MAX_WATCHED: usize = 50;
pub use iyakki_flows_model::channel::{Held, Policy, Watch, MAX_ESCALATE_SECS};

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct File {
    version: u32,
    policy: Policy,
    held: Vec<Held>,
    watch: Vec<Watch>,
}

/// What to do with a message now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Send,
    /// Hold it until this time (unix seconds).
    HoldUntil(u64),
}

/// The routing rules a policy stands for.
pub fn rules_of(p: &Policy) -> Rules {
    Rules { quiet: p.quiet.map(|(s, e)| Quiet { start_min: s, end_min: e }), offset_min: p.offset_min, ..Rules::default() }
}

/// The severity of a notice: an error passes quiet hours, a thing that needs the person is high, the rest normal.
pub fn severity_of(level: &str, needs_you: bool) -> Severity {
    match level {
        "error" => Severity::Critical,
        _ if needs_you => Severity::High,
        _ => Severity::Normal,
    }
}

pub struct PolicyBook {
    path: PathBuf,
    lock: Mutex<()>,
}

impl PolicyBook {
    pub fn new(dir: &Path) -> PolicyBook {
        PolicyBook { path: dir.join("channel-policy.json"), lock: Mutex::new(()) }
    }

    fn load(&self) -> File {
        match std::fs::read(&self.path) {
            Err(_) => File { version: VERSION, ..File::default() },
            Ok(b) => match serde_json::from_slice::<File>(&b) {
                Ok(f) if f.version <= VERSION => f,
                _ => {
                    if let Some(to) = iyakki_schema::move_aside(&self.path, iyakki_schema::now_secs()) {
                        iyakki_schema::report(format!("The flow message policy could not be read; moved to {}. Quiet hours and escalation are off until you set them again.", to.display()));
                    }
                    File { version: VERSION, ..File::default() }
                }
            },
        }
    }

    fn save(&self, f: &File) {
        if let Ok(b) = serde_json::to_vec(f) {
            let _ = iyakki_schema::write_atomic(&self.path, &b, 0o600);
        }
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn policy(&self) -> Policy {
        let _g = self.guard();
        self.load().policy
    }

    pub fn set_policy(&self, p: Policy) -> Result<Policy, String> {
        let p = p.cleaned()?;
        let _g = self.guard();
        let mut f = self.load();
        f.version = VERSION;
        f.policy = p.clone();
        self.save(&f);
        Ok(p)
    }

    /// Send now, or hold until the quiet hours end.
    pub fn decide(&self, level: &str, needs_you: bool, now: u64) -> Decision {
        match rules_of(&self.policy()).route(severity_of(level, needs_you), Severity::Low, now) {
            Route::Send(t) if t > now => Decision::HoldUntil(t),
            _ => Decision::Send,
        }
    }

    pub fn hold(&self, h: Held) {
        let _g = self.guard();
        let mut f = self.load();
        f.version = VERSION;
        if f.held.iter().any(|x| x.key == h.key && x.flow == h.flow) {
            return;
        }
        f.held.push(h);
        while f.held.len() > MAX_HELD {
            f.held.remove(0);
        }
        self.save(&f);
    }

    /// The held messages whose time has come (they leave the file).
    pub fn take_due(&self, now: u64) -> Vec<Held> {
        let _g = self.guard();
        let mut f = self.load();
        let (due, keep): (Vec<Held>, Vec<Held>) = f.held.into_iter().partition(|h| h.until <= now);
        if due.is_empty() {
            return due;
        }
        f.held = keep;
        self.save(&f);
        due
    }

    pub fn held(&self) -> Vec<Held> {
        let _g = self.guard();
        self.load().held
    }

    /// A message that needs the person went out: watch it when escalation is on.
    pub fn watch(&self, flow: &str, key: &str, text: &str, now: u64) {
        let _g = self.guard();
        let mut f = self.load();
        if f.policy.escalate_after_secs.is_none() || f.watch.iter().any(|w| w.flow == flow && w.key == key) {
            return;
        }
        f.version = VERSION;
        f.watch.push(Watch { flow: flow.into(), text: text.chars().take(500).collect(), key: key.into(), since: now });
        while f.watch.len() > MAX_WATCHED {
            f.watch.remove(0);
        }
        self.save(&f);
    }

    /// The person saw it: stop watching everything of this flow. Returns how many were watched.
    pub fn ack(&self, flow: &str) -> usize {
        let _g = self.guard();
        let mut f = self.load();
        let n = f.watch.len();
        f.watch.retain(|w| w.flow != flow);
        let gone = n - f.watch.len();
        if gone > 0 {
            self.save(&f);
        }
        gone
    }

    /// Watched messages nobody acknowledged in time. Each is returned once and then no longer watched.
    pub fn escalations_due(&self, now: u64) -> Vec<Watch> {
        let _g = self.guard();
        let mut f = self.load();
        let Some(after) = f.policy.escalate_after_secs else { return vec![] };
        let (due, keep): (Vec<Watch>, Vec<Watch>) = f.watch.into_iter().partition(|w| now >= w.since.saturating_add(after));
        if due.is_empty() {
            return due;
        }
        f.watch = keep;
        self.save(&f);
        due
    }

    pub fn watching(&self) -> Vec<Watch> {
        let _g = self.guard();
        self.load().watch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-chpol-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const H: u64 = 3600;

    #[test]
    fn quiet_hours_hold_a_message_until_they_end_and_an_error_goes_through() {
        let b = PolicyBook::new(&dir("quiet"));
        assert_eq!(b.decide("info", false, 3 * H), Decision::Send, "no quiet hours set");
        // 22:00 to 07:00 UTC
        b.set_policy(Policy { quiet: Some((22 * 60, 7 * 60)), ..Policy::default() }).unwrap();
        assert_eq!(b.decide("info", false, 3 * H), Decision::HoldUntil(7 * H));
        assert_eq!(b.decide("warn", true, 23 * H), Decision::HoldUntil(31 * H), "past midnight");
        assert_eq!(b.decide("info", false, 12 * H), Decision::Send);
        assert_eq!(b.decide("error", true, 3 * H), Decision::Send, "an error passes");
        // another zone: 10:00 UTC is 22:00 at +12:00
        b.set_policy(Policy { quiet: Some((22 * 60, 7 * 60)), offset_min: 12 * 60, ..Policy::default() }).unwrap();
        assert!(matches!(b.decide("info", false, 10 * H), Decision::HoldUntil(_)));
    }

    #[test]
    fn held_messages_survive_a_restart_and_leave_once_when_due() {
        let d = dir("held");
        let b = PolicyBook::new(&d);
        let h = |k: &str, until| Held { flow: "f".into(), room: String::new(), text: "t".into(), key: k.into(), until };
        b.hold(h("a", 100));
        b.hold(h("a", 100));
        b.hold(h("b", 500));
        let b = PolicyBook::new(&d);
        assert_eq!(b.held().len(), 2, "kept, and the same key twice is one");
        assert!(b.take_due(50).is_empty());
        assert_eq!(b.take_due(200).len(), 1);
        assert!(b.take_due(200).is_empty());
        assert_eq!(b.take_due(600)[0].key, "b");
    }

    #[test]
    fn an_unacknowledged_message_escalates_once_and_an_acknowledgement_stops_it() {
        let b = PolicyBook::new(&dir("esc"));
        b.watch("f", "k0", "ignored: escalation is off", 0);
        assert!(b.watching().is_empty());
        b.set_policy(Policy { escalate_after_secs: Some(600), escalate_room: "ops".into(), ..Policy::default() }).unwrap();
        b.watch("f", "k1", "door open", 1000);
        b.watch("g", "k2", "disk full", 1000);
        b.watch("f", "k1", "again", 1001);
        assert_eq!(b.watching().len(), 2);
        assert!(b.escalations_due(1500).is_empty());
        assert_eq!(b.ack("g"), 1);
        let due = b.escalations_due(1700);
        assert_eq!(due.len(), 1);
        assert_eq!((due[0].flow.as_str(), due[0].text.as_str()), ("f", "door open"));
        assert!(b.escalations_due(99_999).is_empty(), "once");
    }

    #[test]
    fn a_policy_is_validated_and_a_damaged_file_means_defaults() {
        let d = dir("bad");
        let b = PolicyBook::new(&d);
        assert!(b.set_policy(Policy { quiet: Some((1500, 10)), ..Policy::default() }).is_err());
        assert!(b.set_policy(Policy { escalate_after_secs: Some(5), ..Policy::default() }).is_err());
        let p = b.set_policy(Policy { quiet: Some((60, 60)), offset_min: 5000, escalate_after_secs: Some(u64::MAX), ..Policy::default() }).unwrap();
        assert_eq!((p.quiet, p.offset_min, p.escalate_after_secs), (None, 840, Some(MAX_ESCALATE_SECS)));
        std::fs::write(d.join("channel-policy.json"), b"{ not json").unwrap();
        assert_eq!(PolicyBook::new(&d).policy(), Policy::default());
        assert!(Policy { quiet: Some((22 * 60, 7 * 60)), escalate_after_secs: Some(1800), escalate_room: "ops".into(), ..Policy::default() }.summary().contains("22:00 to 07:00"));
    }
}
