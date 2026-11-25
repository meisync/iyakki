//! The DASH tile (alpha.8 E3): dashboards with graphs and tables over pluggable data sources, with the assistant's chat beside them
//! (`docs/DASHBOARD.md`). Depends on the toolkit (`iyakki-uikit`) and the leaf crates `iyakki-dash` (the engine) and `iyakki-edit` (the text
//! engine of the query box) only. What it asks of the shell is [`port::DashPort`] (the person's files) and [`port::ChatPane`] (the assistant's
//! pane); the shell adapter (`crates/iyakki-apps/src/apps/dash.rs`) builds the engine's environment and the chat pane and forwards the `App`
//! methods. The chart drawing is `charts`, the query box `query`, the banner of an unreachable source `repair`.

pub mod app;
pub mod charts;
pub mod port;
/// The query box: the crate `iyakki-querybox`, re-exported under the old path.
pub mod query {
    pub use iyakki_querybox::*;
}
pub mod repair;

pub use app::{DashApp, Flow};
pub use port::{ChatPane, DashPort, NoDashPort};

#[cfg(test)]
mod fake_tests;
