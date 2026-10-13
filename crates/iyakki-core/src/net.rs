//! Network bring-up for bare metal: discovers Ethernet/WiFi interfaces and starts
//! `wpa_supplicant` (WiFi) and `udhcpc` (DHCP) as needed.
//!
//! Configuration lives in `/etc/iyakki/net.conf` (`key = value`, `#` comments):
//! ```text
//! wifi.ssid = HomeNet
//! wifi.psk  = correct horse battery   # WPA2/WPA3-personal passphrase (8..63 chars); omit for open
//! static.eth0 = 192.168.1.50/24 gw 192.168.1.1   # optional; default is DHCP
//! dns = 1.1.1.1
//! ```
//! The planner is pure ([`plan`]) so it is unit tested without hardware; [`execute`] runs it.

use std::path::Path;
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IfaceKind {
    Loopback,
    Ethernet,
    Wifi,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Iface {
    pub name: String,
    pub kind: IfaceKind,
}

/// Classify interfaces under a sysfs-style root (`/sys/class/net`).
pub fn list_ifaces(sys_class_net: &Path) -> Vec<Iface> {
    let mut v: Vec<Iface> = std::fs::read_dir(sys_class_net)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let read = |f: &str| std::fs::read_to_string(p.join(f)).unwrap_or_default().trim().to_string();
            let kind = if p.join("wireless").exists() || p.join("phy80211").exists() {
                IfaceKind::Wifi
            } else if name == "lo" {
                IfaceKind::Loopback
            } else if read("type") == "1" && !["veth", "docker", "br-", "virbr", "cni", "flannel", "tun", "tap"].iter().any(|x| name.starts_with(x)) {
                IfaceKind::Ethernet
            } else {
                IfaceKind::Other
            };
            Iface { name, kind }
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetConfig {
    pub wifi_ssid: Option<String>,
    pub wifi_psk: Option<String>,
    pub static_ips: Vec<(String, String)>, // (iface, "addr/len gw X")
    pub dns: Vec<String>,
    /// Networks joined from Settings (kept in the data folder), tried before the one in the file.
    pub wifi_known: Vec<iyakki_devctl::wifi::Known>,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum NetError {
    #[error("line {0}: expected `key = value`")]
    Syntax(usize),
    #[error("unknown key `{0}`")]
    UnknownKey(String),
    #[error("wifi.ssid must be 1..32 printable characters without quotes or backslashes")]
    BadSsid,
    #[error("wifi.psk must be 8..63 printable characters without quotes or backslashes")]
    BadPsk,
    #[error("invalid value for `{0}`")]
    BadValue(String),
}

fn clean(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len()) && s.chars().all(|c| !c.is_control() && c != '"' && c != '\\')
}

pub fn parse_config(text: &str) -> Result<NetConfig, NetError> {
    let mut c = NetConfig::default();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line.split_once('=').ok_or(NetError::Syntax(i + 1))?;
        let (k, v) = (k.trim(), v.trim());
        match k {
            "wifi.ssid" => {
                if !clean(v, 1, 32) {
                    return Err(NetError::BadSsid);
                }
                c.wifi_ssid = Some(v.into());
            }
            "wifi.psk" => {
                if !clean(v, 8, 63) {
                    return Err(NetError::BadPsk);
                }
                c.wifi_psk = Some(v.into());
            }
            "dns" => {
                if v.parse::<std::net::IpAddr>().is_err() {
                    return Err(NetError::BadValue(k.into()));
                }
                c.dns.push(v.into());
            }
            _ if k.starts_with("static.") => {
                let iface = &k["static.".len()..];
                let ok_if = !iface.is_empty() && iface.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
                let mut it = v.split_whitespace();
                let addr_ok = it.next().is_some_and(|a| {
                    a.split_once('/').is_some_and(|(ip, len)| ip.parse::<std::net::IpAddr>().is_ok() && len.parse::<u8>().is_ok())
                });
                let gw_ok = match (it.next(), it.next(), it.next()) {
                    (None, ..) => true,
                    (Some("gw"), Some(g), None) => g.parse::<std::net::IpAddr>().is_ok(),
                    _ => false,
                };
                if !ok_if || !addr_ok || !gw_ok {
                    return Err(NetError::BadValue(k.into()));
                }
                c.static_ips.push((iface.into(), v.into()));
            }
            other => return Err(NetError::UnknownKey(other.into())),
        }
    }
    if c.wifi_psk.is_some() && c.wifi_ssid.is_none() {
        return Err(NetError::BadValue("wifi.psk without wifi.ssid".into()));
    }
    Ok(c)
}

/// `wpa_supplicant.conf` text (WPA2/WPA3-personal, or open when no PSK).
/// Inputs were validated by [`parse_config`] (no quotes/backslashes/control chars).
pub fn wpa_conf(ssid: &str, psk: Option<&str>) -> String {
    let auth = match psk {
        Some(p) => format!("    psk=\"{p}\"\n    key_mgmt=WPA-PSK SAE\n    ieee80211w=1\n"),
        None => "    key_mgmt=NONE\n".to_string(),
    };
    format!("ctrl_interface=/run/wpa_supplicant\nupdate_config=0\ncountry=00\nnetwork={{\n    ssid=\"{ssid}\"\n{auth}}}\n")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Run to completion.
    Run(Vec<String>),
    /// Start and leave running (daemonize).
    Spawn(Vec<String>),
    /// Write a file (path, contents).
    Write(String, String),
}

fn cmd(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

pub fn plan(cfg: &NetConfig, ifaces: &[Iface]) -> Vec<Step> {
    let mut s = Vec::new();
    for i in ifaces.iter().filter(|i| i.kind == IfaceKind::Loopback) {
        s.push(Step::Run(cmd(&["ip", "link", "set", &i.name, "up"])));
    }
    let mut wifi_done = false;
    for i in ifaces.iter().filter(|i| matches!(i.kind, IfaceKind::Ethernet | IfaceKind::Wifi)) {
        s.push(Step::Run(cmd(&["ip", "link", "set", &i.name, "up"])));
        if i.kind == IfaceKind::Wifi {
            // Only the first WiFi interface is joined; an unconfigured one is just left up.
            if wifi_done || (cfg.wifi_ssid.is_none() && cfg.wifi_known.is_empty()) {
                continue;
            }
            wifi_done = true;
            let path = format!("/run/wpa_supplicant-{}.conf", i.name);
            let conf = if cfg.wifi_known.is_empty() {
                wpa_conf(cfg.wifi_ssid.as_deref().unwrap_or_default(), cfg.wifi_psk.as_deref())
            } else {
                // the networks joined from Settings, then the one in the file (its passphrase made into a key like theirs)
                let mut all = cfg.wifi_known.clone();
                if let Some(ssid) = cfg.wifi_ssid.as_deref().filter(|s| !all.iter().any(|k| k.ssid == *s)) {
                    let sec = if cfg.wifi_psk.is_some() { iyakki_devctl::wifi::Security::Wpa2 } else { iyakki_devctl::wifi::Security::Open };
                    if let Ok(k) = iyakki_devctl::wifi::Known::new(ssid, cfg.wifi_psk.as_deref().unwrap_or_default(), sec, false, 0) {
                        all.push(k);
                    }
                }
                iyakki_devctl::wifi::wpa_conf_for(&all)
            };
            s.push(Step::Write(path.clone(), conf));
            s.push(Step::Spawn(cmd(&["wpa_supplicant", "-B", "-i", &i.name, "-c", &path])));
        }
        match cfg.static_ips.iter().find(|(n, _)| n == &i.name) {
            Some((_, spec)) => {
                let mut it = spec.split_whitespace();
                let addr = it.next().unwrap_or_default();
                s.push(Step::Run(cmd(&["ip", "addr", "add", addr, "dev", &i.name])));
                if let (Some("gw"), Some(g)) = (it.next(), it.next()) {
                    s.push(Step::Run(cmd(&["ip", "route", "add", "default", "via", g, "dev", &i.name])));
                }
            }
            // -b: keep retrying in the background (cable/AP may come later); -R release on exit.
            None => s.push(Step::Spawn(cmd(&["udhcpc", "-b", "-R", "-i", &i.name, "-p", &format!("/run/udhcpc-{}.pid", i.name)]))),
        }
    }
    if !cfg.dns.is_empty() {
        let body: String = cfg.dns.iter().map(|d| format!("nameserver {d}\n")).collect();
        s.push(Step::Write("/etc/resolv.conf".into(), body));
    }
    s
}

/// Execute a plan; returns the failures (a missing cable must not stop boot).
pub fn execute(steps: &[Step]) -> Vec<String> {
    let mut errs = Vec::new();
    for st in steps {
        let r = match st {
            Step::Write(p, c) => std::fs::write(p, c).map_err(|e| format!("write {p}: {e}")),
            Step::Run(a) | Step::Spawn(a) => Command::new(&a[0])
                .args(&a[1..])
                .status()
                .map_err(|e| format!("{}: {e}", a[0]))
                .and_then(|s| if s.success() { Ok(()) } else { Err(format!("{} exited with {s}", a.join(" "))) }),
        };
        if let Err(e) = r {
            errs.push(e);
        }
    }
    errs
}

/// Load config (missing file = defaults), plan, and run.
pub fn bring_up(conf_path: &Path, sys_class_net: &Path) -> Result<Vec<String>, NetError> {
    bring_up_with(conf_path, sys_class_net, Path::new(KNOWN_WIFI))
}

/// Where the networks joined from Settings are kept.
pub const KNOWN_WIFI: &str = "/var/lib/iyakki/wifi.json";

pub fn bring_up_with(conf_path: &Path, sys_class_net: &Path, known: &Path) -> Result<Vec<String>, NetError> {
    let mut cfg = match std::fs::read_to_string(conf_path) {
        Ok(t) => parse_config(&t)?,
        Err(_) => NetConfig::default(),
    };
    cfg.wifi_known = iyakki_devctl::wifi::KnownStore::new(known).list();
    Ok(execute(&plan(&cfg, &list_ifaces(sys_class_net))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eth(n: &str) -> Iface {
        Iface { name: n.into(), kind: IfaceKind::Ethernet }
    }
    fn wifi(n: &str) -> Iface {
        Iface { name: n.into(), kind: IfaceKind::Wifi }
    }

    #[test]
    fn parses_config() {
        let c = parse_config("# c\nwifi.ssid = Home\nwifi.psk = hunter2hunter2\nstatic.eth0 = 10.0.0.2/24 gw 10.0.0.1\ndns = 1.1.1.1\n").unwrap();
        assert_eq!(c.wifi_ssid.as_deref(), Some("Home"));
        assert_eq!(c.static_ips, vec![("eth0".into(), "10.0.0.2/24 gw 10.0.0.1".into())]);
        assert_eq!(c.dns, vec!["1.1.1.1"]);
    }

    #[test]
    fn rejects_bad_and_injecting_config() {
        assert_eq!(parse_config("wifi.ssid = a\"b"), Err(NetError::BadSsid));
        assert_eq!(parse_config("wifi.ssid = x\nwifi.psk = short"), Err(NetError::BadPsk));
        assert_eq!(parse_config("wifi.ssid = x\nwifi.psk = abcdefgh\"}\nnetwork={"), Err(NetError::BadPsk));
        assert!(matches!(parse_config("bogus = 1"), Err(NetError::UnknownKey(_))));
        assert_eq!(parse_config("nonsense"), Err(NetError::Syntax(1)));
        assert!(parse_config("wifi.psk = abcdefgh").is_err());
        assert!(parse_config("static.eth0 = 10.0.0.2 gw 1.1.1.1").is_err());
        assert!(parse_config("static.eth0 = 10.0.0.2/24 junk").is_err());
        assert!(parse_config("dns = not-an-ip").is_err());
    }

    #[test]
    fn wpa_conf_formats() {
        let w = wpa_conf("Home", Some("hunter2hunter2"));
        assert!(w.contains("ssid=\"Home\"") && w.contains("psk=\"hunter2hunter2\"") && w.contains("WPA-PSK SAE"));
        assert!(wpa_conf("Cafe", None).contains("key_mgmt=NONE"));
    }

    #[test]
    fn plans_dhcp_wifi_and_static() {
        let cfg = parse_config("wifi.ssid = Home\nwifi.psk = hunter2hunter2\nstatic.eth1 = 10.0.0.2/24 gw 10.0.0.1\ndns = 9.9.9.9").unwrap();
        let ifs = vec![Iface { name: "lo".into(), kind: IfaceKind::Loopback }, eth("eth0"), eth("eth1"), wifi("wlan0"), wifi("wlan1")];
        let p = plan(&cfg, &ifs);
        assert!(p.contains(&Step::Spawn(cmd(&["udhcpc", "-b", "-R", "-i", "eth0", "-p", "/run/udhcpc-eth0.pid"]))));
        assert!(p.contains(&Step::Run(cmd(&["ip", "addr", "add", "10.0.0.2/24", "dev", "eth1"]))));
        assert!(p.contains(&Step::Run(cmd(&["ip", "route", "add", "default", "via", "10.0.0.1", "dev", "eth1"]))));
        assert!(!p.iter().any(|s| matches!(s, Step::Spawn(a) if a[0] == "udhcpc" && a.contains(&"eth1".to_string()))));
        // wpa_supplicant only for the first wifi iface; second is left alone (no DHCP either needs ssid)
        let wpa: Vec<_> = p.iter().filter(|s| matches!(s, Step::Spawn(a) if a[0] == "wpa_supplicant")).collect();
        assert_eq!(wpa.len(), 1);
        assert!(p.iter().any(|s| matches!(s, Step::Write(path, _) if path == "/etc/resolv.conf")));
    }

    #[test]
    fn unconfigured_wifi_is_not_dhcpd() {
        let p = plan(&NetConfig::default(), &[wifi("wlan0")]);
        assert_eq!(p, vec![Step::Run(cmd(&["ip", "link", "set", "wlan0", "up"]))]);
    }

    #[test]
    fn classifies_sysfs() {
        let d = std::env::temp_dir().join(format!("iyakki-net-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (n, wireless, ty) in [("lo", false, "772"), ("eth0", false, "1"), ("wlan0", true, "1"), ("docker0", false, "1")] {
            std::fs::create_dir_all(d.join(n)).unwrap();
            std::fs::write(d.join(n).join("type"), ty).unwrap();
            if wireless {
                std::fs::create_dir_all(d.join(n).join("wireless")).unwrap();
            }
        }
        let kinds: Vec<_> = list_ifaces(&d).into_iter().map(|i| (i.name, i.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                ("docker0".into(), IfaceKind::Other),
                ("eth0".into(), IfaceKind::Ethernet),
                ("lo".into(), IfaceKind::Loopback),
                ("wlan0".into(), IfaceKind::Wifi)
            ]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn networks_joined_from_settings_are_written_to_the_supplicants_file_before_the_one_in_net_conf() {
        use iyakki_devctl::wifi::{Known, Security};
        let cfg = NetConfig { wifi_ssid: Some("FromFile".into()), wifi_psk: Some("filepassword".into()), wifi_known: vec![Known::new("Joined", "joinedpass1", Security::Wpa2, false, 1).unwrap()], ..Default::default() };
        let steps = plan(&cfg, &[wifi("wlan0")]);
        let conf = steps.iter().find_map(|s| match s { Step::Write(p, c) if p.contains("wpa_supplicant-wlan0") => Some(c.clone()), _ => None }).expect("a conf");
        let (joined, file) = (conf.find("ssid=\"Joined\"").unwrap(), conf.find("ssid=\"FromFile\"").unwrap());
        assert!(joined < file, "{conf}");
        assert!(conf.contains("priority=2") && conf.contains("priority=1") && !conf.contains("filepassword"), "{conf}");
        // with nothing joined from Settings the old way is unchanged; with neither, the interface is only brought up
        let old = plan(&NetConfig { wifi_ssid: Some("FromFile".into()), wifi_psk: Some("filepassword".into()), ..Default::default() }, &[wifi("wlan0")]);
        assert!(old.iter().any(|s| matches!(s, Step::Write(_, c) if c.contains("psk=\"filepassword\""))));
        let none = plan(&NetConfig::default(), &[wifi("wlan0")]);
        assert!(!none.iter().any(|s| matches!(s, Step::Spawn(a) if a[0] == "wpa_supplicant")));
        // only Settings: also joined
        let only = plan(&NetConfig { wifi_known: vec![Known::new("Cafe", "", Security::Open, false, 1).unwrap()], ..Default::default() }, &[wifi("wlan0")]);
        assert!(only.iter().any(|s| matches!(s, Step::Spawn(a) if a[0] == "wpa_supplicant")));
    }
}
