//! The chat tools of the trusted-computer switch (see [`super::trusted`]); served by [`super::restarttools::RestartTools`] so the
//! tool chain needs no new entry.
//!
//! * `trusted_computer_status` and `patch_grant_status` (free, administrators): what each paired computer is trusted for and until when.
//! * `trusted_computer` (hard ask, administrators): switch the "trusted dev computer" mode on or off for one paired computer: every approval
//!   card of that computer's requests is answered yes by itself. FOR DEV AND TEST MACHINES. `hours` 4 or 12 makes it timed; without it, it
//!   is kept until switched off (developer mode then stays on).
//! * `patch_grant` (hard ask, administrators): "Patches without asking": the same store, only the patch approval.
//!
//! Switching ON is a hard ask: it asks on a card every time, Auto mode and a session grant never cover it, and a request that comes from
//! a remote computer (not the person at the chat) cannot tap it. Switching off asks too (cheap), and ends it at once.

use iyakki_sec::policy::Policy;
use serde_json::{json, Value};

use super::trusted::{TrustFor, TrustLevel};
use super::DevMode;
use crate::agent::{Risk, ToolDef};
use crate::chat::ToolSpec;

pub const TRUSTED_STATUS: &str = "trusted_computer_status";
pub const TRUSTED_SET: &str = "trusted_computer";
pub const PATCH_STATUS: &str = "patch_grant_status";
pub const PATCH_SET: &str = "patch_grant";


pub fn is_ours(n: &str) -> bool {
    matches!(n, TRUSTED_STATUS | TRUSTED_SET | PATCH_STATUS | PATCH_SET)
}

fn level_of(name: &str) -> TrustLevel {
    if name == PATCH_SET || name == PATCH_STATUS { TrustLevel::Patches } else { TrustLevel::Everything }
}

pub fn defs() -> Vec<ToolDef> {
    let status = |name: &str, what: &str| ToolDef { spec: ToolSpec { name: name.into(), description: what.into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false };
    let set = |name: &str, what: &str| ToolDef {
        spec: ToolSpec {
            name: name.into(),
            description: what.into(),
            schema: json!({"type": "object", "properties": {
                "computer": {"type": "string", "description": "the paired computer's name"},
                "on": {"type": "boolean"},
                "hours": {"type": "integer", "description": "with on: true, 4 or 12 for a timed switch; leave out to keep it until the person switches it off (developer mode then stays on)"}
            }, "required": ["computer", "on"]}),
        },
        risk: Risk::Ask,
        untrusted: false,
    };
    vec![
        status(TRUSTED_STATUS, "Show which paired computers are trusted dev computers (every approval answered yes by itself) and until when. Administrators only."),
        set(TRUSTED_SET, "FOR DEV AND TEST MACHINES: make one paired computer a trusted dev computer (every approval card of its requests is answered yes by itself: shell, patch, restart; never passwords, the vault or root) or stop. The person confirms on a card every time."),
        status(PATCH_STATUS, "Show which paired computers may patch the system (iyakki dev patch / push-apps) without asking, and until when. Administrators only."),
        set(PATCH_SET, "Let one paired computer patch the system (iyakki dev patch / push-apps: apps program, tile apps, layers) without asking, or stop. The person confirms on a card every time."),
    ]
}

fn parse(args: &Value) -> Result<(String, bool, TrustFor), String> {
    let c = args.get("computer").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if c.is_empty() {
        return Err("say which paired computer (its name from devmode_pairings)".into());
    }
    let on = args.get("on").and_then(Value::as_bool).ok_or_else(|| "say on: true or on: false".to_string())?;
    let hours = args.get("hours").and_then(Value::as_u64).unwrap_or(0);
    if hours != 0 && !on {
        return Err("hours only goes with on: true".into());
    }
    if !matches!(hours, 0 | 4 | 12) {
        return Err("hours is 4 or 12 (leave it out to keep it until you switch it off)".into());
    }
    Ok((c, on, if hours == 0 { TrustFor::Kept } else { TrustFor::Hours(hours as u32) }))
}

pub fn refusal(dm: &DevMode, policy: &Policy, name: &str, args: &Value) -> Option<String> {
    if !matches!(name, TRUSTED_SET | PATCH_SET) {
        return None;
    }
    if !policy.manage_settings {
        return Some("Only an administrator may change what a paired computer is trusted for.".into());
    }
    parse(args).err().or_else(|| if !dm.is_on() && args.get("on").and_then(Value::as_bool) == Some(true) { Some("Developer mode is off: switch it on first.".into()) } else { None })
}

pub fn describe(name: &str, args: &Value) -> String {
    match parse(args) {
        Ok((c, true, f)) => format!("Trust {c} {}{}", if level_of(name) == TrustLevel::Patches { "to patch this device without asking" } else { "as a dev computer: every approval answered yes by itself" }, if f == TrustFor::Kept { " until I switch it off" } else { "" }),
        Ok((c, false, _)) => format!("Make {c} ask for approvals again"),
        Err(_) => "Change what a paired computer is trusted for".into(),
    }
}

pub fn preview(dm: &DevMode, name: &str, args: &Value) -> String {
    match parse(args) {
        Ok((c, true, f)) => {
            let when = match f {
                TrustFor::Kept => "until YOU switch it off: developer mode's automatic switch-off no longer ends it and developer mode stays on until you switch it off by hand".to_string(),
                TrustFor::Hours(h) => format!("for {h} hours (and no longer than developer mode, which {})", { let u = dm.state().until; if u > 0 { format!("switches off {}", super::stamp(u)) } else { "stays on until switched off".into() } }),
            };
            if level_of(name) == TrustLevel::Patches {
                format!("{c} will be able to patch this device (the apps program, tile apps and layers; never the kernel, boot menu or firmware) without a card {when}. Every use is written to the activity log. Meant for dev and test machines.")
            } else {
                format!("FOR DEV AND TEST MACHINES. Every approval card that {c} raises (run commands, patch, restart or switch off) will be answered yes by itself {when}. It never covers passwords, the vault, running as root or a scope {c} does not hold. Every auto-approval is written to the activity log and the top bar shows DEV · TRUSTED COMPUTER {c}.")
            }
        }
        Ok((c, false, _)) => format!("{c} will have to be approved at the device's screen again."),
        Err(e) => format!("Cannot change it: {e}"),
    }
}

pub fn status(dm: &DevMode, policy: &Policy, name: &str) -> String {
    if !policy.manage_settings {
        return "Only an administrator sees what the paired computers are trusted for.".into();
    }
    if !dm.is_on() {
        return "Developer mode is off, so no computer is trusted.".into();
    }
    let level = level_of(name);
    let list = dm.trusted_list();
    let mut out = String::new();
    for k in dm.keys() {
        let line = match list.iter().find(|t| t.computer == k.name) {
            Some(t) if t.level == TrustLevel::Everything && level == TrustLevel::Patches => format!("trusted dev computer: patches without asking too ({})", until(t.until)),
            Some(t) if t.level == level => format!("{} {}", if level == TrustLevel::Patches { "patches without asking" } else { "trusted dev computer: every approval answered yes by itself" }, until(t.until)),
            Some(t) => format!("trusted for {} only ({})", if t.level == TrustLevel::Patches { "patches" } else { "everything" }, until(t.until)),
            None => "asks each time".to_string(),
        };
        out.push_str(&format!("{}: {line}\n", k.name));
    }
    if out.is_empty() { "No computer is paired.".into() } else { out }
}

fn until(u: i64) -> String {
    if u == 0 { "until you switch it off".into() } else { format!("until {}", super::stamp(u)) }
}

pub fn call(dm: &DevMode, policy: &Policy, user: &str, audit: &dyn Fn(&str, &str), name: &str, args: &Value) -> Result<String, String> {
    if matches!(name, TRUSTED_STATUS | PATCH_STATUS) {
        return Ok(status(dm, policy, name));
    }
    let (c, on, f) = parse(args)?;
    if on {
        let v = dm.trusted_on(policy, user, &c, level_of(name), f)?;
        audit("devmode.trusted.on", &format!("{c} (chat)"));
        Ok(format!("{c} is {} {}. {}", if v.level == TrustLevel::Patches { "allowed to patch this device without asking" } else { "a trusted dev computer: its approvals are answered yes by themselves" }, until(v.until), if v.kept() { "Developer mode stays on until you switch it off." } else { "It also ends when developer mode is switched off." }))
    } else {
        dm.trusted_off(policy, user, &c)?;
        audit("devmode.trusted.off", &format!("{c} (chat)"));
        Ok(format!("{c} must ask for approval again."))
    }
}

pub const BLOCKER: &str = "who may act on this device without asking is the person's security choice, so it asks every time";
