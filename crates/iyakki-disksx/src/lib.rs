//! The disk engine as a leaf crate (F-SPLIT-03): `disks` (the model, the plans, the engine), `disks_part` (partition plans), `disks_fmt` (format
//! and resize tools), `disks_helper` (the crypt and program runners), `disks_sys` (the real machine), `disks_fake` (the made-up one) and the
//! `oscompat` names of the Linux calls they use. The agent re-exports these under their old module names.

pub use iyakki_diskcrypt as disks_crypt;
pub mod disks;
pub mod disks_fake;
pub mod disks_fmt;
pub mod disks_dm;
pub mod disks_helper;
pub mod disks_part;
pub mod disks_sys;
pub mod disks_unlock;
pub mod disksformattools;
pub mod oscompat;

#[cfg(test)]
mod disks_fmt_tests;
#[cfg(test)]
mod disks_tests;
#[cfg(test)]
mod disks_unlock_tests;
