//! Chat tools for the firmware start-up entry of this computer (Settings -> This computer -> Startup disk): `startup_disk_status` (free),
//! `startup_disk_register` (asks, always) and `startup_disk_set` (asks, always: `first`, `next`, `revert` or the switch "restart into
//! IYAKKI"). They call the same functions as `iyakki startup-disk` and the Settings page (`startupdisk`); only IYAKKI's own entry,
//! `BootOrder` and the one-time `BootNext` are ever written, never another system's entry.

use serde_json::{json, Value};

use crate::settingstools::{arg_str, no_args, Entry, Env};
use iyakki_core_api::startupdisk::{StartupAction, StartupDisk};
pub use iyakki_shared::settingstools_startup::{PROMPT_RULE};

const TAB: &str = "Computer";

pub const TOOLS: [&str; 3] = ["startup_disk_status", "startup_disk_register", "startup_disk_set"];


/// The core's handle for this computer's firmware (the old boot order is kept in the data folder of the tool's environment).
fn real(env: &Env) -> crate::startupdisk::LocalStartupDisk {
    crate::startupdisk::local(Some(env.dir.clone()))
}

fn setting(env: &Env) -> bool {
    env.settings.lock().map(|s| s.get().startup_reboot_into_iyakki).unwrap_or_else(|p| p.into_inner().get().startup_reboot_into_iyakki)
}

fn say(r: Result<String, String>, env: &Env, what: &str) -> Result<String, String> {
    env.audit("settings.startup_disk", &format!("{what}: {}", if r.is_ok() { "done" } else { "refused" }));
    r
}

pub fn entries(env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let e = env.clone();
        v.push(Entry::read(TAB, "startup_disk_status", "Show what the firmware starts first, whether IYAKKI has its own entry in the boot menu, and whether restarts from developer mode go to IYAKKI.", no_args(), move |_| Ok(real(&e).status_lines(setting(&e)).join("\n"))));
    }
    {
        let e = env.clone();
        v.push(
            Entry::change(TAB, "startup_disk_register", "Make IYAKKI's own entry in the computer's boot menu for the volume it is running from (appended last; the boot order does not change). Writes a firmware boot variable. The person approves a card first.", no_args(), |_| "Make IYAKKI's own entry in this computer's boot menu".into(), move |_| say(real(&e).apply(StartupAction::Register, true), &e, "register"))
                .admin()
                .with_preview(|_| "Writes one firmware boot entry described \"IYAKKI\" for the volume this system started from. Other systems' entries and the boot order are not changed (the entry is added last). `startup_disk_set` revert undoes it.".into()),
        );
    }
    {
        let (e, e2) = (env.clone(), env.clone());
        let schema = json!({"type": "object", "required": ["action"], "properties": {
            "action": {"type": "string", "enum": ["first", "next", "revert", "reboot_into_iyakki"], "description": "first: IYAKKI starts by default (registers when needed); next: IYAKKI starts at the next restart only; revert: put the old order back; reboot_into_iyakki: switch the developer-mode restart behaviour on or off."},
            "enabled": {"type": "boolean", "description": "For reboot_into_iyakki only: true = a restart through `iyakki dev reboot` starts IYAKKI once."}}});
        v.push(
            Entry::change(TAB, "startup_disk_set", "Change what the computer starts: put IYAKKI first (`first`), start it at the next restart only (`next`), put the old order back (`revert`), or set `reboot_into_iyakki` on or off. Writes firmware boot variables. The person approves a card first.", schema, |a| match a.get("action").and_then(Value::as_str) {
                Some("first") => "Make IYAKKI the computer's default start".into(),
                Some("next") => "Start IYAKKI at the next restart only".into(),
                Some("revert") => "Put the earlier boot order back".into(),
                Some("reboot_into_iyakki") => format!("Restarts from developer mode start IYAKKI: {}", if a.get("enabled").and_then(Value::as_bool) == Some(true) { "on" } else { "off" }),
                _ => "Change the startup disk".into(),
            }, move |a| {
                let action = arg_str(a, "action")?;
                let env2 = real(&e);
                match action.as_str() {
                    "first" => say(env2.apply(StartupAction::First, true), &e, "first"),
                    "next" => say(env2.apply(StartupAction::Next, true), &e, "next"),
                    "revert" => say(env2.apply(StartupAction::Revert, true), &e, "revert"),
                    "reboot_into_iyakki" => {
                        let on = a.get("enabled").and_then(Value::as_bool).ok_or("say `enabled`: true or false")?;
                        e.settings.lock().map_err(|_| "the settings are busy".to_string())?.update(|s| s.startup_reboot_into_iyakki = on).map_err(|x| x.to_string())?;
                        e.audit("settings.startup_reboot_into_iyakki", if on { "on" } else { "off" });
                        Ok(format!("Restarts through `iyakki dev reboot` {} start IYAKKI once (the firmware's one-time next boot is set before the restart).", if on { "now" } else { "no longer" }))
                    }
                    _ => Err("action is first, next, revert or reboot_into_iyakki".into()),
                }
            })
            .admin()
            .with_preview(move |a| {
                let now = setting(&e2);
                match a.get("action").and_then(Value::as_str) {
                    Some("reboot_into_iyakki") => format!("Now: {}\nAfter: {}", if now { "on" } else { "off" }, if a.get("enabled").and_then(Value::as_bool).unwrap_or(false) { "on" } else { "off" }),
                    Some("first") => "IYAKKI's entry goes first in the firmware's boot order (made first when missing); the other entries keep their order; the old order is saved for revert.".into(),
                    Some("next") => "The firmware's one-time next boot is set to IYAKKI; the boot order is not changed.".into(),
                    Some("revert") => "The saved boot order comes back and a pending one-time start is cancelled; IYAKKI's entry stays in the menu.".into(),
                    _ => String::new(),
                }
            })
            .with_refusal(|a| match a.get("action").and_then(Value::as_str) {
                Some("first" | "next" | "revert") => None,
                Some("reboot_into_iyakki") if a.get("enabled").and_then(Value::as_bool).is_none() => Some("say `enabled`: true or false".into()),
                Some("reboot_into_iyakki") => None,
                _ => Some("say `action`: first, next, revert or reboot_into_iyakki".into()),
            }),
        );
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Risk, Tools};
    use crate::settingstools_tests::{assert_policy, rig, tools};

    #[test]
    fn status_is_free_and_the_changes_are_administrators_only_with_a_card_and_the_switch_is_kept() {
        let r = rig("startupdisk", true);
        let t = tools(&r, entries(&r.env));
        assert_policy(&t, &["startup_disk_status"], &["startup_disk_register", "startup_disk_set"]);
        assert!(t.refusal("startup_disk_set", &json!({"action": "sideways"})).is_some());
        assert!(t.refusal("startup_disk_set", &json!({"action": "reboot_into_iyakki"})).is_some(), "needs enabled");
        assert!(t.refusal("startup_disk_set", &json!({"action": "next"})).is_none());
        assert!(t.preview("startup_disk_set", &json!({"action": "reboot_into_iyakki", "enabled": true})).unwrap().contains("After: on"));
        t.call("startup_disk_set", &json!({"action": "reboot_into_iyakki", "enabled": true})).unwrap();
        assert!(r.env.settings.lock().unwrap().get().startup_reboot_into_iyakki);
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("settings.startup_reboot_into_iyakki on")));
        t.call("startup_disk_set", &json!({"action": "reboot_into_iyakki", "enabled": false})).unwrap();
        assert!(!r.env.settings.lock().unwrap().get().startup_reboot_into_iyakki);
        // a computer started without UEFI variables (this machine in tests) says so in plain words and changes nothing
        let e = t.call("startup_disk_register", &json!({})).unwrap_err();
        assert!(e.contains("not started through UEFI") || e.contains("Cannot tell"), "{e}");
        assert!(t.call("startup_disk_status", &json!({})).unwrap().contains("UEFI"));
        let user = rig("startupdisk-user", false);
        let ut = tools(&user, entries(&user.env));
        assert!(!ut.defs().iter().any(|d| d.spec.name == "startup_disk_set" || d.spec.name == "startup_disk_register"));
        assert!(ut.defs().iter().any(|d| d.spec.name == "startup_disk_status" && d.risk == Risk::Safe));
        let _ = std::fs::remove_dir_all(&r.dir);
        let _ = std::fs::remove_dir_all(&user.dir);
    }
}
