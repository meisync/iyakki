//! The pure parts of FLOWS, below the agent: no `Services`, no engine host, no network. The agent re-exports each module under its old
//! path (`iyakki_agent::flows::askform` and so on), so nothing that named them before has to change; the UI names them here.
//! See `docs/development/UI-BOUNDARY.md`, section 11, and F-SPLIT-03.

pub mod askform;
pub use iyakki_flows_model::failfix;
pub use iyakki_flows_model::hookstore;
pub mod machines;
pub use iyakki_flows_model::programs;
pub mod remote;
pub use iyakki_flows_model::spec;

pub use iyakki_flows_model::hookview;

/// What the shell and the flows host agree on for permission requests.
pub mod permission {
    /// The popup is shown for this long after the request was made; the INBOX entry stays until it is answered.
    pub const POPUP_SECS: i64 = 600;
}
