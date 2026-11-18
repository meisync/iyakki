//! Types moved out of `iyakki_agent::sessions` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde::Deserialize;
use serde::Serialize;
use crate::chat::Message;

/// A chat opens showing the last three days, at most this many messages; older ones load as the person scrolls up.
pub const WINDOW_DAYS: u64 = 3;

pub const WINDOW_MESSAGES: usize = 1000;

/// An old chat still opens with at least its last few messages.
pub const WINDOW_MIN: usize = 10;

pub const DAY_MS: u64 = 86_400_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub owner: String,
    pub title: String,
    /// Milliseconds since the Unix epoch.
    pub created: u64,
    pub updated: u64,
    pub messages: Vec<Message>,
    /// When each message was added (milliseconds since the epoch), in step with `messages`.
    #[serde(default)]
    pub stamps: Vec<u64>,
    /// A few lines about the part of this chat that is no longer sent to the model word for word (rolling, rebuilt as the
    /// chat grows).
    #[serde(default)]
    pub summary: String,
}

impl Session {
    pub fn stamp(&self, i: usize) -> u64 {
        self.stamps.get(i).copied().unwrap_or(self.updated)
    }

    /// Index of the first message to show when the chat opens: the last `days` days, at most `max` messages, but never
    /// fewer than the last `WINDOW_MIN`.
    pub fn window_start(&self, now_ms: u64, days: u64, max: usize) -> usize {
        let n = self.messages.len();
        let cutoff = now_ms.saturating_sub(days * DAY_MS);
        let by_time = (0..n).find(|&i| self.stamp(i) >= cutoff).unwrap_or(n);
        let start = by_time.max(n.saturating_sub(max));
        if n - start < WINDOW_MIN { n.saturating_sub(WINDOW_MIN) } else { start }
    }

    /// Where the next older stretch begins when the person scrolls up from `shown_from`: another `days` days back,
    /// but at least 20 and at most 500 messages at a time.
    pub fn earlier_start(&self, shown_from: usize, days: u64) -> usize {
        if shown_from == 0 {
            return 0;
        }
        let cutoff = self.stamp(shown_from).saturating_sub(days * DAY_MS);
        let by_time = (0..shown_from).find(|&i| self.stamp(i) >= cutoff).unwrap_or(shown_from);
        by_time.min(shown_from.saturating_sub(20)).max(shown_from.saturating_sub(500))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub updated: u64,
    pub messages: usize,
}
