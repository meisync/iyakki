//! Internet sharing ("tethering"): this computer gives other computers the internet it has, over a cable. Wired Ethernet, a USB network
//! link (gadget, RNDIS/ECM adapter) or a Thunderbolt / USB-C link; a Wi-Fi access point later (plan: docs/development/HOTSPOT-PLAN.md).
//!
//! Layout (the pure parts first, the machine last):
//!
//! * [`plan`]: which uplink, which downstream interface, which private /24; every refusal as one plain line ([`ShareError`]).
//! * [`rules`]: the firewall table `inet iyakki_share` as typed values for the firewall's own netlink writer (no `nft` program, no shell).
//! * [`manager`]: the state machine (start, idle switch-off, uplink loss, clean-up after a crash) over the [`manager::Os`] and
//!   [`manager::Io`] traits. DHCP and DNS are `iyakki_hotspot`'s byte-level servers.
//! * [`io`]: the sockets (a [`io::SocketMaker`] trait; the real one binds with `SO_BINDTODEVICE`), generic so tests use fakes and loopback.
//! * [`real`]: the machine itself (`/sys/class/net`, `/proc`, interface ioctls, the firewall socket). [`cli`]: `iyakki share`.
//! * [`control`]: the request and status files that the shell and the command line use to talk to the service (and a backend trait for
//!   the Settings page).
//!
//! Safe by default: sharing is off after every boot; devices on the cable can only ask for an address and for names (the device's own
//! services stay closed to them); they cannot reach each other or this computer's local network unless an administrator says so; and it
//! switches itself off after N hours (default 2) with nobody using it.

pub mod cli;
pub mod control;
pub mod io;
pub mod manager;
pub mod plan;
pub mod policy;
pub mod real;
pub mod rules;
pub mod view;

#[cfg(test)]
pub mod fake;
#[cfg(test)]
mod tests;

pub use iyakki_hotspot::subnet::Cidr;
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

/// The firewall table that holds everything sharing adds (and nothing else is ever removed by sharing).
pub const TABLE: &str = "iyakki_share";
pub const DEFAULT_IDLE_HOURS: u8 = 2;
pub const MAX_IDLE_HOURS: u8 = 72;
pub const LEASE_SECS: u32 = 3600;
/// Addresses handed out (`.10` upward). A cable has one or a few devices.
pub const POOL_SIZE: u8 = 16;
/// The name of this computer on the shared network.
pub const GATEWAY_NAME: &str = "iyakki.lan";
/// More than this many bytes moved on the shared interface in one check counts as "somebody is using it" (address requests and ARP are far smaller).
pub const ACTIVITY_BYTES: u64 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkKind {
    Ethernet,
    /// A USB network link: a gadget interface (`usb0`), or an RNDIS / ECM / NCM adapter.
    Usb,
    Thunderbolt,
    Wifi,
    /// This computer as a Bluetooth network access point (phones connect to it). Not built yet.
    BluetoothNap,
    /// This computer as a client of a paired phone's tethering (the uplink side). Not built yet.
    BluetoothPanu,
    Other,
}

impl LinkKind {
    /// Can this kind be used on this build? `Err` is the reason in one line. Bluetooth is modelled but not built (BNEP, SDP and
    /// pairing come later, see docs/development/HOTSPOT-PLAN.md, section Bluetooth); Wi-Fi needs a card that can be an access point.
    pub fn probe(self, wifi_ap: bool) -> Result<(), ShareError> {
        match self {
            LinkKind::BluetoothNap | LinkKind::BluetoothPanu => Err(ShareError::BluetoothNotAvailable),
            LinkKind::Wifi if !wifi_ap => Err(ShareError::WifiNotAvailable),
            LinkKind::Other => Err(ShareError::NotShareable("this device".into())),
            _ => Ok(()),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LinkKind::Ethernet => "Ethernet",
            LinkKind::Usb => "USB link",
            LinkKind::Thunderbolt => "Thunderbolt / USB-C link",
            LinkKind::Wifi => "Wi-Fi",
            LinkKind::BluetoothNap => "Bluetooth tethering (phones connect to this computer)",
            LinkKind::BluetoothPanu => "Bluetooth (use a phone's tethering)",
            LinkKind::Other => "other",
        }
    }
}

/// One network interface as sharing needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IfaceInfo {
    pub name: String,
    pub kind: LinkKind,
    /// The cable is plugged in and the other end answers.
    pub carrier: bool,
    pub up: bool,
    /// IPv4 addresses with their prefix.
    pub cidrs: Vec<iyakki_hotspot::subnet::Cidr>,
    pub default_route: bool,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

impl IfaceInfo {
    pub fn new(name: &str, kind: LinkKind) -> IfaceInfo {
        IfaceInfo { name: name.into(), kind, carrier: true, up: true, cidrs: Vec::new(), default_route: false, rx_bytes: 0, tx_bytes: 0 }
    }
}

/// A name that is safe to put in a path, a socket option and a firewall rule.
pub fn valid_iface(n: &str) -> bool {
    crate::fwextra_net::valid_iface(n) && !n.contains(':')
}

/// What was asked for. Written by the shell or `iyakki share`, read by the service. `id` goes up with every change so the service
/// can tell a new request from the old one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Request {
    pub id: u64,
    pub on: bool,
    pub uplink: Option<String>,
    pub via: Option<String>,
    pub idle_hours: u8,
    /// Devices on the shared network cannot talk to each other (on by default).
    pub isolate: bool,
    /// Devices may reach this computer's local network (the uplink's private ranges). Off by default.
    pub allow_lan: bool,
    pub by: String,
    /// The person who asked is an administrator (the service checks the policy again with this).
    pub by_admin: bool,
    /// Share through a Wi-Fi hotspot of this computer's own (the access point is made next to the Wi-Fi client; `via` is then the hotspot).
    pub wifi: bool,
}

impl Default for Request {
    fn default() -> Request {
        Request { id: 0, on: false, uplink: None, via: None, idle_hours: DEFAULT_IDLE_HOURS, isolate: true, allow_lan: false, by: String::new(), by_admin: false, wifi: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Off,
    On,
    Failed,
}

impl State {
    pub fn word(self) -> &'static str {
        match self {
            State::Off => "off",
            State::On => "on",
            State::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Client {
    pub name: String,
    pub ip: String,
    pub mac: String,
    /// Seconds until the address lease ends.
    pub expires_in: u64,
}

/// What the service reports. Never holds a secret.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub state: State,
    pub uplink: String,
    pub downstream: String,
    pub subnet: String,
    pub gateway: String,
    pub clients: Vec<Client>,
    /// Bytes received from the devices / sent to them since sharing started.
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub idle_hours: u8,
    pub isolate: bool,
    /// Seconds until the idle switch-off, when nobody is using it.
    pub off_in_secs: Option<u64>,
    /// One plain line: why it failed, or why it switched itself off.
    pub note: String,
    /// The request this status answers.
    pub request_id: u64,
    pub updated: u64,
}

impl Default for Status {
    fn default() -> Status {
        Status { state: State::Off, uplink: String::new(), downstream: String::new(), subnet: String::new(), gateway: String::new(), clients: Vec::new(), rx_bytes: 0, tx_bytes: 0, idle_hours: DEFAULT_IDLE_HOURS, isolate: true, off_in_secs: None, note: String::new(), request_id: 0, updated: 0 }
    }
}

impl Status {
    /// The one line under the page title.
    pub fn line(&self) -> String {
        match self.state {
            State::Off if self.note.is_empty() => "Sharing is off.".into(),
            State::Off => format!("Sharing is off. {}", self.note),
            State::Failed => format!("Sharing is not running. {}", self.note),
            State::On => {
                let n = self.clients.len();
                let who = match n {
                    0 => "no device yet".to_string(),
                    1 => "1 device".to_string(),
                    n => format!("{n} devices"),
                };
                let left = self.off_in_secs.map(|s| format!(" Switches off in {} if nobody uses it.", human_duration(s))).unwrap_or_default();
                format!("Sharing {} through {} on {} ({who}).{left}", self.uplink, self.downstream, self.subnet)
            }
        }
    }
}

/// `1 h 48 min`, `12 min`, `40 s`.
pub fn human_duration(s: u64) -> String {
    if s >= 3600 {
        let (h, m) = (s / 3600, s % 3600 / 60);
        if m == 0 {
            format!("{h} h")
        } else {
            format!("{h} h {m} min")
        }
    } else if s >= 60 {
        format!("{} min", s / 60)
    } else {
        format!("{s} s")
    }
}

/// Why sharing cannot start (or had to stop). `line` is the whole explanation, one sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareError {
    NoDefaultRoute,
    UnknownInterface(String),
    /// The chosen uplink has no route to the internet.
    UplinkHasNoRoute(String),
    SameInterface,
    /// Not a cable that can carry shared internet (loopback, container bridge, VPN ...).
    NotShareable(String),
    /// The downstream interface is already used for something else (it has an address or the default route).
    Busy(String),
    /// No free private network could be found, or the uplink now uses ours.
    AddressClash,
    WifiNotAvailable,
    BluetoothNotAvailable,
    /// The administrator's policy forbids it (the text is the whole line).
    Blocked(String),
    /// More than one possible device and none named.
    Ambiguous(Vec<String>),
    NoDownstream,
    BadHours,
    Firewall(String),
    Socket(String),
    Machine(String),
    UplinkLost(String),
    DownstreamLost(String),
}

impl ShareError {
    pub fn line(&self) -> String {
        match self {
            ShareError::NoDefaultRoute => "There is no internet to share: no connection on this computer has a route to the internet.".into(),
            ShareError::UnknownInterface(n) => format!("There is no network device called {n} on this computer."),
            ShareError::UplinkHasNoRoute(n) => format!("{n} has no route to the internet, so there is nothing to share from it."),
            ShareError::SameInterface => "The internet connection and the device to share through must be two different devices.".into(),
            ShareError::NotShareable(n) => format!("{n} cannot be used to share the internet: pick an Ethernet, USB or Thunderbolt link."),
            ShareError::Busy(n) => format!("{n} is busy: it already has an address or carries this computer's own internet. Unplug it from its network or choose another device."),
            ShareError::AddressClash => "No private network is free: every address range this computer could give out is already in use.".into(),
            ShareError::WifiNotAvailable => "Wi-Fi hotspot: not available on this build.".into(),
            ShareError::BluetoothNotAvailable => "Bluetooth tethering: not available on this build.".into(),
            ShareError::Blocked(t) => t.clone(),
            ShareError::Ambiguous(v) => format!("More than one device could be used ({}): choose one.", v.join(", ")),
            ShareError::NoDownstream => "No cable device to share through was found: plug in an Ethernet, USB or Thunderbolt link first.".into(),
            ShareError::BadHours => format!("The switch-off time must be between 1 and {MAX_IDLE_HOURS} hours."),
            ShareError::Firewall(e) => format!("The firewall rules for sharing could not be set: {e}."),
            ShareError::Socket(e) => format!("The address and name services could not start: {e}."),
            ShareError::Machine(e) => format!("Could not set up the network device: {e}."),
            ShareError::UplinkLost(n) => format!("Sharing stopped: {n} lost its route to the internet."),
            ShareError::DownstreamLost(n) => format!("Sharing stopped: {n} disappeared (unplugged?)."),
        }
    }
}

impl std::fmt::Display for ShareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.line())
    }
}

/// What the service remembers across a crash so that the next start can put the machine back (it holds no secret).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub downstream: String,
    pub address: Ipv4Addr,
    pub prefix: u8,
    /// `net.ipv4.ip_forward` before sharing switched it on.
    pub previous_forward: String,
}
