//! The contract between IYAKKI apps (tiles, any UI) and IYAKKI core (docs/development/UI-OS-SPLIT-PLAN.md).
//!
//! Not to be confused with the crate `iyakki-core`, the PID-1 supervisor, which keeps its name and role: this crate is only the
//! API that apps call. It holds traits, serde types and errors and does no IO, so it builds fast and can be implemented by
//! `iyakki-core-local` (the only code that touches `/proc`, `/sys`, `/dev`), by `iyakki-core-fake` (tests) and, later, by a remote client.

pub mod appcaps;
pub mod appactions;
pub mod appevents;
pub mod assistant;
pub mod broker;
pub mod cameras;
pub mod browsersession;
pub mod bundle;
pub mod channel;
pub mod client;
pub mod compat;
#[cfg(feature = "native")]
pub mod childstate;
pub mod contract;
pub mod uicore;
pub mod devices;
#[cfg(feature = "native")]
pub mod diskmodel;
pub mod files;
pub mod terminal;
/// The plain types of the device crates, named through the contract (F-ARCH-07, owner route "type-only re-export"): a tile or any other code
/// above the core writes `iyakki_core_api::device_types::devctl::Volume` and not `iyakki_devctl::Volume`, so its own manifest no longer names
/// the device crates. This is a naming path only: it adds no function of the core and reads nothing; the reading and the changing of the
/// machine stay behind the traits of this crate (`cx.os` and the other handles), where the permission and the access log are.
#[cfg(feature = "native")]
pub mod device_types {
    pub use iyakki_devctl as devctl;
    pub use iyakki_drivers as driver_packs;
    pub use iyakki_hwinfo as hwinfo;
    pub use iyakki_netscan as netscan;
    pub use iyakki_storage as storage;
    pub use iyakki_usb as usb;
    pub use iyakki_zram as zram;
}
/// The driver catalogue and its plain data (what a driver is, how it is delivered, what state it is in), published by the core so a tile names
/// these and not the crate that holds the catalogue. The reading of the machine (`Drivers::rows`) is the agent's, over the core's roots.
#[cfg(feature = "native")]
pub mod drivers {
    pub use iyakki_drivers::{appfw, apply, facetime, ihex, known, macboard, modpack, pack, run, t2hold, Availability, Delivery, Fs, Pack, Roots, Status};
    pub use iyakki_drivers::{Machine, Record, Row};

    /// The drivers of this machine as the core serves them (F-ARCH-05, next to [`crate::installer::Installer`]): what the machine is, the
    /// "Drivers for this machine" rows, the installed packs and whether one is turned off. All reads (`system.read`). Installing or removing
    /// a pack is not here: it goes through the store's review, never through this handle.
    pub trait Drivers: Send + Sync {
        /// What this machine is (vendor, product, PCI and USB devices, loaded modules, kernel release).
        fn machine(&self) -> Machine {
            Machine::default()
        }
        /// The rows of the Drivers page (empty for a machine none of the catalogue entries applies to).
        fn rows(&self) -> Vec<Row> {
            Vec::new()
        }
        /// The installed driver packs.
        fn installed(&self) -> Vec<Record> {
            Vec::new()
        }
        /// Is the installed pack `id` set aside (turned off)?
        fn is_disabled(&self, id: &str) -> bool {
            let _ = id;
            false
        }
        /// How many files the record of the installed pack `id` lists and how many of them are in the firmware folder now (`None`: no such
        /// pack is recorded). A read; the install check of the Mac firmware uses it.
        fn pack_files(&self, id: &str) -> Option<(usize, usize)> {
            let _ = id;
            None
        }
        /// Take the installed pack `id` away (its files, its record) and say what was done. Only the uninstall flow calls this, after its own
        /// card and approval; a machine without drivers refuses.
        fn remove_pack(&self, id: &str) -> Result<String, String> {
            let _ = id;
            Err("this machine has no driver packs to remove".into())
        }
    }

    /// The drivers handle of a machine that does not say otherwise: nothing known, nothing installed.
    pub struct NoDrivers;

    impl Drivers for NoDrivers {}
}
pub mod error;
#[cfg(feature = "native")]
pub mod crashlog;
pub mod startupdisk;
pub mod screenfacts;
#[cfg(feature = "native")]
pub mod btaudio;
#[cfg(feature = "native")]
pub mod guarded;
#[cfg(feature = "native")]
pub mod hardware;
#[cfg(feature = "native")]
pub mod host;
#[cfg(feature = "native")]
pub mod installer;
#[cfg(feature = "native")]
pub mod memory;
#[cfg(feature = "native")]
pub mod netscan;
#[cfg(feature = "native")]
pub mod router;
#[cfg(feature = "native")]
pub mod fleet;
pub mod identity;
pub mod media;
pub mod sdkbridge;
#[cfg(feature = "native")]
pub mod service;
pub mod audio;
pub mod audioroute;
pub mod soundctl;
pub mod soundtypes;
pub mod sysinfo;
pub mod wizard;

pub use channel::{Cap, Channel, ChannelError, ChannelIdentity, ChannelInfo, Delivery, DeliveryState, Health, HealthState, InMessage, Limits, OutAttachment, OutMessage, RateInfo, Trust};
#[cfg(feature = "native")]
pub use childstate::{child_status, child_wait, steal_status, steal_status_of_marked, ChildState};
pub use devices::{parse_mounts, parse_net_dev, parse_partitions, BlockDev, Disks, Mount, Net, NetCounters};
pub use uicore::{AppStatus, AppStatuses, Core, Mixer, MixerStats, SoundKind, StatusKind, WaitingCall};
pub use uicore::{MonitorInfo, Monitors, NoMonitors};
pub use browsersession::{BrowserError, BrowserSessions, NoBrowser};
pub use uicore::{AppData, Desktop, NoUiState};
pub use files::{ChangeAsk, ChangeBook, FileChange, FileEntry, Files, Listing, NoFiles};
pub use terminal::{NoTerminal, PtyCell, PtyColor, PtyFrame, PtyKey, PtyMods, PtyOpen, TermApproval, TermOpen, TermReply, Terminal};
#[cfg(feature = "native")]
pub use terminal::PolicySource;
pub use assistant::{Assistant, NoAssistant};
pub use cameras::{Cameras, NoCameras};
pub use appevents::{AppEvents, NoAppEvents};
pub use appactions::{AppActions, NoAppActions};
pub use media::{Media, NoMedia};
pub use uicore::UiSocket;
pub use uicore::{BrailleCells, BrailleKey, BrailleLink};
pub use uicore::{HostFacts, NoHostFacts};
#[cfg(feature = "native")]
#[cfg(feature = "native")]
pub use iyakki_filescope::{FileRoots, HostMount};
#[cfg(feature = "native")]
pub use uicore::{CloudModes, DesktopOnly, FileScope, NoFileScope};
pub use uicore::SessionResume;
pub use uicore::SettingsView;
pub use uicore::VaultView;
pub use uicore::HomeFiles;
pub use uicore::FlowsView;
pub use error::{ApiError, ApiResult};
#[cfg(feature = "native")]
pub use host::{HostApi, WasmTask};
#[cfg(feature = "native")]
pub use host::WasmIo;
pub use identity::{Capabilities, Identity};
#[cfg(feature = "native")]
pub use service::{AuditSink, Authenticator, Principal};
pub use soundctl::{CuePlayer, NoCues, NoOutputState, NoSound, OutputState, SoundControl, SoundState};
pub use audio::{Audio, PlaybackVolume, SoundCard};
pub use audioroute::{AudioRoute, NoAudioRoute, PcmHolder};
pub use sysinfo::{parse_loadavg, parse_meminfo, parse_uptime, LoadAvg, MemInfo, Os, Sysinfo, Uptime};

#[cfg(test)]
mod fuzzkit;
