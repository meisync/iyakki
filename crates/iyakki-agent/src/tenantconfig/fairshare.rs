//! Fair share of a scarce shared resource (a local GPU): per-person and per-team concurrency, a queue with a timeout, and the choice
//! of who goes next. Pure functions over counts; the caller keeps the counts. Health of a local host from the last test.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub per_person: u32,
    pub per_team: u32,
    pub queue_timeout_secs: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Running {
    pub by_person: BTreeMap<String, u32>,
}

impl Running {
    pub fn total(&self) -> u32 {
        self.by_person.values().sum()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admit {
    Run,
    /// Wait (the caller asks again); until this time.
    Wait { until: u64 },
    Reject(String),
}

/// May `user` start now? `waiting_since` is when they first asked (None: now).
pub fn admit(l: &Limits, run: &Running, user: &str, waiting_since: Option<u64>, now: u64) -> Admit {
    let since = waiting_since.unwrap_or(now);
    let until = since + l.queue_timeout_secs;
    let mine = run.by_person.get(user).copied().unwrap_or(0);
    if mine < l.per_person && run.total() < l.per_team {
        return Admit::Run;
    }
    if now >= until {
        return Admit::Reject("the queue timed out".into());
    }
    Admit::Wait { until }
}

/// Of the people waiting (user, since), who goes next: someone under their own limit, the person with the fewest running first,
/// then the one who has waited longest. Waiters past the timeout are skipped. `None` when the team is full or nobody may go.
pub fn next_to_run(l: &Limits, run: &Running, queue: &[(String, u64)], now: u64) -> Option<usize> {
    if run.total() >= l.per_team {
        return None;
    }
    queue
        .iter()
        .enumerate()
        .filter(|(_, (u, since))| run.by_person.get(u).copied().unwrap_or(0) < l.per_person && now < since + l.queue_timeout_secs)
        .min_by_key(|(_, (u, since))| (run.by_person.get(u).copied().unwrap_or(0), *since))
        .map(|(i, _)| i)
}

/// `healthy`, `auth_failed`, `unreachable`, `failing` or `unknown` (no test, or a stale one) from the last test (when, ok, status).
pub fn health_of(test: Option<(u64, bool, u16)>, now: u64, stale_secs: u64) -> &'static str {
    match test {
        None => "unknown",
        Some((at, _, _)) if now.saturating_sub(at) > stale_secs => "unknown",
        Some((_, true, _)) => "healthy",
        Some((_, _, 401 | 403)) => "auth_failed",
        Some((_, _, 0)) => "unreachable",
        Some(_) => "failing",
    }
}
