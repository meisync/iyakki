//! The firmware's startup entry of this computer (Settings -> This computer -> Startup disk, F-ARCH-06): what the firmware starts first, whether
//! IYAKKI has its own entry in the boot menu, and the four changes the person can make (make the entry, put it first, start it at the next restart
//! only, put the old order back). The read side is `system.read`; the plan and apply side needs `system.startup.set` and an administrator, the
//! card (the Settings confirmation or the chat card) comes first as it always did, and only IYAKKI's own entry, `BootOrder` and the one-time
//! `BootNext` are ever written, never another system's entry. The device has `iyakki_core_local::LocalStartupDisk`, a test has
//! `iyakki_core_fake::FakeStartupDisk`.

use serde::{Deserialize, Serialize};

/// What the person can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartupAction {
    /// Make IYAKKI's own entry for the volume the system started from (appended last, the order does not change).
    Register,
    /// Put IYAKKI's entry first (made when missing), the old order is kept for `Revert`.
    First,
    /// Start IYAKKI at the next restart only (the order does not change).
    Next,
    /// Put the saved order back and cancel a pending one-time start.
    Revert,
}

impl StartupAction {
    pub const ALL: [StartupAction; 4] = [StartupAction::Register, StartupAction::First, StartupAction::Next, StartupAction::Revert];

    /// The word of the chat tool and of `iyakki startup-disk`.
    pub fn name(self) -> &'static str {
        match self {
            StartupAction::Register => "register",
            StartupAction::First => "first",
            StartupAction::Next => "next",
            StartupAction::Revert => "revert",
        }
    }

    pub fn parse(word: &str) -> Option<StartupAction> {
        StartupAction::ALL.into_iter().find(|a| a.name() == word)
    }

    /// The sentence a card shows before anything is written (what will and will not change).
    pub fn preview(self) -> &'static str {
        match self {
            StartupAction::Register => "Writes one firmware boot entry described \"IYAKKI\" for the volume this system started from. Other systems' entries and the boot order are not changed (the entry is added last). `revert` undoes it.",
            StartupAction::First => "IYAKKI's entry goes first in the firmware's boot order (made first when missing); the other entries keep their order; the old order is saved for revert.",
            StartupAction::Next => "The firmware's one-time next boot is set to IYAKKI; the boot order is not changed.",
            StartupAction::Revert => "The saved boot order comes back and a pending one-time start is cancelled; IYAKKI's entry stays in the menu.",
        }
    }
}

/// What a change would do on this computer: the card text, or the plain reason it cannot be done here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupPlan {
    pub action: StartupAction,
    pub preview: String,
}

/// The refusal of a person who is not an administrator.
pub const ADMIN_ONLY: &str = "Only administrators change the startup disk.";

pub trait StartupDisk: Send + Sync {
    /// The lines `iyakki startup-disk status` prints: the firmware's order, IYAKKI's entry, the one-time start and the restart setting.
    fn status_lines(&self, _reboot_setting: bool) -> Vec<String> {
        vec![NOT_HERE.into()]
    }
    /// One line for Settings.
    fn status_line(&self, _reboot_setting: bool) -> String {
        "No firmware boot variables on this computer.".into()
    }
    /// What `action` would write, or why it cannot be done (not an administrator, no firmware variables, the variable folder is read-only):
    /// asked before the card, so a refusal never costs a tap. Writes nothing.
    fn plan(&self, _action: StartupAction, _administrator: bool) -> Result<StartupPlan, String> {
        Err(NOT_HERE.into())
    }
    /// Carry the change out (the caller has shown the card and the person said yes); the answer is the sentence to show.
    fn apply(&self, _action: StartupAction, _administrator: bool) -> Result<String, String> {
        Err(NOT_HERE.into())
    }
    /// Does this computer have firmware boot variables (started through UEFI)?
    fn firmware_present(&self) -> bool {
        false
    }
    /// `iyakki startup-disk own-first`, the old name of `first` for a computer that already has the entry: put IYAKKI's entry first and keep the old
    /// order for `revert`. `Ok(None)`: nothing to change (already first, or no entry). Needs an administrator, like every change.
    fn own_entry_first(&self, _administrator: bool) -> Result<Option<String>, String> {
        Err(NOT_HERE.into())
    }
    /// A restart asked for through `iyakki dev reboot`: with the setting on, the one-time start of IYAKKI is set first. Returns what was done for
    /// the audit line, `None` when nothing was.
    fn before_dev_reboot(&self, _setting_on: bool) -> Option<String> {
        None
    }
}

pub const NOT_HERE: &str = "This computer was not started through UEFI (no firmware variables), so there is no boot entry to set.";

/// The handle of a machine that does not say otherwise: no firmware variables.
pub struct NoStartupDisk;

impl StartupDisk for NoStartupDisk {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_round_trip_and_every_action_has_a_card_sentence() {
        for a in StartupAction::ALL {
            assert_eq!(StartupAction::parse(a.name()), Some(a));
            assert!(a.preview().len() > 40);
        }
        assert_eq!(StartupAction::parse("sideways"), None);
    }

    #[test]
    fn a_machine_without_the_handle_has_no_variables_and_refuses_every_change() {
        let n = NoStartupDisk;
        assert_eq!(n.status_lines(true), vec![NOT_HERE.to_string()]);
        assert!(n.status_line(false).contains("No firmware boot variables"));
        for a in StartupAction::ALL {
            assert_eq!(n.plan(a, true).unwrap_err(), NOT_HERE);
            assert_eq!(n.apply(a, true).unwrap_err(), NOT_HERE);
        }
        assert_eq!(n.before_dev_reboot(true), None);
        assert!(!n.firmware_present());
        assert_eq!(n.own_entry_first(true).unwrap_err(), NOT_HERE);
    }
}
