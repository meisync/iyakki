//! The startup disk on the fake machine: a firmware with a boot order of other systems, IYAKKI's own entry (or none), a one-time start, the saved
//! order for `revert`, and the same approval rules as the real one (administrators only, no variables or a read-only folder refuse before anything
//! changes). Every change is recorded.

use std::sync::Mutex;

use iyakki_core_api::startupdisk::{StartupAction, StartupDisk, StartupPlan, ADMIN_ONLY, NOT_HERE};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    /// The computer was started through UEFI.
    pub uefi: bool,
    /// The variable folder is mounted read-only.
    pub read_only: bool,
    /// The boot volume can be told (when not, a change that must make the entry is refused).
    pub boot_volume_known: bool,
    /// The entries other than IYAKKI's, in order.
    pub others: Vec<String>,
    /// IYAKKI's own entry exists.
    pub own: bool,
    /// IYAKKI's entry is first in the order.
    pub own_first: bool,
    /// The one-time start is set to IYAKKI.
    pub next_boot: bool,
    /// An order was saved for `revert`.
    pub saved: bool,
    pub calls: Vec<String>,
}

impl Default for State {
    fn default() -> State {
        State { uefi: true, read_only: false, boot_volume_known: true, others: vec!["Windows Boot Manager".into(), "Ubuntu".into()], own: false, own_first: false, next_boot: false, saved: false, calls: vec![] }
    }
}

#[derive(Default)]
pub struct FakeStartupDisk {
    state: Mutex<State>,
}

impl FakeStartupDisk {
    pub fn set(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.state.lock().unwrap());
    }
    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }
    /// Every change so far, as `apply(word)`.
    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
}

fn check(s: &State, action: StartupAction, administrator: bool) -> Result<(), String> {
    if !administrator {
        return Err(ADMIN_ONLY.into());
    }
    if !s.uefi {
        return Err(NOT_HERE.into());
    }
    if s.read_only {
        return Err("The firmware variables are mounted read-only (efivarfs is read-only), so nothing was changed. Remount it read-write as an administrator and try again.".into());
    }
    let needs_entry = match action {
        StartupAction::Register => true,
        StartupAction::First | StartupAction::Next => !s.own,
        StartupAction::Revert => false,
    };
    if needs_entry && !s.boot_volume_known {
        return Err("Cannot tell which volume IYAKKI was started from (no boot volume is mounted and the data volume's disk was not found).".into());
    }
    Ok(())
}

impl StartupDisk for FakeStartupDisk {
    fn status_lines(&self, reboot_setting: bool) -> Vec<String> {
        let s = self.state.lock().unwrap();
        if !s.uefi {
            return vec![NOT_HERE.into()];
        }
        let mut v = vec![format!("BootOrder: {}", s.others.join(", "))];
        v.push(if s.own { format!("startup disk: IYAKKI's own entry: Boot0002{}", if s.next_boot { " (set to start at the next restart)" } else { "" }) } else { "startup disk: IYAKKI has no entry of its own in this computer's boot menu".into() });
        v.push(format!("startup disk: restart through `iyakki dev reboot` goes to IYAKKI: {}", if reboot_setting { "on" } else { "off" }));
        v
    }
    fn status_line(&self, reboot_setting: bool) -> String {
        let s = self.state.lock().unwrap();
        if !s.uefi {
            return "No firmware boot variables on this computer.".into();
        }
        let state = if !s.own {
            "no IYAKKI entry in the boot menu"
        } else if s.next_boot {
            "starts at the next restart"
        } else if s.own_first {
            "IYAKKI starts first"
        } else {
            "IYAKKI has an entry, another system starts first"
        };
        format!("{state}; restart from `iyakki dev reboot` into IYAKKI: {}", if reboot_setting { "on" } else { "off" })
    }
    fn firmware_present(&self) -> bool {
        self.state.lock().unwrap().uefi
    }
    fn own_entry_first(&self, administrator: bool) -> Result<Option<String>, String> {
        let mut s = self.state.lock().unwrap();
        if !administrator {
            return Err(ADMIN_ONLY.into());
        }
        if !s.uefi {
            return Err(NOT_HERE.into());
        }
        if !s.own || s.own_first {
            return Ok(None);
        }
        s.own_first = true;
        s.saved = true;
        s.calls.push("own_entry_first".into());
        Ok(Some("Boot0002 is first in the boot order now".into()))
    }
    fn plan(&self, action: StartupAction, administrator: bool) -> Result<StartupPlan, String> {
        check(&self.state.lock().unwrap(), action, administrator)?;
        Ok(StartupPlan { action, preview: action.preview().into() })
    }
    fn apply(&self, action: StartupAction, administrator: bool) -> Result<String, String> {
        let mut s = self.state.lock().unwrap();
        check(&s, action, administrator)?;
        s.calls.push(format!("apply({})", action.name()));
        Ok(match action {
            StartupAction::Register => {
                let had = std::mem::replace(&mut s.own, true);
                s.saved = true;
                format!("{} the boot entry Boot0002 \"IYAKKI\" for the volume this system started from.", if had { "Replaced" } else { "Made" })
            }
            StartupAction::First => {
                s.own = true;
                s.saved = true;
                if std::mem::replace(&mut s.own_first, true) { "IYAKKI's entry is already first.".into() } else { "Boot0002 is first in the boot order now.".into() }
            }
            StartupAction::Next => {
                s.own = true;
                s.next_boot = true;
                "Boot0002 is set to start at the next restart only; the boot order is unchanged.".into()
            }
            StartupAction::Revert => {
                let cleared = std::mem::replace(&mut s.next_boot, false);
                let order = std::mem::replace(&mut s.saved, false);
                if order {
                    s.own_first = false;
                }
                match (order, cleared) {
                    (true, true) => "The earlier boot order is back and the one-time start is cancelled.".into(),
                    (true, false) => "The earlier boot order is back.".into(),
                    (false, true) => "The one-time start is cancelled. There was no saved order to put back.".into(),
                    (false, false) => "There is no saved order to put back.".into(),
                }
            }
        })
    }
    fn before_dev_reboot(&self, setting_on: bool) -> Option<String> {
        if !setting_on {
            return None;
        }
        Some(match self.apply(StartupAction::Next, true) {
            Ok(m) => m,
            Err(e) => format!("not set: {e}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_keeps_the_rules_of_the_real_one_and_records_only_what_changed() {
        let f = FakeStartupDisk::default();
        assert!(f.status_line(false).starts_with("no IYAKKI entry"));
        assert_eq!(f.plan(StartupAction::First, false).unwrap_err(), ADMIN_ONLY);
        assert!(f.plan(StartupAction::First, true).is_ok());
        assert!(f.calls().is_empty(), "a plan changes nothing");
        assert!(f.apply(StartupAction::First, true).unwrap().contains("first"));
        assert!(f.status_line(true).starts_with("IYAKKI starts first"));
        assert!(f.apply(StartupAction::Next, true).unwrap().contains("next restart only"));
        assert!(f.status_line(false).starts_with("starts at the next restart"));
        assert!(f.apply(StartupAction::Revert, true).unwrap().contains("earlier boot order is back and the one-time start is cancelled"));
        assert_eq!(f.calls(), vec!["apply(first)", "apply(next)", "apply(revert)"]);
        assert!(f.apply(StartupAction::Revert, true).unwrap().contains("no saved order"));
        f.set(|s| s.read_only = true);
        assert!(f.apply(StartupAction::Register, true).unwrap_err().contains("read-only"));
        f.set(|s| {
            s.read_only = false;
            s.uefi = false;
        });
        assert_eq!(f.plan(StartupAction::Revert, true).unwrap_err(), NOT_HERE);
        assert_eq!(f.status_lines(false), vec![NOT_HERE.to_string()]);
        assert_eq!(f.before_dev_reboot(true).unwrap(), format!("not set: {NOT_HERE}"));
        assert_eq!(f.before_dev_reboot(false), None);
    }

    #[test]
    fn an_unknown_boot_volume_refuses_what_has_to_make_the_entry_but_not_a_revert() {
        let f = FakeStartupDisk::default();
        f.set(|s| s.boot_volume_known = false);
        assert!(f.plan(StartupAction::Register, true).unwrap_err().contains("Cannot tell which volume"));
        assert!(f.plan(StartupAction::Next, true).is_err());
        assert!(f.plan(StartupAction::Revert, true).is_ok());
        f.set(|s| s.own = true);
        assert!(f.plan(StartupAction::Next, true).is_ok(), "the entry exists, so no volume has to be told");
    }

    #[test]
    fn the_fake_answers_firmware_present_and_puts_an_existing_entry_first_only_for_an_administrator() {
        let f = FakeStartupDisk::default();
        assert!(f.firmware_present());
        assert_eq!(f.own_entry_first(false).unwrap_err(), ADMIN_ONLY);
        assert_eq!(f.own_entry_first(true).unwrap(), None, "no entry yet: nothing to change");
        f.set(|s| s.own = true);
        assert!(f.own_entry_first(true).unwrap().unwrap().contains("first"));
        assert_eq!(f.own_entry_first(true).unwrap(), None, "already first");
        assert_eq!(f.calls(), vec!["own_entry_first"]);
        f.set(|s| s.uefi = false);
        assert!(!f.firmware_present());
        assert_eq!(f.own_entry_first(true).unwrap_err(), NOT_HERE);
    }
}
