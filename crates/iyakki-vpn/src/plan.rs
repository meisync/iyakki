//! Routes, DNS text and the kill-switch table as **pure data** (plan section 2.3). Nothing here runs `ip`, writes a file or
//! talks to the kernel: the helper will take these lists and apply them, and undo them on disconnect or after a crash.
//!
//! No policy routing (the aarch64 kernel lacks it): a full tunnel is a host route to the endpoint through the old gateway plus
//! the two "def1" halves (`0.0.0.0/1`, `128.0.0.0/1`, `::/1`, `8000::/1`); the default route stays, so removing the tunnel
//! restores everything. Longer routes (cluster, LAN) keep winning over the halves.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

use crate::error::VpnError;
use crate::net::{check_route, Cidr, Context, RouteVerdict};
use crate::wgnl::valid_ifname;

// ---- routes ---------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub dest: Cidr,
    pub via: Option<IpAddr>,
    pub dev: String,
}

impl Route {
    /// Arguments for `ip` (a fixed argument list, no shell): `route add|del <dest> [via <gw>] dev <dev>`.
    pub fn ip_args(&self, add: bool) -> Vec<String> {
        let mut v = vec!["route".to_string(), if add { "add" } else { "del" }.to_string(), self.dest.to_string()];
        if let Some(g) = self.via {
            v.push("via".into());
            v.push(g.to_string());
        }
        v.push("dev".into());
        v.push(self.dev.clone());
        v
    }
}

pub struct RouteInput<'a> {
    /// The tunnel interface, for example `wg0`.
    pub ifname: &'a str,
    /// The interface the old default route uses.
    pub physical_if: &'a str,
    /// The server addresses, resolved once (the pinned ones).
    pub endpoints: &'a [IpAddr],
    pub allowed_ips: &'a [Cidr],
    pub gateway4: Option<Ipv4Addr>,
    pub gateway6: Option<Ipv6Addr>,
    /// Cluster CIDRs, the current LAN subnets and the tunnel's own addresses.
    pub ctx: &'a Context,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RoutePlan {
    /// In the order to apply them.
    pub add: Vec<Route>,
    /// In the order to undo them (the reverse of `add`).
    pub remove: Vec<Route>,
    pub full_tunnel_v4: bool,
    pub full_tunnel_v6: bool,
    pub warnings: Vec<String>,
}

/// Work out the routes. Refuses (with the reason) anything the route checks refuse, evaluated against the real network.
pub fn plan_routes(i: &RouteInput) -> Result<RoutePlan, VpnError> {
    if !valid_ifname(i.ifname) || !valid_ifname(i.physical_if) {
        return Err(VpnError::Unsafe("not a valid interface name".into()));
    }
    let mut plan = RoutePlan::default();
    let mut rctx = i.ctx.clone();
    // the endpoint pin is not a route the profile asked for
    for e in i.endpoints {
        let host = Cidr::new(*e, if e.is_ipv4() { 32 } else { 128 }).unwrap();
        if i.ctx.lan.iter().any(|l| l.contains(&host)) {
            continue; // on the connected network: already reachable, and a host route could only break roaming
        }
        let via = match e {
            IpAddr::V4(_) => i.gateway4.map(IpAddr::V4),
            IpAddr::V6(_) => i.gateway6.map(IpAddr::V6),
        };
        if via.is_none() {
            plan.warnings.push(format!("no gateway known for {e}; its route is on-link"));
        }
        plan.add.push(Route { dest: host, via, dev: i.physical_if.to_string() });
    }
    rctx.tunnel.extend(i.ctx.tunnel.iter().copied());
    let mut seen: Vec<Cidr> = Vec::new();
    for a in i.allowed_ips {
        let a = a.network();
        match check_route(&a, &rctx) {
            RouteVerdict::Ok => {}
            RouteVerdict::Warn(w) => plan.warnings.push(w),
            RouteVerdict::Refuse(w) => return Err(VpnError::Unsafe(w)),
        }
        let dests = if a.is_default() {
            if a.is_v4() {
                plan.full_tunnel_v4 = true;
            } else {
                plan.full_tunnel_v6 = true;
            }
            Cidr::def1_halves(a.is_v4()).to_vec()
        } else {
            vec![a]
        };
        for d in dests {
            if !seen.contains(&d) {
                seen.push(d);
                plan.add.push(Route { dest: d, via: None, dev: i.ifname.to_string() });
            }
        }
    }
    plan.remove = plan.add.iter().rev().cloned().collect();
    Ok(plan)
}

/// `ip link add dev wg0 type wireguard`
pub fn link_add_args(ifname: &str) -> Result<Vec<String>, VpnError> {
    check_if(ifname)?;
    Ok(["link", "add", "dev", ifname, "type", "wireguard"].iter().map(|s| s.to_string()).collect())
}

/// `ip link set dev wg0 mtu N up` (the MTU is optional).
pub fn link_up_args(ifname: &str, mtu: Option<u16>) -> Result<Vec<String>, VpnError> {
    check_if(ifname)?;
    let mut v: Vec<String> = ["link", "set", "dev", ifname].iter().map(|s| s.to_string()).collect();
    if let Some(m) = mtu {
        v.push("mtu".into());
        v.push(m.to_string());
    }
    v.push("up".into());
    Ok(v)
}

/// `ip address add <addr/prefix> dev wg0`
pub fn addr_add_args(ifname: &str, a: &Cidr) -> Result<Vec<String>, VpnError> {
    check_if(ifname)?;
    Ok(vec!["address".into(), "add".into(), a.to_string(), "dev".into(), ifname.into()])
}

/// `ip link del dev wg0`
pub fn link_del_args(ifname: &str) -> Result<Vec<String>, VpnError> {
    check_if(ifname)?;
    Ok(["link", "del", "dev", ifname].iter().map(|s| s.to_string()).collect())
}

fn check_if(n: &str) -> Result<(), VpnError> {
    if valid_ifname(n) {
        Ok(())
    } else {
        Err(VpnError::Unsafe("not a valid interface name".into()))
    }
}

// ---- DNS ------------------------------------------------------------------------------------------------------------

pub const RESOLV_CONF: &str = "/etc/resolv.conf";
/// Where the helper keeps the original while the VPN's DNS is in place (tmpfs).
pub const RESOLV_BACKUP: &str = "/run/iyakki/vpn/resolv.conf.orig";
/// First line of the file we write; restoring only happens while it is still there.
pub const DNS_MARKER: &str = "# IYAKKI-VPN-DNS: written while a VPN is connected; the original is restored on disconnect";

/// The new `/etc/resolv.conf`. `dns` must already have passed `net::check_dns`.
pub fn resolv_conf(dns: &[IpAddr]) -> String {
    let mut s = format!("{DNS_MARKER}\n");
    for d in dns {
        s.push_str(&format!("nameserver {d}\n"));
    }
    s
}

/// What to write back on disconnect (or after a crash). `None` means leave the file alone: it is not ours any more (a DHCP
/// renewal replaced it, which is correct) or there was no original.
pub fn resolv_restore(current: &str, backup: Option<&str>) -> Option<String> {
    if current.lines().next() != Some(DNS_MARKER) {
        return None;
    }
    backup.map(str::to_string)
}

/// The warning for a full tunnel whose DNS stays outside it.
pub fn dns_leak_warning(full_tunnel: bool, vpn_dns: &[IpAddr]) -> Option<String> {
    (full_tunnel && vpn_dns.is_empty()).then(|| "DNS goes to your normal servers outside the tunnel".to_string())
}

// ---- guard tables: kill switch (FULL) and fail-closed (HYBRID) -------------------------------------------------------

pub const KS_TABLE: &str = "iyakki_vpn";
pub const KS_FAMILY: &str = "inet";
pub const KS_CHAIN: &str = "output";
/// Before the Firewall's own chains and before a future outgoing filter at 0.
pub const KS_PRIORITY: i32 = -10;
/// The developer-mode port (`iyakki-remote`): replies to paired computers must never be dropped by the guard.
pub const DEV_PORT: u16 = 7979;
/// Names of the sets that hold the addresses learned from DNS in HYBRID mode (filled and emptied by the executor).
pub const SET_V4: &str = "vpn_dest4";
pub const SET_V6: &str = "vpn_dest6";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Proto {
    Udp,
    Tcp,
    Icmpv6,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IfMatch {
    Exact(String),
    /// `cni*`: a name that starts with this.
    Prefix(String),
}

impl IfMatch {
    fn matches(&self, n: &str) -> bool {
        match self {
            IfMatch::Exact(e) => e == n,
            IfMatch::Prefix(p) => n.starts_with(p.as_str()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Accept,
    Drop,
}

/// One rule: every given field must match. No field at all matches every packet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Match {
    pub oif: Option<IfMatch>,
    /// The packet leaves through anything but this interface.
    pub oif_not: Option<IfMatch>,
    pub daddr: Option<Cidr>,
    /// The destination is in this named set of the spec.
    pub daddr_set: Option<String>,
    pub proto: Option<Proto>,
    pub sport: Option<u16>,
    pub dport: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KsRule {
    pub m: Match,
    pub verdict: Verdict,
    /// Why it is there, shown in the Firewall tab and in the rendered text. Not carried over the helper's socket.
    #[serde(skip, default)]
    pub why: &'static str,
}

/// A named set of addresses. The spec carries its static members; the executor adds the learned ones with their timeouts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedSet {
    pub name: String,
    pub v4: bool,
    pub members: Vec<Cidr>,
}

/// The table `inet iyakki_vpn`, chain `output` (hook output, priority [`KS_PRIORITY`], policy accept). FULL mode ends in a drop
/// of everything else; HYBRID fail-closed ends in drops of the matched destinations only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillSwitchSpec {
    pub rules: Vec<KsRule>,
    pub sets: Vec<NamedSet>,
}

/// What both guards share. Always allowed whatever the mode: loopback, the tunnel, the VPN server, DHCP, neighbour discovery,
/// the cluster, the local network and replies of developer mode.
pub struct GuardBase<'a> {
    pub ifname: &'a str,
    /// The interfaces that carry the endpoint traffic (the old default route's, and the ones roaming may move to).
    pub physical_ifs: &'a [String],
    pub endpoints: &'a [(IpAddr, u16, Proto)],
    /// Connected subnets of the physical interfaces. **Always allowed**: the local network keeps working.
    pub lan: &'a [Cidr],
    pub cluster: &'a [Cidr],
    /// Name prefixes of the cluster's own devices: `cni`, `flannel`, `veth`.
    pub cluster_if_prefixes: &'a [&'a str],
    /// The developer-mode port; replies from it are allowed to any paired computer.
    pub dev_port: u16,
}

pub struct KillSwitchInput<'a> {
    pub base: GuardBase<'a>,
    /// Drop DNS (port 53) that does not go through the tunnel, even to the LAN. Opt-in; only meaningful when the VPN's DNS has
    /// replaced the system's (FULL mode). Never set for HYBRID.
    pub block_dns_outside: bool,
}

fn rule(m: Match, verdict: Verdict, why: &'static str) -> KsRule {
    KsRule { m, verdict, why }
}

/// The accept rules every guard starts with.
fn base_accepts(i: &GuardBase) -> Result<Vec<KsRule>, VpnError> {
    check_if(i.ifname)?;
    let a = Verdict::Accept;
    let mut r = Vec::new();
    r.push(rule(Match { oif: Some(IfMatch::Exact("lo".into())), ..Default::default() }, a, "loopback"));
    r.push(rule(Match { oif: Some(IfMatch::Exact(i.ifname.into())), ..Default::default() }, a, "the tunnel"));
    for (ip, port, proto) in i.endpoints {
        let host = Cidr::new(*ip, if ip.is_ipv4() { 32 } else { 128 }).unwrap();
        for p in i.physical_ifs {
            check_if(p)?;
            r.push(rule(Match { oif: Some(IfMatch::Exact(p.clone())), daddr: Some(host), proto: Some(*proto), dport: Some(*port), ..Default::default() }, a, "the VPN server"));
        }
    }
    r.push(rule(Match { proto: Some(Proto::Udp), sport: Some(68), dport: Some(67), ..Default::default() }, a, "DHCP"));
    r.push(rule(Match { proto: Some(Proto::Udp), sport: Some(546), dport: Some(547), ..Default::default() }, a, "DHCPv6"));
    for d in [Cidr::parse("fe80::/10").unwrap(), Cidr::parse("ff02::/16").unwrap()] {
        r.push(rule(Match { daddr: Some(d), proto: Some(Proto::Icmpv6), ..Default::default() }, a, "IPv6 neighbour discovery"));
    }
    r.push(rule(Match { proto: Some(Proto::Tcp), sport: Some(i.dev_port), ..Default::default() }, a, "developer mode replies"));
    for p in i.cluster_if_prefixes {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)) {
            return Err(VpnError::Unsafe("not a valid interface prefix".into()));
        }
        r.push(rule(Match { oif: Some(IfMatch::Prefix((*p).to_string())), ..Default::default() }, a, "the cluster's devices"));
    }
    for c in i.cluster {
        r.push(rule(Match { daddr: Some(*c), ..Default::default() }, a, "the cluster's networks"));
    }
    Ok(r)
}

fn lan_accepts(lan: &[Cidr]) -> Vec<KsRule> {
    lan.iter().map(|l| rule(Match { daddr: Some(l.network()), ..Default::default() }, Verdict::Accept, "the local network")).collect()
}

/// FULL mode kill switch. Installed **before** the tunnel comes up and kept after a drop. The local network, the cluster,
/// loopback, DHCP and developer-mode replies are always let through.
pub fn kill_switch(i: &KillSwitchInput) -> Result<KillSwitchSpec, VpnError> {
    let mut r = base_accepts(&i.base)?;
    if i.block_dns_outside {
        for p in [Proto::Udp, Proto::Tcp] {
            r.push(rule(Match { proto: Some(p), dport: Some(53), ..Default::default() }, Verdict::Drop, "DNS outside the tunnel"));
        }
    }
    r.extend(lan_accepts(i.base.lan));
    r.push(rule(Match::default(), Verdict::Drop, "everything else"));
    Ok(KillSwitchSpec { rules: r, sets: vec![] })
}

pub struct FailClosedInput<'a> {
    pub base: GuardBase<'a>,
    /// Address rules of the filters: `(range, include)`. Included ranges are dropped when they would leave outside the tunnel;
    /// excluded ranges are let through first (they may sit inside an included range).
    pub nets: &'a [(Cidr, bool)],
}

/// HYBRID fail-closed guard: only traffic to **matched destinations** is dropped when it would leave through anything but the
/// tunnel (the tunnel being down is exactly that case). Everything else, and everything in the base accepts, is untouched
/// (policy accept). Two named sets receive the addresses learned from DNS.
pub fn fail_closed_guard(i: &FailClosedInput) -> Result<KillSwitchSpec, VpnError> {
    let mut r = base_accepts(&i.base)?;
    r.extend(lan_accepts(i.base.lan));
    let tunnel = IfMatch::Exact(i.base.ifname.to_string());
    for (c, include) in i.nets {
        if !include {
            r.push(rule(Match { daddr: Some(c.network()), ..Default::default() }, Verdict::Accept, "excluded from the VPN"));
        }
    }
    for (c, include) in i.nets {
        if *include {
            r.push(rule(Match { oif_not: Some(tunnel.clone()), daddr: Some(c.network()), ..Default::default() }, Verdict::Drop, "VPN destination: never direct"));
        }
    }
    for set in [SET_V4, SET_V6] {
        r.push(rule(Match { oif_not: Some(tunnel.clone()), daddr_set: Some(set.to_string()), ..Default::default() }, Verdict::Drop, "VPN destination learned from DNS: never direct"));
    }
    Ok(KillSwitchSpec { rules: r, sets: vec![NamedSet { name: SET_V4.into(), v4: true, members: vec![] }, NamedSet { name: SET_V6.into(), v4: false, members: vec![] }] })
}

/// One outgoing packet, for tests and for "would this be blocked" in the UI.
#[derive(Clone, Debug)]
pub struct Packet {
    pub oif: String,
    pub daddr: IpAddr,
    pub proto: Proto,
    pub sport: u16,
    pub dport: u16,
}

impl KillSwitchSpec {
    /// A copy with the learned addresses in the sets (what the executor keeps in the kernel).
    pub fn with_members(&self, name: &str, members: Vec<Cidr>) -> KillSwitchSpec {
        let mut s = self.clone();
        if let Some(x) = s.sets.iter_mut().find(|x| x.name == name) {
            x.members = members;
        }
        s
    }

    pub fn evaluate(&self, p: &Packet) -> Verdict {
        for r in &self.rules {
            let m = &r.m;
            let in_set = |name: &String| self.sets.iter().find(|s| &s.name == name).is_some_and(|s| s.members.iter().any(|c| c.contains_ip(p.daddr)));
            let ok = m.oif.as_ref().is_none_or(|o| o.matches(&p.oif))
                && m.oif_not.as_ref().is_none_or(|o| !o.matches(&p.oif))
                && m.daddr.as_ref().is_none_or(|d| d.contains_ip(p.daddr))
                && m.daddr_set.as_ref().is_none_or(in_set)
                && m.proto.is_none_or(|x| x == p.proto)
                && m.sport.is_none_or(|x| x == p.sport)
                && m.dport.is_none_or(|x| x == p.dport);
            if ok {
                return r.verdict;
            }
        }
        Verdict::Accept // the chain policy
    }

    /// The same table as nft text, for the Firewall tab's one line and for reading in tests. The real thing is written
    /// through the Firewall's netlink writer, not by this text.
    pub fn render(&self) -> String {
        let mut s = format!("table {KS_FAMILY} {KS_TABLE} {{\n");
        for set in &self.sets {
            s.push_str(&format!("  set {} {{ type {}; flags interval, timeout; }}\n", set.name, if set.v4 { "ipv4_addr" } else { "ipv6_addr" }));
        }
        s.push_str(&format!("  chain {KS_CHAIN} {{\n    type filter hook output priority {KS_PRIORITY}; policy accept;\n"));
        for r in &self.rules {
            let m = &r.m;
            let mut parts: Vec<String> = Vec::new();
            match &m.oif {
                Some(IfMatch::Exact(n)) => parts.push(format!("oifname \"{n}\"")),
                Some(IfMatch::Prefix(n)) => parts.push(format!("oifname \"{n}*\"")),
                None => {}
            }
            match &m.oif_not {
                Some(IfMatch::Exact(n)) => parts.push(format!("oifname != \"{n}\"")),
                Some(IfMatch::Prefix(n)) => parts.push(format!("oifname != \"{n}*\"")),
                None => {}
            }
            if let Some(d) = &m.daddr {
                parts.push(format!("{} daddr {d}", if d.is_v4() { "ip" } else { "ip6" }));
            }
            if let Some(n) = &m.daddr_set {
                parts.push(format!("{} daddr @{n}", if n == SET_V6 { "ip6" } else { "ip" }));
            }
            match (m.proto, m.sport, m.dport) {
                (Some(Proto::Icmpv6), ..) => parts.push("meta l4proto icmpv6".into()),
                (Some(p), sp, dp) => {
                    let n = if p == Proto::Udp { "udp" } else { "tcp" };
                    if sp.is_none() && dp.is_none() {
                        parts.push(format!("meta l4proto {n}"));
                    }
                    if let Some(x) = sp {
                        parts.push(format!("{n} sport {x}"));
                    }
                    if let Some(x) = dp {
                        parts.push(format!("{n} dport {x}"));
                    }
                }
                (None, ..) => {}
            }
            parts.push(if r.verdict == Verdict::Accept { "accept".into() } else { "drop".into() });
            s.push_str(&format!("    {} comment \"{}\"\n", parts.join(" "), r.why));
        }
        s.push_str("  }\n}\n");
        s
    }
}

// ---- HYBRID routes ----------------------------------------------------------------------------------------------------

pub struct HybridRouteInput<'a> {
    pub ifname: &'a str,
    pub physical_if: &'a str,
    pub endpoints: &'a [IpAddr],
    /// The peers' AllowedIPs: WireGuard drops what they do not cover, so a route outside them is useless (warned, not added).
    pub allowed_ips: &'a [Cidr],
    pub gateway4: Option<Ipv4Addr>,
    pub gateway6: Option<Ipv6Addr>,
    pub ctx: &'a Context,
    /// The address rules of the filters: `(range, include)`.
    pub nets: &'a [(Cidr, bool)],
}

/// The **static** routes of a HYBRID profile: one route per included address range, a direct route for every exclusion that sits
/// inside an included range, and a pin for the VPN server when an included range covers it. Never `/0` or the `/1` halves, and
/// never the system resolver: the DNS-learned host routes are added later, one at a time, by [`crate::dnsroute`].
pub fn plan_hybrid_routes(i: &HybridRouteInput) -> Result<RoutePlan, VpnError> {
    if !valid_ifname(i.ifname) || !valid_ifname(i.physical_if) {
        return Err(VpnError::Unsafe("not a valid interface name".into()));
    }
    let mut plan = RoutePlan::default();
    let gw = |c: &Cidr| match c.addr {
        IpAddr::V4(_) => i.gateway4.map(IpAddr::V4),
        IpAddr::V6(_) => i.gateway6.map(IpAddr::V6),
    };
    let includes: Vec<Cidr> = i.nets.iter().filter(|(_, inc)| *inc).map(|(c, _)| c.network()).collect();
    for c in &includes {
        if c.prefix <= 1 {
            return Err(VpnError::Unsafe("a HYBRID profile cannot route a default route or a half of the address space".into()));
        }
        match check_route(c, i.ctx) {
            RouteVerdict::Ok => {}
            RouteVerdict::Warn(w) => plan.warnings.push(w),
            RouteVerdict::Refuse(w) => return Err(VpnError::Unsafe(w)),
        }
    }
    // pin the VPN server first when an included range covers it (otherwise its own packets would loop into the tunnel)
    for e in i.endpoints {
        let host = Cidr::new(*e, if e.is_ipv4() { 32 } else { 128 }).unwrap();
        if includes.iter().any(|c| c.contains(&host)) && !i.ctx.lan.iter().any(|l| l.contains(&host)) {
            let via = gw(&host);
            if via.is_none() {
                plan.warnings.push(format!("no gateway known for {e}; its route is on-link"));
            }
            plan.add.push(Route { dest: host, via, dev: i.physical_if.to_string() });
        }
    }
    let mut seen: Vec<Cidr> = Vec::new();
    for c in &includes {
        if !i.allowed_ips.iter().any(|a| a.contains(c)) {
            plan.warnings.push(format!("{c} is not covered by the peer's AllowedIPs; the tunnel would drop it, so no route was made"));
            continue;
        }
        if !seen.contains(c) {
            seen.push(*c);
            plan.add.push(Route { dest: *c, via: None, dev: i.ifname.to_string() });
        }
    }
    for (x, inc) in i.nets {
        let x = x.network();
        if !*inc && seen.iter().any(|c| c.contains(&x)) {
            plan.add.push(Route { dest: x, via: gw(&x), dev: i.physical_if.to_string() });
        }
    }
    plan.remove = plan.add.iter().rev().cloned().collect();
    Ok(plan)
}

// ---- DNS capture (HYBRID) ---------------------------------------------------------------------------------------------

pub const DNS_TABLE: &str = "iyakki_vpn_dns";
pub const DNS_CHAIN: &str = "output";
pub const DNS_PRIORITY: i32 = -100;
/// The socket mark the forwarder puts on its own upstream queries so they are not captured again.
pub const DNS_MARK: u32 = 0x4956_0001;

/// The NAT rule that sends this machine's own DNS queries to the local forwarder **without touching `/etc/resolv.conf`**:
/// `inet iyakki_vpn_dns`, chain `output` of type nat at priority -100. Needs `CONFIG_NFT_REDIR` / `CONFIG_NFT_NAT` (both `=m` in
/// the kernel fragments today). Queries to port 53 of any address are redirected to the forwarder's loopback port unless they
/// carry the forwarder's mark.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsCaptureSpec {
    pub listen_port: u16,
    pub mark: u32,
}

pub fn dns_capture(listen_port: u16) -> Result<DnsCaptureSpec, VpnError> {
    if listen_port < 1024 {
        return Err(VpnError::Unsafe("the DNS forwarder listens on an unprivileged port".into()));
    }
    Ok(DnsCaptureSpec { listen_port, mark: DNS_MARK })
}

impl DnsCaptureSpec {
    /// Would this outgoing packet be redirected to the forwarder?
    pub fn captures(&self, proto: Proto, dport: u16, mark: u32) -> bool {
        matches!(proto, Proto::Udp | Proto::Tcp) && dport == 53 && mark != self.mark
    }
    pub fn render(&self) -> String {
        format!(
            "table inet {DNS_TABLE} {{\n  chain {DNS_CHAIN} {{\n    type nat hook output priority {DNS_PRIORITY}; policy accept;\n    meta mark {:#x} return comment \"the forwarder's own queries\"\n    udp dport 53 redirect to :{} comment \"DNS to the forwarder\"\n    tcp dport 53 redirect to :{} comment \"DNS to the forwarder\"\n  }}\n}}\n",
            self.mark, self.listen_port, self.listen_port
        )
    }
}
