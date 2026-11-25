//! The INBOX tile (alpha.9 P5): notifications, tasks and the approvals that wait for a person, as a crate that depends on the toolkit
//! (`iyakki-uikit`), the facade (`iyakki-tile-api`) and the pure model crate `iyakki-inbox-model` only. Everything it needs from the rest of the
//! system is [`port::InboxPort`]; the shell adapter (`crates/iyakki-apps/src/apps/inbox.rs`) and the real port (`inbox_port.rs`) are the only
//! code that names the services. The browser-consent dialog ([`consent::ConsentDialog`]) is shared with the open chat through it.

pub mod app;
pub mod consent;
pub mod port;

#[cfg(test)]
mod fake_tests;

pub use app::{Filter, Flow, InboxApp, Page};
pub use consent::ConsentDialog;
pub use port::InboxPort;
