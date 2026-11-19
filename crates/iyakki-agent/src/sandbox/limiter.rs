//! Concurrency of runs: per person and per team, with a queue that picks the next person fairly (`tenantconfig::fairshare`).

use super::spec::SandboxError;
use crate::tenantconfig::fairshare::{next_to_run, Limits, Running};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Default)]
struct State {
    running: Running,
    /// (person, asked at, ticket)
    queue: Vec<(String, u64, u64)>,
    next_ticket: u64,
}

pub struct Limiter {
    limits: Limits,
    st: Mutex<State>,
    cv: Condvar,
}

/// A running slot; dropping it frees the slot and wakes the queue.
pub struct Permit {
    l: Arc<Limiter>,
    person: String,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut s = self.l.st.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = s.running.by_person.get_mut(&self.person) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                s.running.by_person.remove(&self.person);
            }
        }
        drop(s);
        self.l.cv.notify_all();
    }
}

impl Limiter {
    pub fn new(per_person: u32, per_team: u32, queue_timeout_secs: u64) -> Arc<Limiter> {
        Arc::new(Limiter { limits: Limits { per_person: per_person.max(1), per_team: per_team.max(1), queue_timeout_secs }, st: Mutex::new(State::default()), cv: Condvar::new() })
    }

    pub fn running(&self) -> u32 {
        self.st.lock().unwrap_or_else(|e| e.into_inner()).running.total()
    }

    pub fn queued(&self) -> usize {
        self.st.lock().unwrap_or_else(|e| e.into_inner()).queue.len()
    }

    /// Wait for a slot. `now` is the clock in seconds (tests pass a fake), `tick` how long to sleep between looks.
    pub fn acquire(self: &Arc<Self>, person: &str, now: &dyn Fn() -> u64, tick: Duration) -> Result<Permit, SandboxError> {
        let since = now();
        let mut s = self.st.lock().unwrap_or_else(|e| e.into_inner());
        let ticket = s.next_ticket;
        s.next_ticket += 1;
        s.queue.push((person.to_string(), since, ticket));
        loop {
            let t = now();
            let pairs: Vec<(String, u64)> = s.queue.iter().map(|(p, at, _)| (p.clone(), *at)).collect();
            if let Some(i) = next_to_run(&self.limits, &s.running, &pairs, t) {
                if s.queue[i].2 == ticket {
                    s.queue.remove(i);
                    *s.running.by_person.entry(person.to_string()).or_insert(0) += 1;
                    return Ok(Permit { l: self.clone(), person: person.to_string() });
                }
            }
            if t >= since + self.limits.queue_timeout_secs {
                s.queue.retain(|q| q.2 != ticket);
                drop(s);
                self.cv.notify_all();
                return Err(SandboxError::QueueTimeout);
            }
            s = self.cv.wait_timeout(s, tick).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}
