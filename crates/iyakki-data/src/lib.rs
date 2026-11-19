//! Tables for IYAKKI: open a CSV/TSV/JSON-lines file, look at it a page at a time, sort, filter, group, join, describe
//! it and export the result, behind one service API ([`DataService`]).
//!
//! * Requests and results are plain structs that serialize with serde ([`model`]), addressed by a dataset handle, so the
//!   service can later sit behind a remote API.
//! * Two engines answer the same API. With the `polars` feature, [Polars](https://pola.rs) (MIT) runs the queries (lazy
//!   API, a minimal feature set). Without it, a small built-in engine ([`builtin`]) pages, sorts, filters, groups and
//!   summarises CSV/TSV files without holding them in memory, which keeps the OS image small.
//! * Filters and derived columns use one small expression language ([`expr`]), parsed and type-checked here, so a
//!   mistake comes back as a plain sentence whichever engine runs.
//!
//! [`service`] gives the best engine this build has.

pub mod builtin;
pub mod csv;
pub mod expr;
pub mod model;
#[cfg(feature = "polars")]
pub mod polars_engine;
pub mod table;

pub use model::*;

use std::sync::Arc;

/// The best engine this build has: Polars with the `polars` feature, the built-in one without.
pub fn service() -> Arc<dyn DataService> {
    #[cfg(feature = "polars")]
    {
        Arc::new(polars_engine::PolarsEngine::new())
    }
    #[cfg(not(feature = "polars"))]
    {
        Arc::new(builtin::Builtin::new())
    }
}

/// The built-in engine, whatever features are on (the reference the Polars engine is tested against).
pub fn builtin_service() -> Arc<dyn DataService> {
    Arc::new(builtin::Builtin::new())
}

#[cfg(test)]
mod tests;
