//! Driver packs for bare-metal machines (Macs first): detection of what the machine has, the catalogue of known
//! drivers and what each one needs, placing firmware and options on the data volume, and checking prebuilt kernel
//! modules against the running kernel. Pure data handling: nothing here compiles, downloads or runs a build.
//!
//! Used by the boot code (`apply_boot`, before the network and sound drivers load) and by the store.

pub mod apply;
pub mod b64;
pub mod camquirk;
pub mod detect;
pub mod facetime;
pub mod known;
pub mod modinfo;
pub mod pack;
pub mod run;

pub use apply::{active_ids, apply_boot, firmware_present, install_pack, installed, remove_pack, Fs, Record, Report};
pub use detect::{Machine, Roots};
pub use known::{Availability, Delivery, Known, Row, Status, KNOWN};
pub use pack::Pack;
