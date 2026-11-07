//! From settings to rules. [`build`] is a pure function: the same settings, resolved addresses and context always give the
//! same [`Ruleset`], which is a list of typed steps (never text), so nothing a person typed can change what a rule *means*.
//! The kernel writer ([`crate::nl`]) encodes the steps; [`Ruleset::to_text`] shows them in nft-like words for the page and
//! the tests; [`eval`] answers "would this packet get in?" for the tests and the Shared folders note.
//!
//! Our table is `inet iyakki_fw` with one chain on the INPUT hook, default policy drop. Nothing else is touched: k3s, kube-proxy
//! and any other table keep their own rules, and the whole ruleset is never flushed.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::model::{Config, Ping, Preset, Proto, Resolved};
use crate::validate::{check_rule, PortRange, Src};

pub const TABLE: &str = "iyakki_fw";
pub const CHAIN: &str = "input";
/// The nflog group the blocked-connection log listens on.
pub const LOG_GROUP: u16 = 217;
pub const MAX_RULES_OUT: usize = 3000;
/// Where `iyakki-remote` listens unless told otherwise.
pub const REMOTE_PORT: u16 = 8443;
pub const SHARE_PORTS: (u16, u16) = (8081, 8180);
/// Developer mode (`iyakki dev`), the only TCP port a direct link (one cable between two computers) lets in by default.
pub const DEV_PORT: u16 = 7979;
/// The small UDP "who is there" beacon of a direct link (`iyakki_agent::directlink`); no secrets in it.
pub const BEACON_PORT: u16 = 7980;

/// ct state bits as the kernel keeps them.
pub const CT_INVALID: u32 = 1;
pub const CT_ESTABLISHED: u32 = 2;
pub const CT_RELATED: u32 = 4;
pub const CT_NEW: u32 = 8;

pub const NFPROTO_IPV4: u8 = 2;
pub const NFPROTO_IPV6: u8 = 10;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Addr {
    Exact(IpAddr),
    Range(IpAddr, IpAddr),
    Cidr(IpAddr, u8),
}

impl Addr {
    pub fn is_v4(&self) -> bool {
        match self {
            Addr::Exact(a) | Addr::Range(a, _) | Addr::Cidr(a, _) => a.is_ipv4(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Arrived on this interface (a name, or a name prefix when `prefix`).
    Iif { name: String, prefix: bool },
    /// Addressed to this machine (not broadcast or multicast).
    PktTypeHost,
    /// The connection tracking state is one of these bits.
    CtState(u32),
    Nfproto(u8),
    L4(u8),
    Saddr(Addr),
    Dport(u16, u16),
    IcmpType(u8, u8),
    Icmp6Type(u8, u8),
    Limit { per_second: u32, burst: u32 },
    Log(u16),
    Counter,
    Accept,
    Drop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleSpec {
    pub steps: Vec<Step>,
    /// Shown in the text form only; never sent to the kernel.
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Ruleset {
    pub rules: Vec<RuleSpec>,
}

/// What the machine looks like right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ctx {
    pub now: i64,
    /// The boot entry asked for remote diagnostics (`iyakki.ssh=1`).
    pub ssh_boot: bool,
    /// IPv6 is switched on in the kernel.
    pub ipv6: bool,
    /// A machine with no screen of its own (cloud mode): web mode must be reachable from anywhere.
    pub cloud: bool,
    /// The port web mode listens on, when it was asked for with a port other than the usual one.
    pub remote_port: Option<u16>,
    /// Point-to-point "direct link" interfaces that are up right now (thunderbolt0, usb0, ...): one cable to one other computer.
    pub direct_ifaces: Vec<String>,
    /// Administrator switch "Allow more on direct links": everything is let in on them (default: only [`DEV_PORT`], the beacon and ICMP).
    pub direct_allow_more: bool,
    /// The interface that internet sharing serves (`iyakki_agent::sharing`) while it is on: only DHCP (UDP 67) and DNS (UDP 53) come in on it.
    pub share_ifaces: Vec<String>,
}

impl Default for Ctx {
    fn default() -> Ctx {
        Ctx { now: 0, ssh_boot: false, ipv6: true, cloud: false, remote_port: None, direct_ifaces: Vec::new(), direct_allow_more: false, share_ifaces: Vec::new() }
    }
}

/// Private and link-local networks: what "the local network" means everywhere in the firewall.
pub fn local_networks(ipv6: bool) -> Vec<Addr> {
    let mut v = vec![
        Addr::Cidr(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 0)), 8),
        Addr::Cidr(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 0)), 12),
        Addr::Cidr(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 0)), 16),
        Addr::Cidr(IpAddr::V4(Ipv4Addr::new(169, 254, 0, 0)), 16),
    ];
    if ipv6 {
        v.push(Addr::Cidr(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0)), 10));
        v.push(Addr::Cidr(IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0)), 7));
    }
    v
}

/// Interface name prefixes of the cluster and container networks on this machine.
pub const CONTAINER_IFACES: [(&str, bool); 8] = [("cni", true), ("flannel", true), ("veth", true), ("docker", true), ("podman", true), ("cali", true), ("cilium", true), ("kube-", true)];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostInfo {
    pub name: String,
    pub addrs: Vec<IpAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Built {
    pub ruleset: Ruleset,
    pub hosts: Vec<HostInfo>,
    /// Rules that have run out (enabled rules only).
    pub expired: Vec<u32>,
    /// Rules that could not be built (kept apart so one bad rule does not stop the rest being reported).
    pub problems: Vec<(u32, String)>,
}

fn rule(note: &str, steps: Vec<Step>) -> RuleSpec {
    RuleSpec { steps, note: note.to_string() }
}

fn proto_steps(v4: Option<bool>, l4: Option<u8>, port: Option<&PortRange>) -> Vec<Step> {
    let mut s = Vec::new();
    match v4 {
        Some(true) => s.push(Step::Nfproto(NFPROTO_IPV4)),
        Some(false) => s.push(Step::Nfproto(NFPROTO_IPV6)),
        None => {}
    }
    if let Some(l) = l4 {
        s.push(Step::L4(l));
    }
    if let Some(p) = port {
        s.push(Step::Dport(p.lo, p.hi));
    }
    s
}

/// Allow rules for every combination of source, protocol and port.
fn allow(note: &str, sources: &[Option<Addr>], proto: Proto, ports: &[PortRange], iface: Option<&str>, out: &mut Vec<RuleSpec>) {
    let l4s: Vec<Option<u8>> = match (proto, ports.is_empty()) {
        (Proto::Both, true) => vec![None],
        (Proto::Both, false) => vec![Some(6), Some(17)],
        (Proto::Tcp, _) => vec![Some(6)],
        (Proto::Udp, _) => vec![Some(17)],
    };
    for src in sources {
        for l4 in &l4s {
            let plist: Vec<Option<&PortRange>> = if ports.is_empty() { vec![None] } else { ports.iter().map(Some).collect() };
            for p in plist {
                let mut steps = Vec::new();
                if let Some(i) = iface {
                    steps.push(Step::Iif { name: i.to_string(), prefix: false });
                }
                let mut tail = proto_steps(src.as_ref().map(|a| a.is_v4()), *l4, p);
                // address match goes right after the family check
                if let Some(a) = src {
                    steps.push(tail.remove(0));
                    steps.push(Step::Saddr(a.clone()));
                }
                steps.extend(tail);
                steps.push(Step::Accept);
                out.push(rule(note, steps));
            }
        }
    }
}

fn locals(ctx: &Ctx) -> Vec<Option<Addr>> {
    local_networks(ctx.ipv6).into_iter().map(Some).collect()
}

fn preset_rules(p: Preset, ctx: &Ctx, out: &mut Vec<RuleSpec>) {
    let local = locals(ctx);
    let anyone = vec![None];
    let one = |p: u16| vec![PortRange { lo: p, hi: p }];
    match p {
        Preset::Ssh => allow("preset: SSH", &local, Proto::Tcp, &one(22), None, out),
        Preset::RemoteDesktop => {
            let who = if ctx.cloud { &anyone } else { &local };
            allow("preset: remote desktop / web mode", who, Proto::Tcp, &one(REMOTE_PORT), None, out);
            if let Some(extra) = ctx.remote_port.filter(|x| *x != REMOTE_PORT) {
                allow("preset: remote desktop / web mode (chosen port)", who, Proto::Tcp, &one(extra), None, out);
            }
        }
        Preset::SharedFolders => allow("preset: shared folders", &local, Proto::Tcp, &[PortRange { lo: SHARE_PORTS.0, hi: SHARE_PORTS.1 }], None, out),
        Preset::K3sApi => allow("preset: Kubernetes API", &local, Proto::Tcp, &one(6443), None, out),
        Preset::NodePorts => allow("preset: Kubernetes NodePorts", &local, Proto::Both, &[PortRange { lo: 30000, hi: 32767 }], None, out),
        Preset::StoreNodePort => allow("preset: private store", &local, Proto::Tcp, &one(30301), None, out),
        Preset::DnsService => allow("preset: DNS service", &local, Proto::Both, &one(53), None, out),
        Preset::WebServer => allow("preset: web server", &local, Proto::Tcp, &[PortRange { lo: 80, hi: 80 }, PortRange { lo: 443, hi: 443 }], None, out),
    }
}

/// An interface name the kernel could have made (so a name from a file can never carry anything else into a rule).
fn valid_iface(n: &str) -> bool {
    !n.is_empty() && n.len() <= 15 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The "direct link" zone of one interface: by default only developer mode's TCP port, the beacon's UDP port and ICMP/ICMPv6;
/// with the administrator's switch everything (and then the presets and rules for the local network apply on the cable too). Nothing here routes or forwards: the chain is INPUT only.
fn direct_rules(name: &str, allow_more: bool, out: &mut Vec<RuleSpec>) {
    let iif = || Step::Iif { name: name.to_string(), prefix: false };
    if allow_more {
        out.push(rule("direct link: everything allowed (Settings -> Developer -> Connect over USB-C)", vec![iif(), Step::Accept]));
        return;
    }
    out.push(rule("direct link: developer mode", vec![iif(), Step::L4(6), Step::Dport(DEV_PORT, DEV_PORT), Step::Accept]));
    out.push(rule("direct link: peer beacon", vec![iif(), Step::L4(17), Step::Dport(BEACON_PORT, BEACON_PORT), Step::Accept]));
    out.push(rule("direct link: ping (IPv4)", vec![iif(), Step::Nfproto(NFPROTO_IPV4), Step::L4(1), Step::Accept]));
    out.push(rule("direct link: ping (IPv6)", vec![iif(), Step::Nfproto(NFPROTO_IPV6), Step::L4(58), Step::Accept]));
    // the presets and rules below mean "the local network", and a cable peer's link-local address is part of that: not here
    out.push(rule("direct link: nothing else (presets and rules for the local network do not apply on the cable)", vec![iif(), Step::Drop]));
}

/// The "internet sharing" zone of one interface: the devices on the other side of the cable may ask for an address (UDP 67) and for names
/// (UDP 53) and nothing else. Nothing is allowed for them to reach the machine's own services (no ping, no SSH, no web).
fn share_rules(name: &str, out: &mut Vec<RuleSpec>) {
    let iif = || Step::Iif { name: name.to_string(), prefix: false };
    out.push(rule("sharing: address requests (DHCP)", vec![iif(), Step::Nfproto(NFPROTO_IPV4), Step::L4(17), Step::Dport(67, 67), Step::Accept]));
    out.push(rule("sharing: name lookups (DNS)", vec![iif(), Step::Nfproto(NFPROTO_IPV4), Step::L4(17), Step::Dport(53, 53), Step::Accept]));
    out.push(rule("sharing: nothing else from the devices that use this machine's internet", vec![iif(), Step::Drop]));
}

/// Build the rules. Rules that fail the checks are reported in `problems` and left out (the page validates before it saves,
/// so this only happens for a hand-edited file).
pub fn build(cfg: &Config, resolved: &Resolved, ctx: &Ctx) -> Result<Built, String> {
    let mut b = Built::default();
    let mut out: Vec<RuleSpec> = Vec::new();
    out.push(rule("replies to connections this machine started", vec![Step::CtState(CT_ESTABLISHED | CT_RELATED), Step::Accept]));
    out.push(rule("broken packets", vec![Step::CtState(CT_INVALID), Step::Drop]));
    out.push(rule("this machine talking to itself", vec![Step::Iif { name: "lo".into(), prefix: false }, Step::Accept]));
    if cfg.trust_containers {
        for (n, prefix) in CONTAINER_IFACES {
            out.push(rule("cluster and container networks on this machine", vec![Step::Iif { name: n.into(), prefix }, Step::Accept]));
        }
    }
    out.push(rule("group membership queries", vec![Step::Nfproto(NFPROTO_IPV4), Step::L4(2), Step::Accept]));
    out.push(rule("DHCP replies", vec![Step::Nfproto(NFPROTO_IPV4), Step::L4(17), Step::Dport(68, 68), Step::Accept]));
    if ctx.ipv6 {
        out.push(rule("IPv6 neighbour discovery and multicast listener", vec![Step::Nfproto(NFPROTO_IPV6), Step::L4(58), Step::Icmp6Type(130, 137), Step::Accept]));
        out.push(rule("IPv6 multicast listener report", vec![Step::Nfproto(NFPROTO_IPV6), Step::L4(58), Step::Icmp6Type(143, 143), Step::Accept]));
        out.push(rule("DHCPv6 replies", vec![Step::Nfproto(NFPROTO_IPV6), Step::Saddr(Addr::Cidr(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0)), 10)), Step::L4(17), Step::Dport(546, 546), Step::Accept]));
    }
    for n in ctx.direct_ifaces.iter().filter(|n| valid_iface(n)) {
        direct_rules(n, ctx.direct_allow_more, &mut out);
    }
    for n in ctx.share_ifaces.iter().filter(|n| valid_iface(n)) {
        share_rules(n, &mut out);
    }
    match cfg.ping {
        Ping::Off => {}
        Ping::Local | Ping::Anyone => {
            let who: Vec<Option<Addr>> = if cfg.ping == Ping::Local { locals(ctx) } else { vec![None] };
            for src in &who {
                let v4 = src.as_ref().map(|a| a.is_v4());
                for fam4 in [true, false] {
                    if !fam4 && !ctx.ipv6 {
                        continue;
                    }
                    if v4.is_some_and(|x| x != fam4) {
                        continue;
                    }
                    let mut steps = vec![Step::Nfproto(if fam4 { NFPROTO_IPV4 } else { NFPROTO_IPV6 })];
                    if let Some(a) = src {
                        steps.push(Step::Saddr(a.clone()));
                    }
                    steps.push(Step::L4(if fam4 { 1 } else { 58 }));
                    steps.push(if fam4 { Step::IcmpType(8, 8) } else { Step::Icmp6Type(128, 128) });
                    steps.push(Step::Accept);
                    out.push(rule("ping", steps));
                }
            }
        }
    }
    for p in Preset::ALL.iter().filter(|p| cfg.has(**p)) {
        preset_rules(*p, ctx, &mut out);
    }
    if ctx.ssh_boot && !cfg.has(Preset::Ssh) {
        allow("remote diagnostics (iyakki.ssh=1)", &locals(ctx), Proto::Tcp, &[PortRange { lo: 22, hi: 22 }], None, &mut out);
    }
    let mut seen_hosts: BTreeMap<String, ()> = BTreeMap::new();
    for r in cfg.rules.iter().filter(|r| r.enabled) {
        if r.expires != 0 && r.expires <= ctx.now {
            b.expired.push(r.id);
            continue;
        }
        let p = match check_rule(r) {
            Ok(p) => p,
            Err(e) => {
                b.problems.push((r.id, e));
                continue;
            }
        };
        let note = if r.comment.is_empty() { format!("rule {}", r.id) } else { format!("rule {}: {}", r.id, r.comment.chars().filter(|c| !c.is_control()).collect::<String>()) };
        let sources: Vec<Option<Addr>> = match &p.src {
            Src::Ip(a) => vec![Some(Addr::Exact(*a))],
            Src::Range(a, c) => vec![Some(Addr::Range(*a, *c))],
            Src::Cidr(a, l) => vec![Some(Addr::Cidr(*a, *l))],
            Src::Host(n) => {
                let addrs = resolved.addrs.get(n).cloned().unwrap_or_default();
                if seen_hosts.insert(n.clone(), ()).is_none() {
                    b.hosts.push(HostInfo { name: n.clone(), addrs: addrs.clone() });
                }
                addrs.into_iter().filter(|a| !(a.is_ipv6() && !ctx.ipv6)).map(|a| Some(Addr::Exact(a))).collect()
            }
        };
        allow(&note, &sources, r.proto, &p.ports, p.iface.as_deref(), &mut out);
    }
    // what is left is counted and (at a limited rate) logged, then the chain's policy drops it
    out.push(rule("count blocked attempts", vec![Step::PktTypeHost, Step::CtState(CT_NEW), Step::Counter]));
    out.push(rule("log blocked attempts (rate limited)", vec![Step::PktTypeHost, Step::CtState(CT_NEW), Step::Limit { per_second: 10, burst: 5 }, Step::Log(LOG_GROUP)]));
    if out.len() > MAX_RULES_OUT {
        return Err(format!("These settings make {} rules; the limit is {MAX_RULES_OUT}. Use fewer ports per rule or CIDR ranges.", out.len()));
    }
    b.ruleset.rules = out;
    Ok(b)
}

fn state_words(m: u32) -> String {
    let mut v = Vec::new();
    for (bit, n) in [(CT_INVALID, "invalid"), (CT_ESTABLISHED, "established"), (CT_RELATED, "related"), (CT_NEW, "new")] {
        if m & bit != 0 {
            v.push(n);
        }
    }
    v.join(",")
}

fn addr_text(a: &Addr) -> String {
    match a {
        Addr::Exact(x) => x.to_string(),
        Addr::Range(x, y) => format!("{x}-{y}"),
        Addr::Cidr(x, l) => format!("{x}/{l}"),
    }
}

fn l4_name(p: u8) -> String {
    match p {
        1 => "icmp".into(),
        2 => "igmp".into(),
        6 => "tcp".into(),
        17 => "udp".into(),
        58 => "icmpv6".into(),
        n => n.to_string(),
    }
}

impl Step {
    pub fn text(&self) -> String {
        match self {
            Step::Iif { name, prefix } => format!("iifname \"{name}{}\"", if *prefix { "*" } else { "" }),
            Step::PktTypeHost => "meta pkttype host".into(),
            Step::CtState(m) => format!("ct state {}", state_words(*m)),
            Step::Nfproto(NFPROTO_IPV4) => "meta nfproto ipv4".into(),
            Step::Nfproto(_) => "meta nfproto ipv6".into(),
            Step::L4(p) => format!("meta l4proto {}", l4_name(*p)),
            Step::Saddr(a) => format!("{} saddr {}", if a.is_v4() { "ip" } else { "ip6" }, addr_text(a)),
            Step::Dport(a, b) if a == b => format!("th dport {a}"),
            Step::Dport(a, b) => format!("th dport {a}-{b}"),
            Step::IcmpType(a, b) if a == b => format!("icmp type {a}"),
            Step::IcmpType(a, b) => format!("icmp type {a}-{b}"),
            Step::Icmp6Type(a, b) if a == b => format!("icmpv6 type {a}"),
            Step::Icmp6Type(a, b) => format!("icmpv6 type {a}-{b}"),
            Step::Limit { per_second, burst } => format!("limit rate {per_second}/second burst {burst} packets"),
            Step::Log(g) => format!("log group {g}"),
            Step::Counter => "counter".into(),
            Step::Accept => "accept".into(),
            Step::Drop => "drop".into(),
        }
    }
}

impl Ruleset {
    /// The ruleset in nft-like words (for the page, the export and the golden tests; the kernel gets the steps, not this text).
    pub fn to_text(&self) -> String {
        let mut s = format!("table inet {TABLE} {{\n  chain {CHAIN} {{\n    type filter hook input priority filter; policy drop;\n");
        for r in &self.rules {
            let note: String = r.note.chars().filter(|c| !c.is_control()).collect();
            s.push_str(&format!("    {}  # {}\n", r.steps.iter().map(Step::text).collect::<Vec<_>>().join(" "), note));
        }
        s.push_str("  }\n}\n");
        s
    }
}

/// A packet as the input chain sees it.
#[derive(Clone, Debug)]
pub struct Packet {
    pub iif: String,
    pub ct: u32,
    pub src: IpAddr,
    pub l4: u8,
    pub dport: u16,
    pub icmp_type: u8,
}

impl Packet {
    pub fn new_tcp(iif: &str, src: IpAddr, dport: u16) -> Packet {
        Packet { iif: iif.into(), ct: CT_NEW, src, l4: 6, dport, icmp_type: 0 }
    }
}

pub fn in_addr(a: &Addr, ip: IpAddr) -> bool {
    fn n(i: IpAddr) -> u128 {
        match i {
            IpAddr::V4(a) => u32::from(a) as u128,
            IpAddr::V6(a) => u128::from(a),
        }
    }
    if a.is_v4() != ip.is_ipv4() {
        return false;
    }
    match a {
        Addr::Exact(x) => *x == ip,
        Addr::Range(x, y) => (n(*x)..=n(*y)).contains(&n(ip)),
        Addr::Cidr(x, l) => crate::validate::network_of(ip, *l) == *x,
    }
}

/// Run a packet through the chain: true if it is accepted, false if dropped (the policy).
pub fn eval(rs: &Ruleset, p: &Packet) -> bool {
    'rules: for r in &rs.rules {
        for s in &r.steps {
            let ok = match s {
                Step::Iif { name, prefix } => {
                    if *prefix {
                        p.iif.starts_with(name.as_str())
                    } else {
                        p.iif == *name
                    }
                }
                Step::PktTypeHost => true,
                Step::CtState(m) => p.ct & m != 0,
                Step::Nfproto(f) => (*f == NFPROTO_IPV4) == p.src.is_ipv4(),
                Step::L4(l) => p.l4 == *l,
                Step::Saddr(a) => in_addr(a, p.src),
                Step::Dport(a, b) => (*a..=*b).contains(&p.dport) && matches!(p.l4, 6 | 17),
                Step::IcmpType(a, b) | Step::Icmp6Type(a, b) => (*a..=*b).contains(&p.icmp_type),
                Step::Limit { .. } | Step::Log(_) | Step::Counter => true,
                Step::Accept => return true,
                Step::Drop => return false,
            };
            if !ok {
                continue 'rules;
            }
        }
    }
    false
}
