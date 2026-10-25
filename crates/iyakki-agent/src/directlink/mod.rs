//! Direct links: two computers joined by ONE cable, checked, paired and deployed to with the developer mode (`iyakki dev`), with no
//! Wi-Fi and no Ethernet. Read `docs/DEVELOPER-MODE.md`, section "Connect two computers with one cable", for the hardware facts.
//!
//! The honest short version: two ordinary PCs cannot talk over a plain USB-C or USB 3 cable (both ports are USB hosts, and PC
//! controllers have no device mode). What works is a Thunderbolt 3/4 or USB4 cable between Thunderbolt/USB4 ports (the kernel's
//! `thunderbolt-net` makes the interface `thunderbolt0`), a host-to-host USB bridge cable whose chip has a Linux `usbnet`
//! driver (`plusb`, `net1080`, `cdc_subset`), or a computer with a USB device controller that runs a gadget (`usb0`).
//!
//! The pieces (every one is pure or goes through [`Env`], so the tests use [`fake::FakeEnv`]):
//! * [`linklocal`]: IPv4 link-local addresses (RFC 3927) with ARP probing written here, and the fixed private pair chosen from the MACs;
//! * [`beacon`]: the small UDP "who is there" message of the link, parsed defensively;
//! * [`tb`]: `/sys/bus/thunderbolt`: controllers, the other computer, security levels and authorization;
//! * [`manager`]: brings a link up when its interface appears, keeps the peer table, tells the firewall which interfaces are direct links;
//! * [`checks`]: the seven-step check list of Settings and `iyakki dev usb`;
//! * [`gadget`]: the configfs plan that makes a device-capable computer look like a USB network card;
//! * [`real`]: the real machine (sysfs, ioctls, raw ARP socket, UDP); [`cli`]: `iyakki dev usb`.

pub mod beacon;
pub mod checks;
pub mod cli;
pub mod fake;
pub mod gadget;
pub mod linklocal;
pub mod manager;
pub mod real;
pub mod tb;
#[cfg(test)]
mod tests;

use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// The developer-mode port: the only TCP port a direct link lets in by default (the firewall zone uses the same number).
pub const DEV_PORT: u16 = crate::devmode::DEFAULT_PORT;
/// The beacon's UDP port.
pub const BEACON_PORT: u16 = 7980;
/// Where the manager leaves the current state for other processes (the CLI, Settings), no secrets in it.
pub const STATE_FILE: &str = "/run/iyakki-directlink.json";
/// The bytes the speed check sends: 16 requests of 4 MiB (the device refuses bigger bodies).
pub const SPEED_CHUNK: usize = 4 << 20;
pub const SPEED_CHUNKS: usize = 16;

/// How the two computers are joined.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transport {
    /// Thunderbolt 3/4 or USB4 cable: `thunderbolt0`, 10 to 20 Gbit/s.
    Thunderbolt,
    /// A host-to-host USB bridge cable with a `usbnet` driver of its own (`plusb`, `net1080`, `cdc_subset`).
    BridgeCable,
    /// This computer is the USB device (gadget mode): `usb0`.
    Gadget,
    /// A USB network class interface (cdc_ether, cdc_ncm, rndis_host, asix, ax88179, r8152) that may be the other end of a
    /// gadget or of an "Ethernet link" cable: handled like a direct link only when no DHCP server gives it an address.
    UsbNet,
}

impl Transport {
    pub fn label(self) -> &'static str {
        match self {
            Transport::Thunderbolt => "Thunderbolt / USB4 cable",
            Transport::BridgeCable => "USB host-to-host bridge cable",
            Transport::Gadget => "USB device (gadget) port",
            Transport::UsbNet => "USB network link",
        }
    }
}

/// Drivers of host-to-host bridge cables.
pub use iyakki_devctl::direct::BRIDGE_DRIVERS;
/// USB network drivers that may sit on a point-to-point link without a DHCP server.
pub const USBNET_DRIVERS: [&str; 7] = ["cdc_ether", "cdc_ncm", "cdc_eem", "rndis_host", "asix", "ax88179_178a", "r8152"];
/// Seconds an ambiguous USB network interface waits for DHCP before it gets a link-local address.
pub const DHCP_GRACE_SECS: i64 = 20;

/// One network interface as `/sys/class/net` shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetIf {
    pub name: String,
    /// The kernel driver's name (basename of `device/driver`), empty when none.
    pub driver: String,
    pub mac: String,
    pub up: bool,
    /// The cable has a link (`carrier` 1).
    pub carrier: bool,
    pub speed_mbps: Option<u32>,
    pub ipv4: Vec<Ipv4Addr>,
    /// IPv6 link-local addresses (the kernel makes them by itself).
    pub ipv6_ll: Vec<String>,
}

/// What kind of direct link an interface is, if any.
pub fn classify(i: &NetIf, udc_present: bool) -> Option<Transport> {
    if i.name == "lo" || i.mac.eq_ignore_ascii_case(iyakki_core_t2_mac()) {
        return None;
    }
    let d = i.driver.replace('-', "_");
    if i.name.starts_with("thunderbolt") || d == "thunderbolt_net" {
        Some(Transport::Thunderbolt)
    } else if BRIDGE_DRIVERS.contains(&d.as_str()) {
        Some(Transport::BridgeCable)
    } else if udc_present && i.name.starts_with("usb") && !USBNET_DRIVERS.contains(&d.as_str()) {
        Some(Transport::Gadget)
    } else if USBNET_DRIVERS.contains(&d.as_str()) {
        Some(Transport::UsbNet)
    } else {
        None
    }
}

/// The T2 chip's own USB link on Apple T2 Macs (see `iyakki-core/src/net.rs`): never a cable to another computer.
fn iyakki_core_t2_mac() -> &'static str {
    "ac:de:48:00:11:22"
}

/// Another computer seen on a link, from its beacon.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub iface: String,
    pub addr: Ipv4Addr,
    pub beacon: beacon::Beacon,
    pub last_seen: i64,
}

/// What the manager knows, written to [`STATE_FILE`] and read by Settings, the CLI and the firewall hook.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub at: i64,
    pub links: Vec<LinkInfo>,
    pub peers: Vec<Peer>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkInfo {
    pub name: String,
    pub transport: Transport,
    pub carrier: bool,
    pub speed_mbps: Option<u32>,
    pub link_local: Option<Ipv4Addr>,
    pub private: Option<Ipv4Addr>,
    pub ipv6_ll: Vec<String>,
    pub mac: String,
}

/// Settings of the feature (kept in the data folder, administrators change them).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// "Allow more on direct links": everything is let in on them. Off by default: developer mode, the beacon and ping only.
    pub allow_more: bool,
    /// Also add the fixed private pair 10.77.0.1 / 10.77.0.2 (off by default: the link-local addresses are enough).
    pub fixed_pair: bool,
    /// Bring links up at all (on by default).
    pub enabled: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config { allow_more: false, fixed_pair: false, enabled: true }
    }
}

/// Everything the engine needs from the machine. The real one is [`real::RealEnv`]; the tests use [`fake::FakeEnv`].
pub trait Env: Send + Sync {
    fn now(&self) -> i64;
    fn net_ifaces(&self) -> Vec<NetIf>;
    /// The kernel offers a USB device controller (`/sys/class/udc` is not empty).
    fn udc_present(&self) -> bool;
    fn tb(&self) -> tb::TbView;
    /// Write the `authorized` attribute of a Thunderbolt device (`1` approve, `2` approve with a challenge of the stored key);
    /// `key` is written to the device's `key` first when given.
    fn tb_write_authorized(&self, device: &str, level: u8, key: Option<&str>) -> Result<(), String>;
    /// The key kept for a device (by its `unique_id`), 64 hex characters.
    fn tb_key(&self, unique_id: &str) -> Option<String>;
    fn tb_store_key(&self, unique_id: &str, key: &str) -> Result<(), String>;
    fn random_hex32(&self) -> String;
    fn set_up(&self, name: &str) -> Result<(), String>;
    /// Give the interface an IPv4 address (`label` 0: the main one, `label` 1: an alias for a second one).
    fn add_ipv4(&self, name: &str, label: u8, ip: Ipv4Addr, prefix: u8) -> Result<(), String>;
    fn del_ipv4(&self, name: &str, label: u8) -> Result<(), String>;
    /// No forwarding between this link and anything else.
    fn no_forwarding(&self, name: &str) -> Result<(), String>;
    /// RFC 3927 probing: three ARP probes. `InUse` when somebody answers for the address or probes for it at the same time.
    fn arp_probe(&self, name: &str, mac: [u8; 6], ip: Ipv4Addr) -> linklocal::Probe;
    fn arp_announce(&self, name: &str, mac: [u8; 6], ip: Ipv4Addr);
    fn beacon_send(&self, name: &str, local: Ipv4Addr, payload: &[u8]) -> Result<(), String>;
    /// What arrived since the last call (non-blocking).
    fn beacon_recv(&self, name: &str) -> Vec<(Ipv4Addr, Vec<u8>)>;
    /// Send one beacon on every direct link and listen `ms` milliseconds (a program that is not the manager).
    fn discover(&self, ms: u64) -> Vec<Peer>;
    fn snapshot(&self) -> Option<Snapshot>;
    fn write_snapshot(&self, s: &Snapshot);
    fn config(&self) -> Config;
    fn save_config(&self, c: &Config) -> Result<(), String>;
    /// This computer, for the beacon: host name, version, developer mode on, a pairing code open.
    fn me(&self) -> beacon::Beacon;
    fn dev_hello(&self, ip: Ipv4Addr) -> Result<Hello, String>;
    fn paired_as(&self, ip: Ipv4Addr) -> Option<String>;
    fn pair(&self, ip: Ipv4Addr, code: &str, expect_fingerprint: &str) -> Result<String, String>;
    fn tcp_rtt_ms(&self, ip: Ipv4Addr, port: u16) -> Result<f64, String>;
    /// Send [`SPEED_CHUNKS`] x [`SPEED_CHUNK`] bytes through the paired developer-mode link; the bytes and seconds it took.
    fn speed(&self, ip: Ipv4Addr) -> Result<SpeedSample, String>;
}

/// What a device says before pairing (`GET /v1/dev/hello`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    pub name: String,
    pub version: String,
    pub fingerprint: String,
    pub pairing_open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedSample {
    pub bytes: u64,
    pub secs: f64,
}

impl SpeedSample {
    /// Gigabits per second (decimal: 10^9 bits), 0 for a sample with no time.
    pub fn gbit_per_s(&self) -> f64 {
        if self.secs <= 0.0 || !self.secs.is_finite() {
            return 0.0;
        }
        self.bytes as f64 * 8.0 / self.secs / 1e9
    }
}
