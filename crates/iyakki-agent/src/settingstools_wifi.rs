//! Chat tools for Settings -> Network: WiFi (`wifi_status`, `wifi_scan`, `wifi_join`, `wifi_forget`) and the WiFi hotspot's country,
//! security and band (`hotspot_status`, `hotspot_set`). Reads are free; every change asks the person on a card, every time, no session
//! grant. They call the same code as the page: `iyakki_core_local::wifijoin::join` for joining, the hotspot file functions of
//! `sharing::real` for the hotspot. The assistant never types a secret: a passphrase, an enterprise password or a WEP key is read from
//! the person's vault entry named in `secret_entry` (made with `vault_request`), and nothing secret is logged or shown on the card.
// SPDX-License-Identifier: Apache-2.0


use iyakki_core_local::wifijoin::{self, Ap, EapMethod, Enterprise, Known, Security};
use serde_json::{json, Value};

use crate::settingstools::{arg_str, no_args, Entry, Env};
pub use iyakki_shared::settingstools_wifi::{PROMPT_RULE, SecretReader, WifiCtl};

const TAB: &str = "Network";

pub const TOOLS: [&str; 9] = ["wifi_status", "wifi_scan", "wifi_join", "wifi_forget", "wifi_adapter_set", "hotspot_status", "hotspot_set", "internet_share_status", "internet_share_set"];

/// How long a start or stop waits for the sharing service to answer.
#[cfg(not(test))]
const SHARE_WAIT_MS: u64 = 6000;
#[cfg(test)]
const SHARE_WAIT_MS: u64 = 0;

/// What internet sharing is doing and what can be shared through (the Settings page's lists, in words).
pub fn share_status_text(b: &dyn crate::sharing::control::Backend) -> String {
    let wifi = b.wifi_ap();
    let c = crate::sharing::view::choices_full(&b.interfaces(), wifi, if wifi { None } else { b.wifi_ap_why() }.as_deref(), b.nap_why().as_deref());
    let mut v = crate::sharing::cli::status_text(&b.status());
    v.push("internet can come from: ".to_string() + &if c.uplinks.is_empty() { "nothing (no connection has a route to the internet)".to_string() } else { c.uplinks.iter().map(|u| u.label.clone()).collect::<Vec<_>>().join("; ") });
    for d in &c.downs {
        v.push(format!("share through: {}{}", d.label, if d.usable { String::new() } else { format!(" (cannot be used: {})", d.why) }));
    }
    v.join("\n")
}

/// Start or stop internet sharing like `iyakki share start|stop`: `through` is `bluetooth` (phones connect to this computer), `wifi`
/// (the hotspot) or the name of a cable's interface. The same plan and policy checks run first, so a mistake is explained before anything is written.
pub fn share_apply(b: &dyn crate::sharing::control::Backend, a: &Value, wait_ms: u64) -> Result<String, String> {
    use crate::sharing::cli::{run, Cmd};
    let action = arg_str(a, "action")?;
    let cmd = match action.as_str() {
        "stop" => Cmd::Stop,
        "start" => {
            let via = match opt(a, "through")?.as_deref() {
                None => None,
                Some("bluetooth") => Some(wifijoin::BT_NAP_IFACE.to_string()),
                Some("wifi") => Some(crate::wifi_ap::AP_IFACE.to_string()),
                Some(n) => Some(n.to_string()),
            };
            if via.as_deref() == Some(wifijoin::BT_NAP_IFACE) {
                if let Some(why) = b.nap_why() {
                    return Err(why);
                }
            }
            let hours = match a.get("hours") {
                None | Some(Value::Null) => crate::sharing::DEFAULT_IDLE_HOURS,
                Some(h) => h.as_u64().filter(|h| (1..=u64::from(crate::sharing::MAX_IDLE_HOURS)).contains(h)).ok_or("`hours` must be a whole number from 1 to 72")? as u8,
            };
            Cmd::Start { uplink: opt(a, "uplink")?, via, hours, isolate: a.get("isolate").and_then(Value::as_bool).unwrap_or(true), allow_lan: a.get("allow_lan").and_then(Value::as_bool).unwrap_or(false) }
        }
        _ => return Err("`action` is start or stop".into()),
    };
    let mut lines = Vec::new();
    let code = run(b, cmd, &mut |l| lines.push(l.to_string()), wait_ms);
    let text = lines.join("\n");
    match code {
        0 => Ok(text),
        // the request is written; the service runs inside the system and answers a moment later
        1 if text.contains("did not answer yet") => Ok(text),
        _ => Err(text),
    }
}





fn kind_of(k: &Known) -> &'static str {
    if k.enterprise.is_some() {
        "enterprise"
    } else if k.wep_key.is_some() {
        "WEP"
    } else if k.passphrase.is_some() {
        "WPA3"
    } else if k.psk.is_some() {
        "WPA/WPA2"
    } else {
        "open"
    }
}

fn status_text(c: &WifiCtl) -> String {
    let mut out = Vec::new();
    match c.iface() {
        Some(i) => {
            out.push(format!("WiFi adapter: {i}{}.", c.wifi.current_ssid(&i).map(|s| format!(", joined to {s}")).unwrap_or_else(|| ", not joined".into())));
            let all = c.adapters();
            if all.len() > 1 {
                out.push(format!("Adapters that can join networks: {} (wifi_adapter_set chooses).", all.join(", ")));
            }
        }
        None => out.push(c.no_adapter()),
    }
    let k = c.known.list();
    if k.is_empty() {
        out.push("No saved networks.".into());
    }
    for (n, x) in k.iter().enumerate() {
        out.push(format!("{}. {} ({}{})", n + 1, x.ssid, kind_of(x), if x.hidden { ", hidden" } else { "" }));
    }
    out.join("\n")
}

fn scan_text(aps: &[Ap], known: &[Known]) -> String {
    if aps.is_empty() {
        return "No networks found in range.".into();
    }
    aps.iter()
        .map(|a| format!("{} ({}, {}, {} of 4 bars){}{}", a.ssid, a.security.label(), a.band(), a.bars(), if a.connected { ", connected" } else { "" }, if known.iter().any(|k| k.ssid == a.ssid) { ", saved" } else { "" }))
        .collect::<Vec<_>>()
        .join("\n")
}

fn opt(a: &Value, k: &str) -> Result<Option<String>, String> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_str().map(|s| Some(s.trim().to_string()).filter(|s| !s.is_empty())).ok_or_else(|| format!("`{k}` must be text")),
    }
}

fn security_of(s: &str) -> Result<Security, String> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "open" => Security::Open,
        "wep" => Security::Wep,
        "wpa" | "wpa2" => Security::Wpa2,
        "wpa3" => Security::Wpa3,
        "enterprise" => Security::Enterprise,
        _ => return Err("security is open, wpa2, wpa3, wep or enterprise".into()),
    })
}

/// The entry to keep for the arguments. Secrets come from `read`; nothing is joined here.
fn build_known(c: &WifiCtl, read: &SecretReader, a: &Value, now: i64) -> Result<Known, String> {
    let ssid = arg_str(a, "ssid")?;
    let hidden = a.get("hidden").and_then(Value::as_bool).unwrap_or(false);
    let wanted = opt(a, "security")?;
    let secret_entry = opt(a, "secret_entry")?;
    if wanted.is_none() && secret_entry.is_none() {
        if let Some(k) = c.known.list().into_iter().find(|k| k.ssid == ssid) {
            return Ok(Known { added: now, ..k });
        }
    }
    let security = match wanted {
        Some(s) => security_of(&s)?,
        None => {
            let iface = c.iface().ok_or_else(|| c.no_adapter())?;
            let aps = c.wifi.scan(&iface)?;
            match aps.iter().find(|x| x.ssid == ssid) {
                Some(ap) => ap.security,
                None if hidden => Security::Wpa2,
                None => return Err(format!("{ssid} is not in range. Call wifi_scan, or pass security (and hidden true for a hidden network).")),
            }
        }
    };
    let secret = || -> Result<String, String> {
        let e = secret_entry.clone().ok_or("this network needs a secret: call vault_request for it and pass its entry name as secret_entry")?;
        read(&e)
    };
    match security {
        Security::Open => Known::new(&ssid, "", Security::Open, hidden, now),
        Security::Wep => {
            if a.get("accept_wep_risk").and_then(Value::as_bool) != Some(true) {
                return Err(format!("{} Pass accept_wep_risk true only after the person has read this.", wifijoin::wep_warning()));
            }
            Known::new_wep(&ssid, &secret()?, hidden, now)
        }
        Security::Enterprise => {
            let method = match opt(a, "method")?.as_deref().map(str::to_ascii_uppercase).as_deref() {
                None | Some("PEAP") => EapMethod::Peap,
                Some("TTLS") => EapMethod::Ttls,
                Some("TLS") => EapMethod::Tls,
                Some(_) => return Err("method is PEAP, TTLS or TLS".into()),
            };
            let e = Enterprise {
                method,
                identity: opt(a, "identity")?.ok_or("an enterprise network needs identity (the user name)")?,
                anonymous_identity: opt(a, "outer_name")?,
                password: if method == EapMethod::Tls { None } else { Some(secret()?) },
                inner: None,
                ca_cert: opt(a, "ca_cert")?.unwrap_or_default(),
                server_name_match: opt(a, "server_name")?,
                client_cert: opt(a, "client_cert")?,
                private_key: opt(a, "private_key")?,
                private_key_password: None,
            };
            Known::new_enterprise(&ssid, hidden, e, now)
        }
        s => Known::new(&ssid, &secret()?, s, hidden, now),
    }
}

fn hotspot_text(dir: &std::path::Path) -> Result<String, String> {
    let (ssid, _) = crate::sharing::real::hotspot_credentials(dir)?;
    let country = crate::sharing::real::hotspot_country(dir);
    Ok(format!(
        "Hotspot name: {ssid} (the password is not shown here). Band: {}. Country: {}. Security: {}.",
        crate::sharing::real::hotspot_band(dir),
        if country.is_empty() { "none chosen (the card's own rules)".to_string() } else { country },
        match crate::sharing::real::hotspot_security(dir).as_str() {
            "transition" => "WPA2 and WPA3 together (a card that cannot do WPA3 itself keeps WPA2)",
            "wpa3" => "WPA3 only (a card that cannot do WPA3 itself keeps WPA2)",
            _ => "WPA2",
        }
    ))
}

pub fn entries(env: &Env, c: WifiCtl, read: SecretReader) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let c = c.clone();
        v.push(Entry::read(TAB, "wifi_status", "List the WiFi adapter, the network it is joined to and the saved networks (never a password).", no_args(), move |_| Ok(status_text(&c))).admin());
    }
    {
        let (c, e) = (c.clone(), env.clone());
        v.push(
            Entry::read(TAB, "wifi_scan", "Scan for WiFi networks in range: name, security, band, signal bars, which is connected and which are saved.", no_args(), move |_| {
                let iface = c.iface().ok_or_else(|| c.no_adapter())?;
                e.audit("settings.wifi.scan", &iface);
                let aps = c.wifi.scan(&iface)?;
                Ok(scan_text(&aps, &c.known.list()))
            })
            .admin(),
        );
    }
    {
        let (c, e, rd) = (c.clone(), env.clone(), read);
        let schema = json!({"type": "object", "properties": {
            "ssid": {"type": "string"},
            "security": {"type": "string", "description": "open, wpa2, wpa3, wep or enterprise; leave out to use what the scan shows (or the saved entry)"},
            "hidden": {"type": "boolean"},
            "secret_entry": {"type": "string", "description": "name of the vault entry holding the passphrase, password or WEP key (made with vault_request); never the secret itself"},
            "accept_wep_risk": {"type": "boolean"},
            "method": {"type": "string", "description": "enterprise: PEAP (default), TTLS or TLS"},
            "identity": {"type": "string"}, "outer_name": {"type": "string"},
            "ca_cert": {"type": "string", "description": "enterprise: full path of the certificate authority file"},
            "server_name": {"type": "string"}, "client_cert": {"type": "string"}, "private_key": {"type": "string"}
        }, "required": ["ssid"]});
        let c2 = c.clone();
        v.push(
            Entry::change(TAB, "wifi_join", "Join a WiFi network and remember it (open, WPA2, WPA3, WEP or enterprise). Secrets come from a vault entry, never from the chat. The person approves a card first.", schema, |a| format!("Join the WiFi network {}", a["ssid"].as_str().unwrap_or("?")), move |a| {
                let now = crate::clock::now_unix();
                let k = build_known(&c, &rd, a, now)?;
                let iface = c.iface().ok_or_else(|| c.no_adapter())?;
                e.audit("settings.wifi.join", &k.ssid);
                wifijoin::join(&c.wifi, &c.known, &c.mac, &iface, k)
            })
            .with_preview(move |a| {
                let ssid = a["ssid"].as_str().unwrap_or("?");
                let sec = a["security"].as_str().unwrap_or("").to_ascii_lowercase();
                let mut s = format!("This computer will join {ssid} and remember it (tried first from now on, also at the next start).");
                if sec == "wep" {
                    s.push(' ');
                    s.push_str(wifijoin::wep_warning());
                }
                if sec == "enterprise" {
                    s.push_str(" The login is kept in an owner-only file of this computer because the network must come up before anyone signs in.");
                }
                let _ = &c2;
                s
            })
            .admin(),
        );
    }
    {
        let (c, e) = (c.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"ssid": {"type": "string"}}, "required": ["ssid"]});
        v.push(
            Entry::change(TAB, "wifi_forget", "Remove a saved WiFi network (it is not joined again). The person approves a card first.", schema, |a| format!("Forget the WiFi network {}", a["ssid"].as_str().unwrap_or("?")), move |a| {
                let ssid = arg_str(a, "ssid")?;
                if !c.known.forget(&ssid)? {
                    return Err(format!("{ssid} is not a saved network"));
                }
                e.audit("settings.wifi.forget", &ssid);
                if let Some(i) = c.iface() {
                    let _ = c.wifi.apply(&i, &c.known.list());
                }
                Ok(format!("Forgot {ssid}."))
            })
            .admin(),
        );
    }
    {
        let (c, e) = (c.clone(), env.clone());
        let schema = json!({"type": "object", "properties": {"adapter": {"type": "string", "description": "the interface name from wifi_status, for example wlan1"}}, "required": ["adapter"]});
        v.push(
            Entry::change(TAB, "wifi_adapter_set", "Choose which WiFi adapter scans and joins when the computer has two or more. The person approves a card first.", schema, |a| format!("Use the WiFi adapter {}", a["adapter"].as_str().unwrap_or("?")), move |a| {
                let name = arg_str(a, "adapter")?;
                if !c.adapters().contains(&name) {
                    return Err(format!("{name} is not a WiFi adapter that can join networks here ({})", c.adapters().join(", ")));
                }
                c.known.set_chosen_adapter(Some(&name))?;
                e.audit("settings.wifi.adapter", &name);
                Ok(format!("WiFi adapter {name} is in use now."))
            })
            .admin(),
        );
    }
    {
        let e = env.clone();
        v.push(Entry::read(TAB, "internet_share_status", "Say whether this computer is sharing its internet with other devices (a cable, the WiFi hotspot or phones over Bluetooth), what it can share through and why something cannot be used.", no_args(), move |_| Ok(share_status_text(&crate::sharing::control::FileBackend { data: e.dir.clone() }))).admin());
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {
            "action": {"type": "string", "description": "start or stop"},
            "through": {"type": "string", "description": "start only: bluetooth (phones connect to this computer), wifi (the hotspot) or the name of a cable's interface; empty: the only free cable"},
            "uplink": {"type": "string", "description": "start only: the connection that has the internet; empty: the one with the default route"},
            "hours": {"type": "integer", "description": "start only: switch off after this many hours with nobody using it (1 to 72, default 2)"},
            "isolate": {"type": "boolean", "description": "start only: devices cannot reach each other (default true)"},
            "allow_lan": {"type": "boolean", "description": "start only: devices may reach this computer's local network (default false)"}}, "required": ["action"]});
        v.push(
            Entry::change(TAB, "internet_share_set", "Start or stop sharing this computer's internet with other devices: through a cable, the WiFi hotspot or phones over Bluetooth. Everything they download counts as the person's. The person approves a card first, every time.", schema, |a| {
                if a["action"].as_str() == Some("stop") {
                    "Stop sharing the internet".to_string()
                } else {
                    let through = a.get("through").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("the free cable");
                    format!("Share this computer's internet through {through}: other devices get access to the connection and what they download counts as yours")
                }
            }, move |a| {
                let text = share_apply(&crate::sharing::control::FileBackend { data: e.dir.clone() }, a, SHARE_WAIT_MS)?;
                e.audit("settings.share.internet", &format!("{} {}", a["action"].as_str().unwrap_or(""), a.get("through").and_then(Value::as_str).unwrap_or("")));
                Ok(text)
            })
            .admin(),
        );
    }
    {
        let e = env.clone();
        v.push(Entry::read(TAB, "hotspot_status", "Say the WiFi hotspot's name, band, country and security (never the password).", no_args(), move |_| hotspot_text(&e.dir)).admin());
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {
            "country": {"type": "string", "description": "two capital letters, for example DE; empty for the card's own rules"},
            "security": {"type": "string", "description": "wpa2, transition or wpa3"},
            "band": {"type": "string", "description": "auto, 2.4 or 5"}}});
        v.push(
            Entry::change(TAB, "hotspot_set", "Change the WiFi hotspot's country, security (wpa2, transition or wpa3) or band; applies the next time sharing starts. The person approves a card first.", schema, |a| {
                let mut p = Vec::new();
                for (k, l) in [("country", "country"), ("security", "security"), ("band", "band")] {
                    if let Some(x) = a.get(k).and_then(Value::as_str) {
                        p.push(format!("{l} to {}", if x.is_empty() { "the card's own rules" } else { x }));
                    }
                }
                format!("Hotspot: set {}", if p.is_empty() { "nothing".to_string() } else { p.join(", ") })
            }, move |a| {
                let (country, security, band) = (opt(a, "country")?, opt(a, "security")?, opt(a, "band")?);
                let has_country = a.get("country").is_some_and(|c| c.is_string());
                if !has_country && security.is_none() && band.is_none() {
                    return Err("give at least one of `country`, `security`, `band`".into());
                }
                if has_country {
                    crate::sharing::real::set_hotspot_country(&e.dir, country.as_deref().unwrap_or(""))?;
                }
                if let Some(s) = security {
                    crate::sharing::real::set_hotspot_security(&e.dir, &s)?;
                }
                if let Some(b) = band {
                    crate::sharing::real::set_hotspot_band(&e.dir, &b)?;
                }
                e.audit("settings.share.hotspot", &hotspot_text(&e.dir).unwrap_or_default().replace("Hotspot name:", "name:"));
                Ok(format!("{} It applies the next time sharing starts.", hotspot_text(&e.dir)?))
            })
            .admin(),
        );
    }
    v
}

#[cfg(test)]
#[path = "settingstools_wifi_tests.rs"]
mod tests;

#[cfg(test)]
use iyakki_core_local::wifijoin::MacEnv;
#[cfg(test)]
use iyakki_core_local::wifijoin::KnownStore;
#[cfg(test)]
use iyakki_core_local::wifijoin::Wifi;
