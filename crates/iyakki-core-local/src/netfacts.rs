//! Small network and key readings the agent used to take with the device crates directly (F-ARCH-07): the routing table with the driver
//! of an interface, this computer's LAN addresses in the order sharing wants them, and the first FIDO security key's report pipe. The agent
//! asks these; the files and device nodes are opened here, from roots a test can replace.

use std::path::Path;

/// The text of the IPv4 routing table (`<proc_>/net/route`), empty when it cannot be read. `proc_` is `/proc` or a made-up tree.
pub fn route_text_in(proc_: &Path) -> String {
    std::fs::read_to_string(proc_.join("net/route")).unwrap_or_default()
}

/// `(driver, is wireless)` of the interface `name` under `<sys>/class/net` (a name that is not a plain interface name gives nothing).
pub fn iface_driver_in(sys: &Path, name: &str) -> (String, bool) {
    if name.is_empty() || name.len() > 32 || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')) {
        return (String::new(), false);
    }
    let base = sys.join("class/net").join(name);
    let driver = std::fs::read_link(base.join("device/driver")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    (driver, base.join("wireless").exists() || base.join("phy80211").exists())
}

/// Does the network interface `name` exist under `<sys>/class/net`?
pub fn iface_exists_in(sys: &Path, name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && sys.join("class/net").join(name).exists()
}

/// [`iface_exists_in`] on this machine.
pub fn iface_exists(name: &str) -> bool {
    iface_exists_in(Path::new("/sys"), name)
}

/// The routing table of this machine.
pub fn route_text() -> String {
    route_text_in(Path::new("/proc"))
}

/// [`iface_driver_in`] on this machine.
pub fn iface_driver(name: &str) -> (String, bool) {
    iface_driver_in(Path::new("/sys"), name)
}

/// The IPv4 addresses of `(interface, address)` pairs that are usable on a LAN (loopback and unparsable ones left out), private ones first.
pub fn lan_order(pairs: &[(String, String)]) -> Vec<String> {
    let mut v: Vec<std::net::Ipv4Addr> = pairs.iter().filter_map(|(_, a)| a.parse().ok()).filter(|a: &std::net::Ipv4Addr| !a.is_loopback()).collect();
    v.sort_by_key(|a| !a.is_private());
    v.into_iter().map(|a| a.to_string()).collect()
}

/// This machine's IPv4 addresses on the networks around it, private ones first (loopback left out).
pub fn lan_addresses() -> Vec<String> {
    lan_order(&iyakki_devctl::ipv4_addresses())
}

/// An opened FIDO security key's HID report pipe: 64-byte packets in and out.
pub struct FidoKey(iyakki_devctl::fidohid::Node);

impl FidoKey {
    pub fn send(&mut self, packet: &[u8; 64]) -> Result<(), String> {
        self.0.send(packet)
    }
    pub fn recv(&mut self) -> Result<[u8; 64], String> {
        self.0.recv()
    }
}

/// The first FIDO HID device found under `<sys>/class/hidraw` (nodes in `<dev>`), opened afresh; `None` when there is none or it will not open.
pub fn first_fido_key() -> Option<FidoKey> {
    let node = iyakki_devctl::fidohid::find(&iyakki_devctl::Roots::real(), Path::new("/dev")).into_iter().next()?;
    iyakki_devctl::fidohid::Node::open(&node).ok().map(FidoKey)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interface_gives_its_driver_and_whether_it_is_wireless_and_odd_names_give_nothing() {
        let d = std::env::temp_dir().join(format!("iyakki-netfacts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("class/net/wlan0/wireless")).unwrap();
        std::fs::create_dir_all(d.join("class/net/eth0/device")).unwrap();
        std::fs::create_dir_all(d.join("drivers/e1000e")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.join("drivers/e1000e"), d.join("class/net/eth0/device/driver")).unwrap();
        assert_eq!(iface_driver_in(&d, "wlan0"), (String::new(), true));
        #[cfg(unix)]
        assert_eq!(iface_driver_in(&d, "eth0"), ("e1000e".to_string(), false));
        assert_eq!(iface_driver_in(&d, "../etc"), (String::new(), false));
        assert_eq!(iface_driver_in(&d, ""), (String::new(), false));
        assert!(iface_exists_in(&d, "wlan0") && !iface_exists_in(&d, "wlan9") && !iface_exists_in(&d, "") && !iface_exists_in(&d, "../net/wlan0"));
        std::fs::create_dir_all(d.join("net")).unwrap();
        std::fs::write(d.join("net/route"), "Iface\tDestination\n").unwrap();
        assert_eq!(route_text_in(&d), "Iface\tDestination\n");
        assert_eq!(route_text_in(&d.join("none")), "");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn lan_addresses_leave_out_loopback_and_junk_and_put_private_ones_first() {
        let p = |a: &str| ("x".to_string(), a.to_string());
        let v = lan_order(&[p("203.0.113.5"), p("127.0.0.1"), p("192.168.1.9"), p("not-an-address"), p("10.0.0.2")]);
        assert_eq!(v, vec!["192.168.1.9".to_string(), "10.0.0.2".to_string(), "203.0.113.5".to_string()]);
        assert!(lan_order(&[]).is_empty());
    }
}
