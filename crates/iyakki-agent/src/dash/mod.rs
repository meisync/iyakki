//! DASH: dashboards with graphs and tables over pluggable data sources. See `docs/DASHBOARD.md`.
//!
//! * [`connector`]: the seam (`DataSource`, `QueryRequest`, `Frame`), the HTTP client with limits and good errors.
//! * [`prom`], [`es`], [`clickhouse`], [`duckdb`]: the connectors; [`registry`] lists and builds them.
//! * [`readonly`]: the rule that nothing here ever writes.
//! * [`model`], [`engine`], [`templates`], [`store`]: the dashboard, running a panel, ready-made dashboards, saving.
//! * [`session`], [`tools`]: the open dashboard shared with the assistant, and the assistant's tools.

pub mod clickhouse;
pub mod connector;
pub mod duckdb;
pub mod engine;
pub mod es;
pub mod expr;
pub mod multi;
pub mod explain;
pub mod frame;
#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod security_tests;
pub mod model;
pub mod prom;
pub mod readonly;
pub mod registry;
pub mod session;
pub mod store;
pub mod templates;
pub mod tools;

pub use connector::{Cancel, DashError, DataSource, Deps, QueryRequest, SourceConfig, TimeRange};
pub use frame::Frame;
