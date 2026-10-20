//! The approval mode of a chat, live: one cell the chat (or a task) owns and every run it starts reads at each decision.
//!
//! The person can change the mode while a run is going (the chip and Ctrl+Q / Ctrl+Shift+A stay enabled). The cell is read:
//! * by the built-in loop before it asks for a tool approval, and again every tenth of a second while a card waits
//!   (a switch to Auto-approve safe approves a waiting card that Auto would have run; always-ask calls stay waiting);
//! * by the external-agent bridge for every permission prompt and every IYAKKI tool call, and while its card waits;
//! * by Autopilot at every step boundary (switched off: the current step finishes, then the loop stops with a summary).
//!
//! Session grants ([`crate::grants`]) are separate and unaffected.

use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;

/// The three modes the person sees on the chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Ask first: every call that is not safe asks.
    Ask,
    /// Auto-approve safe: only the always-ask list asks.
    Auto,
    /// Autopilot: Auto approvals, and the plan loop runs to the end.
    Autopilot,
}

impl Mode {
    fn from_u8(v: u8) -> Mode {
        match v {
            1 => Mode::Auto,
            2 => Mode::Autopilot,
            _ => Mode::Ask,
        }
    }
    fn as_u8(self) -> u8 {
        match self {
            Mode::Ask => 0,
            Mode::Auto => 1,
            Mode::Autopilot => 2,
        }
    }
    /// What the person reads.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Ask => "Ask first",
            Mode::Auto => "Auto-approve safe",
            Mode::Autopilot => "Autopilot",
        }
    }
    /// The stable id kept in the audit log.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::Auto => "auto",
            Mode::Autopilot => "autopilot",
        }
    }
    /// Calls that are not on the always-ask list run without a card.
    pub fn auto(self) -> bool {
        self != Mode::Ask
    }
}

/// The live cell. Cheap to read (one atomic load); shared as `Arc<LiveMode>` through [`crate::agent::RunLimits::live`].
#[derive(Debug)]
pub struct LiveMode {
    mode: AtomicU8,
    changes: AtomicU32,
}

/// Cells compare by identity: two run limits are equal when they share one cell.
impl PartialEq for LiveMode {
    fn eq(&self, o: &LiveMode) -> bool {
        std::ptr::eq(self, o)
    }
}
impl Eq for LiveMode {}

impl LiveMode {
    pub fn new(m: Mode) -> Arc<LiveMode> {
        Arc::new(LiveMode { mode: AtomicU8::new(m.as_u8()), changes: AtomicU32::new(0) })
    }
    pub fn get(&self) -> Mode {
        Mode::from_u8(self.mode.load(Ordering::SeqCst))
    }
    /// Change the mode; true when it really changed (then [`LiveMode::changes`] counts it).
    pub fn set(&self, m: Mode) -> bool {
        let old = self.mode.swap(m.as_u8(), Ordering::SeqCst);
        if old != m.as_u8() {
            self.changes.fetch_add(1, Ordering::SeqCst);
            return true;
        }
        false
    }
    pub fn auto(&self) -> bool {
        self.get().auto()
    }
    pub fn autopilot(&self) -> bool {
        self.get() == Mode::Autopilot
    }
    /// How many times the mode changed since the cell was made.
    pub fn changes(&self) -> u32 {
        self.changes.load(Ordering::SeqCst)
    }
}

/// The timeline line of a waiting card that a switch to Auto-approve safe (or Autopilot) approved.
pub fn switched_note(tool: &str, m: Mode) -> String {
    format!("{tool}: approved by switching to {}", if m == Mode::Autopilot { "Autopilot" } else { "Auto-approve safe" })
}

/// The `by` of a decision made by a mode switch (audited as `agent.tool <name>:approved:mode-switch`).
pub const BY_SWITCH: &str = "mode-switch";

/// What changes now and what changes next when the mode is switched during a run (the chip's hover text while a run is going;
/// docs/USER-GUIDE.md repeats it).
pub const DURING_RUN: &str = "Changes apply to this run now: Auto-approve safe approves a waiting card it would have run (always-ask items keep waiting); Ask first makes the next risky call ask (a running tool keeps running); Autopilot on lets this turn finish, then works through the plan; Autopilot off finishes the current step, then stops with a summary (jobs keep running).";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cell_reads_what_was_last_set_and_counts_real_changes() {
        let m = LiveMode::new(Mode::Ask);
        assert!(!m.auto() && !m.autopilot());
        assert!(m.set(Mode::Auto) && m.auto() && !m.autopilot());
        assert!(!m.set(Mode::Auto), "the same mode is not a change");
        assert!(m.set(Mode::Autopilot) && m.auto() && m.autopilot());
        assert_eq!(m.changes(), 2);
        assert_eq!([Mode::Ask.name(), Mode::Auto.name(), Mode::Autopilot.name()], ["ask", "auto", "autopilot"]);
        assert_eq!(Mode::Auto.label(), "Auto-approve safe");
        assert!(switched_note("write_file", Mode::Auto).contains("approved by switching to Auto-approve safe"));
    }
}
