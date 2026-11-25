//! The SYSTEM tile (alpha.9 P6): what the machine is doing, as a crate that depends on the toolkit (`iyakki-uikit`), the facade
//! (`iyakki-tile-api`) and the pure leaf crate `iyakki-sysinfo-model` only. Everything it needs from the rest of the system is
//! [`port::SystemPort`] (with [`port::SysRead`] for the kernel's figures); the shell adapter (`crates/iyakki-apps/src/apps/system.rs`) and the
//! real port (`system_port.rs`) are the only code that names the core, the session and the services.

pub mod app;
pub mod port;

#[cfg(test)]
mod fake_tests;

pub use app::{memory_extra_line, Confirm, Flow, Page, Sample, Sort, SystemApp};
pub use port::{OwnedProc, SysRead, SystemPort};
