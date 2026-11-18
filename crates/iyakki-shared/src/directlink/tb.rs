//! Thunderbolt / USB4 as `/sys/bus/thunderbolt/devices` and the PCI ids show it, and what to do about authorization.
//!
//! Names in that folder (kernel `Documentation/admin-guide/thunderbolt.rst`): `domainN` (attribute `security`), `N-R` for a router
//! (attributes `authorized`, `vendor_name`, `device_name`, `unique_id`) or, when the other end is a computer, a host-to-host
//! connection (an XDomain: `unique_id`, `vendor_name`, `device_name`, `rx_speed`, `tx_speed`, `rx_lanes`, `tx_lanes`, no
//! `authorized`), and `N-R.S` for a service of it (`network` is the one `thunderbolt-net` binds to). Told apart here by which
//! attributes exist. Host-to-host networking is done by the XDomain and needs no PCIe tunnel, so the PCIe authorization
//! levels do not stop it (this is from the kernel documentation; it has not been tried on two real machines by this code).

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::Env;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Router,
    /// The other computer, joined host to host.
    XDomain,
    Service,
    /// This computer's own router (`N-0`, one per controller). It is always there, so it never counts as something connected.
    Host,
}

/// A Thunderbolt/USB4 host controller on the PCI bus with its power state (`power/control`, `power/runtime_status`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Controller {
    pub pci: String,
    /// `vendor:device`, for example `8086:15eb`.
    pub id: String,
    pub control: String,
    pub runtime_status: String,
}

/// What a lane adapter (one half of a Thunderbolt port) says about its cable link: the state field of its register (bits 26 to 29 of
/// the second word of the lane capability, capability id 1), as the kernel's debug files show the registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaneState {
    Disabled,
    /// A cable is seen and the link is being set up.
    Connecting,
    /// The link is up (also the low-power link states).
    Up,
    /// Nothing is seen on the port.
    Unplugged,
    Unknown(u8),
}

impl LaneState {
    pub fn from_bits(v: u8) -> LaneState {
        match v {
            0 => LaneState::Disabled,
            1 => LaneState::Connecting,
            2..=6 => LaneState::Up,
            7 => LaneState::Unplugged,
            n => LaneState::Unknown(n),
        }
    }
    pub fn word(self) -> &'static str {
        match self {
            LaneState::Disabled => "disabled",
            LaneState::Connecting => "connecting",
            LaneState::Up => "up",
            LaneState::Unplugged => "nothing plugged in",
            LaneState::Unknown(_) => "unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lane {
    /// The host router it belongs to (`0-0`).
    pub router: String,
    pub port: u32,
    pub state: LaneState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub name: String,
    pub kind: Kind,
    /// `authorized` of a router: 0 no, 1 yes, 2 yes after a key challenge.
    pub authorized: Option<u8>,
    pub vendor: String,
    pub model: String,
    pub unique_id: String,
    /// Link speed per direction in Gbit/s and the number of lanes, from an XDomain.
    pub rx_gbps: Option<f64>,
    pub tx_gbps: Option<f64>,
    pub lanes: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TbView {
    /// A Thunderbolt or USB4 host controller is on the PCI bus.
    pub controller_present: bool,
    /// `/sys/bus/thunderbolt` exists (the `thunderbolt` driver is loaded).
    pub driver_loaded: bool,
    /// Security level of the first domain: none, user, secure, dponly, usbonly, nopcie.
    pub security: String,
    pub devices: Vec<Device>,
    pub controllers: Vec<Controller>,
    /// The lane adapters of this computer's own routers, when the kernel's debug files could be read (root, debugfs mounted).
    pub lanes: Vec<Lane>,
    pub lanes_readable: bool,
    /// Interrupts the `thunderbolt` driver has taken since the start (all processors, all controllers), from `/proc/interrupts`.
    pub irqs: Option<u64>,
    /// An Apple computer (`sys_vendor`): the kernel drives its Thunderbolt ports itself, with no help from the firmware.
    pub apple: bool,
}

impl TbView {
    /// The other computer, when one is connected host to host.
    pub fn peer_computer(&self) -> Option<&Device> {
        self.devices.iter().find(|d| d.kind == Kind::XDomain)
    }
    /// Something other than a computer is connected (a dock, a display, a drive), approved or not.
    pub fn other_device(&self) -> Option<&Device> {
        self.devices.iter().find(|d| d.kind == Kind::Router)
    }
    /// Routers (docks, displays, drives) that are not authorized yet.
    pub fn pending(&self) -> Vec<&Device> {
        self.devices.iter().filter(|d| d.kind == Kind::Router && d.authorized == Some(0)).collect()
    }
}

pub fn security_words(level: &str) -> &'static str {
    match level {
        "none" => "no approval needed: devices are connected at once",
        "user" => "you approve each new device (the Authorize button below)",
        "secure" => "you approve each new device once, then it must prove it knows a key every time",
        "dponly" => "only display tunnels and host-to-host links; no PCIe devices (nothing to approve)",
        "usbonly" => "only USB and display tunnels; no PCIe devices (nothing to approve)",
        "nopcie" => "no PCIe tunnels (nothing to approve); host-to-host links still work",
        _ => "unknown security level",
    }
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// "20.0 Gb/s" to 20.0.
pub fn parse_gbps(s: &str) -> Option<f64> {
    let n: f64 = s.split_whitespace().next()?.parse().ok()?;
    (n.is_finite() && (0.0..=1000.0).contains(&n)).then_some(n)
}

fn pci_is_thunderbolt(class: &str, vendor: &str) -> bool {
    // USB4 host interface (class 0x0c0340), or the Intel Thunderbolt NHI of Thunderbolt 3 (class 0x088000, vendor Intel)
    let c = class.trim().trim_start_matches("0x");
    c == "0c0340" || (c == "088000" && vendor.trim().trim_start_matches("0x") == "8086")
}

/// Read the Thunderbolt view under `sys` (a copy of `/sys` in the tests).
pub fn read_view(sys: &Path) -> TbView {
    let mut v = TbView::default();
    if let Ok(rd) = std::fs::read_dir(sys.join("bus/pci/devices")) {
        for e in rd.flatten().take(4096) {
            let p = e.path();
            let vendor = read(&p.join("vendor"));
            if pci_is_thunderbolt(&read(&p.join("class")), &vendor) {
                let hex = |s: &str| s.trim().trim_start_matches("0x").to_string();
                v.controllers.push(Controller { pci: e.file_name().to_string_lossy().into_owned(), id: format!("{}:{}", hex(&vendor), hex(&read(&p.join("device")))), control: read(&p.join("power/control")), runtime_status: read(&p.join("power/runtime_status")) });
            }
        }
        v.controllers.sort_by(|a, b| a.pci.cmp(&b.pci));
        v.controller_present = !v.controllers.is_empty();
    }
    v.apple = read(&sys.join("class/dmi/id/sys_vendor")).starts_with("Apple");
    let dir = sys.join("bus/thunderbolt/devices");
    let Ok(rd) = std::fs::read_dir(&dir) else { return v };
    v.driver_loaded = true;
    let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    for name in names.into_iter().take(64) {
        let p = dir.join(&name);
        if name.starts_with("domain") {
            v.controller_present = true;
            if v.security.is_empty() {
                v.security = read(&p.join("security"));
            }
            continue;
        }
        let has = |f: &str| p.join(f).exists();
        let kind = if name.contains('.') {
            Kind::Service
        } else if name.ends_with("-0") {
            Kind::Host
        } else if has("authorized") {
            Kind::Router
        } else if has("unique_id") {
            Kind::XDomain
        } else {
            continue;
        };
        let lanes = |f: &str| read(&p.join(f)).parse::<u32>().ok();
        v.devices.push(Device {
            kind,
            authorized: if kind == Kind::Router { read(&p.join("authorized")).parse().ok() } else { None },
            vendor: read(&p.join("vendor_name")),
            model: read(&p.join("device_name")),
            unique_id: read(&p.join("unique_id")),
            rx_gbps: parse_gbps(&read(&p.join("rx_speed"))),
            tx_gbps: parse_gbps(&read(&p.join("tx_speed"))),
            lanes: lanes("rx_lanes"),
            name,
        });
    }
    v
}

/// One line of a `regs` file under `/sys/kernel/debug/thunderbolt` (`offset relative_offset cap_id vs_cap_id value`).
fn reg_line(l: &str) -> Option<(u32, u32, u32, u32)> {
    let f: Vec<&str> = l.split_whitespace().collect();
    if f.len() != 5 {
        return None;
    }
    let hex = |s: &str| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok();
    Some((f[1].parse().ok()?, hex(f[2])?, hex(f[3])?, hex(f[4])?))
}

/// The adapter type and, for a lane adapter, its link state, from the text of a port's `regs` file. `None`: not a lane adapter, or
/// the text is not a register list.
pub fn lane_state_from_regs(text: &str) -> Option<LaneState> {
    let mut kind = None;
    let mut state = None;
    for (rel, cap, vs, val) in text.lines().take(4096).filter_map(reg_line) {
        if cap == 0 && vs == 0 && rel == 2 && kind.is_none() {
            kind = Some(val & 0x00ff_ffff);
        } else if cap == 1 && vs == 0 && rel == 1 && state.is_none() {
            state = Some(LaneState::from_bits(((val >> 26) & 0xf) as u8));
        }
    }
    // adapter type 1 is a lane adapter (the others are the controller itself, DisplayPort, PCIe and USB adapters)
    if kind == Some(1) { state } else { None }
}

/// The lane adapters of this computer's own routers from the kernel's debug files, `None` when none of them can be read (not root, or
/// debugfs is not there). Reading them makes the kernel ask the controller for every register of every port, which takes a moment and
/// raises interrupts, so [`read_view`] leaves them out and the callers ask only when no other computer is seen.
pub fn read_lanes(sys: &Path, view: &TbView) -> Option<Vec<Lane>> {
    // a Secure Boot image runs under kernel lockdown: debugfs reads are refused for everyone and each try lands in the kernel log
    if debugfs_locked(sys) {
        return None;
    }
    let mut readable = false;
    let mut lanes = Vec::new();
    for router in view.devices.iter().filter(|d| d.kind == Kind::Host).take(8) {
        let dir = sys.join("kernel/debug/thunderbolt").join(&router.name);
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut ports: Vec<u32> = rd.flatten().filter_map(|e| e.file_name().to_string_lossy().strip_prefix("port").and_then(|n| n.parse().ok())).collect();
        ports.sort_unstable();
        for port in ports.into_iter().take(64) {
            let Ok(text) = std::fs::read_to_string(dir.join(format!("port{port}/regs"))) else { continue };
            readable = true;
            if let Some(state) = lane_state_from_regs(&text) {
                lanes.push(Lane { router: router.name.clone(), port, state });
            }
        }
    }
    readable.then_some(lanes)
}

/// [`read_view`] with the lane states filled in (the tests; the real machine goes through [`with_lanes`]).
pub fn read_view_full(sys: &Path) -> TbView {
    let mut v = read_view(sys);
    if let Some(l) = read_lanes(sys, &v) {
        v.lanes_readable = true;
        v.lanes = l;
    }
    v
}

/// The view with the lane states added when they are worth asking for: a controller is there and nothing is connected to it.
pub fn with_lanes(env: &dyn Env) -> TbView {
    let mut v = env.tb();
    if !v.lanes_readable && v.controller_present && v.driver_loaded && v.peer_computer().is_none() && v.other_device().is_none() {
        if let Some(l) = env.tb_lanes() {
            v.lanes_readable = true;
            v.lanes = l;
        }
    }
    v
}

/// The interrupts of the `thunderbolt` driver in the text of `/proc/interrupts`, summed over processors and lines. `None` when it has no line.
pub fn irq_total(interrupts: &str) -> Option<u64> {
    let mut total = None;
    for l in interrupts.lines().filter(|l| l.split_whitespace().last() == Some("thunderbolt")) {
        let n: u64 = l.split_whitespace().skip(1).map_while(|f| f.parse::<u64>().ok()).fold(0, u64::saturating_add);
        total = Some(total.unwrap_or(0u64).saturating_add(n));
    }
    total
}

/// The USB port named in a kernel log line "usb usb2-port2: Cannot enable. Maybe the USB cable is bad?" (the newest one). A SuperSpeed
/// port says this when the other end of the cable is a USB host too (seen on the Lenovo joined to a Mac by a USB-C cable, 2026-11-15);
/// a broken cable or device gives the same line.
pub fn two_hosts_port(log: &[String]) -> Option<String> {
    log.iter().rev().find(|l| l.contains("Cannot enable. Maybe the USB cable is bad?")).map(|l| {
        let port = l.split_whitespace().find(|w| w.contains("-port")).unwrap_or("a USB port").trim_end_matches(':');
        port.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == ' ').take(24).collect()
    })
}

/// The words for [`two_hosts_port`].
pub fn two_hosts_words(port: &str) -> String {
    format!("The kernel log says {port} could not be enabled (\"Maybe the USB cable is bad?\"). Most often that means both ends are USB hosts: the port tried plain USB and found another computer instead of a device. This cable needs a Thunderbolt/USB4 port on both computers, or use a host-to-host bridge cable.")
}

/// Why no other computer is seen, as far as this computer can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoPeer {
    /// The controller is in its power-saving sleep.
    Asleep,
    /// The controller is awake and no port sees a cable link.
    NoLink,
    /// A port sees a cable and is still setting the link up.
    Training,
    /// The link is up, but what is at the other end did not introduce itself as a computer.
    LinkNoAnswer,
    /// The port states cannot be read here.
    Unknown,
}

/// The likely cause when nothing is connected, and plain words for it.
pub fn no_peer(v: &TbView) -> (NoPeer, String) {
    const CABLE: &str = "the cable is not a Thunderbolt 3/4 or USB4 cable (a charging cable, a USB-only cable and many monitor cables have no Thunderbolt wiring or marking), it is not pushed in fully, the port on the other computer is not a Thunderbolt/USB4 port, or the other computer is switched off";
    if !v.controllers.is_empty() && v.controllers.iter().all(|c| c.runtime_status == "suspended") {
        return (NoPeer::Asleep, "The Thunderbolt controller is asleep to save power. It should wake when a cable goes in; if plugging the cable in again changes nothing, restart this computer with the cable already connected.".into());
    }
    if v.lanes_readable && !v.lanes.is_empty() {
        let port = |l: &Lane| format!("{} port {}", l.router, l.port);
        if let Some(l) = v.lanes.iter().find(|l| l.state == LaneState::Up) {
            return (NoPeer::LinkNoAnswer, format!("A cable link is up on {}, but the other end did not introduce itself as a computer. The other computer must run IYAKKI (or a system with Thunderbolt networking); wait ten seconds and check again.", port(l)));
        }
        if let Some(l) = v.lanes.iter().find(|l| l.state == LaneState::Connecting) {
            return (NoPeer::Training, format!("A cable is seen on {} and the link is still being set up. If it stays like this, the cable is too long or not good enough for Thunderbolt, or the other computer is starting.", port(l)));
        }
        return (NoPeer::NoLink, format!("The controller is awake and none of its {} port lanes sees a cable link, so the port never switched to Thunderbolt mode. Most likely: {CABLE}.", v.lanes.len()));
    }
    (NoPeer::Unknown, "To find out whether this port notices the cable at all, run `iyakki dev usb watch` and plug the cable out and in while it counts.".into())
}

/// What the Authorize button does for a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Auth {
    /// Nothing to do (already authorized, a host-to-host connection, or a level without PCIe tunnels).
    NotNeeded(&'static str),
    /// Write 1.
    Approve,
    /// Make and store a key, write it, then 1 (security level `secure`, first time).
    ApproveWithNewKey,
    /// Write 2: the stored key is challenged (level `secure`, a device approved before).
    Challenge,
}

pub fn auth_plan(security: &str, d: &Device, have_key: bool) -> Auth {
    if d.kind == Kind::Host {
        return Auth::NotNeeded("this is the computer's own controller");
    }
    if d.kind != Kind::Router {
        return Auth::NotNeeded("host-to-host connections need no approval");
    }
    match d.authorized {
        Some(1) | Some(2) => return Auth::NotNeeded("already approved"),
        None => return Auth::NotNeeded("this kernel does not offer an approval for it"),
        _ => {}
    }
    match security {
        "dponly" | "usbonly" | "nopcie" => Auth::NotNeeded("this security level never tunnels PCIe, so there is nothing to approve"),
        "secure" if have_key => Auth::Challenge,
        "secure" => Auth::ApproveWithNewKey,
        _ => Auth::Approve,
    }
}

/// Approve a device by name, as an administrator pressed the button. Returns what was done, in words.
pub fn authorize(env: &dyn Env, device: &str) -> Result<String, String> {
    let view = env.tb();
    let d = view.devices.iter().find(|d| d.name == device).ok_or_else(|| format!("{device} is not connected any more"))?;
    // the name goes into a file path under /sys: only what the kernel makes, nothing else
    if !device.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.')) {
        return Err("not a Thunderbolt device name".into());
    }
    let key = env.tb_key(&d.unique_id);
    match auth_plan(&view.security, d, key.is_some()) {
        Auth::NotNeeded(why) => Ok(format!("Nothing to do for {device}: {why}.")),
        Auth::Approve => env.tb_write_authorized(device, 1, None).map(|_| format!("{device} is approved.")),
        Auth::ApproveWithNewKey => {
            let k = env.random_hex32();
            env.tb_store_key(&d.unique_id, &k)?;
            env.tb_write_authorized(device, 1, Some(&k)).map(|_| format!("{device} is approved and its key is kept for next time."))
        }
        Auth::Challenge => {
            let k = key.unwrap_or_default();
            env.tb_write_authorized(device, 2, Some(&k)).map(|_| format!("{device} proved its key and is approved."))
        }
    }
}

/// Kernel lockdown is on (a Secure Boot image): debugfs files cannot be read. Same rule as `usbports::debugfs_locked` of the agent.
fn debugfs_locked(sys: &Path) -> bool {
    std::fs::read_to_string(sys.join("kernel/security/lockdown")).is_ok_and(|t| t.split_whitespace().any(|w| w.starts_with('[') && w.ends_with(']') && w != "[none]"))
}
