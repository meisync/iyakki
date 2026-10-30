//! The lead assistant's tools for VPN profiles (see `docs/development/VPN-CONNECTOR-PLAN.md`): `vpn_list` reads (no card, no
//! secrets); `vpn_add`, `vpn_enable`, `vpn_disable` and `vpn_delete` each ask on a card that says in plain words what will change,
//! and are on the hard-ask list (`agent::hard_ask`): they ask every time, in Auto mode too, and "Allow for this session" does not
//! remember them.
//!
//! * The assistant never sees, sends or hears a key: `vpn_add` takes the **path of the profile file** the person already has (read
//!   through the same file scope as `read_file`, inside this tool, never echoed), not its text. Text that looks like key material
//!   in any argument is refused without being repeated.
//! * Mode, filters, the kill switch and the guard are the person's choices on the page; no tool changes them.
//! * The checks are the manager's own ([`crate::vpn_mgr`]); there is no second path around them.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::vpn_mgr::{describe, Manager};
use iyakki_vpn::profile::Profile;

pub const PROMPT_RULE: &str = "\nVPN: vpn_list shows the person's VPN profiles and their state. To add (import a profile FILE by its path), enable, disable or delete one, call the matching vpn_* tool: each asks the person every time. Never ask for or send key text; the person's own WireGuard or OpenVPN file is imported by path and its keys go to their vault. Mode, filters and the kill switch are set by the person in Settings -> Network -> VPN. IYAKKI provides no VPN service and no server list.";

pub const WRITE_TOOLS: [&str; 4] = ["vpn_add", "vpn_enable", "vpn_disable", "vpn_delete"];
pub const READ_TOOL: &str = "vpn_list";

pub fn is_vpn_tool(n: &str) -> bool {
    n == READ_TOOL || WRITE_TOOLS.contains(&n)
}

pub struct VpnTools {
    inner: Arc<dyn Tools>,
    mgr: Arc<Manager>,
}

impl VpnTools {
    pub fn new(inner: Arc<dyn Tools>, mgr: Arc<Manager>) -> VpnTools {
        VpnTools { inner, mgr }
    }

    fn find(&self, args: &Value) -> Result<Profile, String> {
        let w = text(args, "profile");
        if w.is_empty() {
            return Err("name the VPN profile (its id or its name; vpn_list shows them)".into());
        }
        self.mgr.profiles().into_iter().find(|p| p.id == w || p.name.eq_ignore_ascii_case(&w)).ok_or_else(|| "there is no such VPN profile (vpn_list shows them)".into())
    }

    /// The file named by `path`, read through the file scope of the inner tools. Its text is used here and goes nowhere else.
    fn read(&self, args: &Value) -> Result<Vec<u8>, String> {
        let path = text(args, "path");
        if path.is_empty() {
            return Err("path is the file the person has (their WireGuard .conf or OpenVPN .ovpn); never send the text of the file".into());
        }
        if let Some(r) = self.inner.refusal("read_file", &json!({"path": path})) {
            return Err(r);
        }
        self.inner.call("read_file", &json!({"path": path})).map(String::into_bytes)
    }

    fn card(&self, name: &str, args: &Value) -> Result<String, String> {
        reject_secrets(args)?;
        match name {
            "vpn_add" => {
                let n = text(args, "name");
                let data = self.read(args)?;
                let imp = self.mgr.preview(&n, &data)?;
                let mut s = format!(
                    "Import the VPN profile “{}” from the file {}. It starts disabled, in FULL mode, with the kill switch off.\n• {}\n• Its keys and passwords go into your vault and are never shown again.",
                    imp.profile.name,
                    text(args, "path"),
                    describe(&imp.profile)
                );
                for w in &imp.warnings {
                    s.push_str(&format!("\n• Note: {w}"));
                }
                Ok(s)
            }
            "vpn_enable" => {
                let p = self.find(args)?;
                Ok(format!(
                    "Enable the VPN “{}”: it connects now and after every sign-in, and it changes this device's routes{}. Any other enabled VPN is disabled.\n• {}",
                    p.name,
                    if p.settings.mode == iyakki_vpn::Mode::Full { " (all traffic goes through the VPN)" } else { " (only what the person's filters match)" },
                    describe(&p)
                ))
            }
            "vpn_disable" => {
                let p = self.find(args)?;
                Ok(format!("Disable the VPN “{}”: it disconnects now and everything it set up (interface, routes, DNS, guard) is removed. The profile is kept.", p.name))
            }
            "vpn_delete" => {
                let p = self.find(args)?;
                Ok(format!("DELETE the VPN profile “{}”. It disconnects now and the profile and its keys in your vault are removed. This cannot be undone.\n• {}", p.name, describe(&p)))
            }
            _ => Err("not a VPN tool".into()),
        }
    }
}

fn text(args: &Value, k: &str) -> String {
    args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

/// Text that looks like a key, a whole profile or a token is refused, and never repeated.
fn reject_secrets(args: &Value) -> Result<(), String> {
    fn walk(v: &Value) -> bool {
        match v {
            Value::String(s) => s.contains("-----BEGIN") || s.contains("PRIVATE KEY") || s.contains("PrivateKey") || s.contains("[Interface]") || s.contains('\n') || s.len() > 300,
            Value::Array(a) => a.iter().any(walk),
            Value::Object(o) => o.values().any(walk),
            _ => false,
        }
    }
    if walk(args) {
        return Err("a value looks like key text or a whole profile. Never send keys or profile text: give the path of the person's file.".into());
    }
    Ok(())
}

fn one_line(r: &crate::vpn_mgr::Row, now: u64) -> String {
    format!(
        "{} · {} · {} · {} · {}{}",
        r.profile.id,
        r.profile.name,
        describe(&r.profile),
        if r.enabled { "enabled" } else { "disabled" },
        r.status.line(r.enabled, now),
        r.status.error.as_ref().map(|e| format!(" · {e}")).unwrap_or_default()
    )
}

impl Tools for VpnTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_vpn_tool(&d.spec.name));
        let profile = |desc: &str| json!({"type": "object", "properties": {"profile": {"type": "string", "description": "The VPN profile's id or name."}}, "required": ["profile"], "description": desc});
        let ask = |name: &str, desc: &str, schema: Value| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk: Risk::Ask, untrusted: false };
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "List the person's VPN profiles with their state (no keys).".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ask(
            "vpn_add",
            "Import a VPN profile from a FILE the person has (WireGuard .conf or OpenVPN .ovpn), by path. The file is read by the tool and its keys go to the vault; send the path, never the text. Starts disabled. The person approves a card.",
            json!({"type": "object", "properties": {"name": {"type": "string", "description": "Short name of the profile."}, "path": {"type": "string", "description": "Path of the person's profile file."}}, "required": ["name", "path"]}),
        ));
        v.push(ask("vpn_enable", "Enable a VPN profile: it connects now and after every sign-in. The person approves a card.", profile("enable")));
        v.push(ask("vpn_disable", "Disable a VPN profile: it disconnects and everything it set up is removed. The profile is kept.", profile("disable")));
        v.push(ask("vpn_delete", "Delete a VPN profile and its keys in the vault.", profile("delete")));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        let now = crate::clock::now_unix().max(0) as u64;
        match name {
            READ_TOOL => {
                let rows = self.mgr.rows();
                if rows.is_empty() {
                    return Ok("No VPN profiles yet. IYAKKI provides no VPN service: the person imports the file their provider or their own server gave them.".into());
                }
                Ok(rows.iter().map(|r| one_line(r, now)).collect::<Vec<_>>().join("\n"))
            }
            "vpn_add" => {
                reject_secrets(args)?;
                let data = self.read(args)?;
                let imp = self.mgr.import(&text(args, "name"), &data)?;
                Ok(format!(
                    "Imported “{}” ({}). It is disabled; enable it with vpn_enable.{}",
                    imp.profile.name,
                    imp.profile.id,
                    imp.warnings.iter().map(|w| format!("\nNote: {w}")).collect::<String>()
                ))
            }
            "vpn_enable" => {
                reject_secrets(args)?;
                let p = self.find(args)?;
                self.mgr.set_enabled(&p.id, true, now)?;
                self.mgr.ensure_supervised();
                let st = self.mgr.status(&p.id);
                Ok(format!("Enabled “{}”: {}.{}", p.name, st.line(true, now), st.error.map(|e| format!(" {e}")).unwrap_or_default()))
            }
            "vpn_disable" => {
                reject_secrets(args)?;
                let p = self.find(args)?;
                self.mgr.set_enabled(&p.id, false, now)?;
                Ok(format!("Disabled “{}”; it is disconnected and kept.", p.name))
            }
            "vpn_delete" => {
                reject_secrets(args)?;
                let p = self.find(args)?;
                Ok(format!("Deleted {}.", self.mgr.remove(&p.id)?.join(", ")))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if WRITE_TOOLS.contains(&name) {
            return match self.card(name, args) {
                Ok(c) => c.lines().next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            };
        }
        if name == READ_TOOL {
            return "List the VPN profiles".into();
        }
        self.inner.describe(name, args)
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).ok();
        }
        if name == READ_TOOL { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).err();
        }
        if name == READ_TOOL { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_vpn_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !WRITE_TOOLS.contains(&name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) { Some("changing VPN profiles asks every time".into()) } else { self.inner.grant_blocker(name, args) }
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }

    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }

    fn begin_step(&self) {
        self.inner.begin_step()
    }

    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }

    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }

    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
}
