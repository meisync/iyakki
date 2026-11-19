//! Chat tools for the Linux apps page (Store, Setup, Linux apps): `linux_sandboxes` (free) lists the person's sandboxes and how each reaches
//! the network, and `linux_network_set` (asks every time) changes that: off, outbound only (with an optional allow-list), or shared with
//! the computer. They call the same `LinuxApps` code as the page and the `linux net` command, with the same check of which hosts the
//! account may reach. Creating, deleting and running things in sandboxes stay with the page and the command line.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::linuxapps::LinuxApps;
use crate::linuxapps_net::{self as net, Mode, NetPolicy};
use iyakki_shared::linuxapps_devices::DevicePolicy;
use crate::settingstools::{arg_str, no_args, Entry, Env};
pub use iyakki_shared::settingstools_linux::{Access, PROMPT_RULE};

const TAB: &str = "LinuxApps";

pub const TOOLS: [&str; 3] = ["linux_sandboxes", "linux_network_set", "linux_devices_set"];



fn manager(env: &Env) -> Result<LinuxApps, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
    Ok(LinuxApps::new(&env.dir, &env.user, &exe, Arc::new(crate::wsexec::System)))
}

fn wanted(a: &Value) -> Result<(String, Mode, Vec<String>, Vec<String>), String> {
    let name = arg_str(a, "name")?;
    let mode = match a.get("mode").and_then(Value::as_str) {
        Some("off") => Mode::Off,
        Some("outbound_only") => Mode::OutboundOnly,
        Some("host_shared") => Mode::HostShared,
        _ => return Err("mode is one of: off, outbound_only, host_shared".into()),
    };
    let list = |k: &str| -> Vec<String> { a.get(k).and_then(Value::as_array).map(|v| v.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect()).unwrap_or_default() };
    Ok((name, mode, list("allow_list"), list("dns")))
}

fn check(env: &Env, acc: &Access, a: &Value) -> Result<(String, Mode, Vec<String>, Vec<String>), String> {
    if !acc.allowed {
        return Err("your role may not use Linux apps".into());
    }
    let (name, mode, allow, dns) = wanted(a)?;
    manager(env)?.get(&name)?;
    let p = NetPolicy { mode, allow: if mode == Mode::OutboundOnly { NetPolicy::parse_allow(&allow)? } else { vec![] }, ..Default::default() };
    net::host_gate(&p, &acc.net_hosts)?;
    Ok((name, mode, allow, dns))
}

pub fn entries(env: &Env, acc: Access) -> Vec<Entry> {
    let mut v = Vec::new();
    let acc2 = acc.clone();
    {
        let (e, a) = (env.clone(), acc.clone());
        v.push(Entry::read(TAB, "linux_sandboxes", "List the person's Linux sandboxes with their release and how each reaches the network.", no_args(), move |_| {
            if !a.allowed {
                return Err("your role may not use Linux apps".into());
            }
            let la = manager(&e)?;
            let list = la.list();
            if list.is_empty() {
                return Ok("There are no Linux sandboxes. They are made on the Linux apps page.".into());
            }
            Ok(list.iter().map(|s| format!("{} ({}): {}. {}", s.name, s.title(), net::describe(&la.net_policy(&s.name).unwrap_or_default()).join("; "), la.device_policy(&s.name).unwrap_or_default().describe())).collect::<Vec<_>>().join("\n"))
        }));
    }
    {
        let (e, e2, e3, a, a2, a3) = (env.clone(), env.clone(), env.clone(), acc.clone(), acc.clone(), acc);
        let schema = json!({"type": "object", "required": ["name", "mode"], "properties": {
            "name": {"type": "string", "description": "The sandbox's name."},
            "mode": {"type": "string", "enum": ["off", "outbound_only", "host_shared"]},
            "allow_list": {"type": "array", "items": {"type": "string"}, "description": "outbound_only: the only places it may reach (host:port, address/prefix). Empty means any public address."},
            "dns": {"type": "array", "items": {"type": "string"}, "description": "outbound_only: name server addresses, used when the computer's own resolver is local."}}});
        v.push(
            Entry::change(TAB, "linux_network_set", "Change how one Linux sandbox reaches the network. The person approves a card first.", schema, |a| format!("Change the network of Linux sandbox {}", a.get("name").and_then(Value::as_str).unwrap_or("?")), move |args| {
                let (name, mode, allow, dns) = check(&e, &a, args)?;
                let p = manager(&e)?.set_net_policy(&name, mode, &allow, &dns)?;
                e.audit("linuxapps.network", &format!("{name} {}", p.mode.id()));
                Ok(format!("{name}: {}", net::describe(&p).join("; ")))
            })
            .with_preview(move |args| match check(&e2, &a2, args) {
                Ok((name, mode, allow, _)) => {
                    let p = NetPolicy { mode, allow: NetPolicy::parse_allow(&allow).unwrap_or_default(), ..Default::default() };
                    format!("{name} will be: {}", net::describe(&p).join("; "))
                }
                Err(m) => format!("Cannot change it: {m}"),
            })
            .with_refusal(move |args| check(&e3, &a3, args).err()),
        );
    }
    {
        let (e, e2, a, a2) = (env.clone(), env.clone(), acc2.clone(), acc2);
        let schema = json!({"type": "object", "required": ["name"], "properties": {
            "name": {"type": "string", "description": "The sandbox's name."},
            "sound": {"type": "boolean", "description": "Give the sandbox sound output (playback only, no microphone). A program that opens the sound card holds it for itself."},
            "graphics": {"type": "boolean", "description": "Give the sandbox the render nodes of the graphics card (drawing and compute, no control of the screen)."}}});
        let parse = |env: &Env, acc: &Access, a: &Value| -> Result<(String, DevicePolicy), String> {
            if !acc.allowed {
                return Err("your role may not use Linux apps".into());
            }
            let name = arg_str(a, "name")?;
            manager(env)?.get(&name)?;
            let flag = |k: &str| -> Result<bool, String> { a.get(k).map_or(Ok(false), |v| v.as_bool().ok_or_else(|| format!("{k} is true or false"))) };
            Ok((name, DevicePolicy { sound: flag("sound")?, gpu: flag("graphics")? }))
        };
        v.push(
            Entry::change(TAB, "linux_devices_set", "Choose whether one Linux sandbox may use sound output and the graphics card's render nodes (both off unless set). The person approves a card first.", schema, |a| format!("Change the sound and graphics hardware of Linux sandbox {}", a.get("name").and_then(Value::as_str).unwrap_or("?")), move |args| {
                let (name, p) = parse(&e, &a, args)?;
                manager(&e)?.set_devices(&name, p)?;
                e.audit("linuxapps.devices", &format!("{name} sound={} graphics={}", p.sound, p.gpu));
                Ok(format!("{name}: {}", p.describe()))
            })
            .with_preview(move |args| match parse(&e2, &a2, args) {
                Ok((name, p)) => format!("{name} will have: {}", p.describe()),
                Err(m) => format!("Cannot change it: {m}"),
            }),
        );
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Tools;
    use crate::settingstools_tests::{assert_policy, rig, tools};

    fn sandbox(dir: &std::path::Path) {
        let sdir = dir.join("linuxapps/ada/dev");
        std::fs::create_dir_all(sdir.join("rootfs")).unwrap();
        let s = crate::linuxapps::Sandbox { name: "dev".into(), owner: "ada".into(), release: "debian-12".into(), created: 1, network: false, mounts: vec![], id_base: 0, mem_mb: 2048, cpu_pct: 0, pids: 1024, verified: "x".into(), source: "x".into(), arch: "amd64".into() };
        std::fs::write(sdir.join("sandbox.json"), serde_json::to_string(&s).unwrap()).unwrap();
    }

    #[test]
    fn the_tools_list_and_change_the_network_ask_every_time_and_check_the_hosts_the_account_may_reach() {
        let r = rig("linux", false);
        sandbox(&r.dir);
        let some = Access { allowed: true, net_hosts: vec!["pkg.example.org".into()] };
        let t = tools(&r, entries(&r.env, some));
        assert_policy(&t, &["linux_sandboxes"], &["linux_network_set"]);
        assert!(t.call("linux_sandboxes", &json!({})).unwrap().contains("dev (Debian 12"));
        assert!(t.call("linux_sandboxes", &json!({})).unwrap().contains("Off: no network"));
        // this account may reach one host: shared and open outbound are refused before the person is asked
        for bad in [json!({"name": "dev", "mode": "host_shared"}), json!({"name": "dev", "mode": "outbound_only"}), json!({"name": "dev", "mode": "outbound_only", "allow_list": ["evil.example.net"]}), json!({"name": "dev", "mode": "outbound_only", "allow_list": ["203.0.113.0/24"]}), json!({"name": "nope", "mode": "off"}), json!({"name": "dev", "mode": "wide_open"})] {
            assert!(t.refusal("linux_network_set", &bad).is_some(), "{bad}");
        }
        let ok = json!({"name": "dev", "mode": "outbound_only", "allow_list": ["pkg.example.org:443"], "dns": ["9.9.9.9"]});
        assert!(t.refusal("linux_network_set", &ok).is_none());
        assert!(t.preview("linux_network_set", &ok).unwrap().contains("Allowed only: pkg.example.org:443"));
        assert!(t.call("linux_network_set", &ok).unwrap().contains("outbound only"));
        assert!(t.call("linux_sandboxes", &json!({})).unwrap().contains("Allowed only: pkg.example.org:443"));
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("linuxapps.network dev outbound_only")));
        t.call("linux_network_set", &json!({"name": "dev", "mode": "off"})).unwrap();
        assert!(t.call("linux_sandboxes", &json!({})).unwrap().contains("Off: no network"));
        // an account allowed every host may share the computer's network; one without the Linux apps permission may do nothing
        let all = tools(&r, entries(&r.env, Access { allowed: true, net_hosts: vec!["*".into()] }));
        assert!(all.refusal("linux_network_set", &json!({"name": "dev", "mode": "host_shared"})).is_none());
        let none = tools(&r, entries(&r.env, Access { allowed: false, net_hosts: vec!["*".into()] }));
        assert!(none.call("linux_sandboxes", &json!({})).is_err());
        assert!(none.refusal("linux_network_set", &json!({"name": "dev", "mode": "off"})).is_some());
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn the_devices_tool_asks_every_time_changes_only_that_sandbox_and_the_listing_says_what_is_on() {
        let r = rig("linuxdev", false);
        sandbox(&r.dir);
        let t = tools(&r, entries(&r.env, Access { allowed: true, net_hosts: vec![] }));
        assert_policy(&t, &["linux_sandboxes"], &["linux_network_set", "linux_devices_set"]);
        assert!(t.call("linux_sandboxes", &json!({})).unwrap().contains("No sound or graphics hardware"));
        assert!(t.refusal("linux_devices_set", &json!({"name": "nope", "sound": true})).is_none(), "the card shows the reason; the call itself fails");
        assert!(t.call("linux_devices_set", &json!({"name": "nope", "sound": true})).is_err());
        assert!(t.call("linux_devices_set", &json!({"name": "dev", "sound": "yes"})).is_err());
        assert!(t.preview("linux_devices_set", &json!({"name": "dev", "graphics": true})).unwrap().contains("Graphics hardware"));
        let out = t.call("linux_devices_set", &json!({"name": "dev", "sound": true})).unwrap();
        assert!(out.contains("Sound output"), "{out}");
        assert!(t.call("linux_sandboxes", &json!({})).unwrap().contains("Sound output"));
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("linuxapps.devices dev sound=true graphics=false")));
        t.call("linux_devices_set", &json!({"name": "dev"})).unwrap();
        assert!(t.call("linux_sandboxes", &json!({})).unwrap().contains("No sound or graphics hardware"));
        let none = tools(&r, entries(&r.env, Access { allowed: false, net_hosts: vec![] }));
        assert!(none.call("linux_devices_set", &json!({"name": "dev", "sound": true})).is_err());
        let _ = std::fs::remove_dir_all(&r.dir);
    }
}
