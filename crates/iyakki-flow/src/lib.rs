//! The workflow engine of FLOWS. No UI, no network, no process, no agent code: the world is the [`host::Host`] trait.
//! See `docs/FLOWS.md`.
//!
//! * [`model`]: flows, nodes, edges, runs (all `serde`). [`retry`]: attempts, backoff, error classes.
//! * [`expr`]: the small safe expression and template language. [`graph`]: which node may run, what flows along an edge.
//! * [`kinds`]: the registry of node kinds. [`validate`]: checks a flow.
//! * [`engine`], [`driver`], [`runstore`]: runs, threads, persistence, recovery.
//! * [`sched`]: cron, intervals, missed and overlap policies. [`store`]: files, versions, publishing, the library.
//! * [`heal`]: patches the agent may propose and the harness's classification. [`templates`]: ready-made flows.

pub mod driver;
pub mod engine;
pub mod expr;
pub mod graph;
pub mod heal;
pub mod host;
pub mod kinds;
pub mod layout;
pub mod minire;
pub mod report;
pub mod model;
pub mod retry;
pub mod runstore;
pub mod sched;
pub mod store;
pub mod templates;
pub mod validate;

pub use engine::{ApplyScope, CardDecision, Counts, Engine, EngineCfg, StartOutcome, StartRequest};
pub use host::{Cancel, Clock, FakeClock, FakeHost, Host, SystemClock};
pub use kinds::Registry;
pub use model::*;
pub use retry::{Backoff, ErrClass, NodeError, RetryPolicy};
pub use store::{FlowSource, FlowStore};
