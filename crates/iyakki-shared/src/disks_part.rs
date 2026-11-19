//! Types moved out of `iyakki_agent::disks_part` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// What the person did to confirm: typed the drive's name or the last four characters of its serial, and pressed the second button.
#[derive(Clone, Debug, Default)]
pub struct Confirm {
    pub typed: String,
    pub second: bool,
}

/// How many seconds the screen waits, with a visible countdown the person can cancel, after the last button and before the first write.
pub const CANCEL_WINDOW_SECS: u64 = 5;
