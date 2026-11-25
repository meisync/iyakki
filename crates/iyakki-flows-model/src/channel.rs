//! The outside-message policy of flows as plain data (F-MSG-04; moved out of `iyakki-shared` for the FLOWS tile crate, alpha.8 N1): quiet hours,
//! escalation, a message held for the end of the quiet hours and a message watched for an acknowledgement. The book that keeps them in a
//! file and routes a message (`PolicyBook`) stays in `iyakki_shared::flows_channel_policy`, which re-exports these types.

use serde::{Deserialize, Serialize};

/// The longest escalation time that can be set (one week).
pub const MAX_ESCALATE_SECS: u64 = 7 * 24 * 3600;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Policy {
    /// Quiet hours as minutes after local midnight (start, end); `None`: none.
    pub quiet: Option<(u32, u32)>,
    /// Minutes east of UTC for the quiet hours.
    pub offset_min: i32,
    /// Ask again in `escalate_room` after this many seconds without an acknowledgement.
    pub escalate_after_secs: Option<u64>,
    /// A saved room (empty: the main room).
    pub escalate_room: String,
}

impl Policy {
    pub fn cleaned(mut self) -> Result<Policy, String> {
        if let Some((s, e)) = self.quiet {
            if s >= 1440 || e >= 1440 {
                return Err("quiet hours are minutes after midnight, below 1440".into());
            }
            if s == e {
                self.quiet = None;
            }
        }
        self.offset_min = self.offset_min.clamp(-840, 840);
        if let Some(n) = self.escalate_after_secs {
            if n < 60 {
                return Err("escalate after at least a minute".into());
            }
            self.escalate_after_secs = Some(n.min(MAX_ESCALATE_SECS));
        }
        self.escalate_room = self.escalate_room.trim().chars().filter(|c| !c.is_control()).take(80).collect();
        Ok(self)
    }

    pub fn summary(&self) -> String {
        let hm = |m: u32| format!("{:02}:{:02}", m / 60, m % 60);
        let q = self.quiet.map_or("no quiet hours".to_string(), |(s, e)| format!("quiet hours {} to {} (errors still come through)", hm(s), hm(e)));
        let e = self.escalate_after_secs.map_or("no escalation".to_string(), |n| format!("escalates to {} after {} min without an acknowledgement", if self.escalate_room.is_empty() { "the main room" } else { &self.escalate_room }, n / 60));
        format!("{q}; {e}")
    }
}

/// A message held for the end of the quiet hours.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Held {
    pub flow: String,
    pub room: String,
    pub text: String,
    pub key: String,
    pub until: u64,
}

/// A message that needs the person, watched for an acknowledgement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watch {
    pub flow: String,
    pub text: String,
    pub key: String,
    pub since: u64,
}

