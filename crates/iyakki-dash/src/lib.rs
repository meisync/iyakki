//! DASH: dashboards with graphs and tables over pluggable data sources. See `docs/DASHBOARD.md`.
//! The engine half, split out of `iyakki-agent` (F-SPLIT-03); the agent re-exports every module under `iyakki_agent::dash`
//! and keeps `tools` (the assistant's tools, which need `Services`).
//!
//! * [`connector`]: the seam (`DataSource`, `QueryRequest`, `Frame`), the HTTP client with limits and good errors.
//! * [`prom`], [`es`], [`clickhouse`], [`duckdb`], [`neo4j`]: the connectors; [`registry`] lists and builds them.
//! * [`readonly`]: the rule that nothing here ever writes.
//! * [`model`], [`engine`], [`templates`], [`store`]: the dashboard, running a panel, ready-made dashboards, saving.
//! * [`session`]: the open dashboard shared with the assistant.

pub mod clickhouse;
pub mod cluster;
pub mod connector;
pub mod duckdb;
pub mod engine;
pub mod es;
pub mod expr;
pub mod explain;
pub mod frame;
pub mod kube;
pub mod model;
pub mod multi;
pub mod neo4j;
pub mod prom;
pub mod readonly;
pub mod registry;
pub mod s3;
pub mod seed;
pub mod session;
pub mod share;
pub mod store;
pub mod templates;
pub mod tools;
#[cfg(test)]
mod kube_real_tests;
#[cfg(test)]
mod kube_tests;
#[cfg(test)]
#[cfg(test)]
mod loopback_tests;
#[cfg(test)]
mod security_tests;
#[cfg(test)]
mod share_tests;

pub use connector::{Cancel, DashError, DataSource, Deps, QueryRequest, SourceConfig, TimeRange};
pub use frame::Frame;
