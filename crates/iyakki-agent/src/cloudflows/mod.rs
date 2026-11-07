//! Schedules and workflows in cloud mode: the tenant scheduler service and what it needs
//! (plan and audit: `docs/development/CLOUD-FLOWS-PLAN.md`).
//!
//! Design property (owner, 2026-11-04): a person's session may go away and scheduled workflows keep running; notifications and
//! approvals are received later. Schedules and runs belong to the TENANT SERVICE, not to a session:
//!
//! * [`service`]: one scheduler per tenant volume, run only by the replica that holds the [`lease`]; it starts the accounts' flow
//!   engines with nobody signed in, watches run time, pauses schedules whose owner lost access, counts what happened.
//! * [`identity`]: the identity a schedule runs as (owner with a role snapshot, or a team service identity), the rule for secrets
//!   without a person (only `flows.unattended` team secrets), the refusal of personal logins, and run tokens that are never stored.
//! * [`quota`]: limits from the tenant configuration (`flows.quota`), with safe defaults.
//! * [`tz`]: the tenant's time zone (`schedule.timezone`) for cron triggers.
//! * [`executor`] and [`job`]: where a run executes (in this process with limits, or as a Job on the team's cluster binding) and the
//!   Job manifest as text.
//! * [`approval`]: a flow `approval` node as a tenant approval (router in `tenantconfig::approvals`), with timeout, reminders and
//!   exactly-once resolution.
//! * [`notify`]: durable notifications from flows, behind a small sink trait so channels can be plugged in.
//!
//! Everything takes the time as a number, so tests use a fake clock. Nothing here starts a cluster.

pub mod approval;
pub mod executor;
pub mod identity;
pub mod job;
pub mod lease;
pub mod notify;
pub mod quota;
pub mod real;
pub mod sandboxed;
pub mod service;
pub mod tz;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

/// The tenant's folder for everything here: `<data>/cloud/flows`.
pub fn dir(data: &Path) -> PathBuf {
    data.join("cloud").join("flows")
}

/// Write a small state file atomically (private to the service user).
pub(crate) fn save_json<T: serde::Serialize>(path: &Path, v: &T) -> Result<(), String> {
    iyakki_schema::write_atomic(path, serde_json::to_string_pretty(v).unwrap_or_default().as_bytes(), 0o600)
}

pub(crate) fn load_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}
