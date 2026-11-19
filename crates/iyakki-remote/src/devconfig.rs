//! The settings of the device that live outside `settings.json`, for `iyakki dev config`: the host name (saved for the next boots in
//! `system.conf`, like Settings → System), the display size of the account (Settings → Display), and the firewall (the same engine
//! and the same 60-second keep-or-revert as Settings → Firewall, so a rule that cuts the computer off undoes itself).

use std::path::PathBuf;

use iyakki_agent::devmode::config::{ConfigExt, Ty};
use iyakki_fw::model::{Kind, Proto, Rule};
use iyakki_fw::service::Handle;
use serde_json::{json, Value};

const SIZES: &[&str] = &["Small", "Normal", "Large", "Larger", "Huge"];

/// What the device side of `config` talks to: the firewall engine, and where the running host name is applied (None: only saved).
#[derive(Clone)]
pub struct Options {
    pub fw: Handle,
    pub etc_hostname: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Options {
        Options { fw: Handle::Global, etc_hostname: Some("/etc/hostname".into()) }
    }
}

pub struct ShellConfig {
    data: PathBuf,
    hostname: String,
    fw: Handle,
    etc_hostname: Option<PathBuf>,
}

impl ShellConfig {
    pub fn new(data: PathBuf, hostname: String, o: Options) -> ShellConfig {
        ShellConfig { data, hostname, fw: o.fw, etc_hostname: o.etc_hostname }
    }

    fn conf(&self) -> PathBuf {
        self.data.join("system.conf")
    }
}

fn rule_json(r: &Rule) -> Value {
    json!({ "id": r.id, "enabled": r.enabled, "value": r.value, "proto": format!("{:?}", r.proto).to_lowercase(), "ports": r.ports, "comment": r.comment, "expires": r.expires })
}

fn kind_of(v: &str) -> Kind {
    if v.parse::<std::net::IpAddr>().is_ok() {
        Kind::Ip
    } else if v.contains('/') {
        Kind::Cidr
    } else if v.contains('-') && v.split('-').all(|p| p.parse::<std::net::IpAddr>().is_ok()) {
        Kind::Range
    } else {
        Kind::Host
    }
}

impl ConfigExt for ShellConfig {
    fn defs(&self) -> Vec<(String, Ty, String, bool)> {
        vec![
            ("system.hostname".into(), Ty::Text(63), "host name (lower-case letters, digits, -); saved for the next boots and applied now".into(), true),
            ("display.size".into(), Ty::Enum(SIZES), "size of text and buttons of this account".into(), false),
            ("firewall.enabled".into(), Ty::Bool, "the firewall (applies with a 60-second keep-or-revert)".into(), true),
        ]
    }

    fn get(&self, key: &str, account: &str) -> Option<Result<Value, String>> {
        Some(Ok(match key {
            "system.hostname" => json!(iyakki_shell::hostname::saved(&self.conf()).unwrap_or_else(|| iyakki_shell::hostname::current(&self.hostname))),
            "display.size" => json!(iyakki_shell::uisize::LEVELS[iyakki_shell::uisize::load(&self.data, account)].0),
            "firewall.enabled" => json!(self.fw.lock().config().enabled),
            _ => return None,
        }))
    }

    fn set(&self, key: &str, v: &Value, account: &str, who: &str) -> Option<Result<String, String>> {
        Some(match key {
            "system.hostname" => (|| {
                let name = iyakki_shell::hostname::check(v.as_str().unwrap_or(""))?;
                iyakki_shell::hostname::save(&self.conf(), &name)?;
                let applied = self.etc_hostname.as_ref().map(|p| iyakki_shell::hostname::apply(&name, p));
                Ok(format!("system.hostname = {name} (saved for the next boots{})", match applied { Some(Ok(())) => ", applied now", Some(Err(_)) => "; could not be applied to the running system", None => "" }))
            })(),
            "display.size" => (|| {
                let s = v.as_str().unwrap_or("");
                let i = iyakki_shell::uisize::LEVELS.iter().position(|(n, _)| *n == s).ok_or("unknown size")?;
                iyakki_shell::uisize::save(&self.data, account, i);
                Ok(format!("display.size = {s} (applies when {account} signs in again)"))
            })(),
            "firewall.enabled" => (|| {
                let on = v.as_bool().ok_or("expected true or false")?;
                let mut e = self.fw.lock();
                let mut cfg = e.config().clone();
                cfg.enabled = on;
                e.propose(cfg, who)?;
                Ok(format!("firewall.enabled = {on}{}", if on { "; send `config firewall keep` within 60 seconds or it reverts" } else { "" }))
            })(),
            _ => return None,
        })
    }

    fn call(&self, action: &str, args: &Value, who: &str) -> Option<Result<Value, String>> {
        if !action.starts_with("firewall.") {
            return None;
        }
        let mut e = self.fw.lock();
        Some((|| match action {
            "firewall.list" => {
                let cfg = e.config().clone();
                Ok(json!({ "enabled": cfg.enabled, "rules": cfg.rules.iter().map(rule_json).collect::<Vec<_>>(), "status": e.status().headline(), "message": "firewall rules" }))
            }
            "firewall.allow" => {
                let value = args["value"].as_str().unwrap_or("").trim().to_string();
                if value.is_empty() {
                    return Err("value: an address, range, network (CIDR) or host name that may connect".into());
                }
                let mut r = Rule::new(kind_of(&value), &value);
                r.proto = match args["proto"].as_str().unwrap_or("both") {
                    "tcp" => Proto::Tcp,
                    "udp" => Proto::Udp,
                    "both" => Proto::Both,
                    _ => return Err("proto: tcp, udp or both".into()),
                };
                r.ports = args["ports"].as_str().unwrap_or("").trim().to_string();
                r.comment = format!("dev: {}", args["comment"].as_str().unwrap_or("").chars().filter(|c| !c.is_control()).take(60).collect::<String>());
                if let Some(m) = args["expires_minutes"].as_i64().filter(|m| *m > 0) {
                    r.expires = iyakki_agent::clock::now_unix() + m.min(60 * 24 * 30) * 60;
                }
                let mut cfg = e.config().clone();
                r.id = cfg.next_id;
                cfg.next_id += 1;
                cfg.rules.push(r.clone());
                e.propose(cfg, who)?;
                Ok(json!({ "id": r.id, "pending_keep": true, "message": format!("rule {} added; it reverts in 60 seconds unless kept", r.id) }))
            }
            "firewall.remove" => {
                let id = args["id"].as_u64().ok_or("id: the number of the rule")? as u32;
                let mut cfg = e.config().clone();
                let before = cfg.rules.len();
                cfg.rules.retain(|r| r.id != id);
                if cfg.rules.len() == before {
                    return Err(format!("no rule number {id}"));
                }
                e.propose(cfg, who)?;
                Ok(json!({ "removed": id, "pending_keep": true, "message": format!("rule {id} removed; it reverts in 60 seconds unless kept") }))
            }
            "firewall.keep" => e.keep(who).map(|_| json!({ "message": "the firewall settings are kept" })),
            "firewall.revert" => e.revert(&format!("{who} asked")).map(|_| json!({ "message": "the previous firewall settings are back" })),
            other => Err(format!("{other} is not a firewall action (list, allow, remove, keep, revert)")),
        })())
    }
}
