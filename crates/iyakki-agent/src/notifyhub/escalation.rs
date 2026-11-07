//! The escalation chain as a pure state machine: nobody acknowledged within T, so the next level hears of it (the people it was for,
//! then the team's admins, then the tenant's admins). The clock is a parameter; the hub stores the state and acts on the steps.

use super::sink::Severity;
use crate::inbox::Link;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    TeamAdmins,
    TenantAdmins,
}

/// Levels after the first; each waits `after` seconds from the step before.
pub fn chain(after: u64) -> [(u64, Target); 2] {
    [(after, Target::TeamAdmins), (after, Target::TenantAdmins)]
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Esc {
    /// The hub's id of the notification.
    pub id: String,
    pub team: Option<String>,
    pub after: u64,
    /// 0: the people it was for; 1: the team's admins; 2: the tenant's admins.
    pub level: usize,
    /// When the last step happened.
    pub last: u64,
    pub acked: bool,
    pub done: bool,
    pub severity: Severity,
    pub title: String,
    pub body: String,
    pub link: Option<Link>,
    pub source: String,
    /// person -> the id of their INBOX note (an acknowledgement names it).
    pub inbox: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Idle,
    Escalate(Target),
    /// The chain is used up and nobody answered.
    Exhausted,
}

impl Esc {
    pub fn step(&self, now: u64) -> Step {
        if self.acked || self.done {
            return Step::Idle;
        }
        let c = chain(self.after);
        if now < self.last.saturating_add(self.after) {
            return Step::Idle;
        }
        match c.get(self.level) {
            Some((_, t)) => Step::Escalate(*t),
            None => Step::Exhausted,
        }
    }

    pub fn apply(&mut self, s: Step, now: u64) {
        match s {
            Step::Idle => {}
            Step::Escalate(_) => {
                self.level += 1;
                self.last = now;
            }
            Step::Exhausted => self.done = true,
        }
    }

    pub fn ack(&mut self) {
        self.acked = true;
    }
}
