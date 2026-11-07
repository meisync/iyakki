//! Routing as pure data: which severities a channel takes, quiet hours, the daily digest. The clock is always passed in; the tenant's
//! time zone is a fixed offset in minutes (`utc_offset_minutes`), which is enough and never changes under a running tenant.

use super::sink::Severity;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quiet {
    /// Minutes after local midnight.
    pub start_min: u32,
    pub end_min: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    pub quiet: Option<Quiet>,
    /// Local minute of the day when the digest goes; `None`: no digest, low severity is sent like the others.
    pub digest_min: Option<u32>,
    pub offset_min: i32,
    /// No acknowledgement within this long moves a notification up one level (owner, team admin, tenant admin).
    pub escalate_after_secs: Option<u64>,
    pub escalate_min: Severity,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules { quiet: None, digest_min: None, offset_min: 0, escalate_after_secs: None, escalate_min: Severity::High }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// Below the channel's threshold.
    Skip,
    /// Held for the digest.
    Digest,
    /// Deliver at this time (now, or when the quiet hours end).
    Send(u64),
}

const DAY: i64 = 86_400;

pub fn local_secs(now: u64, offset_min: i32) -> i64 {
    (now as i64 + offset_min as i64 * 60).rem_euclid(DAY)
}

pub fn local_day(now: u64, offset_min: i32) -> i64 {
    (now as i64 + offset_min as i64 * 60).div_euclid(DAY)
}

impl Rules {
    pub fn in_quiet(&self, now: u64) -> bool {
        let Some(q) = self.quiet else { return false };
        if q.start_min == q.end_min {
            return false;
        }
        let m = (local_secs(now, self.offset_min) / 60) as u32;
        if q.start_min < q.end_min { m >= q.start_min && m < q.end_min } else { m >= q.start_min || m < q.end_min }
    }

    /// The first moment at or after `now` that is outside the quiet hours.
    pub fn quiet_end(&self, now: u64) -> u64 {
        if !self.in_quiet(now) {
            return now;
        }
        let Some(q) = self.quiet else { return now };
        let delta = (q.end_min as i64 * 60 - local_secs(now, self.offset_min)).rem_euclid(DAY);
        now + delta.max(1) as u64
    }

    /// The local day to flush the digest for, when it is due (after the digest minute, once a day).
    pub fn digest_due(&self, now: u64, last_day: i64) -> Option<i64> {
        let d = self.digest_min?;
        let day = local_day(now, self.offset_min);
        ((local_secs(now, self.offset_min) / 60) as u32 >= d && day != last_day).then_some(day)
    }

    /// What to do with a notification of `sev` on a channel that takes `min` and above.
    pub fn route(&self, sev: Severity, min: Severity, now: u64) -> Route {
        if sev == Severity::Low && self.digest_min.is_some() && min <= Severity::Low {
            return Route::Digest;
        }
        if sev < min {
            return Route::Skip;
        }
        if sev == Severity::Critical {
            return Route::Send(now);
        }
        Route::Send(self.quiet_end(now))
    }
}
