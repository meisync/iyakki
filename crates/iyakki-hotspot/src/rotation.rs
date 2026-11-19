//! When the hotspot changes its group key (F-HOTSPOT-04): the decision only, with the time passed in. The change itself is
//! [`crate::hostap::HostAp::rotate_group_key`]. Two reasons to change it: a station has left (it still knows the old key and could
//! read broadcast traffic that follows) and time (a key that is never changed lives as long as the hotspot). Changes are spaced
//! out so a crowd leaving does not cause one exchange per person, and nothing is changed while nobody is connected (the next
//! station gets a fresh key anyway when the access point starts, and an idle hotspot has no traffic to protect).
// SPDX-License-Identifier: Apache-2.0

/// The person's choice. The default changes the key one hour after the last change and a minute after somebody left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RotationPolicy {
    /// Seconds between changes while stations are connected; 0 turns the timer off.
    pub interval_secs: u64,
    /// Change the key after a station has left.
    pub on_leave: bool,
    /// The least time between two changes.
    pub min_gap_secs: u64,
}

impl Default for RotationPolicy {
    fn default() -> Self {
        RotationPolicy { interval_secs: 3600, on_leave: true, min_gap_secs: 60 }
    }
}

impl RotationPolicy {
    /// Limits that keep the policy sensible: a timer of at least 5 minutes, a gap of at least 10 seconds and at most an hour.
    pub fn checked(self) -> Result<RotationPolicy, String> {
        if self.interval_secs != 0 && self.interval_secs < 300 {
            return Err("the key change interval must be at least 5 minutes (or 0 for never)".into());
        }
        if !(10..=3600).contains(&self.min_gap_secs) {
            return Err("the least time between two key changes must be 10 seconds to one hour".into());
        }
        Ok(self)
    }
}

/// Why a change is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    StationLeft,
    Timer,
}

impl Reason {
    pub fn text(self) -> &'static str {
        match self {
            Reason::StationLeft => "a device left the hotspot",
            Reason::Timer => "the key has been in use for the set time",
        }
    }
}

/// Follows the last change and the stations that left since.
pub struct Rotation {
    policy: RotationPolicy,
    last: u64,
    left_since: bool,
}

impl Rotation {
    /// `now` in seconds on any steady clock (the start of the hotspot counts as the first key).
    pub fn new(policy: RotationPolicy, now: u64) -> Rotation {
        Rotation { policy, last: now, left_since: false }
    }

    pub fn station_left(&mut self) {
        self.left_since = true;
    }

    /// Is a change due? `connected` is the number of stations that finished the handshake.
    pub fn due(&self, now: u64, connected: usize) -> Option<Reason> {
        if connected == 0 || now.saturating_sub(self.last) < self.policy.min_gap_secs {
            return None;
        }
        if self.policy.on_leave && self.left_since {
            return Some(Reason::StationLeft);
        }
        if self.policy.interval_secs != 0 && now.saturating_sub(self.last) >= self.policy.interval_secs {
            return Some(Reason::Timer);
        }
        None
    }

    /// The change has been made (whether or not every station answered).
    pub fn done(&mut self, now: u64) {
        self.last = now;
        self.left_since = false;
    }

    /// A change was asked for and refused: keep the reason (a leaver still counts) but wait the least gap before asking again.
    pub fn done_failed(&mut self, now: u64) {
        self.last = now;
    }

    /// When to look again: the earliest second at which `due` could say yes, for a loop that sleeps.
    pub fn next_check(&self, now: u64) -> u64 {
        let gap_end = self.last + self.policy.min_gap_secs;
        let mut at = if self.policy.on_leave && self.left_since { gap_end } else { u64::MAX };
        if self.policy.interval_secs != 0 {
            at = at.min(self.last + self.policy.interval_secs);
        }
        if at == u64::MAX { now + 60 } else { at.max(now) }
    }
}

/// The key index after `id` (1, 2, 3, 1, ...): never the one in use, so a station keeps the old key until it has the new one.
pub fn next_key_id(id: u8) -> u8 {
    if (1..=2).contains(&id) { id + 1 } else { 1 }
}

/// What the access point loop calls on every beat: asks the decision and, when a change is due, runs `rotate` with the next key
/// index. `secs` is a steady clock in seconds, `connected` the stations that finished their handshake. Returns the reason when a
/// change was made. A refused change (one is still going on, the card cannot switch) is not counted as made and is tried again at
/// the next beat after the least gap, so a card that cannot change the key is not asked every 100 ms.
pub fn step(rot: &mut Rotation, secs: u64, connected: usize, key_id: u8, rotate: &mut dyn FnMut(u8) -> Result<(), String>, log: &mut dyn FnMut(String)) -> Option<Reason> {
    let reason = rot.due(secs, connected)?;
    match rotate(next_key_id(key_id)) {
        Ok(()) => {
            rot.done(secs);
            log(format!("hotspot: changing the group key ({})", reason.text()));
            Some(reason)
        }
        Err(e) => {
            // wait for the least gap before the next try
            rot.done_failed(secs);
            log(format!("hotspot: group key not changed: {e}"));
            None
        }
    }
}

#[cfg(test)]
#[path = "rotation_tests.rs"]
mod tests;
