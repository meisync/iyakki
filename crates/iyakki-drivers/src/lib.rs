//! Driver packs for bare-metal machines (Macs first): detection of what the machine has, the catalogue of known
//! drivers and what each one needs, placing firmware and options on the data volume, and checking prebuilt kernel
//! modules against the running kernel. Pure data handling: nothing here compiles, downloads or runs a build.
//!
//! Used by the boot code (`apply_boot`, before the network and sound drivers load) and by the store.

pub mod appfw;
pub mod applespi;
pub mod apply;
pub mod b64;
pub mod camquirk;
pub mod cs8409;
pub mod detect;
pub mod facetime;
pub mod ihex;
pub mod known;
pub mod macboard;
pub mod modinfo;
pub mod pack;
pub mod packfit;
pub mod run;
pub mod t2hold;
pub mod t2input;
pub mod touchbar;
#[cfg(target_os = "linux")]
pub mod usbfs;
pub mod wifirecover;

pub use apply::{active_ids, apply_boot, firmware_present, install_pack, installed, remove_pack, Fs, Record, Report, apply_boot_for};
pub use detect::{Machine, Roots};
pub use known::{Availability, Delivery, Known, Row, Status, KNOWN};
pub use pack::Pack;
