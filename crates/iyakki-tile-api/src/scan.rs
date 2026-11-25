//! The plain data and the port of the NET SCAN tile (D-S1-E4-01, pattern b): what a scan finds, as plain mirrors of the types of
//! `iyakki-netscan` (which stays out of the tile: it holds the scanner and its HTTP stack), the new-device watch's types, and what the
//! tile asks of the rest of the system. The shell converts both ways (`apps/scan_port.rs`) and a mirror test keeps the two sides in step.

use std::net::Ipv4Addr;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// What kind of thing a service is (mirror of `iyakki_netscan::Kind`; the order is the same: it decides which kind a host shows).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Ollama,
    LlmServer,
    Camera,
    Media,
    Printer,
    SmartHome,
    Data,
    Device,
    Access,
    Platform,
    Web,
    Other,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Ollama => "Ollama",
            Kind::LlmServer => "Model server",
            Kind::Camera => "Camera",
            Kind::Media => "Media",
            Kind::Printer => "Printer",
            Kind::SmartHome => "Smart home",
            Kind::Data => "Data",
            Kind::Device => "Device",
            Kind::Access => "Remote access",
            Kind::Platform => "Platform",
            Kind::Web => "Web",
            Kind::Other => "Other",
        }
    }

    /// Can it provide AI models?
    pub fn is_model_server(self) -> bool {
        matches!(self, Kind::Ollama | Kind::LlmServer)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub port: u16,
    /// `tcp` or `udp`.
    pub proto: &'static str,
    pub kind: Kind,
    pub name: String,
    pub detail: String,
    pub confidence: Confidence,
    pub risk: Option<String>,
    pub models: Vec<String>,
    pub url: Option<String>,
}

impl Service {
    pub fn new(port: u16, kind: Kind, name: impl Into<String>) -> Service {
        Service { port, proto: "tcp", kind, name: name.into(), detail: String::new(), confidence: Confidence::Medium, risk: None, models: vec![], url: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostInfo {
    pub ip: Ipv4Addr,
    pub mac: Option<String>,
    pub name: Option<String>,
    pub services: Vec<Service>,
}

impl HostInfo {
    pub fn new(ip: Ipv4Addr) -> HostInfo {
        HostInfo { ip, mac: None, name: None, services: vec![] }
    }

    /// The most interesting kind on this host (model servers and cameras first).
    pub fn main_kind(&self) -> Kind {
        self.services.iter().map(|s| s.kind).filter(|k| !matches!(k, Kind::Web | Kind::Other)).min().or_else(|| self.services.first().map(|s| s.kind)).unwrap_or(Kind::Other)
    }

    pub fn has(&self, k: Kind) -> bool {
        self.services.iter().any(|s| s.kind == k)
    }

    pub fn risky(&self) -> bool {
        self.services.iter().any(|s| s.risk.is_some())
    }
}

/// One network of this machine (mirror of `iyakki_netscan::net::Subnet`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subnet {
    pub iface: String,
    pub addr: Ipv4Addr,
    pub prefix: u8,
}

impl Subnet {
    /// `192.168.1.0/24`: the network address and the size.
    pub fn cidr(&self) -> String {
        let mask = if self.prefix == 0 { 0 } else { u32::MAX << (32 - self.prefix.min(32) as u32) };
        format!("{}/{}", Ipv4Addr::from(u32::from(self.addr) & mask), self.prefix)
    }
}

/// What a running scan reports (mirror of `iyakki_netscan::Event`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Started { hosts: usize, ports: usize },
    Progress { done: usize, total: usize },
    Host(HostInfo),
    Finished { hosts: usize, services: usize, cancelled: bool },
}

/// How a scan is asked for. `ports: None` is the scanner's default list (and its announcement listening); a list is for tests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanOpts {
    pub ports: Option<Vec<u16>>,
}

/// The answers of a running scan, taken without waiting (the tile asks on every tick).
pub trait ScanFeed {
    fn next(&mut self) -> Option<Event>;
}

/// A device the new-device watch knows (mirror of `iyakki_shared::devwatch::Device`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub mac: String,
    pub ip: String,
    pub iface: String,
    pub first_seen: i64,
    pub last_seen: i64,
    pub trusted: bool,
    pub name: String,
}

/// A neighbour a scan saw (mirror of `iyakki_shared::devwatch::Neighbor`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Neighbor {
    pub ip: String,
    pub mac: String,
    pub iface: String,
}

/// The new-device watch, as far as the tile uses it (the shell's bridge is over `DevWatchApi`).
pub trait DevWatch {
    fn enabled(&self) -> bool;
    fn set_enabled(&self, on: bool);
    fn observe_scan(&self, seen: &[Neighbor], now: i64);
    fn list(&self) -> Vec<Device>;
    fn untrusted_count(&self) -> usize;
    fn trust(&self, mac: &str) -> bool;
    fn trust_all(&self) -> usize;
    fn rename(&self, mac: &str, name: &str) -> bool;
    fn forget(&self, mac: &str) -> bool;
}

/// What the NET SCAN tile asks of the rest of the system.
pub trait ScanPort {
    /// The subnets this machine is on.
    fn subnets(&self) -> Vec<Subnet>;
    /// The host addresses of a subnet (cut to the /24 around this machine when it is big).
    fn hosts_of(&self, s: &Subnet) -> Vec<Ipv4Addr>;
    /// What a person typed (`192.168.1.0/24`, `192.168.1.20`, `192.168.1.10-50`), checked to be on the local network.
    fn parse_targets(&self, text: &str) -> Result<Vec<Ipv4Addr>, String>;
    /// Start a scan; refused (nothing is touched) when the permission is not given. Setting `cancel` stops it quickly.
    fn start_scan(&self, targets: Vec<Ipv4Addr>, opts: ScanOpts, cancel: Arc<AtomicBool>) -> Result<Box<dyn ScanFeed>, String>;
    /// The account's host policy lets it touch `ip`.
    fn allows_host(&self, ip: &str) -> bool;
    /// The account may change settings (administrators).
    fn admin(&self) -> bool;
    /// The new-device watch.
    fn devwatch(&self) -> Arc<dyn DevWatch>;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
    /// Add a found model server to Settings -> Models (administrators); what to tell the person.
    fn add_model_server(&self, ip: Ipv4Addr, svc: &Service, id: &str, address: &str, model: &str, key: &str) -> Result<String, String>;
    /// Now, in seconds since 1970 (the ages in the device list; a fixed value in tests).
    fn now(&self) -> i64;
}
