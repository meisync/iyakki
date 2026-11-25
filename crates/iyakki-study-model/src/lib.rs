//! The LEARN model (alpha.8 E3: the LEARN tile becomes the crate `iyakki-tile-learn`). `iyakki-shared` re-exports every module under its
//! old path (`iyakki_shared::study`, `::studylesson`, `::studypdf`), so no other crate changes. The store (the person's file, the calendar
//! link, saving a lesson) stays in `iyakki-shared`.
//! Rule (crates/iyakki-core/tests/dependency_ratchet.rs): depends on `iyakki-clock` and `iyakki-sec` only, never on the agent, the shell or a tile.

pub mod study;
pub mod studylesson;
pub mod studypdf;

#[cfg(test)]
mod tests;
