//! The contract between IYAKKI apps (tiles, any UI) and IYAKKI core (docs/development/UI-OS-SPLIT-PLAN.md).
//!
//! Not to be confused with the crate `iyakki-core`, the PID-1 supervisor, which keeps its name and role: this crate is only the
//! API that apps call. It holds traits, serde types and errors and does no IO, so it builds fast and can be implemented by
//! `iyakki-core-local` (the only code that touches `/proc`, `/sys`, `/dev`), by `iyakki-core-fake` (tests) and, later, by a remote client.

pub mod channel;
pub mod childstate;
pub mod contract;
pub mod error;
pub mod host;
pub mod identity;
pub mod service;
pub mod sysinfo;

pub use channel::{Cap, Channel, ChannelError, ChannelIdentity, ChannelInfo, Delivery, DeliveryState, Health, HealthState, InMessage, Limits, OutAttachment, OutMessage, RateInfo, Trust};
pub use childstate::{child_status, child_wait, steal_status, steal_status_of_marked, ChildState};
pub use error::{ApiError, ApiResult};
pub use host::HostApi;
pub use identity::{Capabilities, Identity};
pub use service::{AuditSink, Authenticator, Principal};
pub use sysinfo::{parse_loadavg, parse_meminfo, parse_uptime, LoadAvg, MemInfo, Sysinfo, Uptime};
