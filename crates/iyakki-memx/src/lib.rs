//! The assistant's memory as a leaf crate (F-SPLIT-03): `kb` (namespaces of embedded chunks on disk), `embed` (the embedder trait and the
//! hashing default) and `history` (what the assistant did in each workspace, by person and chat). The agent re-exports them under the
//! old module names; the services supply a model-backed embedder through the `TextEmbedder` trait.

pub use iyakki_clock as clock;
pub use iyakki_shared::credguard;
pub mod embed;
pub mod history;
pub mod kb;
