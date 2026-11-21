//! Types moved out of `iyakki_agent::disks_part` (F-SPLIT-03); `Confirm` now lives in `iyakki_core_api::diskmodel` (F-ARCH-06) and is re-exported here.

pub use iyakki_core_api::diskmodel::Confirm;

/// How many seconds the screen waits, with a visible countdown the person can cancel, after the last button and before the first write.
pub const CANCEL_WINDOW_SECS: u64 = 5;
