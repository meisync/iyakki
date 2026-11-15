//! The contract between IYAKKI apps (tiles, any UI) and IYAKKI core (docs/development/UI-OS-SPLIT-PLAN.md).
//!
//! Not to be confused with the crate `iyakki-core`, the PID-1 supervisor, which keeps its name and role: this crate is only the
//! API that apps call. It holds traits, serde types and errors and does no IO, so it builds fast and can be implemented by
//! `iyakki-core-local` (the only code that touches `/proc`, `/sys`, `/dev`), by `iyakki-core-fake` (tests) and, later, by a remote client.

pub mod appcaps;
pub mod broker;
pub mod bundle;
pub mod channel;
pub mod client;
pub mod compat;
#[cfg(feature = "native")]
pub mod childstate;
pub mod contract;
pub mod devices;
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
pub mod identity;
pub mod sdkbridge;
#[cfg(feature = "native")]
pub mod service;
pub mod audio;
pub mod sysinfo;
pub mod wizard;

pub use channel::{Cap, Channel, ChannelError, ChannelIdentity, ChannelInfo, Delivery, DeliveryState, Health, HealthState, InMessage, Limits, OutAttachment, OutMessage, RateInfo, Trust};
#[cfg(feature = "native")]
pub use childstate::{child_status, child_wait, steal_status, steal_status_of_marked, ChildState};
pub use devices::{parse_mounts, parse_net_dev, parse_partitions, BlockDev, Disks, Mount, Net, NetCounters};
pub use error::{ApiError, ApiResult};
#[cfg(feature = "native")]
pub use host::{HostApi, WasmTask};
#[cfg(feature = "native")]
pub use host::WasmIo;
pub use identity::{Capabilities, Identity};
#[cfg(feature = "native")]
pub use service::{AuditSink, Authenticator, Principal};
pub use audio::{Audio, PlaybackVolume, SoundCard};
pub use sysinfo::{parse_loadavg, parse_meminfo, parse_uptime, LoadAvg, MemInfo, Os, Sysinfo, Uptime};

#[cfg(test)]
mod fuzzkit;
