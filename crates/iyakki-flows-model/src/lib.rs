//! The data of FLOWS the tile needs, below the agent and the shared crate (alpha.8 N1, D-S2-01 option (a)): the failure book and fix prompts
//! (`failfix`), the webhook store and its view (`hookstore`, `hookview`), the flow spec builder (`spec`), the allow list of programs
//! (`programs`), the outside-message policy types (`channel`) and the jobs the tile asks of its environment (`port`). `iyakki-flowsx` and
//! `iyakki-shared` re-export the old paths, so nothing that named them before changes.

pub mod channel;
pub mod failfix;
pub mod hookstore;
pub mod hookview;
pub mod programs;
pub mod spec;
pub mod port;
