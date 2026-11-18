//! Events as a first-class shape: every event has a sequence number, a client resumes from the last one it saw instead of polling.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event<T> {
    /// Strictly increasing per log, starting at 1; never reused.
    pub seq: u64,
    pub at_ms: u64,
    pub topic: String,
    pub body: T,
}

/// Where a subscriber starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resume {
    /// Only what happens from now on.
    Latest,
    /// Everything after this sequence number (0 = from the oldest kept).
    After(u64),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventBatch<T> {
    pub events: Vec<Event<T>>,
    /// Pass `Resume::After(next)` to continue.
    pub next: u64,
    /// True when events the client missed were already dropped from the log: it must re-read the state before trusting the stream.
    pub gap: bool,
}

/// A bounded log: the newest `capacity` events are kept. Pure; the clock is passed in.
#[derive(Debug)]
pub struct EventLog<T> {
    capacity: usize,
    last: u64,
    items: VecDeque<Event<T>>,
}

impl<T: Clone> EventLog<T> {
    pub fn new(capacity: usize) -> EventLog<T> {
        EventLog { capacity: capacity.max(1), last: 0, items: VecDeque::new() }
    }
    pub fn push(&mut self, topic: &str, at_ms: u64, body: T) -> u64 {
        self.last += 1;
        self.items.push_back(Event { seq: self.last, at_ms, topic: topic.to_string(), body });
        while self.items.len() > self.capacity {
            self.items.pop_front();
        }
        self.last
    }
    pub fn last_seq(&self) -> u64 {
        self.last
    }
    /// Events after `resume` whose topic matches (`None` = all), at most `limit`, oldest first.
    pub fn read(&self, resume: Resume, topic: Option<&str>, limit: usize) -> EventBatch<T> {
        let after = match resume {
            Resume::Latest => return EventBatch { events: vec![], next: self.last, gap: false },
            Resume::After(n) => n,
        };
        let oldest_kept = self.items.front().map(|e| e.seq).unwrap_or(self.last + 1);
        let gap = after + 1 < oldest_kept;
        let mut events = Vec::new();
        let mut next = after;
        for e in self.items.iter().filter(|e| e.seq > after) {
            if events.len() >= limit {
                break;
            }
            next = e.seq;
            if topic.is_none_or(|t| t == e.topic) {
                events.push(e.clone());
            }
        }
        if events.len() < limit {
            next = self.last.max(after);
        }
        EventBatch { events, next, gap }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log() -> EventLog<u32> {
        let mut l = EventLog::new(4);
        for i in 1..=6u32 {
            l.push(if i % 2 == 0 { "even" } else { "odd" }, i as u64 * 10, i);
        }
        l
    }

    #[test]
    fn resume_returns_exactly_the_missed_events_in_order() {
        let mut l = EventLog::new(10);
        for i in 1..=5u32 {
            l.push("t", i as u64, i);
        }
        let b = l.read(Resume::After(2), None, 100);
        assert_eq!(b.events.iter().map(|e| (e.seq, e.body)).collect::<Vec<_>>(), [(3, 3), (4, 4), (5, 5)]);
        assert_eq!((b.next, b.gap), (5, false));
        assert!(l.read(Resume::After(5), None, 100).events.is_empty());
    }

    #[test]
    fn latest_sees_nothing_old_and_a_dropped_range_is_reported_as_a_gap() {
        let l = log();
        assert!(l.read(Resume::Latest, None, 10).events.is_empty());
        let b = l.read(Resume::After(0), None, 10);
        assert!(b.gap, "events 1 and 2 were dropped");
        assert_eq!(b.events.iter().map(|e| e.seq).collect::<Vec<_>>(), [3, 4, 5, 6]);
        assert!(!l.read(Resume::After(2), None, 10).gap, "nothing the client had not seen was lost");
    }

    #[test]
    fn a_limit_pages_without_skipping_or_repeating() {
        let l = log();
        let a = l.read(Resume::After(2), None, 2);
        let b = l.read(Resume::After(a.next), None, 2);
        let seqs: Vec<u64> = a.events.iter().chain(b.events.iter()).map(|e| e.seq).collect();
        assert_eq!(seqs, [3, 4, 5, 6]);
    }

    #[test]
    fn a_topic_filter_keeps_order_and_still_advances_past_other_topics() {
        let l = log();
        let b = l.read(Resume::After(2), Some("even"), 10);
        assert_eq!(b.events.iter().map(|e| e.seq).collect::<Vec<_>>(), [4, 6]);
        assert_eq!(b.next, 6);
    }
}
