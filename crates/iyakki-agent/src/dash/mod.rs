//! DASH: dashboards with graphs and tables over pluggable data sources. See `docs/DASHBOARD.md`.
//! The engine lives in the leaf crate `iyakki-dash` and is re-exported here under the old paths; `tools` (the assistant's tools,
//! which need `Services`) stays in the agent.

pub use iyakki_dash::{
    clickhouse, cluster, connector, duckdb, engine, es, explain, expr, frame, kube, model, multi, neo4j, prom, readonly, registry, s3, seed, session, share,
    store, templates,
};
pub use iyakki_dash::{Cancel, DashError, DataSource, Deps, Frame, QueryRequest, SourceConfig, TimeRange};
pub mod tools;
#[cfg(test)]
mod live_tests;
