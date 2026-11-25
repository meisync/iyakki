//! A clock that looks wrong for the device-key sign-in and the firewall's expiry times (F-LIFE-19, owner decision P-15: warn, do not
//! refuse). Both compare a time the device holds with the wall clock: a device-key request is refused when its time is more than
//! [`KEY_WINDOW_SECS`] away from the server's clock (`iyakki-remote::devicekeys::WINDOW_SECS`), and a firewall rule that has an
//! expiry is kept or dropped by the same clock. The check is written against [`WallClock`] so a test moves time with a manual clock.
//!
//! Two signs of a wrong clock:
//! * the clock is earlier than the build of this system ([`iyakki_clockfloor::COMPILED_FLOOR`]): no real time is before it, so the
//!   clock is unset or damaged (the clock floor normally moves it forward at boot; this shows when it could not);
//! * the network time says the clock is ahead by more than the window and the clock was left alone (the person has not agreed to put
//!   it back, `ntp::status().pending_back`).
//! A clock that is behind the network time by a small amount is stepped by the sync itself and is not a skew.

/// The device-key sign-in accepts a time this many seconds away from the server's clock.
pub const KEY_WINDOW_SECS: u64 = 60;

/// The source of "now" (unix seconds). The product uses [`SystemClock`]; a test uses a manual clock.
pub trait WallClock: Send + Sync {
    fn now_unix(&self) -> i64;
}

/// The real clock of the machine.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl WallClock for SystemClock {
    fn now_unix(&self) -> i64 {
        iyakki_clock::now_unix()
    }
}

/// What the check found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skew {
    /// Nothing to warn about.
    Fine,
    /// The clock is before the build of this system (`now`, and the earliest time it can have, the build floor).
    BeforeBuild { now: i64, floor: i64 },
    /// The network time is this many seconds behind our clock and the clock was left alone.
    Ahead { secs: u64 },
}

/// The check, given the clock and the seconds the clock is ahead of the network time that was left alone (`None` when there is none).
pub fn judge(clock: &dyn WallClock, ahead_secs: Option<u64>) -> Skew {
    judge_at(clock.now_unix(), iyakki_clockfloor::COMPILED_FLOOR, ahead_secs)
}

/// The pure decision: `now` against `floor`, and the ahead value against the device-key window.
pub fn judge_at(now: i64, floor: i64, ahead_secs: Option<u64>) -> Skew {
    if now < floor {
        return Skew::BeforeBuild { now, floor };
    }
    match ahead_secs {
        Some(secs) if secs > KEY_WINDOW_SECS => Skew::Ahead { secs },
        _ => Skew::Fine,
    }
}

/// The sentence the person reads (the doctor shows it with the fix that opens Settings -> Time).
pub fn text(s: Skew) -> Option<String> {
    match s {
        Skew::Fine => None,
        Skew::BeforeBuild { .. } => Some("The clock is set before this system was built, so device-key sign-in and firewall expiry times are wrong until the time is set from the network.".into()),
        Skew::Ahead { secs } => Some(format!("The clock is {secs} seconds ahead of the network time: device-key sign-in (a {KEY_WINDOW_SECS} second window) and firewall expiry times are wrong until it is put back.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_testkit::clock::ManualClock;
    use std::time::Duration;

    impl WallClock for ManualClock {
        fn now_unix(&self) -> i64 {
            self.now_secs() as i64
        }
    }

    const FLOOR: i64 = 1_790_812_800;

    #[test]
    fn a_clock_after_the_build_and_not_ahead_is_fine() {
        let c = ManualClock::new(FLOOR as u64 * 1000);
        assert_eq!(judge(&c, None), Skew::Fine);
        assert_eq!(judge(&c, Some(KEY_WINDOW_SECS)), Skew::Fine, "exactly the window is still in it");
        assert!(text(Skew::Fine).is_none());
    }

    #[test]
    fn a_clock_before_the_build_is_warned_and_the_moment_it_is_not_clears_it() {
        let c = ManualClock::new((FLOOR as u64 - 1) * 1000);
        assert_eq!(judge(&c, None), Skew::BeforeBuild { now: FLOOR - 1, floor: FLOOR });
        assert!(text(judge(&c, None)).unwrap().contains("before this system was built"));
        c.advance(Duration::from_secs(1));
        assert_eq!(judge(&c, None), Skew::Fine, "at the floor it is no longer before the build");
    }

    #[test]
    fn an_ahead_clock_is_warned_only_past_the_device_key_window() {
        let c = ManualClock::new(FLOOR as u64 * 1000);
        assert_eq!(judge(&c, Some(KEY_WINDOW_SECS + 1)), Skew::Ahead { secs: KEY_WINDOW_SECS + 1 });
        assert!(text(Skew::Ahead { secs: 90 }).unwrap().contains("90 seconds ahead"));
        assert_eq!(judge(&c, Some(5)), Skew::Fine, "a small difference is stepped by the sync, not warned");
    }

    #[test]
    fn the_pure_decision_checks_the_floor_first() {
        assert_eq!(judge_at(FLOOR - 5, FLOOR, Some(500)), Skew::BeforeBuild { now: FLOOR - 5, floor: FLOOR });
        assert_eq!(judge_at(FLOOR + 10, FLOOR, Some(500)), Skew::Ahead { secs: 500 });
        assert_eq!(judge_at(FLOOR + 10, FLOOR, None), Skew::Fine);
    }
}
