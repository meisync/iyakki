//! Choosing what to share and through what: pure functions over the list of interfaces.

use super::policy::Policy;
use super::{valid_iface, IfaceInfo, LinkKind, Request, ShareError, MAX_IDLE_HOURS};
use iyakki_hotspot::subnet::{choose_subnet, Cidr, HotspotNet};
use std::net::Ipv4Addr;

/// Everything decided before anything is touched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub uplink: String,
    pub down: String,
    pub net: HotspotNet,
    pub isolate: bool,
    pub allow_lan: bool,
    pub idle_secs: u64,
    pub resolvers: Vec<Ipv4Addr>,
    /// The ports of a bridged downstream (several member ports under one address, e.g. a Bluetooth access point later); empty: `down` is a single port.
    pub members: Vec<String>,
    /// Addresses in the DHCP pool (the policy's device limit, at most [`super::POOL_SIZE`]).
    pub pool: u8,
    pub kind: LinkKind,
}

impl Plan {
    pub fn gateway(&self) -> Ipv4Addr {
        self.net.gateway()
    }
}

fn link_local(c: &Cidr) -> bool {
    c.addr.octets()[..2] == [169, 254]
}

/// Can this interface carry shared internet at all (by kind)?
pub fn shareable_kind(k: LinkKind) -> bool {
    matches!(k, LinkKind::Ethernet | LinkKind::Usb | LinkKind::Thunderbolt | LinkKind::Wifi)
}

/// Is the interface free to be given to other computers? `Err` is the reason in a few words.
pub fn free_for_sharing(i: &IfaceInfo) -> Result<(), ShareError> {
    if !shareable_kind(i.kind) {
        return Err(ShareError::NotShareable(i.name.clone()));
    }
    // a Wi-Fi interface is only ever shared as the hotspot this computer made itself, never a client interface
    if i.kind == LinkKind::Wifi && i.name != crate::wifi_ap::AP_IFACE {
        return Err(ShareError::NotShareable(i.name.clone()));
    }
    if i.default_route || i.cidrs.iter().any(|c| !link_local(c)) {
        return Err(ShareError::Busy(i.name.clone()));
    }
    Ok(())
}

/// The interfaces that have a route to the internet, in the order of the list.
pub fn uplink_names(ifs: &[IfaceInfo]) -> Vec<String> {
    ifs.iter().filter(|i| i.default_route && i.up).map(|i| i.name.clone()).collect()
}

/// The networks (not the default route) that the routing table (`/proc/net/route` text) sends through each interface, as `(interface, network)`.
/// A VPN or a router can route a range the machine has no address in; the hotspot must not take that range.
pub fn routed_ranges_from(text: &str) -> Vec<(String, Cidr)> {
    let mut v = Vec::new();
    for l in text.lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 8 {
            continue;
        }
        let (Ok(dest), Ok(mask)) = (u32::from_str_radix(f[1], 16), u32::from_str_radix(f[7], 16)) else { continue };
        let (dest, mask) = (dest.swap_bytes(), mask.swap_bytes());
        // a mask is a run of ones then zeros; anything else is not a network
        if mask == 0 || mask.leading_ones() + mask.trailing_zeros() != 32 {
            continue;
        }
        if let Some(c) = Cidr::new(Ipv4Addr::from(dest), mask.leading_ones() as u8) {
            if !link_local(&c) && !v.iter().any(|(i, x): &(String, Cidr)| i == f[0] && *x == c) {
                v.push((f[0].to_string(), c));
            }
        }
    }
    v
}

/// The first resolvers in the text of `/etc/resolv.conf` (IPv4 only), at most three.
pub fn resolvers_from(text: &str) -> Vec<Ipv4Addr> {
    let mut v: Vec<Ipv4Addr> = Vec::new();
    for l in text.lines() {
        let mut w = l.split_whitespace();
        if w.next() == Some("nameserver") {
            if let Some(Ok(ip)) = w.next().map(|x| x.parse::<Ipv4Addr>()) {
                if !ip.is_unspecified() && !v.contains(&ip) {
                    v.push(ip);
                }
            }
        }
        if v.len() == 3 {
            break;
        }
    }
    v
}

/// The public resolvers used when the machine names none (the same two the firmware flow uses).
pub const FALLBACK_RESOLVERS: [Ipv4Addr; 2] = [Ipv4Addr::new(9, 9, 9, 9), Ipv4Addr::new(1, 1, 1, 1)];

/// Decide everything for a request. `wifi_ap` says whether this build can start a Wi-Fi access point (it cannot yet).
pub fn make_plan(ifs: &[IfaceInfo], req: &Request, wifi_ap: bool, resolvers: &[Ipv4Addr]) -> Result<Plan, ShareError> {
    if req.idle_hours == 0 || req.idle_hours > MAX_IDLE_HOURS {
        return Err(ShareError::BadHours);
    }
    // a hotspot request: the access point is made by the service when it starts, so it is planned as if it were already there
    let (with_ap, req_ap);
    let (ifs, req) = if req.wifi {
        let mut v = ifs.to_vec();
        if !v.iter().any(|i| i.name == crate::wifi_ap::AP_IFACE) {
            v.push(IfaceInfo::new(crate::wifi_ap::AP_IFACE, LinkKind::Wifi));
        }
        with_ap = v;
        req_ap = Request { via: Some(crate::wifi_ap::AP_IFACE.into()), ..req.clone() };
        (&with_ap[..], &req_ap)
    } else {
        (ifs, req)
    };
    for n in req.uplink.iter().chain(req.via.iter()) {
        if !valid_iface(n) || !ifs.iter().any(|i| &i.name == n) {
            return Err(ShareError::UnknownInterface(n.clone()));
        }
    }
    let find = |n: &str| ifs.iter().find(|i| i.name == n);
    // the interface to share through
    let down: &IfaceInfo = match &req.via {
        Some(v) => find(v).ok_or_else(|| ShareError::UnknownInterface(v.clone()))?,
        None => {
            let ok: Vec<&IfaceInfo> = ifs.iter().filter(|i| free_for_sharing(i).is_ok() && i.up && Some(&i.name) != req.uplink.as_ref()).collect();
            match ok.as_slice() {
                [one] => one,
                [] => return Err(ShareError::NoDownstream),
                many => return Err(ShareError::Ambiguous(many.iter().map(|i| i.name.clone()).collect())),
            }
        }
    };
    if down.kind == LinkKind::Other {
        return Err(ShareError::NotShareable(down.name.clone()));
    }
    down.kind.probe(wifi_ap)?;
    // the internet connection
    let up: &IfaceInfo = match &req.uplink {
        Some(u) => {
            let i = find(u).ok_or_else(|| ShareError::UnknownInterface(u.clone()))?;
            if i.name == down.name {
                return Err(ShareError::SameInterface);
            }
            if !i.default_route {
                return Err(ShareError::UplinkHasNoRoute(i.name.clone()));
            }
            i
        }
        None => ifs.iter().find(|i| i.default_route && i.up && i.name != down.name).ok_or(ShareError::NoDefaultRoute)?,
    };
    if up.name == down.name {
        return Err(ShareError::SameInterface);
    }
    free_for_sharing(down)?;
    // a private /24 that overlaps nothing on this computer
    let avoid: Vec<Cidr> = ifs.iter().flat_map(|i| i.cidrs.iter().chain(i.routed.iter()).copied()).filter(|c| !link_local(c)).collect();
    let net = choose_subnet(&avoid).ok_or(ShareError::AddressClash)?;
    let mut r = resolvers.to_vec();
    if r.is_empty() {
        r = FALLBACK_RESOLVERS.to_vec();
    }
    Ok(Plan { uplink: up.name.clone(), down: down.name.clone(), net, isolate: req.isolate, allow_lan: req.allow_lan, idle_secs: u64::from(req.idle_hours) * 3600, resolvers: r, members: Vec::new(), pool: super::POOL_SIZE, kind: down.kind })
}

/// The administrator's policy applied to a plan: who may start through this kind, which interfaces, how many devices.
pub fn apply_policy(plan: &mut Plan, policy: &Policy, req: &Request) -> Result<(), ShareError> {
    policy.may_start(plan.kind, req.by_admin)?;
    policy.check_names(&plan.down, &plan.uplink)?;
    if policy.max_clients > 0 {
        plan.pool = plan.pool.min(policy.max_clients).max(1);
    }
    Ok(())
}
