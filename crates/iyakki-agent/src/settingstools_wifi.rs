//! Chat tools for Settings -> Network: WiFi (`wifi_status`, `wifi_scan`, `wifi_join`, `wifi_forget`) and the WiFi hotspot's country,
//! security and band (`hotspot_status`, `hotspot_set`). Reads are free; every change asks the person on a card, every time, no session
//! grant. They call the same code as the page: `iyakki_devctl::wifi::join_now` for joining, the hotspot file functions of
//! `sharing::real` for the hotspot. The assistant never types a secret: a passphrase, an enterprise password or a WEP key is read from
//! the person's vault entry named in `secret_entry` (made with `vault_request`), and nothing secret is logged or shown on the card.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use iyakki_devctl::mac::MacEnv;
use iyakki_devctl::wifi::{Ap, Known, KnownStore, Security, Wifi};
use iyakki_devctl::wifi_ext::{self, EapMethod, Enterprise};
use serde_json::{json, Value};

use crate::settingstools::{arg_str, no_args, Entry, Env};

const TAB: &str = "Network";

pub const TOOLS: [&str; 6] = ["wifi_status", "wifi_scan", "wifi_join", "wifi_forget", "hotspot_status", "hotspot_set"];

pub const PROMPT_RULE: &str = "\nWIFI (Settings -> Network, administrators): wifi_status lists the saved networks and the adapter, wifi_scan lists the networks in range with signal, security and band (both free). wifi_join joins and remembers a network (asks; a saved or open network needs only the name; for WPA, WPA3, WEP and enterprise networks first call vault_request for the passphrase, password or key and pass its entry name as secret_entry; never ask for or type it in the chat. An enterprise network also needs identity, ca_cert (a full path of the authority file that checks the network) and optionally method (PEAP, TTLS or TLS), server_name, client_cert and private_key. WEP is broken: only with accept_wep_risk true after the person has read the warning). wifi_forget removes a saved network (asks). HOTSPOT: hotspot_status says the hotspot's name, band, country and security (never the password; free); hotspot_set changes the country (two capital letters, empty for the card's own rules), the security (wpa2, transition or wpa3; a card that cannot do WPA3 itself keeps WPA2) and the band (auto, 2.4 or 5); it asks, and it applies the next time sharing starts.";

/// What the WiFi tools act on: the programs that scan and join, the saved networks, the machine's links and the private-address settings.
#[derive(Clone)]
pub struct WifiCtl {
    pub wifi: Arc<Wifi>,
    pub known: Arc<KnownStore>,
    pub roots: iyakki_devctl::Roots,
    pub mac: MacEnv,
}

impl WifiCtl {
    pub fn real() -> WifiCtl {
        WifiCtl { wifi: Arc::new(Wifi::real()), known: Arc::new(KnownStore::new(std::path::Path::new("/var/lib/iyakki/wifi.json"))), roots: iyakki_devctl::Roots::real(), mac: MacEnv::real() }
    }

    /// Points at nothing (unit tests of other crates never scan the real air or touch the real files).
    pub fn offline() -> WifiCtl {
        WifiCtl {
            wifi: Arc::new(Wifi { run_dir: "/nonexistent/run".into(), path: Some("/nonexistent/bin".into()) }),
            known: Arc::new(KnownStore::new(std::path::Path::new("/nonexistent/wifi.json"))),
            roots: iyakki_devctl::Roots { sys: "/nonexistent/sys".into(), proc_: "/nonexistent/proc".into() },
            mac: MacEnv::real(),
        }
    }

    pub fn iface(&self) -> Option<String> {
        iyakki_devctl::links(&self.roots).into_iter().find(|l| l.kind == iyakki_devctl::LinkKind::Wifi).map(|l| l.name)
    }

    fn no_adapter(&self) -> String {
        if iyakki_devctl::broadcom_wifi_without_driver(&self.roots) { iyakki_devctl::BROADCOM_NO_DRIVER_TEXT.to_string() } else { "This computer has no WiFi adapter.".to_string() }
    }
}

/// Reads the text of a vault entry of the signed-in person (the shell gives the real one; it fails when the vault is locked).
pub type SecretReader = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

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
        Some(i) => out.push(format!("WiFi adapter: {i}{}.", c.wifi.current_ssid(&i).map(|s| format!(", joined to {s}")).unwrap_or_else(|| ", not joined".into()))),
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
                return Err(format!("{} Pass accept_wep_risk true only after the person has read this.", wifi_ext::wep_warning()));
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
                iyakki_devctl::wifi::join_now(&c.wifi, &c.known, &c.mac, &iface, k)
            })
            .with_preview(move |a| {
                let ssid = a["ssid"].as_str().unwrap_or("?");
                let sec = a["security"].as_str().unwrap_or("").to_ascii_lowercase();
                let mut s = format!("This computer will join {ssid} and remember it (tried first from now on, also at the next start).");
                if sec == "wep" {
                    s.push(' ');
                    s.push_str(wifi_ext::wep_warning());
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
