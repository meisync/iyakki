//! The notification hub of a cloud tenant (plan: `docs/development/NOTIFICATIONS-PLAN.md`).
//!
//! Everything here is DURABLE and independent of sessions: an emitted notification is written to the tenant's volume (a fan-out job
//! first, then each person's INBOX file) before any delivery is tried; the channel queue, escalations, reminders and digests are in
//! the same state file and survive a restart. The workers (`Hub::tick`) run in the service process without a signed-in person, on a
//! lease so that only one replica delivers. Channels (webhook, email) are OFF unless the tenant configures a `notify.channel` item;
//! they send data out of the tenant, so every delivery passes the tenant's egress allow-list and, by default, carries only a short
//! non-sensitive summary and a link, never an action link.
//!
//! Layout: [`sink`] (the contract other modules emit through), [`redact`], [`expand`] (recipients), [`rules`] (severity, quiet hours,
//! digest), [`escalation`] and [`reminders`] (pure state machines on a clock that is passed in), [`queue`] (retry and dead letters),
//! [`lease`], [`webhook`], [`smtp`], [`channel`] (configuration), [`transport`] (the real network), [`events`] (live stream), [`service`]
//! (the hub) and [`api`] (what the web routes call).

pub mod api;
pub mod channel;
pub mod escalation;
pub mod events;
pub mod expand;
pub mod lease;
pub mod queue;
pub mod redact;
pub mod reminders;
pub mod rules;
pub mod service;
pub mod sink;
pub mod smtp;
pub mod transport;
pub mod webhook;

#[cfg(test)]
mod tests;
