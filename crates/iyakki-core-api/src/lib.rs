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
pub mod files;
pub mod terminal;
/// The driver catalogue and its plain data (what a driver is, how it is delivered, what state it is in), published by the core so a tile names
/// these and not the crate that holds the catalogue. The reading of the machine (`Drivers::rows`) is the agent's, over the core's roots.
#[cfg(feature = "native")]
pub mod drivers {
    pub use iyakki_drivers::{apply, facetime, known, Availability, Delivery, Roots, Status};
}
pub mod error;
#[cfg(feature = "native")]
pub mod crashlog;
#[cfg(feature = "native")]
pub mod guarded;
#[cfg(feature = "native")]
pub mod hardware;
#[cfg(feature = "native")]
pub mod host;
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
pub use soundctl::{CuePlayer, NoCues, NoSound, SoundControl, SoundState};
pub use audio::{Audio, PlaybackVolume, SoundCard};
pub use sysinfo::{parse_loadavg, parse_meminfo, parse_uptime, LoadAvg, MemInfo, Os, Sysinfo, Uptime};

#[cfg(test)]
mod fuzzkit;
