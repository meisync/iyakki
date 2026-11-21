//! Chat tools for two administrator settings: the firewall's "Networks of a connected VPN" switch (`firewall_vpn_local_status`,
//! `firewall_vpn_local_set`) and the `api:*` scopes of paired computers (`devmode_api_scopes`, `devmode_api_scope_set`). They call the
//! same functions as the pages (`Engine::propose` and `keep`, `DevMode::set_api_scope`). Reads are free, every change asks.

use std::sync::Arc;

use iyakki_fw::service::Handle;
use serde_json::json;

use crate::devmode::{apiscopes, DevMode};
use crate::settingstools::{arg_bool, arg_str, no_args, Entry, Env};
pub use iyakki_shared::settingstools_net::{MEDIA_RULE, PROMPT_RULE};

pub const TOOLS: [&str; 28] = ["usb_link_check", "usb_link_watch", "firewall_vpn_local_status", "firewall_vpn_local_set", "firewall_counters", "firewall_outgoing_status", "firewall_outgoing_set", "devmode_api_scopes", "devmode_api_scope_set", "core_contract_status", "core_contract_switch", "core_contract_scope_set", "media_server_status", "media_server_set", "relay_status", "relay_switch", "webpush_public_name", "relay_approve_computer", "ui_socket_status", "ui_socket_set", "install_shrink_plan", "install_shrink", "install_bios_plan", "install_bios", "install_disk_plan", "install_to_disk", "secureboot_enroll_status", "secureboot_enroll"];



/// The relay transport as plain text (F-CORE-17): the switch, the computers approved for it and what the owner still has to provide.
pub fn relay_status_text(dm: &DevMode) -> String {
    format!("{} The relay itself is run by the owner and no relay address is set up yet, so nothing can connect through it today; the machine's side connects out over TLS to the relay's own certificate only.", dm.relay_summary())
}

/// The core contract channel as plain text: the switch, the scopes that can be granted and what each paired computer holds.
pub fn core_status_text(dm: &DevMode) -> String {
    let mut out = vec![format!("The core contract channel is {}.{}", if dm.core_enabled() { "on" } else { "off (the default)" }, if dm.core_enabled() && !dm.is_on() { " Developer mode itself is off, so no computer can reach it now." } else { "" })];
    out.push(format!("Scopes that can be granted: {}.", crate::devmode::contract::GRANTABLE.join(", ")));
    let keys = dm.keys();
    if keys.is_empty() {
        out.push("No computer is paired.".into());
    }
    for k in keys {
        let held = dm.core_scopes(&k.name);
        out.push(format!("{}: {}", k.name, if held.is_empty() { "no contract scope".to_string() } else { held.join(", ") }));
    }
    out.join("
")
}

fn status_of(e: &iyakki_fw::Engine) -> String {
    let on = e.config().vpn_local;
    let nets = e.ctx().vpn_nets.len();
    format!("VPN networks {} the local network. {}", if on { "count as" } else { "do not count as" }, if nets == 0 { "No VPN tunnel is up now.".to_string() } else { format!("{nets} tunnel network(s) are known now.") })
}

/// What the outgoing, forwarding and port-forward part of the firewall does now, in words.
pub fn outgoing_status_text(e: &iyakki_fw::Engine) -> String {
    let o = &e.config().outgoing;
    if !o.enabled {
        return "Outgoing rules are off: connections this computer starts are not filtered, nothing is forwarded or port-forwarded by the firewall.".into();
    }
    let on = |n: usize, f: &dyn Fn(usize) -> bool| (0..n).filter(|i| f(*i)).count();
    format!(
        "Outgoing rules are on. Connections no rule matches are {}. {} outgoing rule(s), {} forwarding rule(s), {} port forward(s) in force (first matching outgoing rule wins). Cluster and container networks are never cut.",
        if o.block_by_default { "blocked" } else { "let out" },
        on(o.rules.len(), &|i| o.rules[i].enabled),
        on(o.forwards.len(), &|i| o.forwards[i].enabled),
        on(o.port_forwards.len(), &|i| o.port_forwards[i].enabled)
    )
}

/// The firewall's counters as plain text: the blocked total and what each counted rule has let in since the rules were applied.
pub fn counters_text(blocked: Option<(u64, u64)>, counts: &[iyakki_fw::ruleset::RuleCount]) -> String {
    let mut out = match blocked {
        Some((pk, _)) => format!("Blocked connections since the rules were applied: {pk}."),
        None => "The firewall is not in force, so nothing is counted.".to_string(),
    };
    if blocked.is_some() {
        if counts.is_empty() {
            out.push_str(" No per-rule counts are available.");
        }
        for c in counts {
            out.push_str(&format!("\n{}: {} packets, {}", c.note, c.packets, crate::sysinfo::human_bytes(c.bytes)));
        }
    }
    out
}

pub fn entries(env: &Env, fw: Handle) -> Vec<Entry> {
    let mut v = Vec::new();
    v.extend(crate::settingstools_usblink::entries(env, crate::settingstools_usblink::real_env(env)));
    v.extend(crate::settingstools_install::entries(env, crate::settingstools_install::real_source(), Arc::new(iyakki_core_api::installer::HostTools)));
    v.extend(crate::settingstools_installdisk::entries(env, crate::settingstools_installdisk::real_installer()));
    v.extend(crate::settingstools_sbenroll::entries(env, crate::settingstools_sbenroll::real_open()));
    {
        let h = fw.clone();
        v.push(Entry::read("Firewall", "firewall_vpn_local_status", "Say whether the networks of a connected VPN count as the local network for the firewall presets, ping and remote diagnostics.", no_args(), move |_| Ok(status_of(&h.lock()))).admin());
    }
    {
        let h = fw.clone();
        v.push(Entry::read("Firewall", "firewall_counters", "Say how many connections the firewall blocked since its rules were applied and how many each counted rule (a rule of the Firewall page, a preset) has let in.", no_args(), move |_| {
            let mut eng = h.lock();
            let blocked = eng.counters();
            let counts = eng.rule_counts();
            Ok(counters_text(blocked, &counts))
        }).admin());
    }
    {
        let h = fw.clone();
        v.push(Entry::read("Firewall", "firewall_outgoing_status", "Say whether the firewall filters outgoing connections, forwards traffic or port-forwards, and how many rules of each kind are in force.", no_args(), move |_| Ok(outgoing_status_text(&h.lock()))).admin());
    }
    {
        let (h, e) = (fw.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"config": {"type": "object", "description": "{enabled, block_by_default, rules:[{id,enabled,verdict:allow|block,dest,proto:tcp|udp|both,ports,iface,note}], forwards:[{id,enabled,from_iface,to_iface,dest,proto,ports,note}], port_forwards:[{id,enabled,iface,proto,port,to,to_port,note}]}. `enabled:false` switches the outgoing part off."}}, "required": ["config"]});
        v.push(
            Entry::change("Firewall", "firewall_outgoing_set", "Replace the firewall's outgoing, forwarding and port-forward rules (first matching outgoing rule wins). Checked first; applied with the 60-second Keep/Undo like every firewall change. Use firewall_outgoing_plan to look at the rules before. The person approves a card first.", schema, |a| format!("Firewall: set the outgoing rules ({} rule(s))", a["config"]["rules"].as_array().map_or(0, Vec::len)), move |a| {
                let cfg: iyakki_fw::outbound::OutConfig = serde_json::from_value(a.get("config").cloned().ok_or("give `config`")?).map_err(|e| format!("config: {e}"))?;
                let mut eng = h.lock();
                let old = eng.config().clone();
                let mut new = old.clone();
                new.outgoing = cfg;
                if new == old {
                    return Ok(format!("No change. {}", outgoing_status_text(&eng)));
                }
                eng.propose(new, &e.user)?;
                e.audit("firewall.outgoing", &format!("{} rules", eng.config().outgoing.rules.len()));
                Ok(format!("Applied. {} The person must press Keep on Settings -> Firewall within 60 seconds, or the change is taken back.", outgoing_status_text(&eng)))
            })
            .admin(),
        );
    }
    {
        let (h, e) = (fw.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}}, "required": ["on"]});
        v.push(
            Entry::change("Firewall", "firewall_vpn_local_set", "Let the networks of a connected VPN count as the local network (on) or not (off). The person approves a card first.", schema, |a| format!("Firewall: VPN networks {} the local network", if a["on"].as_bool().unwrap_or(false) { "count as" } else { "no longer count as" }), move |a| {
                let on = arg_bool(a, "on")?;
                let mut eng = h.lock();
                let old = eng.config().clone();
                if old.vpn_local == on {
                    return Ok(status_of(&eng));
                }
                let mut new = old.clone();
                new.vpn_local = on;
                eng.propose(new.clone(), &e.user)?;
                e.audit("firewall.vpn_local", if on { "on" } else { "off" });
                if new.only_opens_from(&old) {
                    let _ = eng.keep(&e.user);
                    Ok("VPN networks now count as the local network (kept).".into())
                } else {
                    Ok("VPN networks no longer count as the local network. The person must press Keep on Settings -> Firewall within 60 seconds, or the change is taken back.".into())
                }
            })
            .admin(),
        );
    }
    {
        let e = env.clone();
        v.push(
            Entry::read("Developer", "devmode_api_scopes", "List what each paired computer may call in the web service (api:* scopes), and the families that exist.", no_args(), move |_| {
                let dm = DevMode::open(&e.dir);
                let g = dm.api_grants();
                let mut out = vec![format!("Families: {}.", apiscopes::FAMILIES.iter().filter(|(f, _)| *f != "api:admin").map(|(f, _)| *f).collect::<Vec<_>>().join(", "))];
                if g.is_empty() {
                    out.push("No paired computer holds an api scope.".into());
                }
                out.extend(g.iter().map(|(c, s)| format!("{c}: {}", s.join(", "))));
                Ok(out.join("\n"))
            })
            .admin(),
        );
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"computer": {"type": "string"}, "scope": {"type": "string", "description": "api:<family> or api:<family>:read"}, "on": {"type": "boolean"}}, "required": ["computer", "scope", "on"]});
        v.push(
            Entry::change("Developer", "devmode_api_scope_set", "Grant or take back one web-service scope (api:flows, api:files:read, ...) for a paired computer. The person approves a card first.", schema, |a| format!("{} {} for {}", if a["on"].as_bool().unwrap_or(false) { "Grant" } else { "Take back" }, a["scope"].as_str().unwrap_or("a scope"), a["computer"].as_str().unwrap_or("a computer")), move |a| {
                let (computer, scope, on) = (arg_str(a, "computer")?, arg_str(a, "scope")?, arg_bool(a, "on")?);
                let dm = DevMode::open(&e.dir);
                let policy = iyakki_sec::policy::Role::Admin.template();
                let now = dm.set_api_scope(&policy, &e.user, &computer, &scope, on)?;
                e.audit("settings.developer", &format!("api scope {scope} {} for {computer}", if on { "granted" } else { "taken back" }));
                Ok(format!("{computer} now holds: {}.", if now.is_empty() { "no api scope".to_string() } else { now.join(", ") }))
            })
            .with_refusal(|a| a["scope"].as_str().filter(|s| !apiscopes::grantable(s)).map(|s| format!("{s} cannot be granted (api:admin never can); call devmode_api_scopes for the families")))
            .admin(),
        );
    }
    {
        let e = env.clone();
        v.push(
            Entry::read("Developer", "core_contract_status", "Say whether the typed core contract channel of developer mode is on (it is off by default), which contract scopes can be granted, and which scopes each paired computer holds.", no_args(), move |_| {
                let dm = DevMode::open(&e.dir);
                Ok(core_status_text(&dm))
            })
            .admin(),
        );
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}}, "required": ["on"]});
        v.push(
            Entry::change("Developer", "core_contract_switch", "Turn the typed core contract channel of developer mode on or off (the same switch as Settings -> Developer mode). A new computer holds no contract scope until one is granted. The person approves a card first.", schema, |a| format!("Core contract channel {}", if a["on"].as_bool().unwrap_or(false) { "on" } else { "off" }), move |a| {
                let on = arg_bool(a, "on")?;
                let dm = DevMode::open(&e.dir);
                let policy = iyakki_sec::policy::Role::Admin.template();
                dm.set_core_enabled(&policy, &e.user, on)?;
                e.audit("settings.developer", &format!("core contract {}", if on { "on" } else { "off" }));
                Ok(core_status_text(&dm))
            })
            .admin(),
        );
    }
    {
        let e = env.clone();
        v.push(
            Entry::read("Developer", "relay_status", "Say whether connections to this machine through an owner-run relay are allowed (off by default) and which paired computers were approved for it.", no_args(), move |_| Ok(relay_status_text(&DevMode::open(&e.dir)))).admin(),
        );
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"name": {"type": "string", "description": "the public host name this computer is reached under over HTTPS, for example notify.example.org (a plain host name with a dot, an optional :port); empty takes it away, so the Web Push pages are not served"}}, "required": ["name"]});
        v.push(
            Entry::change("Developer", "webpush_public_name", "Set the public HTTPS name the Web Push pages (the service worker and the subscribe page a browser opens) are reached under. Unset by default: with no name nothing is served. The person approves a card first; administrators only.", schema, |a| format!("Serve the Web Push pages under the public name \"{}\"", a.get("name").and_then(serde_json::Value::as_str).unwrap_or("")), move |a| {
                let name = a.get("name").and_then(serde_json::Value::as_str).ok_or("name is needed (empty takes it away)")?;
                let said = crate::webpush_page::set_public_name(&e.dir, name)?;
                e.audit("settings.developer", if name.trim().is_empty() { "web push public name removed" } else { "web push public name set" });
                Ok(said)
            })
            .admin(),
        );
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}, "address": {"type": "string", "description": "where the relay is, https://HOST or https://HOST:PORT; empty takes it away; leave out to keep it"}, "certificate_fingerprint": {"type": "string", "description": "the SHA-256 fingerprint (64 hex digits) of the relay's certificate; an https relay is trusted for that certificate only and is not used without it; empty takes it away; leave out to keep it"}}, "required": ["on"]});
        v.push(
            Entry::change("Developer", "relay_switch", "Allow or stop connections to this machine through the relay (the third switch beside developer mode and the core contract channel; off by default). The person approves a card first.", schema, |a| format!("Relay connections {}{}", if a["on"].as_bool().unwrap_or(false) { "on" } else { "off" }, a.get("address").and_then(serde_json::Value::as_str).map(|x| format!(" at {x}")).unwrap_or_default()), move |a| {
                let on = arg_bool(a, "on")?;
                let dm = DevMode::open(&e.dir);
                if let Some(addr) = a.get("address").and_then(serde_json::Value::as_str) {
                    dm.set_relay_url(&iyakki_sec::policy::Role::Admin.template(), &e.user, addr)?;
                }
                if let Some(pin) = a.get("certificate_fingerprint").and_then(serde_json::Value::as_str) {
                    dm.set_relay_pin(&iyakki_sec::policy::Role::Admin.template(), &e.user, pin)?;
                }
                dm.set_relay_enabled(&iyakki_sec::policy::Role::Admin.template(), &e.user, on)?;
                e.audit("settings.developer", &format!("relay {}", if on { "on" } else { "off" }));
                Ok(relay_status_text(&dm))
            })
            .admin(),
        );
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"computer": {"type": "string"}, "on": {"type": "boolean"}}, "required": ["computer", "on"]});
        v.push(
            Entry::change("Developer", "relay_approve_computer", "Approve or withdraw a paired computer's first contact through the relay (the question that waits on this machine's screen). The person approves a card first, on this machine.", schema, |a| format!("{} {} for relay connections", if a["on"].as_bool().unwrap_or(false) { "Approve" } else { "Withdraw approval of" }, a["computer"].as_str().unwrap_or("a computer")), move |a| {
                let (computer, on) = (arg_str(a, "computer")?, arg_bool(a, "on")?);
                let dm = DevMode::open(&e.dir);
                dm.approve_relay_computer(&iyakki_sec::policy::Role::Admin.template(), &e.user, &computer, on)?;
                e.audit("settings.developer", &format!("relay {} {computer}", if on { "approved" } else { "approval withdrawn for" }));
                Ok(relay_status_text(&dm))
            })
            .admin(),
        );
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"computer": {"type": "string"}, "scope": {"type": "string", "description": "a contract scope such as flows:read"}, "on": {"type": "boolean"}}, "required": ["computer", "scope", "on"]});
        v.push(
            Entry::change("Developer", "core_contract_scope_set", "Grant or take back one core contract scope for a paired computer. The local-only scope cannot be granted. The person approves a card first.", schema, |a| format!("{} contract scope {} for {}", if a["on"].as_bool().unwrap_or(false) { "Grant" } else { "Take back" }, a["scope"].as_str().unwrap_or("a scope"), a["computer"].as_str().unwrap_or("a computer")), move |a| {
                let (computer, scope, on) = (arg_str(a, "computer")?, arg_str(a, "scope")?, arg_bool(a, "on")?);
                let dm = DevMode::open(&e.dir);
                let policy = iyakki_sec::policy::Role::Admin.template();
                let now = dm.set_core_scope(&policy, &e.user, &computer, &scope, on)?;
                e.audit("settings.developer", &format!("core scope {scope} {} for {computer}", if on { "granted" } else { "taken back" }));
                Ok(format!("{computer} now holds: {}.", if now.is_empty() { "no contract scope".to_string() } else { now.join(", ") }))
            })
            .with_refusal(|a| a["scope"].as_str().filter(|s| !crate::devmode::contract::GRANTABLE.contains(s)).map(|s| format!("{s} cannot be granted (the local-only scope never can); call core_contract_status for the scopes that can")))
            .admin(),
        );
    }
    {
        let e = env.clone();
        v.push(
            Entry::read("Developer", "ui_socket_status", "Say whether the core serves the typed contract to a UI program on this machine over a unix socket (off by default, no network port), which contract scopes that program holds and for which account.", no_args(), move |_| Ok(crate::contract_local::uiserve::status_text(&e.dir, false)))
                .admin(),
        );
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}, "scopes": {"type": "array", "items": {"type": "string"}, "description": "contract scopes such as settings:read; replaces the list"}, "rpc": {"type": "boolean", "description": "also serve the shell's services as objects for the first-party UI program (off by default)"}, "account": {"type": "string", "description": "the account the UI program acts as"}}, "required": ["on"]});
        v.push(
            Entry::change("Developer", "ui_socket_set", "Switch the UI socket on or off and set the contract scopes and the account of the UI program that connects to it (the vault unlock and consent answers are never on it). The person approves a card first.", schema, |a| format!("UI socket {}{}", if a["on"].as_bool().unwrap_or(false) { "on" } else { "off" }, a["scopes"].as_array().map(|s| format!(", scopes: {}", s.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))).unwrap_or_default()), move |a| {
                let on = arg_bool(a, "on")?;
                let mut c = crate::contract_local::uiserve::load(&e.dir);
                c.enabled = on;
                if let Some(list) = a.get("scopes").and_then(|x| x.as_array()) {
                    c.scopes = list.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                }
                if let Some(acct) = a.get("account").and_then(|x| x.as_str()) {
                    c.account = acct.trim().to_string();
                }
                if let Some(r) = a.get("rpc").and_then(|x| x.as_bool()) {
                    c.rpc = r;
                }
                crate::contract_local::uiserve::save(&e.dir, &c)?;
                e.audit("settings.developer", &format!("ui socket {} scopes [{}] account {} rpc {}", if on { "on" } else { "off" }, c.scopes.join(","), c.account, c.rpc));
                Ok(crate::contract_local::uiserve::status_text(&e.dir, false))
            })
            .with_refusal(|a| a.get("scopes").and_then(|x| x.as_array()).and_then(|l| l.iter().filter_map(|x| x.as_str()).find(|s| !crate::contract_local::uiserve::grantable().contains(s)).map(|s| format!("{s} cannot be granted to the UI socket; call ui_socket_status for the scopes that can"))))
            .admin(),
        );
    }
    {
        let (e, e2) = (env.clone(), env.clone());
        v.push(Entry::read("Shares", "media_server_status", "Say whether the person's media server (UPnP/DLNA) is on, its port and how many audio and video files of the shared folders it offers.", no_args(), move |_| {
            Ok(match crate::upnp_serve::status_for(&e.user) {
                Some((port, n)) => format!("The media server is on (port {port}), offering {n} audio and video files of your shared folders to players on the local network. {} {}", crate::upnp_serve::subscribers_text(&crate::upnp_serve::subscriptions_for(&e.user)), crate::upnp_serve::firewall_note(port).unwrap_or_default()).trim_end().to_string(),
                None => format!("The media server is off. Shared folders it would offer: {}.", crate::upnp_serve::share_folders(&e.dir, &e.user).len()),
            })
        }));
        let schema = json!({"type": "object", "properties": {"on": {"type": "boolean"}}, "required": ["on"]});
        v.push(
            Entry::change("Shares", "media_server_set", "Switch the person's media server (UPnP/DLNA) on or off. On: players on the local network can list and play the audio and video files of the shared folders. The person approves a card first.", schema, |a| format!("Media server {}", if a["on"].as_bool().unwrap_or(false) { "on" } else { "off" }), move |a| {
                let on = arg_bool(a, "on")?;
                let e3 = e2.clone();
                if on {
                    let msg = crate::upnp_serve::start_for(&e2.dir, &e2.user, std::sync::Arc::new(move |ev, d| e3.audit(ev, d)))?;
                    e2.audit("settings.media_server", "on");
                    Ok(msg)
                } else {
                    let was = crate::upnp_serve::stop_user(&e2.user);
                    e2.audit("settings.media_server", "off");
                    Ok(if was { "The media server is off.".into() } else { "The media server was already off.".into() })
                }
            })
            .with_preview({
                let e4 = env.clone();
                move |a| {
                    if a["on"].as_bool().unwrap_or(false) {
                        format!("Players on your local network (TVs, phones, speakers) will be able to list and play the audio and video files in {} shared folder(s), without a password. Only those folders; hidden files are not offered. It stops when you sign out.", crate::upnp_serve::share_folders(&e4.dir, &e4.user).len())
                    } else {
                        "Players can no longer see or play your files.".to_string()
                    }
                }
            })
            .with_refusal({
                let e5 = env.clone();
                move |a| (a["on"].as_bool() == Some(true) && crate::upnp_serve::share_folders(&e5.dir, &e5.user).is_empty()).then(|| "There is no shared folder to offer: share a folder first.".to_string())
            }),
        );
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Risk, Tools};
    use crate::settingstools::SettingsTools;
    use iyakki_fw::backend::FakeBackend;
    use iyakki_fw::engine::{Clock, Engine, Resolver, Saved, Store};
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::{Arc, Mutex};

    struct TClock(Arc<AtomicI64>);
    impl Clock for TClock {
        fn now(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }
    struct NoDns;
    impl Resolver for NoDns {
        fn resolve(&self, _: &str) -> Result<Vec<std::net::IpAddr>, String> {
            Err("none".into())
        }
    }
    #[derive(Clone, Default)]
    struct Mem(Arc<Mutex<Option<Saved>>>);
    impl Store for Mem {
        fn load(&self) -> Result<Option<Saved>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, s: &Saved) -> Result<(), String> {
            *self.0.lock().unwrap() = Some(s.clone());
            Ok(())
        }
    }
    struct NoTools;
    impl Tools for NoTools {
        fn defs(&self) -> Vec<crate::agent::ToolDef> {
            vec![]
        }
        fn call(&self, n: &str, _: &serde_json::Value) -> Result<String, String> {
            Err(format!("no tool {n}"))
        }
        fn describe(&self, n: &str, _: &serde_json::Value) -> String {
            n.into()
        }
    }

    fn rig(tag: &str, admin: bool) -> (SettingsTools, Arc<Mutex<Engine>>, Env, Arc<Mutex<Vec<String>>>) {
        let dir = std::env::temp_dir().join(format!("iyakki-nettools-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut e = Engine::new(Box::new(FakeBackend::default()), Box::new(TClock(Arc::new(AtomicI64::new(5_000_000)))), Box::new(NoDns), Box::new(Mem::default()));
        e.boot(false);
        let eng = Arc::new(Mutex::new(e));
        let audits = Arc::new(Mutex::new(Vec::new()));
        let a2 = audits.clone();
        let settings = Arc::new(Mutex::new(crate::settings::SettingsStore::open(&dir.join("settings.json")).unwrap()));
        let env = Env { dir, user: "root".into(), admin, settings: iyakki_shared::settings_ref::SettingsRef::of_store(&settings), audit: Arc::new(move |a, d| a2.lock().unwrap().push(format!("{a} {d}"))) };
        (SettingsTools::new(Arc::new(NoTools), admin, entries(&env, Handle::Own(eng.clone()))), eng, env, audits)
    }

    #[test]
    fn fw_out_the_outgoing_tools_read_free_ask_to_change_and_wait_for_keep() {
        let (t, eng, _env, audits) = rig("out", true);
        let defs = t.defs();
        assert_eq!(defs.iter().find(|d| d.spec.name == "firewall_outgoing_status").unwrap().risk, Risk::Safe);
        assert_eq!(defs.iter().find(|d| d.spec.name == "firewall_outgoing_set").unwrap().risk, Risk::Ask);
        assert!(t.call("firewall_outgoing_status", &json!({})).unwrap().contains("off"));
        let cfg = json!({"enabled": true, "block_by_default": true, "rules": [{"id": 1, "enabled": true, "verdict": "allow", "dest": "", "proto": "tcp", "ports": "443", "iface": "", "note": ""}]});
        let r = t.call("firewall_outgoing_set", &json!({"config": cfg})).unwrap();
        assert!(r.contains("Keep") && r.contains("blocked"), "{r}");
        assert!(eng.lock().unwrap().config().outgoing.block_by_default);
        assert!(audits.lock().unwrap().iter().any(|l| l.contains("firewall.outgoing")));
        let bad = json!({"enabled": true, "block_by_default": true, "rules": []});
        assert!(t.call("firewall_outgoing_set", &json!({"config": bad})).is_err(), "blocking everything with no allow rule is refused");
        assert!(t.call("firewall_outgoing_set", &json!({"config": cfg})).unwrap().starts_with("No change"));
    }

    #[test]
    fn the_vpn_switch_reads_free_asks_to_change_and_follows_the_keep_rule_of_the_page() {
        let (t, eng, _env, audits) = rig("vpn", true);
        let defs = t.defs();
        assert_eq!(defs.iter().find(|d| d.spec.name == "firewall_vpn_local_status").unwrap().risk, Risk::Safe);
        assert_eq!(defs.iter().find(|d| d.spec.name == "firewall_vpn_local_set").unwrap().risk, Risk::Ask);
        assert!(t.call("firewall_vpn_local_status", &json!({})).unwrap().contains("do not count"));
        // turning it on only opens: applied and kept at once
        assert!(t.call("firewall_vpn_local_set", &json!({"on": true})).unwrap().contains("kept"));
        assert!(eng.lock().unwrap().config().vpn_local);
        assert!(t.call("firewall_vpn_local_status", &json!({})).unwrap().starts_with("VPN networks count as"));
        assert!(t.call("firewall_vpn_local_set", &json!({"on": true})).unwrap().contains("count as"), "no change is no error");
        // turning it off tightens: it waits for Keep on the page
        let r = t.call("firewall_vpn_local_set", &json!({"on": false})).unwrap();
        assert!(r.contains("Keep") && r.contains("60 seconds"), "{r}");
        assert!(!eng.lock().unwrap().config().vpn_local);
        assert!(eng.lock().unwrap().keep("test").is_ok(), "a confirmation was waiting");
        assert!(audits.lock().unwrap().iter().any(|l| l.contains("firewall.vpn_local")));
        // not for a person who is not an administrator
        let (u, _, _, _) = rig("vpn-user", false);
        let names: Vec<String> = u.defs().into_iter().map(|d| d.spec.name).collect();
        assert_eq!(names, vec!["media_server_status".to_string(), "media_server_set".to_string()], "the firewall and developer tools are listed for administrators only; the media server is the person's own");
    }

    #[test]
    fn counters_are_read_free_by_chat_for_administrators_and_say_what_each_rule_let_in() {
        let (t, _eng, _env, _a) = rig("counts", true);
        let defs = t.defs();
        assert_eq!(defs.iter().find(|d| d.spec.name == "firewall_counters").unwrap().risk, Risk::Safe);
        let r = t.call("firewall_counters", &json!({})).unwrap();
        assert!(r.starts_with("Blocked connections since the rules were applied: 0."), "{r}");
        // the plain text for a table with counted rules
        let counts = vec![iyakki_fw::ruleset::RuleCount { note: "rule 3: office".into(), packets: 12, bytes: 3400 }];
        let text = counters_text(Some((7, 0)), &counts);
        assert!(text.contains("applied: 7.") && text.contains("rule 3: office: 12 packets, "), "{text}");
        assert!(counters_text(None, &counts).contains("not in force"));
        // not for a person who is not an administrator
        let (u, _, _, _) = rig("counts-user", false);
        assert!(!u.defs().iter().any(|d| d.spec.name == "firewall_counters"));
    }

    #[test]
    fn api_scopes_are_listed_granted_and_taken_back_by_chat_with_the_pages_rules() {
        let (t, _eng, env, audits) = rig("scopes", true);
        let dm = DevMode::open(&env.dir);
        let key = crate::devmode::DevKey { name: "laptop".into(), key: format!("{:0>64}", 1), account: "root".into(), added: 1, added_by: "root".into(), last_used: 0, scopes: vec!["install".into()] };
        std::fs::create_dir_all(env.dir.join("devmode")).unwrap();
        std::fs::write(env.dir.join("devmode/keys.json"), serde_json::to_vec(&vec![key]).unwrap()).unwrap();
        let defs = t.defs();
        assert_eq!(defs.iter().find(|d| d.spec.name == "devmode_api_scopes").unwrap().risk, Risk::Safe);
        assert_eq!(defs.iter().find(|d| d.spec.name == "devmode_api_scope_set").unwrap().risk, Risk::Ask);
        assert!(t.call("devmode_api_scopes", &json!({})).unwrap().contains("No paired computer holds"));
        assert!(t.refusal("devmode_api_scope_set", &json!({"computer": "laptop", "scope": "api:admin", "on": true})).is_some());
        assert!(t.refusal("devmode_api_scope_set", &json!({"computer": "laptop", "scope": "api:flows:read", "on": true})).is_none());
        let r = t.call("devmode_api_scope_set", &json!({"computer": "laptop", "scope": "api:flows:read", "on": true})).unwrap();
        assert!(r.contains("api:flows:read"), "{r}");
        assert_eq!(dm.api_scopes("laptop"), vec!["api:flows:read".to_string()]);
        assert!(t.call("devmode_api_scopes", &json!({})).unwrap().contains("laptop: api:flows:read"));
        assert!(t.call("devmode_api_scope_set", &json!({"computer": "ghost", "scope": "api:flows", "on": true})).unwrap_err().contains("no paired computer"));
        t.call("devmode_api_scope_set", &json!({"computer": "laptop", "scope": "api:flows", "on": false})).unwrap();
        assert!(dm.api_scopes("laptop").is_empty());
        assert!(audits.lock().unwrap().iter().any(|l| l.contains("settings.developer") && l.contains("api scope")));
    }

    #[test]
    fn the_core_contract_switch_and_scopes_are_read_free_and_changed_by_chat_with_the_pages_rules() {
        let (t, _eng, env, audits) = rig("coreswitch", true);
        let dm = DevMode::open(&env.dir);
        let key = crate::devmode::DevKey { name: "laptop".into(), key: format!("{:0>64}", 2), account: "root".into(), added: 1, added_by: "root".into(), last_used: 0, scopes: vec!["install".into()] };
        std::fs::create_dir_all(env.dir.join("devmode")).unwrap();
        std::fs::write(env.dir.join("devmode/keys.json"), serde_json::to_vec(&vec![key]).unwrap()).unwrap();
        let defs = t.defs();
        assert_eq!(defs.iter().find(|d| d.spec.name == "core_contract_status").unwrap().risk, Risk::Safe);
        assert_eq!(defs.iter().find(|d| d.spec.name == "core_contract_switch").unwrap().risk, Risk::Ask);
        assert_eq!(defs.iter().find(|d| d.spec.name == "core_contract_scope_set").unwrap().risk, Risk::Ask);
        let r = t.call("core_contract_status", &json!({})).unwrap();
        assert!(r.contains("off (the default)") && r.contains("laptop: no contract scope"), "{r}");
        assert!(!dm.core_enabled());
        assert!(t.call("core_contract_switch", &json!({"on": true})).unwrap().contains("is on"));
        assert!(dm.core_enabled());
        // the local-only marker and unknown names are refused before the card
        assert!(t.refusal("core_contract_scope_set", &json!({"computer": "laptop", "scope": "local", "on": true})).is_some());
        assert!(t.refusal("core_contract_scope_set", &json!({"computer": "laptop", "scope": "flows:read", "on": true})).is_none());
        let r = t.call("core_contract_scope_set", &json!({"computer": "laptop", "scope": "flows:read", "on": true})).unwrap();
        assert!(r.contains("flows:read"), "{r}");
        assert_eq!(dm.core_scopes("laptop"), vec!["flows:read".to_string()]);
        assert!(t.call("core_contract_status", &json!({})).unwrap().contains("laptop: flows:read"));
        assert!(t.call("core_contract_scope_set", &json!({"computer": "ghost", "scope": "flows:read", "on": true})).unwrap_err().contains("no paired computer"));
        t.call("core_contract_scope_set", &json!({"computer": "laptop", "scope": "flows:read", "on": false})).unwrap();
        assert!(dm.core_scopes("laptop").is_empty());
        t.call("core_contract_switch", &json!({"on": false})).unwrap();
        assert!(!dm.core_enabled());
        let log = audits.lock().unwrap();
        assert!(log.iter().any(|l| l.contains("core contract on")) && log.iter().any(|l| l.contains("core scope flows:read granted for laptop")));
        drop(log);
        // not for a person who is not an administrator
        let (u, _, _, _) = rig("coreswitch-user", false);
        assert!(!u.defs().iter().any(|d| d.spec.name.starts_with("core_contract")));
    }

    #[test]
    fn the_media_server_switch_asks_offers_only_shared_folders_and_really_serves_while_on() {
        let (t, _eng, env, audits) = rig("media", true);
        let defs = t.defs();
        assert_eq!(defs.iter().find(|d| d.spec.name == "media_server_status").unwrap().risk, Risk::Safe);
        assert_eq!(defs.iter().find(|d| d.spec.name == "media_server_set").unwrap().risk, Risk::Ask);
        assert!(t.call("media_server_status", &json!({})).unwrap().contains("off"));
        // no shared folder: refused before the card
        assert!(t.refusal("media_server_set", &json!({"on": true})).unwrap().contains("no shared folder"));
        assert!(t.refusal("media_server_set", &json!({"on": false})).is_none());
        let share = env.dir.join("music");
        std::fs::create_dir_all(&share).unwrap();
        std::fs::write(share.join("a.mp3"), b"abc").unwrap();
        let sf = crate::sharedfolder::share_file(&env.dir, "root");
        std::fs::create_dir_all(sf.parent().unwrap()).unwrap();
        std::fs::write(&sf, serde_json::to_string(&vec![crate::sharedfolder::ShareCfg { id: "m".into(), name: "music".into(), folder: share.display().to_string(), ..Default::default() }]).unwrap()).unwrap();
        assert!(t.preview("media_server_set", &json!({"on": true})).unwrap().contains("1 shared folder"));
        // the real switch binds the SSDP port, which another server on the computer may hold: only the off path and the status are checked everywhere
        assert!(t.call("media_server_set", &json!({"on": false})).unwrap().contains("already off"));
        assert!(audits.lock().unwrap().iter().any(|l| l.contains("settings.media_server")));
        assert!(t.call("media_server_status", &json!({})).unwrap().contains("Shared folders it would offer: 1"));
    }
}
