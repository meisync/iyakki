//! Chat tools for the two helper services that init keeps running (`iyakki-core::helpers`): fan control (Settings -> Power,
//! F-DRIVERS-14) and the privileged VPN helper (Settings -> VPN, F-VPN-05). A status tool is free; a change asks the person every
//! time and is for administrators. Both write the same setting file the service reads, then write one audit line; init starts
//! or stops the service within a few seconds of the file changing.

use std::path::{Path, PathBuf};

use iyakki_drivers::fanservice::{self, Setting};
use serde_json::{json, Value};

use crate::settingstools::{no_args, Entry, Env};

pub const TOOLS: [&str; 5] = ["fan_control_status", "fan_control_set", "vpn_helper_status", "vpn_helper_set", "firewall_outgoing_plan"];


fn fan_line(sys: &Path, dir: &Path) -> String {
    let s = fanservice::load(dir);
    let hw = if fanservice::supported(sys) { "this computer has fans IYAKKI can control" } else { "this computer has no applesmc fan with a temperature input, so nothing can be controlled" };
    format!("Fan control is {}. {hw}. Service report: {}", s.describe(), fanservice::status(dir))
}

fn next_setting(cur: Setting, a: &Value) -> Result<Setting, String> {
    let mut s = cur;
    let mut any = false;
    if let Some(v) = a.get("enabled") {
        s.enabled = v.as_bool().ok_or("`enabled` is true or false")?;
        any = true;
    }
    for (key, slot) in [("low", &mut s.low), ("mark", &mut s.high), ("max", &mut s.max), ("hysteresis", &mut s.hysteresis)] {
        if let Some(v) = a.get(key) {
            *slot = v.as_i64().and_then(|n| i32::try_from(n).ok()).ok_or_else(|| format!("`{key}` is a whole number of degrees"))?;
            any = true;
        }
    }
    if !any {
        return Err("say `enabled`, or a curve: `low`, `mark`, `max` (degrees C) or `hysteresis`".into());
    }
    if !s.curve().valid() {
        return Err("the curve needs 20 <= low < mark <= max <= 110 and a hysteresis of 0 to 10".into());
    }
    Ok(s)
}

fn vpn_dir(env: &Env) -> PathBuf {
    let safe: String = env.user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).collect();
    env.dir.join("vpn").join(safe)
}

fn helper_line(dir: &Path) -> String {
    let s = crate::vpn_helper::read_setting(dir);
    let listening = std::fs::symlink_metadata(&s.socket).is_ok();
    format!(
        "The privileged VPN helper is {}. Socket: {} ({}). It answers only user id {}. The VPN session still runs as the same user as the core on images, so the helper adds no protection until the session runs without root.",
        if s.enabled { "on" } else { "off" },
        s.socket,
        if listening { "present" } else { "not there" },
        s.allow_uid.map_or("of the core that starts it".to_string(), |u| u.to_string())
    )
}

/// The outgoing-connection and forwarding rules a person describes, checked and written out as the nft rules they would become.
/// Nothing is applied: the kernel writer cannot yet apply the output and forward hooks or NAT (F-FW-10).
fn outgoing_plan(a: &Value) -> Result<String, String> {
    let mut cfg: iyakki_fw::outbound::OutConfig = serde_json::from_value(a.get("config").cloned().ok_or("give `config`: {block_by_default, rules, forwards, port_forwards}")?).map_err(|e| format!("config: {e}"))?;
    cfg.enabled = true;
    let script = iyakki_fw::outbound::generate(&cfg)?;
    Ok(format!("These settings are valid. They would become the rules below. NOTHING WAS APPLIED by this tool: to put the rules in force use firewall_outgoing_set (the person approves, then has 60 seconds to Keep).\n\n{script}"))
}

/// `sys` is the folder the fan controller reads (`/sys` on a machine, a fake tree in tests).
pub fn entries(env: &Env, sys: PathBuf) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let (e, s) = (env.clone(), sys.clone());
        v.push(Entry::read("Power", "fan_control_status", "Show fan control: whether IYAKKI or the firmware runs the fans, the curve, the hottest temperature and the fan speeds.", no_args(), move |_| Ok(fan_line(&s, &e.dir))));
    }
    {
        let (e1, e2, e3) = (env.clone(), env.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {
            "enabled": {"type": "boolean", "description": "IYAKKI runs the fans (true) or the firmware does (false)."},
            "low": {"type": "integer", "description": "Degrees C up to which the fans run at their slowest (20 to 110)."},
            "mark": {"type": "integer", "description": "Degrees C of the middle mark of the curve."},
            "max": {"type": "integer", "description": "Degrees C at which the fans run at their fastest."},
            "hysteresis": {"type": "integer", "description": "Degrees the temperature must fall before the speed is lowered (0 to 10)."}}});
        v.push(
            Entry::change("Power", "fan_control_set", "Turn fan control on or off and set its curve. Only the fields you give change. The person approves a card first.", schema, |_| "Change fan control".into(), move |a| {
                let next = next_setting(fanservice::load(&e1.dir), a)?;
                fanservice::save(&e1.dir, &next)?;
                e1.audit("settings.fan", &next.describe());
                Ok(format!("Saved: {}. Init applies it within a few seconds; fan_control_status shows what the service reports.", next.describe()))
            })
            .with_preview(move |a| match next_setting(fanservice::load(&e2.dir), a) {
                Ok(n) => format!("Now: {}\nAfter: {}", fanservice::load(&e2.dir).describe(), n.describe()),
                Err(m) => format!("Cannot change it: {m}"),
            })
            .with_refusal(move |a| next_setting(fanservice::load(&e3.dir), a).err())
            .admin(),
        );
    }
    {
        let e = env.clone();
        v.push(Entry::read("Vpn", "vpn_helper_status", "Show whether the privileged VPN helper is switched on and listening.", no_args(), move |_| Ok(helper_line(&vpn_dir(&e)))));
    }
    {
        let e = env.clone();
        v.push(
            Entry::change("Vpn", "vpn_helper_set", "Switch the privileged VPN helper on or off (`enabled`). Init starts or stops it within a few seconds; the VPN manager uses it from the next sign-in. The person approves a card first.", json!({"type": "object", "properties": {"enabled": {"type": "boolean"}}, "required": ["enabled"]}), |a| format!("Switch the VPN helper {}", if a["enabled"].as_bool() == Some(true) { "on" } else { "off" }), move |a| {
                let on = a.get("enabled").and_then(Value::as_bool).ok_or("`enabled` is true or false")?;
                let dir = vpn_dir(&e);
                let mut s = crate::vpn_helper::read_setting(&dir);
                s.enabled = on;
                crate::vpn_helper::write_setting(&dir, &s)?;
                e.audit("settings.vpn-helper", if on { "on" } else { "off" });
                Ok(format!("Saved. {}", helper_line(&dir)))
            })
            .admin(),
        );
    }
    v.push(Entry::read(
        "Firewall",
        "firewall_outgoing_plan",
        "Check rules for outgoing connections, forwarding and port forwards and show the nft rules they would become. Nothing is applied. `config` is {block_by_default, rules:[{id,enabled,verdict:allow|block,dest,proto:tcp|udp|both,ports,iface,note}], forwards:[{id,enabled,from_iface,to_iface,dest,proto,ports,note}], port_forwards:[{id,enabled,iface,proto,port,to,to_port,note}]}.",
        json!({"type": "object", "properties": {"config": {"type": "object"}}, "required": ["config"]}),
        |a| outgoing_plan(a),
    ).admin());
    v
}

#[cfg(test)]
#[path = "settingstools_helpers_tests.rs"]
mod tests;
