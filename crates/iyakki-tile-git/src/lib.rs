//! The GIT tile (alpha.8 N1): hosts, repositories and the clones on this computer (`docs/GIT-AND-SSH.md`) as a crate that depends on the toolkit
//! (`iyakki-uikit`) and the leaf crates `iyakki-git-model` and `iyakki-k8s-model` only. What it asks of the shell is [`port::GitPort`]; the shell
//! adapter (`crates/iyakki-apps/src/apps/git_tile.rs`) builds the real port (`git_port.rs`) and forwards the `App` methods. The AI IDE's Git tab
//! draws this same tile, so its screens and helpers are public here.

pub mod git_tile;
pub mod git_tile_repo;
pub mod git_tile_repo_ui;
pub mod git_tile_ui;
pub mod port;

pub use git_tile::*;
pub use port::{Flow, GitPort, JobSetup};

#[cfg(test)]
pub mod fake;
#[cfg(test)]
mod fake_tests;
