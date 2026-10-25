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
}

impl TbView {
    /// The other computer, when one is connected host to host.
    pub fn peer_computer(&self) -> Option<&Device> {
        self.devices.iter().find(|d| d.kind == Kind::XDomain)
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
        v.controller_present = rd.flatten().any(|e| pci_is_thunderbolt(&read(&e.path().join("class")), &read(&e.path().join("vendor"))));
    }
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
