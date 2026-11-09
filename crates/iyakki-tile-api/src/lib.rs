//! The facade between the shell and a tile crate (docs/development/COMPILE-SPLIT-PILOT.md, "Second step").
//!
//! A tile crate implements [`App`] and gets a [`TileCx`] on every call. `TileCx` holds a few plain fields and trait objects, only for what
//! the extracted tiles use; the shell fills it from its own `AppCx` and wraps each tile crate's `App` in an adapter for the shell's own
//! `App` trait. Tests of a tile crate fill it with fakes. Add a field or a trait here when the next tile needs it, not before.

use iyakki_uikit::gfx::{Gfx, Icon};
use iyakki_uikit::ui::{Event, R};

pub mod calendar;
pub mod cameras;
pub mod remote;
pub mod usage;

/// What a tile answers to an event: stay on the screen, or go back to the launcher.
pub enum Flow {
    Stay,
    Back,
}

/// What a tile may use. Built by the shell for every call from the signed-in person's `AppCx`.
pub struct TileCx<'a> {
    /// The signed-in person's name.
    pub user: &'a str,
    /// The person's policy allows managing settings (administrators).
    pub manage_settings: bool,
    /// Token usage, as the agent keeps it.
    pub usage: &'a dyn usage::UsageSource,
    /// Events and alarms, and the machine's time zone.
    pub calendar: &'a dyn calendar::CalendarSource,
    /// The audit log.
    pub audit: &'a dyn AuditSink,
    /// Saved connections, sessions and transfers of the remote files tile.
    pub remote: &'a dyn remote::RemoteSource,
    /// Saved and USB cameras, their streams and pictures (the Cameras tile).
    pub cameras: &'a dyn cameras::CameraSource,
}

/// Where a tile records what the person did (the shell's audit log; a fake in tests).
pub trait AuditSink {
    fn record(&self, user: &str, action: &str, subject: &str);
}

/// The tile contract. The same names and meaning as the shell's `App` trait, for the methods the extracted tiles implement.
pub trait App {
    fn title(&self) -> &'static str;
    fn icon(&self) -> Icon;
    fn accent(&self) -> [u8; 4];
    /// One-line key help shown in the header.
    fn hint(&self) -> String;
    /// Every key and click this screen understands, one per line, for the F1 help popup. By default the header hint split into its parts.
    fn help(&self) -> Vec<String> {
        self.hint().split(" · ").map(str::to_string).collect()
    }
    fn handle(&mut self, ev: &Event, cx: &TileCx) -> Flow;
    /// Draw into `area` (below the header). `tick` advances ~30 times a second.
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &TileCx, tick: u64);
}

/// An audit sink that records nothing.
pub struct NoAudit;

impl AuditSink for NoAudit {
    fn record(&self, _: &str, _: &str, _: &str) {}
}
