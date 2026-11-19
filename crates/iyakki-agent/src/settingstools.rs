//! One small frame for the assistant's Settings tools (docs/development/CHAT-REACHABILITY.md). A page of Settings gets its chat tools as a
//! list of [`Entry`] values; this wrapper puts the list in front of the other tools. Reading is free. Every change asks the person each time
//! and never rides on a session grant. Every entry calls the same service code the Settings page calls (a `views::Views`, a
//! `screensaver::Config`, ...), over the person's data folder, and writes one audit line.
//!
//! The frame knows nothing about the pages; each page's entries live in `settingstools_<page>.rs`.



pub use iyakki_shared::settingstools::{SettingsTools};
pub use iyakki_shared::settingstools::{CallFn, Entry, TextFn};

pub use iyakki_shared::settingstools::TypedEnv as Env;






pub use iyakki_shared::settingstools::{arg_bool, arg_str, no_args};

