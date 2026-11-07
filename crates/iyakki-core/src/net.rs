//! Network bring-up for bare metal: discovers Ethernet/WiFi interfaces and starts
//! `wpa_supplicant` (WiFi) and `udhcpc` (DHCP) as needed.
//!
//! Configuration lives in `/etc/iyakki/net.conf` (`key = value`, `#` comments):
//! ```text
//! wifi.ssid = HomeNet
//! wifi.psk  = correct horse battery   # WPA2/WPA3-personal passphrase (8..63 chars); omit for open
//! static.eth0 = 192.168.1.50/24 gw 192.168.1.1   # optional; default is DHCP
//! dns = 1.1.1.1
//! mac.ethernet = off          # private MAC address: off | per-network | random (Settings -> Network, `iyakki net mac`)
//! mac.wifi = per-network      # default; mac.iface.<name> and mac.ssid.<network> override for one adapter or network
//! ```
//! A private address is set first, while the link is still down, so neither the supplicant nor the DHCP client ever sees the
//! hardware address. The planner is pure ([`plan`]) so it is unit tested without hardware; [`execute`] runs it.

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

/// The address Apple gives the T2's internal USB Ethernet (NCM) link on every T2 Mac. With the apple-bce driver loaded it
/// shows up as an extra `ethN`; seen on a Mac mini 2018 as `cdc_ncm ... eth1 ... ac:de:48:00:11:22`. It is not a network
/// (no DHCP server), so IYAKKI neither brings it up nor runs a DHCP client on it.
pub const T2_INTERNAL_MAC: &str = "ac:de:48:00:11:22";

pub fn is_t2_internal_link(address: &str) -> bool {
    address.trim().eq_ignore_ascii_case(T2_INTERNAL_MAC)
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
            let kind = if name == iyakki_devctl::HOTSPOT_IFACE {
                // the hotspot's access point (made by the sharing service next to the Wi-Fi client): never a client to join or run DHCP on,
                // which would stop the access point a second after it started
                IfaceKind::Other
            } else if p.join("wireless").exists() || p.join("phy80211").exists() {
                IfaceKind::Wifi
            } else if name == "lo" {
                IfaceKind::Loopback
            } else if is_t2_internal_link(&read("address")) {
                // the T2 chip's own USB network link (apple-bce, cdc_ncm): it leads into the T2, never to a network
                IfaceKind::Other
            } else if iyakki_devctl::direct::is_direct_link(&name, &std::fs::read_link(p.join("device/driver")).ok().and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default()) {
                // a Thunderbolt/USB4 cable or a host-to-host USB bridge cable to another computer: no DHCP server there; the link
                // manager (`iyakki_agent::directlink`) gives it a link-local address instead of a DHCP client running here
                IfaceKind::Other
            } else if read("type") == "1" &&!["veth", "docker", "br-", "virbr", "cni", "flannel", "tun", "tap"].iter().any(|x| name.starts_with(x)) {
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
    /// Private MAC address choices (`mac.*` keys; the defaults when there are none).
    pub mac: iyakki_devctl::mac::MacConfig,
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
            // a bad `mac.` line is skipped, never a reason for the network not to come up
            _ if k.starts_with("mac.") => {
                let _ = c.mac.set_key(k, v);
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
    wpa_conf_mode(ssid, psk, iyakki_devctl::mac::MacMode::Off)
}

/// [`wpa_conf`] with the scan randomisation that goes with a private MAC address.
pub fn wpa_conf_mode(ssid: &str, psk: Option<&str>, mode: iyakki_devctl::mac::MacMode) -> String {
    let rand = iyakki_devctl::mac::supplicant_lines(mode);
    let auth = match psk {
        Some(p) => format!("    psk=\"{p}\"\n    key_mgmt=WPA-PSK SAE\n    ieee80211w=1\n"),
        None => "    key_mgmt=NONE\n".to_string(),
    };
    format!("ctrl_interface=/run/wpa_supplicant\nupdate_config=0\ncountry=00\n{rand}network={{\n    ssid=\"{ssid}\"\n{auth}}}\n")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Run to completion.
    Run(Vec<String>),
    /// Start and leave running (daemonize).
    Spawn(Vec<String>),
    /// Write a file (path, contents).
    Write(String, String),
    /// Give an adapter its private MAC address (with the link still down, before the supplicant and the DHCP client start).
    Mac { iface: String, mode: iyakki_devctl::mac::MacMode, ssid: Option<String> },
}

fn cmd(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

pub fn plan(cfg: &NetConfig, ifaces: &[Iface]) -> Vec<Step> {
    use iyakki_devctl::mac::MacMode;
    use iyakki_devctl::LinkKind;
    let mut s = Vec::new();
    for i in ifaces.iter().filter(|i| i.kind == IfaceKind::Loopback) {
        s.push(Step::Run(cmd(&["ip", "link", "set", &i.name, "up"])));
    }
    let mut wifi_done = false;
    for i in ifaces.iter().filter(|i| matches!(i.kind, IfaceKind::Ethernet | IfaceKind::Wifi)) {
        // The WiFi network this adapter will join (the newest saved one, else the one in the file); the private address
        // for "per network" is made for it.
        let joins = i.kind == IfaceKind::Wifi && !wifi_done && (cfg.wifi_ssid.is_some() || !cfg.wifi_known.is_empty());
        let ssid: Option<&str> = if joins { cfg.wifi_known.first().map(|k| k.ssid.as_str()).or(cfg.wifi_ssid.as_deref()) } else { None };
        let mode = cfg.mac.mode_for(if i.kind == IfaceKind::Wifi { LinkKind::Wifi } else { LinkKind::Ethernet }, &i.name, ssid);
        // First of all, while the link is still down: nothing (supplicant, DHCP) may see the hardware address.
        if mode != MacMode::Off {
            s.push(Step::Mac { iface: i.name.clone(), mode, ssid: ssid.map(str::to_string) });
        }
        s.push(Step::Run(cmd(&["ip", "link", "set", &i.name, "up"])));
        if i.kind == IfaceKind::Wifi {
            // Only the first WiFi interface is joined; an unconfigured one is just left up.
            if wifi_done || (cfg.wifi_ssid.is_none() && cfg.wifi_known.is_empty()) {
                continue;
            }
            wifi_done = true;
            let path = format!("/run/wpa_supplicant-{}.conf", i.name);
            let conf = if cfg.wifi_known.is_empty() {
                wpa_conf_mode(cfg.wifi_ssid.as_deref().unwrap_or_default(), cfg.wifi_psk.as_deref(), mode)
            } else {
                // the networks joined from Settings, then the one in the file (its passphrase made into a key like theirs)
                let mut all = cfg.wifi_known.clone();
                if let Some(ssid) = cfg.wifi_ssid.as_deref().filter(|s| !all.iter().any(|k| k.ssid == *s)) {
                    let sec = if cfg.wifi_psk.is_some() { iyakki_devctl::wifi::Security::Wpa2 } else { iyakki_devctl::wifi::Security::Open };
                    if let Ok(k) = iyakki_devctl::wifi::Known::new(ssid, cfg.wifi_psk.as_deref().unwrap_or_default(), sec, false, 0) {
                        all.push(k);
                    }
                }
                iyakki_devctl::wifi::wpa_conf_for_mode(&all, mode)
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
            // -b: keep retrying in the background (cable/AP may come later); -R release on exit. No host name is sent.
            None => {
                let mut a = vec!["udhcpc".to_string()];
                a.extend(iyakki_devctl::mac::dhcp_args(&i.name, &format!("/run/udhcpc-{}.pid", i.name)));
                s.push(Step::Spawn(a));
            }
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
    execute_with(steps, &iyakki_devctl::System, &iyakki_devctl::Roots::real(), Path::new("/var/lib/iyakki/mac-secret"))
}

/// [`execute`] against a given machine (tests pass a fake one). A private address that cannot be set is reported and the
/// adapter keeps its hardware address, so the network still comes up.
pub fn execute_with(steps: &[Step], ctl: &dyn iyakki_devctl::Control, roots: &iyakki_devctl::Roots, secret_path: &Path) -> Vec<String> {
    let mut errs = Vec::new();
    let mut secret: Option<Vec<u8>> = None;
    for st in steps {
        let r = match st {
            Step::Write(p, c) => std::fs::write(p, c).map_err(|e| format!("write {p}: {e}")),
            Step::Mac { iface, mode, ssid } => {
                let sec = secret.get_or_insert_with(|| iyakki_devctl::mac::load_secret(secret_path).0);
                iyakki_devctl::mac::apply_mode(ctl, roots, *mode, sec, iface, ssid.as_deref()).map(|_| ()).map_err(|e| format!("private MAC address: {e}; using the hardware address"))
            }
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

/// Pure: the interfaces of `all` that boot sets up on the wired path now: Ethernet ones that are not set up yet.
pub fn wired_todo(all: &[Iface], done: &std::collections::HashMap<String, IfaceKind>) -> Vec<Iface> {
    all.iter().filter(|i| i.kind == IfaceKind::Ethernet && !done.contains_key(&i.name)).cloned().collect()
}

/// Pure: the interfaces for the quick, synchronous part of boot (loopback and the like): neither Ethernet nor Wi-Fi.
pub fn basic_ifaces(all: &[Iface]) -> Vec<Iface> {
    all.iter().filter(|i| !matches!(i.kind, IfaceKind::Ethernet | IfaceKind::Wifi)).cloned().collect()
}

fn load_cfg(conf_path: &Path, known: &Path) -> Result<NetConfig, NetError> {
    let mut cfg = match std::fs::read_to_string(conf_path) {
        Ok(t) => parse_config(&t)?,
        Err(_) => NetConfig::default(),
    };
    cfg.wifi_known = iyakki_devctl::wifi::KnownStore::new(known).list();
    Ok(cfg)
}

/// Boot, first: set up the Ethernet adapters that exist and are not set up yet, on a thread of their own (the first `ip link set up` of
/// a Tigon3 takes seconds), so nothing else in boot waits for the cable and the cable waits for nothing. Safe to call again: an
/// adapter is set up once. Returns the number of adapters started.
pub fn start_wired(conf_path: &Path, sys_class_net: &Path, known: &Path) -> Result<usize, NetError> {
    let cfg = load_cfg(conf_path, known)?;
    let todo = {
        let g = SET_UP.lock().unwrap_or_else(|e| e.into_inner());
        wired_todo(&list_ifaces(sys_class_net), &g.clone().unwrap_or_default())
    };
    if todo.is_empty() {
        return Ok(0);
    }
    remember(&todo);
    let n = todo.len();
    let steps = plan(&cfg, &todo);
    let spawned = std::thread::Builder::new().name("iyakki-net-wired".into()).spawn(move || {
        for e in execute(&steps) {
            log::warn!("net: {e}");
        }
        iyakki_fw::service::network_changed();
    });
    if let Err(e) = spawned {
        log::warn!("net: wired thread: {e}");
    }
    Ok(n)
}

/// Boot: loopback and the DNS file, now and quickly; Ethernet goes by [`start_wired`] and Wi-Fi by [`bring_up_new`] on a background
/// thread, so neither can hold the boot back.
pub fn bring_up_basic(conf_path: &Path, sys_class_net: &Path, known: &Path) -> Result<Vec<String>, NetError> {
    let cfg = load_cfg(conf_path, known)?;
    Ok(execute(&plan(&cfg, &basic_ifaces(&list_ifaces(sys_class_net)))))
}

/// Where the networks joined from Settings are kept.
pub const KNOWN_WIFI: &str = "/var/lib/iyakki/wifi.json";

pub fn bring_up_with(conf_path: &Path, sys_class_net: &Path, known: &Path) -> Result<Vec<String>, NetError> {
    let mut cfg = match std::fs::read_to_string(conf_path) {
        Ok(t) => parse_config(&t)?,
        Err(_) => NetConfig::default(),
    };
    cfg.wifi_known = iyakki_devctl::wifi::KnownStore::new(known).list();
    let ifaces = list_ifaces(sys_class_net);
    remember(&ifaces);
    Ok(execute(&plan(&cfg, &ifaces)))
}

/// Interfaces that were set up (at boot or later) and what they are, so a hot-plug event never starts a second DHCP client.
static SET_UP: std::sync::Mutex<Option<std::collections::HashMap<String, IfaceKind>>> = std::sync::Mutex::new(None);

/// Boot has set the network up: from now on [`bring_up_new`] acts on interfaces that appear.
pub static NETWORK_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn remember(ifaces: &[Iface]) {
    let mut g = SET_UP.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(Default::default);
    for i in ifaces.iter().filter(|i| matches!(i.kind, IfaceKind::Ethernet | IfaceKind::Wifi)) {
        m.insert(i.name.clone(), i.kind);
    }
}

/// What [`bring_up_new`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct NewIfaces {
    /// Names of the interfaces that were set up now.
    pub started: Vec<String>,
}

/// Set up the Ethernet and Wi-Fi interfaces that exist but were not set up yet (`only`: just that one), the way boot does:
/// link up, private address, DHCP or the static address from the file; a Wi-Fi adapter joins the saved network only when no other
/// Wi-Fi adapter is already doing it. Called when an adapter is plugged in later and once boot has finished.
pub fn bring_up_new(conf_path: &Path, sys_class_net: &Path, known: &Path, only: Option<&str>) -> Result<NewIfaces, NetError> {
    let mut cfg = match std::fs::read_to_string(conf_path) {
        Ok(t) => parse_config(&t)?,
        Err(_) => NetConfig::default(),
    };
    cfg.wifi_known = iyakki_devctl::wifi::KnownStore::new(known).list();
    let (todo, wifi_busy) = {
        let g = SET_UP.lock().unwrap_or_else(|e| e.into_inner());
        let done = g.as_ref();
        let todo: Vec<Iface> = list_ifaces(sys_class_net)
            .into_iter()
            .filter(|i| matches!(i.kind, IfaceKind::Ethernet | IfaceKind::Wifi))
            .filter(|i| only.is_none_or(|o| o == i.name))
            .filter(|i| !done.is_some_and(|m| m.contains_key(&i.name)))
            .collect();
        (todo, done.is_some_and(|m| m.values().any(|k| *k == IfaceKind::Wifi)))
    };
    if todo.is_empty() {
        return Ok(NewIfaces::default());
    }
    if wifi_busy {
        // another adapter already joined the network: this one is only switched on
        cfg.wifi_ssid = None;
        cfg.wifi_psk = None;
        cfg.wifi_known.clear();
    }
    remember(&todo);
    for e in execute(&plan(&cfg, &todo)) {
        log::warn!("net: {e}");
    }
    Ok(NewIfaces { started: todo.into_iter().map(|i| i.name).collect() })
}

/// An interface is gone (the adapter was unplugged): forget it and stop the clients that were started for it, so the same
/// name is set up afresh when an adapter comes back.
pub fn interface_gone(name: &str) {
    let known = SET_UP.lock().unwrap_or_else(|e| e.into_inner()).as_mut().is_some_and(|m| m.remove(name).is_some());
    if !known {
        return;
    }
    for pidfile in [format!("/run/udhcpc-{name}.pid")] {
        if let Some(pid) = std::fs::read_to_string(&pidfile).ok().and_then(|t| t.trim().parse::<u32>().ok()) {
            // only a process that really is the DHCP client of this interface
            let line = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            let line = String::from_utf8_lossy(&line);
            if line.contains("udhcpc") && line.contains(name) {
                let _ = Command::new("kill").arg(pid.to_string()).status();
            }
            let _ = std::fs::remove_file(&pidfile);
        }
    }
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

    fn fake_net(tag: &str, names: &[&str]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("net-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for n in names {
            std::fs::create_dir_all(d.join(n)).unwrap();
            std::fs::write(d.join(n).join("type"), "1\n").unwrap();
            std::fs::write(d.join(n).join("address"), "9c:eb:e8:00:00:01\n").unwrap();
        }
        d
    }

    #[test]
    fn boot_brings_the_cable_up_on_its_own_and_leaves_wifi_and_the_rest_out_of_the_quick_part() {
        let all = vec![Iface { name: "lo".into(), kind: IfaceKind::Loopback }, eth("eth0"), wifi("wlan0"), eth("eth1")];
        let mut done = std::collections::HashMap::new();
        assert_eq!(wired_todo(&all, &done).iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["eth0", "eth1"], "Ethernet only, never Wi-Fi");
        done.insert("eth0".to_string(), IfaceKind::Ethernet);
        assert_eq!(wired_todo(&all, &done).iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["eth1"], "an adapter is set up once");
        assert_eq!(basic_ifaces(&all).iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["lo"]);
        // the plan of the wired part has no supplicant and no Wi-Fi command
        let steps = plan(&NetConfig::default(), &wired_todo(&all, &Default::default()));
        assert!(steps.iter().all(|s| !format!("{s:?}").contains("wlan0") && !format!("{s:?}").contains("wpa_supplicant")));
        assert!(steps.iter().any(|s| format!("{s:?}").contains("udhcpc")));
    }

    #[test]
    fn an_interface_that_appears_later_is_set_up_once_and_again_after_it_went_away() {
        let d = fake_net("late", &["ethlate0"]);
        let none = d.join("no-such-file");
        // the commands run for real here (`ip`, `udhcpc` are absent or fail on a build machine: only log lines)
        let first = bring_up_new(&none, &d, &none, Some("ethlate0")).unwrap();
        assert_eq!(first.started, vec!["ethlate0"]);
        let again = bring_up_new(&none, &d, &none, Some("ethlate0")).unwrap();
        assert!(again.started.is_empty(), "no second DHCP client for an interface that is already set up");
        interface_gone("ethlate0");
        let back = bring_up_new(&none, &d, &none, Some("ethlate0")).unwrap();
        assert_eq!(back.started, vec!["ethlate0"], "the same name comes up again when the adapter returns");
        let other = bring_up_new(&none, &d, &none, Some("ethother9")).unwrap();
        assert!(other.started.is_empty(), "only the named interface");
        let _ = std::fs::remove_dir_all(&d);
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
        // only brought up (no supplicant, no DHCP); with WiFi private by default it first gets its private address, so even
        // its probe requests do not carry the hardware address
        assert_eq!(p, vec![Step::Mac { iface: "wlan0".into(), mode: iyakki_devctl::mac::MacMode::PerNetwork, ssid: None }, Step::Run(cmd(&["ip", "link", "set", "wlan0", "up"]))]);
        let off = plan(&parse_config("mac.wifi = off").unwrap(), &[wifi("wlan0")]);
        assert_eq!(off, vec![Step::Run(cmd(&["ip", "link", "set", "wlan0", "up"]))]);
    }

    #[test]
    fn classifies_sysfs() {
        let d = std::env::temp_dir().join(format!("iyakki-net-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (n, wireless, ty) in [("lo", false, "772"), ("eth0", false, "1"), ("wlan0", true, "1"), ("docker0", false, "1"), ("eth1", false, "1"), ("iyakap0", true, "1")] {
            std::fs::create_dir_all(d.join(n)).unwrap();
            std::fs::write(d.join(n).join("type"), ty).unwrap();
            std::fs::write(d.join(n).join("address"), if n == "eth1" { "ac:de:48:00:11:22\n" } else { "68:fe:f7:14:24:16\n" }).unwrap();
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
                // the T2's internal link (apple-bce + cdc_ncm on a Mac mini 2018) is not a network
                ("eth1".into(), IfaceKind::Other),
                // the hotspot's access point is never set up as a Wi-Fi client
                ("iyakap0".into(), IfaceKind::Other),
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

    // ---- private MAC addresses

    use iyakki_devctl::mac::MacMode;

    #[test]
    fn mac_keys_are_read_old_files_still_parse_and_a_bad_mac_line_never_stops_the_network() {
        let old = parse_config("wifi.ssid = Home\ndns = 1.1.1.1\n").unwrap();
        assert_eq!(old.mac, iyakki_devctl::mac::MacConfig::default(), "no mac keys: Ethernet off, WiFi per network");
        let c = parse_config("mac.ethernet = random\nmac.wifi = off\nmac.iface.wlan1 = per-network\nmac.ssid.Work Net = off\n").unwrap();
        assert_eq!((c.mac.ethernet, c.mac.wifi), (MacMode::Random, MacMode::Off));
        assert_eq!(c.mac.ifaces.get("wlan1"), Some(&MacMode::PerNetwork));
        assert_eq!(c.mac.ssids.get("Work Net"), Some(&MacMode::Off));
        let bad = parse_config("mac.wifi = sometimes\nmac.nonsense = 1\nmac.iface.a/b = off\nwifi.ssid = Home\n").unwrap();
        assert_eq!(bad.mac, iyakki_devctl::mac::MacConfig::default());
        assert_eq!(bad.wifi_ssid.as_deref(), Some("Home"));
    }

    fn pos(p: &[Step], f: impl Fn(&Step) -> bool) -> usize {
        p.iter().position(f).unwrap_or_else(|| panic!("step not found in {p:?}"))
    }

    #[test]
    fn the_private_address_is_set_before_the_link_comes_up_before_the_supplicant_and_before_dhcp() {
        let cfg = parse_config("wifi.ssid = Home\nwifi.psk = hunter2hunter2\nmac.ethernet = random\n").unwrap();
        let p = plan(&cfg, &[eth("eth0"), wifi("wlan0")]);
        // Ethernet: random
        let (m, up, dhcp) = (
            pos(&p, |s| matches!(s, Step::Mac { iface, mode: MacMode::Random, ssid: None } if iface == "eth0")),
            pos(&p, |s| *s == Step::Run(cmd(&["ip", "link", "set", "eth0", "up"]))),
            pos(&p, |s| matches!(s, Step::Spawn(a) if a[0] == "udhcpc" && a.contains(&"eth0".to_string()))),
        );
        assert!(m < up && up < dhcp, "{p:?}");
        // WiFi: per network (the default), for the network it will join, before the supplicant's file and the supplicant
        let (m, up, w, sup, d) = (
            pos(&p, |s| matches!(s, Step::Mac { iface, mode: MacMode::PerNetwork, ssid: Some(n) } if iface == "wlan0" && n == "Home")),
            pos(&p, |s| *s == Step::Run(cmd(&["ip", "link", "set", "wlan0", "up"]))),
            pos(&p, |s| matches!(s, Step::Write(path, c) if path.contains("wlan0") && c.contains("preassoc_mac_addr=1"))),
            pos(&p, |s| matches!(s, Step::Spawn(a) if a[0] == "wpa_supplicant")),
            pos(&p, |s| matches!(s, Step::Spawn(a) if a[0] == "udhcpc" && a.contains(&"wlan0".to_string()))),
        );
        assert!(m < up && up < w && w < sup && sup < d, "{p:?}");
        // no host name goes out
        assert!(!p.iter().any(|s| matches!(s, Step::Spawn(a) if a.iter().any(|x| x == "-x" || x == "-F" || x == "-H" || x == "-h"))));
    }

    #[test]
    fn mode_off_adds_no_step_and_no_scan_randomisation_and_the_newest_saved_network_decides_the_wifi_address() {
        use iyakki_devctl::wifi::{Known, Security};
        let p = plan(&parse_config("wifi.ssid = Home\nwifi.psk = hunter2hunter2\nmac.wifi = off\n").unwrap(), &[eth("eth0"), wifi("wlan0")]);
        assert!(!p.iter().any(|s| matches!(s, Step::Mac { .. })), "Off is the hardware address: nothing to do: {p:?}");
        assert!(!p.iter().any(|s| matches!(s, Step::Write(_, c) if c.contains("preassoc_mac_addr"))));
        // the default plan for plain Ethernet also has no step (Ethernet defaults to Off)
        assert!(!plan(&NetConfig::default(), &[eth("eth0")]).iter().any(|s| matches!(s, Step::Mac { .. })));
        // saved networks: the newest first; a per-network override can switch it off for one network
        let mut cfg = NetConfig { wifi_known: vec![Known::new("Newest", "joinedpass1", Security::Wpa2, false, 2).unwrap(), Known::new("Older", "joinedpass2", Security::Wpa2, false, 1).unwrap()], ..Default::default() };
        let p = plan(&cfg, &[wifi("wlan0")]);
        assert!(p.iter().any(|s| matches!(s, Step::Mac { ssid: Some(n), .. } if n == "Newest")), "{p:?}");
        assert!(p.iter().any(|s| matches!(s, Step::Write(_, c) if c.contains("preassoc_mac_addr=1") && c.contains("ssid=\"Older\""))));
        cfg.mac.ssids.insert("Newest".into(), MacMode::Off);
        assert!(!plan(&cfg, &[wifi("wlan0")]).iter().any(|s| matches!(s, Step::Mac { .. })));
        // an unconfigured second adapter gets an address for its interface name, not for a network
        let two = plan(&NetConfig { wifi_ssid: Some("Home".into()), ..Default::default() }, &[wifi("wlan0"), wifi("wlan1")]);
        assert!(two.iter().any(|s| matches!(s, Step::Mac { iface, ssid: None, .. } if iface == "wlan1")), "{two:?}");
    }

    /// A machine under a temp folder with one adapter whose driver can be told to refuse.
    struct FakeNic {
        sys: std::path::PathBuf,
        refuse: bool,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl iyakki_devctl::Control for FakeNic {
        fn set_link(&self, n: &str, up: bool) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("link {n} {}", if up { "up" } else { "down" }));
            std::fs::write(self.sys.join("class/net").join(n).join("flags"), if up { "0x1003\n" } else { "0x1002\n" }).map_err(|e| e.to_string())
        }
        fn ipv4(&self) -> Vec<(String, String)> {
            vec![]
        }
        fn volume(&self, _: u32) -> Option<iyakki_devctl::Volume> {
            None
        }
        fn change_volume(&self, _: u32, _: Option<i32>) -> Result<iyakki_devctl::Volume, String> {
            Err("no".into())
        }
        fn set_volume(&self, _: u32, _: u32) -> Result<iyakki_devctl::Volume, String> {
            Err("no".into())
        }
        fn dhcp(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn permanent_mac(&self, _: &str) -> Option<String> {
            Some("00:11:22:33:44:55".into())
        }
        fn set_mac(&self, n: &str, mac: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("mac {n} {mac}"));
            if self.refuse {
                return Err("not supported by this driver".into());
            }
            std::fs::write(self.sys.join("class/net").join(n).join("address"), format!("{mac}\n")).map_err(|e| e.to_string())
        }
    }

    #[test]
    fn executing_applies_the_address_in_order_and_a_driver_that_refuses_does_not_stop_the_rest_of_the_plan() {
        let d = std::env::temp_dir().join(format!("iyakki-net-mac-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (k, v) in [("type", "1\n"), ("flags", "0x1002\n"), ("carrier", "1\n"), ("address", "00:11:22:33:44:55\n"), ("addr_assign_type", "0\n"), ("device/x", "")] {
            let p = d.join("sys/class/net/eth0").join(k);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, v).unwrap();
        }
        let roots = iyakki_devctl::Roots { sys: d.join("sys"), proc_: d.join("proc") };
        let witness = d.join("witness");
        let steps = vec![Step::Mac { iface: "eth0".into(), mode: MacMode::PerNetwork, ssid: None }, Step::Write(witness.display().to_string(), "later steps still run".into())];
        let nic = FakeNic { sys: d.join("sys"), refuse: false, calls: Default::default() };
        let errs = execute_with(&steps, &nic, &roots, &d.join("var/mac-secret"));
        assert!(errs.is_empty(), "{errs:?}");
        let now = std::fs::read_to_string(d.join("sys/class/net/eth0/address")).unwrap();
        let m = iyakki_devctl::mac::parse_mac(&now).unwrap();
        assert!(iyakki_devctl::mac::valid_unicast(&m) && iyakki_devctl::mac::locally_administered(&m), "{now}");
        assert!(d.join("var/mac-secret").exists(), "the device secret was made");
        assert!(witness.exists());
        // the same plan again gives the same address (stable), without touching the link
        let n = nic.calls.lock().unwrap().len();
        assert!(execute_with(&steps, &nic, &roots, &d.join("var/mac-secret")).is_empty());
        assert_eq!(nic.calls.lock().unwrap().len(), n, "already that address");
        // a driver that refuses: reported, hardware address kept, the witness is still written
        std::fs::write(d.join("sys/class/net/eth0/address"), "00:11:22:33:44:55\n").unwrap();
        std::fs::write(d.join("sys/class/net/eth0/flags"), "0x1003\n").unwrap();
        let _ = std::fs::remove_file(&witness);
        let bad = FakeNic { sys: d.join("sys"), refuse: true, calls: Default::default() };
        let errs = execute_with(&steps, &bad, &roots, &d.join("var/mac-secret"));
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].contains("not supported by this driver") && errs[0].contains("using the hardware address"), "{errs:?}");
        assert!(witness.exists(), "the plan carried on");
        assert_eq!(bad.calls.lock().unwrap().last().unwrap(), "link eth0 up", "the link is back up");
        let _ = std::fs::remove_dir_all(&d);
    }
}
