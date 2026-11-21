//! The disk engine's names for the UI crates (F-SPLIT-03, F-ARCH-07). The engine itself (`disks`, `disks_part`, `disks_fmt`, `disks_helper`,
//! `disks_sys`, `disks_fake`, `disks_dm`, `disks_unlock`, `disksformattools`, `oscompat`) lives in `iyakki-core-local`, the one owner of
//! kernel access; this crate re-exports it under the old module names so the UI code keeps naming `iyakki_disksx::disks::...`.

pub use iyakki_core_local::disks_crypt;
pub use iyakki_core_local::{disks, disks_dm, disks_fake, disks_fmt, disks_helper, disks_part, disks_sys, disks_unlock, disksformattools, oscompat};
