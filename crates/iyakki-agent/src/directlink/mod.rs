//! Direct links: the pure engine (types, beacon, checks, Thunderbolt view, link-local, the fake machine) lives in `iyakki_shared::directlink`;
//! this module keeps the parts that touch the machine: [`manager`], [`real`], [`gadget`] and [`cli`]. Read `docs/DEVELOPER-MODE.md`,
//! section "Connect two computers with one cable".

pub mod cli;
pub use iyakki_core_local::gadget;
pub mod manager;
pub mod real;
#[cfg(test)]
mod tests;

pub use iyakki_shared::directlink::*;
pub use iyakki_shared::directlink::{beacon, checks, fake, linklocal, tb};
