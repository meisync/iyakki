//! The pair group (fleet) door to the core (F-FLEET-07; the owner's core rule: nothing that joins this computer to others, sends them work or
//! reads their numbers is reached around the core). The fleet itself (the signed group record, the keys, the channel to the other members,
//! the heartbeats) lives in `iyakki-agent`; what it must ask the core is small:
//!
//! * `fleet.read` (no card, one access-log line) before the members and their numbers are shown;
//! * `fleet.join` (a hard ask, a card every time) before this computer asks to join a group or accepts an offer to join;
//! * `fleet.manage` (a hard ask) before a group is created, a join code is offered, a member is approved, made an administrator or removed,
//!   or this computer leaves;
//! * `fleet.assign` (a hard ask) before work is sent to another member;
//! * `fleet.receive` (a hard ask) before work another member sent is accepted, or before the choice of whether to accept work changes;
//! * one access-log line for every automatic step of the channel (`record`): ids and numbers only, never content.
//!
//! The device gives the agent `iyakki_core_local::LocalFleet` (the broker has already decided), a test gives `iyakki_core_fake::FakeFleet`. A machine
//! whose `Os` does not say otherwise has no fleet and refuses ([`NoFleet`]).

pub trait Fleet: Send + Sync {
    /// Permission to see the groups, members and numbers; `label` says what.
    fn read(&self, label: &str) -> Result<(), String>;
    fn join(&self, label: &str) -> Result<(), String>;
    fn manage(&self, label: &str) -> Result<(), String>;
    fn assign(&self, label: &str) -> Result<(), String>;
    fn receive(&self, label: &str) -> Result<(), String>;
    /// One step of the channel for the access log (`what` like `fleet.heartbeat`, `detail` ids and numbers).
    fn record(&self, what: &str, detail: &str);
}

/// A machine without a fleet: every request is refused and nothing is written.
pub struct NoFleet;

const NONE: &str = "this machine has no pair group";

impl Fleet for NoFleet {
    fn read(&self, _: &str) -> Result<(), String> {
        Err(NONE.into())
    }
    fn join(&self, _: &str) -> Result<(), String> {
        Err(NONE.into())
    }
    fn manage(&self, _: &str) -> Result<(), String> {
        Err(NONE.into())
    }
    fn assign(&self, _: &str) -> Result<(), String> {
        Err(NONE.into())
    }
    fn receive(&self, _: &str) -> Result<(), String> {
        Err(NONE.into())
    }
    fn record(&self, _: &str, _: &str) {}
}
