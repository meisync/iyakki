//! Types moved out of `iyakki_agent::disks` (F-SPLIT-03); they now live in `iyakki-core-api::diskmodel` (F-ARCH-05) and are re-exported here.

pub use iyakki_core_api::diskmodel::{Actor, Fs, MountFlags, MountOpts, MountPlan, Table, Transport};
