//! The machine side of internet sharing: the interface list from `/sys/class/net` and `/proc/net/route`, addresses with the interface
//! ioctls, the forwarding switch in `/proc/sys`, the firewall through the firewall's own netlink writer, and UDP sockets tied to one
//! interface with `SO_BINDTODEVICE`. Nothing here starts a program. The service loop ([`start_system`]) polls `request.json` and drives
//! the pure [`Manager`](super::manager::Manager). Entries are in BOUNDARY.md.

use super::control;
use super::io::{self, Datagram, Ports, SocketMaker};
use super::manager::{Io, Manager, Os};
use super::plan::resolvers_from;
use super::{valid_iface, IfaceInfo, LinkKind, Saved, TABLE};
use crate::fwextra_net::{classify, gateways_from_route, Kind};
use iyakki_fw::nl::tablegen::TableSpec;
use iyakki_hotspot::subnet::Cidr;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The link kind from what the kernel says about the interface.
pub fn kind_of(name: &str, driver: &str, wireless: bool) -> LinkKind {
    match classify(name, driver, wireless) {
        Kind::Ethernet => LinkKind::Ethernet,
        Kind::Wifi => LinkKind::Wifi,
        Kind::Link => {
            if name.starts_with("thunderbolt") || driver.replace('-', "_") == "thunderbolt_net" {
                LinkKind::Thunderbolt
            } else {
                LinkKind::Usb
            }
        }
        Kind::Other => LinkKind::Other,
    }
}

fn read(p: &str) -> String {
    std::fs::read_to_string(p).unwrap_or_default().trim().to_string()
}

/// Every network interface except `lo`, with its state, addresses, default-route flag and byte counters.
pub fn list_interfaces() -> Vec<IfaceInfo> {
    let routes = gateways_from_route(&read("/proc/net/route"));
    let addrs = sys::ipv4_addresses();
    let routed = super::plan::routed_ranges_from(&read("/proc/net/route"));
    let mut names: Vec<String> = std::fs::read_dir("/sys/class/net").map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).collect()).unwrap_or_default();
    names.sort();
    names
        .into_iter()
        .filter(|n| n != "lo" && valid_iface(n))
        .map(|n| {
            let base = format!("/sys/class/net/{n}");
            let driver = std::fs::read_link(format!("{base}/device/driver")).ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned())).unwrap_or_default();
            let wireless = Path::new(&format!("{base}/wireless")).exists() || Path::new(&format!("{base}/phy80211")).exists();
            let flags = u32::from_str_radix(read(&format!("{base}/flags")).trim_start_matches("0x"), 16).unwrap_or(0);
            let num = |f: &str| read(&format!("{base}/statistics/{f}")).parse::<u64>().unwrap_or(0);
            IfaceInfo {
                kind: kind_of(&n, &driver, wireless),
                carrier: read(&format!("{base}/carrier")) == "1",
                up: flags & 1 != 0,
                cidrs: addrs.iter().filter(|(i, _)| i == &n).map(|(_, c)| *c).collect(),
                routed: routed.iter().filter(|(i, _)| i == &n).map(|(_, c)| *c).collect(),
                default_route: routes.iter().any(|(r, _)| r == &n),
                rx_bytes: num("rx_bytes"),
                tx_bytes: num("tx_bytes"),
                name: n,
            }
        })
        .collect()
}

/// Is there a Wi-Fi card that can be an access point next to its own client connection, with the card running the handshake? (The software
/// handshake path is only for a card of its own, see `crate::wifi_ap`.)
pub fn wifi_ap_available() -> bool {
    use crate::nl80211_os::{probe, Nl80211Socket};
    use iyakki_hotspot::caps::{can_host_ap, ApMode, Want, BAND_2GHZ};
    let Ok(mut sock) = Nl80211Socket::open(std::time::Duration::from_secs(2)) else { return false };
    let Ok(pr) = probe(&mut sock) else { return false };
    pr.phys.iter().any(|p| can_host_ap(p, &Want { concurrent_sta: true, ..Want::new(BAND_2GHZ, &pr.country) }).is_ok_and(|plan| plan.mode == ApMode::PskOffload))
}

/// The hotspot's name and password, kept in the sharing folder (readable by root only). Created with a generated password the first time.
#[derive(serde::Serialize, serde::Deserialize)]
struct HotspotFile {
    ssid: String,
    passphrase: String,
    /// "auto" (the client's band), "2.4" or "5"
    #[serde(default)]
    band: String,
}

/// The band the hotspot should use: "auto", "2.4" or "5".
pub fn hotspot_band(data: &Path) -> String {
    let b = std::fs::read_to_string(hotspot_path(data)).ok().and_then(|t| serde_json::from_str::<HotspotFile>(&t).ok()).map(|h| h.band).unwrap_or_default();
    match b.as_str() {
        "2.4" | "5" => b,
        _ => "auto".into(),
    }
}

/// Save the band choice ("auto", "2.4" or "5"), keeping the name and password.
pub fn set_hotspot_band(data: &Path, band: &str) -> Result<(), String> {
    if !matches!(band, "auto" | "2.4" | "5") {
        return Err("the band is auto, 2.4 or 5".into());
    }
    let (ssid, passphrase) = hotspot_credentials(data)?;
    write_hotspot(data, &HotspotFile { ssid, passphrase, band: band.into() })
}

fn hotspot_path(data: &Path) -> PathBuf {
    control::dir(data).join("hotspot.json")
}

/// `(name, password)` of the hotspot; made (and saved) when there is none yet. A file that cannot be read as the hotspot's settings is moved to
/// `hotspot.json.corrupt-<time>` first (the old name and password stay in it); when it cannot be moved aside nothing is made over it.
pub fn hotspot_credentials(data: &Path) -> Result<(String, String), String> {
    let id = std::fs::read_to_string("/etc/machine-id").unwrap_or_default();
    hotspot_credentials_for(data, id.trim(), iyakki_schema::now_secs())
}

/// [`hotspot_credentials`] with the device id and the time given (the id names the default network; the time names a set-aside copy).
pub fn hotspot_credentials_for(data: &Path, device_id: &str, now: u64) -> Result<(String, String), String> {
    let p = hotspot_path(data);
    match std::fs::read_to_string(&p) {
        Ok(t) => {
            if let Ok(h) = serde_json::from_str::<HotspotFile>(&t) {
                if iyakki_hotspot::settings::validate_ssid(&h.ssid).is_ok() && iyakki_hotspot::settings::validate_passphrase(&h.passphrase).is_ok() {
                    return Ok((h.ssid, h.passphrase));
                }
            }
            match iyakki_schema::move_aside(&p, now) {
                Some(to) => log::warn!("hotspot: the saved name and password could not be read; kept as {} and new ones made", to.display()),
                None => return Err(format!("The hotspot's saved name and password ({}) cannot be read and could not be moved aside; they are left as they are", p.display())),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}; it is left as it is", p.display())),
    }
    let s = iyakki_hotspot::settings::HotspotSettings::new_default(device_id).map_err(|e| format!("making a password: {e:?}"))?;
    write_hotspot(data, &HotspotFile { ssid: s.ssid.clone(), passphrase: s.passphrase.clone(), band: String::new() })?;
    Ok((s.ssid, s.passphrase))
}

/// Save a new name and password (checked first).
pub fn set_hotspot_credentials(data: &Path, ssid: &str, passphrase: &str) -> Result<(), String> {
    iyakki_hotspot::settings::validate_ssid(ssid).map_err(|e| format!("the network name: {e:?}"))?;
    iyakki_hotspot::settings::validate_passphrase(passphrase).map_err(|e| format!("the password: {e:?}"))?;
    write_hotspot(data, &HotspotFile { ssid: ssid.into(), passphrase: passphrase.into(), band: hotspot_band(data) })
}

fn write_hotspot(data: &Path, f: &HotspotFile) -> Result<(), String> {
    let p = hotspot_path(data);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let text = serde_json::to_string(f).map_err(|e| e.to_string())?;
    // whole and atomic, readable by the owner only: a cut during the write never leaves half a password
    iyakki_schema::write_atomic(&p, text.as_bytes(), 0o600)
}

/// Default routes taken off the paused Wi-Fi client (a route through a card that is not joined would swallow the hotspot's traffic).
static PARKED_ROUTES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The `ip` calls the route parking makes, so a test can use a fake and never touch the real routes.
trait IpRoute {
    /// The default-route lines of `client` (`ip -4 route show default dev <client>`); `None` when `ip` cannot be run.
    fn show_default(&self, client: &str) -> Option<String>;
    /// `ip -4 route <args>`; true on success.
    fn route(&self, args: &[&str]) -> bool;
}

struct RealIp;

impl IpRoute for RealIp {
    fn show_default(&self, client: &str) -> Option<String> {
        let out = std::process::Command::new("ip").args(["-4", "route", "show", "default", "dev", client]).output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    }
    fn route(&self, args: &[&str]) -> bool {
        std::process::Command::new("ip").arg("-4").arg("route").args(args).status().is_ok_and(|s| s.success())
    }
}

fn park_default_routes(client: &str) {
    let mut parked = PARKED_ROUTES.lock().unwrap_or_else(|e| e.into_inner());
    park_with(&RealIp, &mut parked, client);
}

/// Take every default route off `client`; the ones that were removed are remembered in `parked` (one that could not be removed is not).
fn park_with(ip: &dyn IpRoute, parked: &mut Vec<String>, client: &str) {
    let Some(shown) = ip.show_default(client) else { return };
    for line in shown.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let args: Vec<&str> = std::iter::once("del").chain(line.split_whitespace()).collect();
        if ip.route(&args) {
            parked.push(line.to_string());
            log::info!("hotspot: default route \"{line}\" taken off {client} while sharing");
        }
    }
}

fn restore_default_routes() {
    let mut parked = PARKED_ROUTES.lock().unwrap_or_else(|e| e.into_inner());
    restore_with(&RealIp, &mut parked);
}

/// Put every parked route back, once; the list is empty afterwards (a second call does nothing).
fn restore_with(ip: &dyn IpRoute, parked: &mut Vec<String>) {
    for line in parked.drain(..) {
        let args: Vec<&str> = std::iter::once("add").chain(line.split_whitespace()).collect();
        let _ = ip.route(&args);
    }
}

#[cfg(test)]
mod credential_tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-hotspot-cred-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_first_start_makes_the_name_from_the_device_id_and_a_strong_password_and_keeps_them() {
        let d = dir("first");
        let (ssid, pass) = hotspot_credentials_for(&d, "a1b2c3d4-9f3e", 1).unwrap();
        assert_eq!(ssid, "IYAKKI-9F3E");
        assert!(iyakki_hotspot::settings::validate_passphrase(&pass).is_ok() && pass.replace('-', "").len() == 16, "{pass}");
        assert_eq!(hotspot_credentials_for(&d, "another-id", 2).unwrap(), (ssid, pass), "the saved ones are used, not new ones");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(hotspot_path(&d)).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn a_changed_name_and_password_and_the_band_are_kept_together() {
        let d = dir("change");
        hotspot_credentials_for(&d, "x", 1).unwrap();
        set_hotspot_credentials(&d, "Cafe", "correct horse").unwrap();
        set_hotspot_band(&d, "5").unwrap();
        assert_eq!(hotspot_credentials_for(&d, "x", 2).unwrap(), ("Cafe".to_string(), "correct horse".to_string()));
        assert_eq!(hotspot_band(&d), "5");
        assert!(set_hotspot_credentials(&d, "", "correct horse").is_err() && set_hotspot_credentials(&d, "Cafe", "short").is_err());
        assert_eq!(hotspot_credentials_for(&d, "x", 3).unwrap().0, "Cafe", "a refused change leaves the saved one");
    }

    #[test]
    fn a_damaged_file_is_set_aside_with_its_bytes_and_new_ones_are_made() {
        let d = dir("damaged");
        let p = hotspot_path(&d);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"{\"ssid\": \"Cafe\", \"passph").unwrap();
        let (ssid, _) = hotspot_credentials_for(&d, "abcd", 77).unwrap();
        assert_eq!(ssid, "IYAKKI-ABCD");
        assert_eq!(std::fs::read(p.with_file_name("hotspot.json.corrupt-77")).unwrap(), b"{\"ssid\": \"Cafe\", \"passph");
        // a file with a name that is too long counts as damaged too
        std::fs::write(&p, serde_json::json!({"ssid": "x".repeat(40), "passphrase": "correct horse"}).to_string()).unwrap();
        assert!(hotspot_credentials_for(&d, "abcd", 78).is_ok());
        assert!(p.with_file_name("hotspot.json.corrupt-78").exists());
    }

    #[test]
    fn a_damaged_file_that_cannot_be_moved_aside_is_not_written_over() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("nomove");
        let p = hotspot_path(&d);
        let sub = p.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(&p, b"not json").unwrap();
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o500)).unwrap();
        let cannot_write = std::fs::write(sub.join(".probe"), b"x").is_err();
        let r = hotspot_credentials_for(&d, "abcd", 5);
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o700)).unwrap();
        if cannot_write {
            assert!(r.unwrap_err().contains("could not be moved aside"));
            assert_eq!(std::fs::read(&p).unwrap(), b"not json");
        }
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeIp {
        shown: Option<String>,
        calls: RefCell<Vec<String>>,
        fail_del: bool,
    }
    impl IpRoute for FakeIp {
        fn show_default(&self, client: &str) -> Option<String> {
            self.calls.borrow_mut().push(format!("show {client}"));
            self.shown.clone()
        }
        fn route(&self, args: &[&str]) -> bool {
            self.calls.borrow_mut().push(args.join(" "));
            !(self.fail_del && args[0] == "del")
        }
    }
    fn fake(shown: Option<&str>, fail_del: bool) -> FakeIp {
        FakeIp { shown: shown.map(String::from), calls: RefCell::new(vec![]), fail_del }
    }

    #[test]
    fn the_default_route_of_the_paused_client_is_taken_off_and_put_back_once() {
        let ip = fake(Some("default via 192.168.1.1 dev wlan0 proto dhcp metric 600\n"), false);
        let mut parked = Vec::new();
        park_with(&ip, &mut parked, "wlan0");
        assert_eq!(parked, vec!["default via 192.168.1.1 dev wlan0 proto dhcp metric 600".to_string()]);
        assert_eq!(ip.calls.borrow()[1], "del default via 192.168.1.1 dev wlan0 proto dhcp metric 600");
        restore_with(&ip, &mut parked);
        assert!(parked.is_empty());
        assert_eq!(ip.calls.borrow()[2], "add default via 192.168.1.1 dev wlan0 proto dhcp metric 600");
        restore_with(&ip, &mut parked);
        assert_eq!(ip.calls.borrow().len(), 3, "a second restore does nothing");
    }

    #[test]
    fn a_route_that_could_not_be_removed_is_not_remembered_and_no_ip_means_nothing_happens() {
        let ip = fake(Some("default via 10.0.0.1 dev wlan0\n"), true);
        let mut parked = Vec::new();
        park_with(&ip, &mut parked, "wlan0");
        assert!(parked.is_empty());
        restore_with(&ip, &mut parked);
        assert_eq!(ip.calls.borrow().len(), 2);
        let none = fake(None, false);
        park_with(&none, &mut parked, "wlan0");
        assert!(parked.is_empty() && none.calls.borrow().len() == 1);
    }

    #[test]
    fn several_default_routes_are_all_parked_and_blank_lines_are_ignored() {
        let ip = fake(Some("default via 10.0.0.1 dev wlan0 metric 100\n\ndefault via 10.0.0.2 dev wlan0 metric 200\n"), false);
        let mut parked = Vec::new();
        park_with(&ip, &mut parked, "wlan0");
        assert_eq!(parked.len(), 2);
    }
}

/// The Wi-Fi client interface the hotspot is made next to: the wireless interface with the internet route, else the first one.
fn wifi_client_iface() -> Option<String> {
    let ifs = list_interfaces();
    let wifi: Vec<&IfaceInfo> = ifs.iter().filter(|i| i.kind == LinkKind::Wifi && i.name != crate::wifi_ap::AP_IFACE).collect();
    wifi.iter().find(|i| i.default_route).or(wifi.first()).map(|i| i.name.clone())
}

pub struct RealOs {
    data: PathBuf,
    makers: sys::SysSockets,
    ports: Ports,
}

impl RealOs {
    pub fn new(data: &Path) -> RealOs {
        RealOs { data: data.to_path_buf(), makers: sys::SysSockets, ports: Ports::default() }
    }
    fn saved_path(&self) -> PathBuf {
        control::dir(&self.data).join("session.json")
    }
    fn nf(&self) -> Result<iyakki_fw::nl::NfSocket, String> {
        iyakki_fw::nl::NfSocket::open(5).map_err(|e| e.to_string())
    }
}

impl Os for RealOs {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }
    fn random_u16(&mut self) -> u16 {
        let mut b = [0u8; 2];
        let _ = iyakki_sec::random_fill(&mut b);
        u16::from_be_bytes(b)
    }
    fn interfaces(&mut self) -> Vec<IfaceInfo> {
        list_interfaces()
    }
    fn wifi_ap_available(&mut self) -> bool {
        wifi_ap_available()
    }
    fn resolvers(&mut self) -> Vec<Ipv4Addr> {
        resolvers_from(&read("/etc/resolv.conf"))
    }
    fn wifi_ap_start(&mut self) -> Result<String, String> {
        let client = wifi_client_iface().ok_or("this computer has no Wi-Fi card")?;
        let (ssid, pass) = hotspot_credentials(&self.data)?;
        let mut lines = Vec::new();
        // internet from a cable: the Wi-Fi client is not needed, and its roaming on the card's one radio makes the hotspot deaf
        let cable_has_internet = list_interfaces().iter().any(|i| i.default_route && i.kind != LinkKind::Wifi && i.name != crate::wifi_ap::AP_IFACE);
        let paused = cable_has_internet && crate::wifi_ap::pause_client(&client).map_err(|e| log::warn!("hotspot: not pausing {client}: {e}")).is_ok();
        if paused {
            log::info!("hotspot: {client} let go of its router while sharing (the internet comes from a cable)");
            park_default_routes(&client);
        }
        let started = crate::wifi_ap::start(&client, &ssid, &pass, &hotspot_band(&self.data), paused, &mut |l| lines.push(l));
        if started.is_err() && paused {
            restore_default_routes();
            let _ = crate::wifi_ap::resume_client(&client);
        }
        let up = started?;
        lines.iter().for_each(|l| log::info!("{l}"));
        Ok(up.ifname)
    }
    fn wifi_ap_stop(&mut self) {
        restore_default_routes();
        if let Some(c) = wifi_client_iface() {
            let _ = crate::wifi_ap::resume_client(&c);
        }
        if let Err(e) = crate::wifi_ap::stop() {
            log::warn!("hotspot: {e}");
        }
    }
    fn policy(&mut self) -> super::policy::Policy {
        super::policy::read(&self.data)
    }
    fn forward_get(&mut self) -> Result<String, String> {
        std::fs::read_to_string("/proc/sys/net/ipv4/ip_forward").map(|s| s.trim().to_string()).map_err(|e| format!("ip_forward: {e}"))
    }
    fn forward_set(&mut self, value: &str) -> Result<(), String> {
        if !matches!(value, "0" | "1") {
            return Err("not a forwarding value".into());
        }
        std::fs::write("/proc/sys/net/ipv4/ip_forward", value).map_err(|e| format!("ip_forward: {e}"))
    }
    fn link_up(&mut self, iface: &str) -> Result<(), String> {
        sys::link_up(iface)
    }
    fn add_address(&mut self, iface: &str, ip: Ipv4Addr, prefix: u8) -> Result<(), String> {
        sys::set_addr(iface, ip, prefix)
    }
    fn remove_address(&mut self, iface: &str, _ip: Ipv4Addr, _prefix: u8) -> Result<(), String> {
        sys::set_addr(iface, Ipv4Addr::UNSPECIFIED, 0)
    }
    fn apply_rules(&mut self, spec: &TableSpec) -> Result<(), String> {
        if spec.name != TABLE {
            return Err("sharing only writes its own table".into());
        }
        self.nf()?.apply_table(spec).map_err(|e| e.to_string())
    }
    fn remove_rules(&mut self) -> Result<(), String> {
        let mut s = self.nf()?;
        s.remove_table(TABLE).map_err(|e| e.to_string())
    }
    fn rules_present(&mut self) -> Result<bool, String> {
        self.nf()?.table_exists(TABLE).map_err(|e| e.to_string())
    }
    fn set_firewall_zone(&mut self, ifaces: Vec<String>) {
        iyakki_fw::service::set_share_zone(ifaces);
    }
    fn open_io(&mut self, device: &str, gateway: Ipv4Addr, resolvers: &[Ipv4Addr]) -> Result<Box<dyn Io>, String> {
        if !valid_iface(device) {
            return Err("not an interface name".into());
        }
        let wire = io::open(&mut self.makers, device, gateway, resolvers, self.ports)?;
        Ok(Box::new(wire))
    }
    fn load_saved(&mut self) -> Option<Saved> {
        std::fs::read_to_string(self.saved_path()).ok().and_then(|t| serde_json::from_str(&t).ok())
    }
    fn save_saved(&mut self, s: Option<&Saved>) {
        let p = self.saved_path();
        match s {
            Some(s) => {
                if let Ok(t) = serde_json::to_string(s) {
                    let _ = std::fs::create_dir_all(control::dir(&self.data));
                    let _ = std::fs::write(p, t);
                }
            }
            None => {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    fn audit(&mut self, line: &str) {
        log::info!("{line}");
        use std::io::Write;
        let _ = std::fs::create_dir_all(control::dir(&self.data));
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(control::dir(&self.data).join("audit.log")) {
            let _ = writeln!(f, "{} {line}", self.now_ms() / 1000);
        }
    }
}

/// Start the service thread: sharing is off after every boot (the request file is reset), leftovers of a crash are removed, then the
/// request file is polled once a second and the session is stepped ten times a second while it runs.
pub fn start_system(data: &Path) {
    let data = data.to_path_buf();
    let _ = std::thread::Builder::new().name("iyakki-share".into()).spawn(move || {
        let mut os = RealOs::new(&data);
        let mut m = Manager::new();
        m.recover(&mut os);
        // off after every boot: the person has to switch it on again
        let mut req = control::read_request(&data);
        if req.on {
            req = control::stop_request(&req, "boot");
            let _ = control::write_request(&data, &req);
        }
        let mut last_read = 0u64;
        let mut last_status = super::Status::default();
        loop {
            let now = os.now_ms();
            if now.saturating_sub(last_read) >= 1000 {
                req = control::read_request(&data);
                last_read = now;
            }
            let st = m.tick(&mut os, &req);
            if st != last_status {
                let _ = control::write_status(&data, &st);
                last_status = st;
            }
            std::thread::sleep(std::time::Duration::from_millis(if m.running() { 100 } else { 500 }));
        }
    });
}

/// `iyakki usb share-internet --apply`: the cable gets 10.77.0.1/30, forwarding goes on, the typed table goes in. Nothing is spawned.
pub fn legacy_apply(link: &str, uplink: &str) -> Result<(), String> {
    use crate::fwextra_net::{SHARE_HOST, SHARE_PREFIX};
    if !valid_iface(link) || !valid_iface(uplink) {
        return Err("an interface name is not valid".into());
    }
    let mut os = RealOs::new(Path::new("/var/lib/iyakki"));
    os.link_up(link)?;
    os.add_address(link, SHARE_HOST, SHARE_PREFIX)?;
    os.apply_rules(&super::rules::legacy_table(link, uplink))?;
    os.forward_set("1")
}

/// `--undo`: best effort, every step is tried; forwarding goes back to what it was.
pub fn legacy_undo(link: &str, previous_forward: &str) -> Result<(), String> {
    use crate::fwextra_net::{SHARE_HOST, SHARE_PREFIX};
    let mut os = RealOs::new(Path::new("/var/lib/iyakki"));
    let _ = os.remove_rules();
    if matches!(previous_forward, "0" | "1") {
        let _ = os.forward_set(previous_forward);
    }
    if valid_iface(link) {
        let _ = os.remove_address(link, SHARE_HOST, SHARE_PREFIX);
    }
    Ok(())
}

/// Run one manager against the real machine for the command line when no service answers (`iyakki share` never does this by itself).
pub fn arc_backend(data: &Path) -> Arc<dyn control::Backend> {
    Arc::new(control::FileBackend { data: data.to_path_buf() })
}

#[cfg(target_os = "linux")]
pub(crate) mod sys {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd};

    const SIOCSIFADDR: libc::c_ulong = 0x8916;
    const SIOCSIFNETMASK: libc::c_ulong = 0x891c;
    const SIOCSIFBRDADDR: libc::c_ulong = 0x891a;
    const SIOCGIFFLAGS: libc::c_ulong = 0x8913;
    const SIOCSIFFLAGS: libc::c_ulong = 0x8914;

    #[repr(C)]
    struct IfReqAddr {
        name: [u8; 16],
        family: libc::sa_family_t,
        port: u16,
        addr: [u8; 4],
        zero: [u8; 8],
        pad: [u8; 8],
    }

    #[repr(C)]
    struct IfReqFlags {
        name: [u8; 16],
        flags: i16,
        pad: [u8; 22],
    }

    fn name16(name: &str) -> Result<[u8; 16], String> {
        if !valid_iface(name) {
            return Err(format!("not an interface name: {name:?}"));
        }
        let mut n = [0u8; 16];
        n[..name.len()].copy_from_slice(name.as_bytes());
        Ok(n)
    }

    fn dgram() -> Result<i32, String> {
        // SAFETY: a plain datagram socket, closed by the caller.
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            Err(std::io::Error::last_os_error().to_string())
        } else {
            Ok(fd)
        }
    }

    /// Set (or, with 0.0.0.0, remove) the IPv4 address of `name`, with netmask and broadcast address.
    pub fn set_addr(name: &str, ip: Ipv4Addr, prefix: u8) -> Result<(), String> {
        if prefix > 32 {
            return Err("bad prefix".into());
        }
        let n = name16(name)?;
        let mask = if prefix == 0 { 0u32 } else { u32::MAX << (32 - u32::from(prefix)) };
        let req = |a: Ipv4Addr| IfReqAddr { name: n, family: libc::AF_INET as libc::sa_family_t, port: 0, addr: a.octets(), zero: [0; 8], pad: [0; 8] };
        let fd = dgram()?;
        let mut result = Ok(());
        // SAFETY: every `IfReqAddr` matches the kernel's `struct ifreq` (16-byte name, a `sockaddr_in` in the union, padded to 40
        // bytes) and outlives its call; `fd` is a valid socket closed below.
        unsafe {
            let mut a = req(ip);
            if libc::ioctl(fd, SIOCSIFADDR as _, &mut a as *mut IfReqAddr) < 0 {
                result = Err(std::io::Error::last_os_error().to_string());
            } else if !ip.is_unspecified() {
                let mut m = req(Ipv4Addr::from(mask));
                if libc::ioctl(fd, SIOCSIFNETMASK as _, &mut m as *mut IfReqAddr) < 0 {
                    result = Err(std::io::Error::last_os_error().to_string());
                }
                let mut b = req(Ipv4Addr::from(u32::from(ip) | !mask));
                let _ = libc::ioctl(fd, SIOCSIFBRDADDR as _, &mut b as *mut IfReqAddr);
            }
            libc::close(fd);
        }
        result
    }

    pub fn link_up(name: &str) -> Result<(), String> {
        let n = name16(name)?;
        let fd = dgram()?;
        let mut r = IfReqFlags { name: n, flags: 0, pad: [0; 22] };
        // SAFETY: `IfReqFlags` matches `struct ifreq` with the flags member of the union; valid for both calls.
        let res = unsafe {
            let mut res = Ok(());
            if libc::ioctl(fd, SIOCGIFFLAGS as _, &mut r as *mut IfReqFlags) < 0 {
                res = Err(std::io::Error::last_os_error().to_string());
            } else if r.flags & (libc::IFF_UP as i16) == 0 {
                r.flags |= libc::IFF_UP as i16;
                if libc::ioctl(fd, SIOCSIFFLAGS as _, &mut r as *mut IfReqFlags) < 0 {
                    res = Err(std::io::Error::last_os_error().to_string());
                }
            }
            libc::close(fd);
            res
        };
        res
    }

    /// The IPv4 addresses of every interface with their prefix length.
    pub fn ipv4_addresses() -> Vec<(String, Cidr)> {
        let mut out = Vec::new();
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: getifaddrs fills a linked list that is read only until freeifaddrs; every pointer is checked for null before use.
        unsafe {
            if libc::getifaddrs(&mut head) != 0 {
                return out;
            }
            let mut p = head;
            while !p.is_null() {
                let a = &*p;
                if !a.ifa_addr.is_null() && !a.ifa_netmask.is_null() && i32::from((*a.ifa_addr).sa_family) == libc::AF_INET {
                    let ip = u32::from_be((*(a.ifa_addr as *const libc::sockaddr_in)).sin_addr.s_addr);
                    let mask = u32::from_be((*(a.ifa_netmask as *const libc::sockaddr_in)).sin_addr.s_addr);
                    let name = std::ffi::CStr::from_ptr(a.ifa_name).to_string_lossy().into_owned();
                    if let Some(c) = Cidr::new(Ipv4Addr::from(ip), mask.count_ones() as u8) {
                        out.push((name, c));
                    }
                }
                p = a.ifa_next;
            }
            libc::freeifaddrs(head);
        }
        out
    }

    struct Udp(UdpSocket);

    impl Datagram for Udp {
        fn recv(&mut self, buf: &mut [u8]) -> Option<(usize, SocketAddrV4)> {
            match self.0.recv_from(buf) {
                Ok((n, std::net::SocketAddr::V4(a))) => Some((n, a)),
                _ => None,
            }
        }
        fn send_to(&mut self, data: &[u8], to: SocketAddrV4) -> bool {
            self.0.send_to(data, to).is_ok()
        }
    }

    pub struct SysSockets;

    impl SocketMaker for SysSockets {
        fn bind(&mut self, addr: SocketAddrV4, device: Option<&str>, broadcast: bool) -> Result<Box<dyn Datagram>, String> {
            if let Some(d) = device {
                name16(d)?;
            }
            // SAFETY: socket(2) with constant arguments; the descriptor is owned below (UdpSocket closes it) or closed on error.
            let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, 0) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            // SAFETY: fd is valid and owned from here on.
            let sock = unsafe { UdpSocket::from_raw_fd(fd) };
            if let Some(d) = device {
                // the device is set before bind, so the port is free for other interfaces and nothing else can see our traffic
                // SAFETY: setsockopt with a byte string that lives through the call.
                let r = unsafe { libc::setsockopt(sock.as_raw_fd(), libc::SOL_SOCKET, libc::SO_BINDTODEVICE, d.as_ptr().cast(), d.len() as u32) };
                if r < 0 {
                    return Err(format!("{d}: bind to the interface: {}", std::io::Error::last_os_error()));
                }
            }
            // SAFETY: a zeroed sockaddr_in with family, port and address set is valid; its size is passed.
            let r = unsafe {
                let mut sin: libc::sockaddr_in = std::mem::zeroed();
                sin.sin_family = libc::AF_INET as libc::sa_family_t;
                sin.sin_port = addr.port().to_be();
                sin.sin_addr.s_addr = u32::from(*addr.ip()).to_be();
                libc::bind(sock.as_raw_fd(), (&sin as *const libc::sockaddr_in).cast(), std::mem::size_of::<libc::sockaddr_in>() as u32)
            };
            if r < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            if broadcast {
                sock.set_broadcast(true).map_err(|e| e.to_string())?;
            }
            Ok(Box::new(Udp(sock)))
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) mod sys {
    use super::*;
    pub fn set_addr(_: &str, _: Ipv4Addr, _: u8) -> Result<(), String> {
        Err("internet sharing needs Linux".into())
    }
    pub fn link_up(_: &str) -> Result<(), String> {
        Err("internet sharing needs Linux".into())
    }
    pub fn ipv4_addresses() -> Vec<(String, Cidr)> {
        Vec::new()
    }
    pub struct SysSockets;
    impl SocketMaker for SysSockets {
        fn bind(&mut self, _: SocketAddrV4, _: Option<&str>, _: bool) -> Result<Box<dyn Datagram>, String> {
            Err("internet sharing needs Linux".into())
        }
    }
}
