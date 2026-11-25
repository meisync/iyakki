//! Types moved out of `iyakki_agent::disks_part` (F-SPLIT-03); `Confirm` now lives in `iyakki_core_api::diskmodel` (F-ARCH-06) and is re-exported here.

pub use iyakki_core_api::diskmodel::Confirm;

pub use iyakki_core_api::diskmodel::CANCEL_WINDOW_SECS;
