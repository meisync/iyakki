//! The delivery queue: entries wait for their time, fail with a backoff, and after too many tries become dead letters that stay for a
//! person to see. Bounded: a full queue refuses new work (and says so) instead of growing without end. Pure functions over the lists the
//! hub keeps in its state file.

use super::sink::Severity;
use serde::{Deserialize, Serialize};

pub const MAX_QUEUE: usize = 2000;
pub const MAX_DEAD: usize = 200;
pub const MAX_ATTEMPTS: u32 = 6;
pub const MAX_BODY: usize = 8192;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: String,
    /// The channel's key: `<team>/<name>`, or `/<name>` for a tenant-level channel.
    pub channel: String,
    /// The person it is for (email), or `*` for a webhook call.
    pub person: String,
    pub subject: String,
    pub body: String,
    pub severity: Severity,
    pub attempts: u32,
    pub next_at: u64,
    pub created: u64,
    #[serde(default)]
    pub last_error: String,
}

/// Seconds to wait after the `attempt`-th failure: 30, 60, 120 ... up to an hour.
pub fn backoff(attempt: u32) -> u64 {
    (30u64.saturating_mul(1u64 << attempt.saturating_sub(1).min(20))).min(3600)
}

pub fn push(q: &mut Vec<Delivery>, mut d: Delivery) -> Result<(), String> {
    if q.len() >= MAX_QUEUE {
        return Err("the delivery queue is full".into());
    }
    let mut n = MAX_BODY.min(d.body.len());
    while !d.body.is_char_boundary(n) {
        n -= 1;
    }
    d.body.truncate(n);
    q.push(d);
    Ok(())
}

pub fn due(q: &[Delivery], now: u64) -> Vec<String> {
    let mut v: Vec<&Delivery> = q.iter().filter(|d| d.next_at <= now).collect();
    v.sort_by_key(|d| (d.next_at, d.created));
    v.into_iter().map(|d| d.id.clone()).collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Retry(u64),
    Dead,
}

/// Record a failure of `id`: back off, or move to the dead letters after [`MAX_ATTEMPTS`].
pub fn failed(q: &mut Vec<Delivery>, dead: &mut Vec<Delivery>, id: &str, now: u64, err: &str) -> Option<Outcome> {
    let i = q.iter().position(|d| d.id == id)?;
    q[i].attempts += 1;
    q[i].last_error = err.chars().filter(|c| !c.is_control()).take(200).collect();
    if q[i].attempts >= MAX_ATTEMPTS {
        let d = q.remove(i);
        dead.push(d);
        while dead.len() > MAX_DEAD {
            dead.remove(0);
        }
        return Some(Outcome::Dead);
    }
    q[i].next_at = now + backoff(q[i].attempts);
    Some(Outcome::Retry(q[i].next_at))
}

pub fn done(q: &mut Vec<Delivery>, id: &str) {
    q.retain(|d| d.id != id);
}
