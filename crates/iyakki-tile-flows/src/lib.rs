//! The FLOWS tile (alpha.8 N1): the workflow IDE (`docs/FLOWS.md`) as a crate that depends on the toolkit (`iyakki-uikit`) and the leaf crates
//! `iyakki-flow` (the engine's model), `iyakki-flows-model` (the tile's data and the jobs it asks of its environment, `FlowsPort`),
//! `iyakki-querybox` and `iyakki-clock` only. What it asks of the shell is [`port::FlowsShell`] and [`port::ChatPane`]; the shell adapter
//! (`crates/iyakki-apps/src/apps/flows.rs`) builds the environment and the chat pane and forwards the `App` methods.

pub mod flows;
pub mod flows_assist;
pub mod flows_board;
pub mod flows_canvas;
pub mod flows_defaults;
pub mod flows_fail;
pub mod flows_hook;
pub mod flows_icons;
pub mod flows_keys;
pub mod flows_paint;
pub mod flows_props;
pub mod flows_remote;
pub mod flows_run;
pub mod flows_tip;
pub mod flows_view;
pub mod port;

pub use flows::{FlowsApp, ACCENT, CHAT_SLOT};
pub use port::{AppFlow, ChatPane, DropAction, FlowsShell, PictureFiles};

#[cfg(test)]
pub mod fake;
#[cfg(test)]
mod fake_tests;
