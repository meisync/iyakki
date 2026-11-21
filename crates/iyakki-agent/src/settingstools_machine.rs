//! Chat tools for the machine's own switches that are plain settings: System sounds (`system_sounds_status`, `system_sounds_set`), Battery
//! (`battery_settings_status`, `battery_settings_set`), Power (`power_settings_status`, `power_settings_set`) and Cloud mode
//! (`cloud_mode_status`, `cloud_mode_set`). They update the same `settings::SettingsStore` the pages update (`validate` runs there); the
//! shell reads the values again as it does after a page change. Reads are free (anyone), changes ask every time and are for administrators.

use serde_json::{json, Value};

use crate::cloudmode::Setting;
use crate::power::{Profile, SLEEP_CHOICES};
use crate::settings::Settings;
use crate::settingstools::{no_args, Entry, Env};
use crate::syssounds::{Cue, MAX_OFFSET_DB, MIN_OFFSET_DB};
pub use iyakki_shared::settingstools_machine::{PROMPT_RULE};

pub const TOOLS: [&str; 11] = ["startup_status", "system_sounds_status", "system_sounds_set", "battery_settings_status", "battery_settings_set", "power_settings_status", "power_settings_set", "cloud_mode_status", "cloud_mode_set", "radio_settings_status", "radio_settings_set"];


fn read(env: &Env, f: impl Fn(&Settings) -> String) -> Result<String, String> {
    Ok(f(env.settings.lock().map_err(|_| "settings are busy".to_string())?.get()))
}

fn change(env: &Env, what: &str, f: impl Fn(&mut Settings) -> Result<(), String>, show: impl Fn(&Settings) -> String) -> Result<String, String> {
    let mut store = env.settings.lock().map_err(|_| "settings are busy".to_string())?;
    // try it on a copy first: a refusal keeps nothing, not even the fields before the bad one
    let mut probe = store.get().clone();
    f(&mut probe)?;
    store.update(|s| {
        let _ = f(s);
    })
    .map_err(|e| e.to_string())?;
    env.audit(what, "chat");
    Ok(show(store.get()))
}

fn sounds_text(s: &Settings) -> String {
    let c = &s.sounds;
    format!(
        "System sounds are {}. Level: {} dB below IYAKKI's volume. {}",
        if c.enabled { "on" } else { "off" },
        c.offset_db,
        Cue::ALL.iter().map(|q| format!("{}: {}", q.label(), if c.get(*q) { "on" } else { "off" })).collect::<Vec<_>>().join(", ")
    )
}

fn battery_text(s: &Settings) -> String {
    let b = &s.battery;
    format!("Percentage in the top bar: {}. Low-battery warnings: {}. Shutdown at the critical level: {}.", if b.show_percent() { "shown" } else { "hidden" }, if b.warnings() { "on" } else { "off" }, if b.no_shutdown { "off" } else { "on" })
}

fn power_text(s: &Settings) -> String {
    let p = &s.power;
    format!("Power profile: {}. Sleep after idle: {}. Sleep even while busy: {}. Closing the lid: {}. Power button: {}.", p.profile.label(), if p.sleep_after_min == 0 { "never".to_string() } else { format!("{} minutes", p.sleep_after_min) }, if p.sleep_when_busy { "yes" } else { "no" }, p.lid.label(), p.button.label())
}

fn radio_text(s: &Settings) -> String {
    format!("Radio: reconnect tries {} (0 to {}), moving on to the next station {}. {}", s.radio.retries(), iyakki_shared::radiorecover::MAX_RETRIES, if s.radio.auto_switch() { "on" } else { "off" }, s.radio.describe())
}

fn cloud_text(s: &Settings) -> String {
    format!("Cloud mode (hides what needs a local screen, speakers or camera): {}.", s.cloud.label())
}

pub fn entries(env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    v.push(
        Entry::read("Startup", "startup_status", "Show every service of the machine with where its start stands: ready, starting, pending, failed, stopped or off, and the step or the reason.", no_args(), |_| {
            let rows = crate::startup::global().snapshot();
            Ok(rows
                .iter()
                .map(|r| {
                    let (word, why) = match &r.state {
                        crate::startup::StartupState::Failed(w) => ("failed", w.clone()),
                        s => (s.word(), r.step.clone()),
                    };
                    if why.is_empty() { format!("{}: {word}", r.name) } else { format!("{}: {word} ({why})", r.name) }
                })
                .collect::<Vec<_>>()
                .join("\n"))
        })
        .admin(),
    );
    let (e1, e2) = (env.clone(), env.clone());
    v.push(Entry::read("SystemSounds", "system_sounds_status", "Show the system sound cues: master switch, level, and each cue.", no_args(), move |_| read(&e1, sounds_text)));
    let cues: Vec<&str> = Cue::ALL.iter().map(|c| c.id()).collect();
    v.push(
        Entry::change(
            "SystemSounds",
            "system_sounds_set",
            "Change the system sound cues: the master switch, the level in dB below IYAKKI's volume (-30 to 0), and single cues. The person approves a card first.",
            json!({"type": "object", "properties": {"enabled": {"type": "boolean"}, "offset_db": {"type": "integer", "minimum": MIN_OFFSET_DB, "maximum": MAX_OFFSET_DB}, "cue": {"type": "string", "enum": cues}, "cue_on": {"type": "boolean"}}}),
            |_| "Change the system sounds".into(),
            move |a| {
                if a.get("enabled").is_none() && a.get("offset_db").is_none() && a.get("cue").is_none() {
                    return Err("say `enabled`, `offset_db`, or `cue` with `cue_on`".into());
                }
                let cue = match a.get("cue").and_then(Value::as_str) {
                    Some(id) => Some(Cue::ALL.iter().copied().find(|c| c.id() == id).ok_or("unknown cue")?),
                    None => None,
                };
                change(&e2, "settings.sounds", |s| {
                    if let Some(b) = a.get("enabled").and_then(Value::as_bool) {
                        s.sounds.enabled = b;
                    }
                    if let Some(n) = a.get("offset_db").and_then(Value::as_i64) {
                        s.sounds.offset_db = i32::try_from(n).map_err(|_| "offset_db is between -30 and 0".to_string())?;
                    }
                    if let Some(c) = cue {
                        s.sounds.set(c, a.get("cue_on").and_then(Value::as_bool).ok_or("say `cue_on`: true or false")?);
                    }
                    Ok(())
                }, sounds_text)
            },
        )
        .admin(),
    );
    let (e3, e4) = (env.clone(), env.clone());
    v.push(Entry::read("Battery", "battery_settings_status", "Show the battery switches: percentage in the top bar, warnings, shutdown at the critical level.", no_args(), move |_| read(&e3, battery_text)));
    v.push(
        Entry::change("Battery", "battery_settings_set", "Change the battery switches: show_percent, warnings, shutdown_at_critical. The person approves a card first.", json!({"type": "object", "properties": {"show_percent": {"type": "boolean"}, "warnings": {"type": "boolean"}, "shutdown_at_critical": {"type": "boolean"}}}), |_| "Change the battery switches".into(), move |a| {
            if !["show_percent", "warnings", "shutdown_at_critical"].iter().any(|k| a.get(*k).is_some()) {
                return Err("say `show_percent`, `warnings` or `shutdown_at_critical`".into());
            }
            change(&e4, "settings.battery", |s| {
                if let Some(b) = a.get("show_percent").and_then(Value::as_bool) {
                    s.battery.hide_percent = !b;
                }
                if let Some(b) = a.get("warnings").and_then(Value::as_bool) {
                    s.battery.no_warnings = !b;
                }
                if let Some(b) = a.get("shutdown_at_critical").and_then(Value::as_bool) {
                    s.battery.no_shutdown = !b;
                }
                Ok(())
            }, battery_text)
        })
        .admin(),
    );
    let (e5, e6) = (env.clone(), env.clone());
    v.push(Entry::read("Power", "power_settings_status", "Show the power profile and sleep after idle.", no_args(), move |_| read(&e5, power_text)));
    v.push(
        Entry::change(
            "Power",
            "power_settings_set",
            "Change the power profile, the minutes of idle before sleep (0 never, 5, 10, 15, 30, 60), whether the computer sleeps while busy, what closing the lid does (nothing, lock, sleep) and what the power button does (card, lock, sleep, nothing). The person approves a card first.",
            json!({"type": "object", "properties": {"profile": {"type": "string", "enum": ["performance", "balanced", "power_saver"]}, "sleep_after_min": {"type": "integer", "enum": SLEEP_CHOICES}, "sleep_when_busy": {"type": "boolean"}, "lid": {"type": "string", "enum": ["nothing", "lock", "sleep"]}, "power_button": {"type": "string", "enum": ["card", "lock", "sleep", "nothing"]}}}),
            |_| "Change the power settings".into(),
            move |a| {
                if !["profile", "sleep_after_min", "sleep_when_busy", "lid", "power_button"].iter().any(|k| a.get(*k).is_some()) {
                    return Err("say `profile`, `sleep_after_min`, `sleep_when_busy`, `lid` or `power_button`".into());
                }
                change(&e6, "settings.power", |s| {
                    if let Some(p) = a.get("profile").and_then(Value::as_str) {
                        s.power.profile = Profile::ALL.iter().copied().find(|x| format!("{x:?}").to_lowercase() == p.replace('_', "")).ok_or("profile is performance, balanced or power_saver")?;
                    }
                    if let Some(n) = a.get("sleep_after_min").and_then(Value::as_u64) {
                        let n = u32::try_from(n).ok().filter(|n| SLEEP_CHOICES.contains(n)).ok_or("sleep_after_min is 0, 5, 10, 15, 30 or 60")?;
                        s.power.sleep_after_min = n;
                    }
                    if let Some(b) = a.get("sleep_when_busy").and_then(Value::as_bool) {
                        s.power.sleep_when_busy = b;
                    }
                    if let Some(w) = a.get("lid").and_then(Value::as_str) {
                        s.power.lid = crate::power::LidAction::parse(w).ok_or("lid is nothing, lock or sleep")?;
                    }
                    if let Some(w) = a.get("power_button").and_then(Value::as_str) {
                        s.power.button = crate::power::ButtonAction::parse(w).ok_or("power_button is card, lock, sleep or nothing")?;
                    }
                    Ok(())
                }, power_text)
            },
        )
        .admin(),
    );
    let (e7, e8) = (env.clone(), env.clone());
    v.push(Entry::read("Cloud", "cloud_mode_status", "Show cloud mode: whether the desktop hides what needs a local screen, speakers or camera.", no_args(), move |_| read(&e7, cloud_text)));
    v.push(
        Entry::change("Cloud", "cloud_mode_set", "Set cloud mode to automatic, on or off. It is read when someone signs in. The person approves a card first.", json!({"type": "object", "properties": {"mode": {"type": "string", "enum": ["auto", "on", "off"]}}, "required": ["mode"]}), |a| format!("Set cloud mode to {}", a["mode"].as_str().unwrap_or("?")), move |a| {
            let m = match a.get("mode").and_then(Value::as_str) {
                Some("auto") => Setting::Auto,
                Some("on") => Setting::On,
                Some("off") => Setting::Off,
                _ => return Err("mode is auto, on or off".into()),
            };
            change(&e8, "settings.cloud", |s| {
                s.cloud = m;
                Ok(())
            }, cloud_text)
        })
        .admin(),
    );
    let (e9, e10) = (env.clone(), env.clone());
    v.push(Entry::read("Radio", "radio_settings_status", "Show what the radio does when a live station drops: how many times it reconnects the same station, and whether it then goes on to the next station of the list.", no_args(), move |_| read(&e9, radio_text)));
    v.push(
        Entry::change(
            "Radio",
            "radio_settings_set",
            "Change what the radio does when a live station drops: `retries` (0 to 5 reconnects of the same station, default 2) and `auto_switch` (true: go on to the next station of the list after the retries, default; false: stop). Playing the first station still needs the person to start it. The person approves a card first.",
            json!({"type": "object", "properties": {"retries": {"type": "integer", "minimum": 0, "maximum": 5}, "auto_switch": {"type": "boolean"}}}),
            |_| "Change the radio's reconnect settings".into(),
            move |a| {
                if a.get("retries").is_none() && a.get("auto_switch").is_none() {
                    return Err("say `retries` (0 to 5) or `auto_switch` (true or false)".into());
                }
                change(&e10, "settings.radio", |s| {
                    if let Some(n) = a.get("retries") {
                        let n = n.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| *n <= iyakki_shared::radiorecover::MAX_RETRIES).ok_or("retries is 0 to 5")?;
                        s.radio.retries = Some(n);
                    }
                    if let Some(b) = a.get("auto_switch").and_then(Value::as_bool) {
                        s.radio.no_auto_switch = !b;
                    }
                    Ok(())
                }, radio_text)
            },
        )
        .admin(),
    );
    // fan control (Settings -> Power) and the privileged VPN helper (Settings -> VPN): the services init keeps running
    v.extend(crate::settingstools_helpers::entries(env, std::path::PathBuf::from("/sys")));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Tools;
    use crate::settingstools_tests::{assert_policy, rig, tools};

    #[test]
    fn the_switches_change_the_settings_store_validate_and_ask_every_time() {
        let r = rig("machine", true);
        let t = tools(&r, entries(&r.env));
        assert_policy(&t, &["startup_status", "system_sounds_status", "battery_settings_status", "power_settings_status", "cloud_mode_status", "radio_settings_status"], &["system_sounds_set", "battery_settings_set", "power_settings_set", "cloud_mode_set", "radio_settings_set"]);
        assert!(t.call("system_sounds_set", &json!({"offset_db": -40})).is_err(), "out of range is refused by validate and nothing is kept");
        assert_eq!(r.env.settings.lock().unwrap().get().sounds.offset_db, crate::syssounds::DEFAULT_OFFSET_DB);
        assert!(t.call("system_sounds_set", &json!({"cue": "notify", "cue_on": false})).unwrap().contains("off"));
        assert!(!r.env.settings.lock().unwrap().get().sounds.notify);
        t.call("battery_settings_set", &json!({"show_percent": false, "shutdown_at_critical": false})).unwrap();
        let b = r.env.settings.lock().unwrap().get().battery.clone();
        assert!(b.hide_percent && b.no_shutdown && !b.no_warnings);
        assert!(t.call("power_settings_set", &json!({"profile": "power_saver", "sleep_after_min": 15})).unwrap().contains("Power saver"));
        assert!(t.call("power_settings_set", &json!({"sleep_after_min": 7})).is_err());
        assert!(t.call("power_settings_set", &json!({"lid": "sleep", "power_button": "lock"})).unwrap().contains("Closing the lid: sleep"));
        let p = r.env.settings.lock().unwrap().get().power.clone();
        assert_eq!((p.lid, p.button), (crate::power::LidAction::Sleep, crate::power::ButtonAction::Lock));
        assert!(t.call("power_settings_set", &json!({"lid": "explode"})).is_err() && t.call("power_settings_set", &json!({"power_button": "off"})).is_err());
        assert!(t.call("power_settings_set", &json!({"profile": "turbo"})).is_err());
        t.call("radio_settings_set", &json!({"retries": 1, "auto_switch": false})).unwrap();
        let rr = r.env.settings.lock().unwrap().get().radio.clone();
        assert_eq!((rr.retries(), rr.auto_switch()), (1, false));
        assert!(t.call("radio_settings_set", &json!({"retries": 9})).is_err() && t.call("radio_settings_set", &json!({})).is_err());
        assert_eq!(r.env.settings.lock().unwrap().get().radio.retries(), 1, "a refused change keeps nothing");
        t.call("cloud_mode_set", &json!({"mode": "on"})).unwrap();
        assert_eq!(r.env.settings.lock().unwrap().get().cloud, Setting::On);
        assert!(t.call("cloud_mode_set", &json!({"mode": "maybe"})).is_err());
        assert!(r.audits.lock().unwrap().iter().any(|a| a == "settings.power chat"));
        // a person who is not an administrator reads but cannot change
        let p = rig("machine2", false);
        let t2 = tools(&p, entries(&p.env));
        assert!(t2.defs().iter().all(|d| d.spec.name.ends_with("_status")), "{:?}", t2.defs().iter().map(|d| d.spec.name.clone()).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(&r.dir);
        let _ = std::fs::remove_dir_all(&p.dir);
    }
}
