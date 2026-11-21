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
/// The note of the rule whose counter is the blocked total.
pub const BLOCKED_NOTE: &str = "count blocked attempts";
/// Where `iyakki-remote` listens unless told otherwise.
pub const REMOTE_PORT: u16 = 8443;
pub const SHARE_PORTS: (u16, u16) = (8081, 8180);
/// The TCP ports the media server (UPnP/DLNA) plays from, next to the shared folders' range, and the UDP port players find it by.
pub const MEDIA_PORTS: (u16, u16) = (8181, 8190);
pub const SSDP_PORT: u16 = 1900;
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
    /// The source address is a member of the named set of the ruleset ([`AddrSet`]): one rule instead of one per address.
    SaddrSet(String),
    /// The source address and the destination port together are in a named `address . port` set ([`AddrSet::concat`]).
    SaddrDportSet(String),
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

/// A named set of single addresses (one family) that rules look the source up in. Used when one rule would otherwise be
/// written once per address (a host name with many answers): see [`SET_MIN`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddrSet {
    pub name: String,
    pub v4: bool,
    /// Single addresses (a plain set).
    pub members: Vec<IpAddr>,
    /// Address ranges (first and last, merged so none overlap or touch). A set with any is an interval set: network prefixes
    /// and ranges of the rules it replaced, together with their single addresses, are kept here and `members` stays empty.
    pub ranges: Vec<(IpAddr, IpAddr)>,
    /// Source address spans each with the destination ports allowed from them (`address . port` interval set, F-FW-14). When not
    /// empty it is the only content; the set is only made when [`BuildOptions::concat_sets`] is on (off by default).
    pub concat: Vec<((IpAddr, IpAddr), (u16, u16))>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Ruleset {
    pub rules: Vec<RuleSpec>,
    pub sets: Vec<AddrSet>,
}

/// From this many addresses that differ in nothing else, the rules are written as one rule and a set.
pub const SET_MIN: usize = 16;
/// The most addresses in one set (more are split over several sets).
pub const SET_MAX: usize = 4096;

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
    /// The networks of the connected VPN tunnel (`service::set_vpn_networks`); they join "the local network" only when the
    /// settings say `vpn_local`.
    pub vpn_nets: Vec<Addr>,
}

impl Default for Ctx {
    fn default() -> Ctx {
        Ctx { now: 0, ssh_boot: false, ipv6: true, cloud: false, remote_port: None, direct_ifaces: Vec::new(), direct_allow_more: false, share_ifaces: Vec::new(), vpn_nets: Vec::new() }
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
    local_networks(ctx.ipv6).into_iter().chain(ctx.vpn_nets.iter().filter(|a| a.is_v4() || ctx.ipv6).cloned()).map(Some).collect()
}

/// A VPN tunnel's network as the firewall will treat it: only private ranges (10/8, 172.16/12, 192.168/16, 100.64/10, fc00::/7) and
/// nothing wider than a /8 (IPv4) or /16 (IPv6), so a tunnel can never make a public range "local". The host bits are cleared.
pub fn vpn_network(text: &str) -> Result<Addr, String> {
    let (a, p) = text.trim().split_once('/').ok_or_else(|| format!("{text}: write the network as address/length"))?;
    let ip: IpAddr = a.trim().parse().map_err(|_| format!("{text}: not an address"))?;
    let len: u8 = p.trim().parse().map_err(|_| format!("{text}: not a length"))?;
    let (min, max, private): (u8, u8, bool) = match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            (8, 32, o[0] == 10 || (o[0] == 172 && (16..=31).contains(&o[1])) || (o[0] == 192 && o[1] == 168) || (o[0] == 100 && (64..=127).contains(&o[1])))
        }
        IpAddr::V6(v) => (16, 128, (v.segments()[0] & 0xfe00) == 0xfc00),
    };
    if !private {
        return Err(format!("{text}: only private ranges can count as the local network"));
    }
    if len < min || len > max {
        return Err(format!("{text}: the length must be between {min} and {max}"));
    }
    let masked = match ip {
        IpAddr::V4(v) => IpAddr::V4(Ipv4Addr::from(u32::from(v) & if len == 0 { 0 } else { u32::MAX << (32 - len as u32) })),
        IpAddr::V6(v) => IpAddr::V6(Ipv6Addr::from(u128::from(v) & if len == 0 { 0 } else { u128::MAX << (128 - len as u32) })),
    };
    Ok(Addr::Cidr(masked, len))
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
        Preset::MediaServer => {
            allow("preset: media server (playing)", &local, Proto::Tcp, &[PortRange { lo: MEDIA_PORTS.0, hi: MEDIA_PORTS.1 }], None, out);
            allow("preset: media server (finding players)", &local, Proto::Udp, &one(SSDP_PORT), None, out);
        }
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
    build_with(cfg, resolved, ctx, SET_MIN)
}

/// [`build`] with the number of same-looking rules from which a set is used (tests pass `usize::MAX` for the flat form).
pub fn build_with(cfg: &Config, resolved: &Resolved, ctx: &Ctx, set_min: usize) -> Result<Built, String> {
    build_opts(cfg, resolved, ctx, BuildOptions { set_min, ..BuildOptions::default() })
}

/// What the generator may do beyond the plain form. Everything here is off (or the usual value) by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildOptions {
    pub set_min: usize,
    /// Rules that differ only in the source network and the destination port become one rule and an `address . port` interval set.
    /// Off by default: the bytes are tested and the nft text is accepted by a real kernel, but nothing ships it until it has run in an image.
    pub concat_sets: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions { set_min: SET_MIN, concat_sets: false }
    }
}

/// [`build_with`] with every option.
pub fn build_opts(cfg: &Config, resolved: &Resolved, ctx: &Ctx, opts: BuildOptions) -> Result<Built, String> {
    let set_min = opts.set_min;
    // the tunnel's networks are "local" only when the person said so
    let ctx = &Ctx { vpn_nets: if cfg.vpn_local { ctx.vpn_nets.clone() } else { Vec::new() }, ..ctx.clone() };
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
    // the rules the person or a preset made are counted, and long lists of addresses become one rule and a set
    count_accepts(&mut out);
    // pairs first: the plain address sets would otherwise take the single addresses and leave nothing for them
    let mut pair_sets = Vec::new();
    let out = if opts.concat_sets { collapse_concat(out, set_min, &mut pair_sets) } else { out };
    let (mut out, mut sets) = collapse_long_lists(out, set_min);
    sets.extend(pair_sets);
    // what is left is counted and (at a limited rate) logged, then the chain's policy drops it
    out.push(rule(BLOCKED_NOTE, vec![Step::PktTypeHost, Step::CtState(CT_NEW), Step::Counter]));
    out.push(rule("log blocked attempts (rate limited)", vec![Step::PktTypeHost, Step::CtState(CT_NEW), Step::Limit { per_second: 10, burst: 5 }, Step::Log(LOG_GROUP)]));
    if out.len() > MAX_RULES_OUT {
        return Err(format!("These settings make {} rules; the limit is {MAX_RULES_OUT}. Use fewer ports per rule or CIDR ranges.", out.len()));
    }
    b.ruleset.rules = out;
    b.ruleset.sets = sets;
    Ok(b)
}

/// A rule that only lets traffic in (no drop, no rate limit, no log): the order of such rules does not change any verdict.
fn accept_only(r: &RuleSpec) -> bool {
    r.steps.last() == Some(&Step::Accept) && !r.steps.iter().any(|s| matches!(s, Step::Drop | Step::Limit { .. } | Step::Log(_)))
}

/// Rules made by a person ("rule 3: ...") or a preset ("preset: SSH") count what they let in; the kernel keeps the counter in the rule.
/// The invariant [`crate::nl::counters_of_dump`] relies on: the "count blocked attempts" rule comes last of all counted rules.
fn count_accepts(out: &mut [RuleSpec]) {
    for r in out.iter_mut() {
        if (r.note.starts_with("rule ") || r.note.starts_with("preset: ")) && accept_only(r) && !r.steps.contains(&Step::Counter) {
            let at = r.steps.len() - 1;
            r.steps.insert(at, Step::Counter);
        }
    }
}

/// The one exact source address of a rule (and where it sits in the steps), when it has exactly one.
fn exact_source(r: &RuleSpec) -> Option<(usize, IpAddr)> {
    let mut f = r.steps.iter().enumerate().filter_map(|(k, s)| match s {
        Step::Saddr(Addr::Exact(ip)) => Some((k, *ip)),
        _ => None,
    });
    let one = f.next()?;
    if f.next().is_some() {
        return None;
    }
    Some(one)
}

/// Rules that are the same except for one exact source address, next to each other among the accept-only rules, become one rule
/// with a set lookup when there are `set_min` or more of them. Accept-only rules can be merged in any order without changing a
/// verdict (a packet is let in if any of them lets it in); a rule with a drop, limit or log in it is never touched and stays where it is.
pub(crate) fn collapse_long_lists(out: Vec<RuleSpec>, set_min: usize) -> (Vec<RuleSpec>, Vec<AddrSet>) {
    let mut res: Vec<RuleSpec> = Vec::new();
    let mut sets: Vec<AddrSet> = Vec::new();
    let mut i = 0;
    while i < out.len() {
        if !accept_only(&out[i]) {
            res.push(out[i].clone());
            i += 1;
            continue;
        }
        let mut j = i;
        while j < out.len() && accept_only(&out[j]) {
            j += 1;
        }
        collapse_block(&out[i..j], set_min, &mut res, &mut sets);
        i = j;
    }
    (res, sets)
}

fn collapse_block(block: &[RuleSpec], set_min: usize, res: &mut Vec<RuleSpec>, sets: &mut Vec<AddrSet>) {
    // (steps with the source blanked, source position, family, note) -> indexes in the block
    type Group<'a> = (Vec<Step>, usize, bool, &'a str, Vec<usize>);
    let mut groups: Vec<Group> = Vec::new();
    for (idx, r) in block.iter().enumerate() {
        let Some((k, ip)) = exact_source(r) else { continue };
        let mut key = r.steps.clone();
        key[k] = Step::SaddrSet(String::new());
        match groups.iter_mut().find(|g| g.0 == key && g.1 == k && g.2 == ip.is_ipv4() && g.3 == r.note) {
            Some(g) => g.4.push(idx),
            None => groups.push((key, k, ip.is_ipv4(), &r.note, vec![idx])),
        }
    }
    let mut skip = vec![false; block.len()];
    let mut put_at: BTreeMap<usize, Vec<RuleSpec>> = BTreeMap::new();
    collapse_intervals(block, set_min, &mut groups_for_intervals(block), &mut skip, &mut put_at, sets);
    for (key, k, v4, note, members) in &groups {
        if members.iter().any(|m| skip[*m]) {
            continue;
        }
        if members.len() < set_min {
            continue;
        }
        let mut addrs: Vec<IpAddr> = members.iter().filter_map(|m| exact_source(&block[*m]).map(|(_, ip)| ip)).collect();
        addrs.sort();
        addrs.dedup();
        let mut reps = Vec::new();
        for chunk in addrs.chunks(SET_MAX) {
            let name = format!("l{}", sets.len() + 1);
            let mut steps = key.clone();
            steps[*k] = Step::SaddrSet(name.clone());
            reps.push(RuleSpec { steps, note: (*note).to_string() });
            sets.push(AddrSet { name, v4: *v4, members: chunk.to_vec(), ranges: Vec::new(), concat: Vec::new() });
        }
        for m in members {
            skip[*m] = true;
        }
        put_at.insert(members[0], reps);
    }
    for (idx, r) in block.iter().enumerate() {
        if let Some(reps) = put_at.remove(&idx) {
            res.extend(reps);
        } else if !skip[idx] {
            res.push(r.clone());
        }
    }
}

/// Rules that are the same except for one source (address, range or prefix) and one destination port range, next to each other
/// among the accept-only rules, become one rule and an `address . port` interval set when there are `set_min` or more of them.
/// A group whose elements overlap (different but intersecting in both address and port) is left as it is, so the kernel's set
/// never has to take overlapping elements.
pub(crate) fn collapse_concat(out: Vec<RuleSpec>, set_min: usize, sets: &mut Vec<AddrSet>) -> Vec<RuleSpec> {
    let mut res: Vec<RuleSpec> = Vec::new();
    let mut i = 0;
    while i < out.len() {
        if !accept_only(&out[i]) {
            res.push(out[i].clone());
            i += 1;
            continue;
        }
        let mut j = i;
        while j < out.len() && accept_only(&out[j]) {
            j += 1;
        }
        concat_block(&out[i..j], set_min, &mut res, sets);
        i = j;
    }
    res
}

type ConcatElem = (u128, u128, u16, u16);

fn concat_parts(r: &RuleSpec) -> Option<(usize, usize, bool, ConcatElem)> {
    let mut sa = r.steps.iter().enumerate().filter(|(_, s)| matches!(s, Step::Saddr(_)));
    let mut dp = r.steps.iter().enumerate().filter(|(_, s)| matches!(s, Step::Dport(..)));
    let (Some((k, Step::Saddr(a))), None) = (sa.next(), sa.next()) else { return None };
    let (Some((d, Step::Dport(plo, phi))), None) = (dp.next(), dp.next()) else { return None };
    let (v4, lo, hi) = addr_span(a)?;
    Some((k, d, v4, (lo, hi, *plo, *phi)))
}

fn concat_block(block: &[RuleSpec], set_min: usize, res: &mut Vec<RuleSpec>, sets: &mut Vec<AddrSet>) {
    // (steps without the source, with the port step turned into the lookup; positions; family; note) -> (indexes, elements)
    type Group = (Vec<Step>, usize, bool, String, Vec<usize>, Vec<ConcatElem>);
    let mut groups: Vec<Group> = Vec::new();
    for (idx, r) in block.iter().enumerate() {
        let Some((k, d, v4, e)) = concat_parts(r) else { continue };
        let mut key = r.steps.clone();
        key[d] = Step::SaddrDportSet(String::new());
        key.remove(k);
        let at = if k < d { d - 1 } else { d };
        match groups.iter_mut().find(|g| g.0 == key && g.1 == at && g.2 == v4 && g.3 == r.note) {
            Some(g) => {
                g.4.push(idx);
                g.5.push(e);
            }
            None => groups.push((key, at, v4, r.note.clone(), vec![idx], vec![e])),
        }
    }
    let mut skip = vec![false; block.len()];
    let mut put_at: BTreeMap<usize, RuleSpec> = BTreeMap::new();
    for (key, at, v4, note, idxs, elems) in groups {
        if idxs.len() < set_min {
            continue;
        }
        let mut el = elems;
        el.sort_unstable();
        el.dedup();
        let overlap = el.iter().enumerate().any(|(n, a)| el[n + 1..].iter().any(|b| a.0 <= b.1 && b.0 <= a.1 && a.2 <= b.3 && b.2 <= a.3));
        if overlap || el.len() > SET_MAX {
            continue;
        }
        let name = format!("c{}", sets.len() + 1);
        let mut steps = key;
        steps[at] = Step::SaddrDportSet(name.clone());
        sets.push(AddrSet { name, v4, members: Vec::new(), ranges: Vec::new(), concat: el.iter().map(|(lo, hi, plo, phi)| ((num_ip(v4, *lo), num_ip(v4, *hi)), (*plo, *phi))).collect() });
        for m in &idxs {
            skip[*m] = true;
        }
        put_at.insert(idxs[0], RuleSpec { steps, note });
    }
    for (idx, r) in block.iter().enumerate() {
        if let Some(rep) = put_at.remove(&idx) {
            res.push(rep);
        } else if !skip[idx] {
            res.push(r.clone());
        }
    }
}

/// First and last address of a prefix or range, as numbers, with the family. `None` for "any" (`/0`), a range that runs backwards
/// and a prefix longer than the family allows.
fn addr_span(a: &Addr) -> Option<(bool, u128, u128)> {
    fn n(i: &IpAddr) -> u128 {
        match i {
            IpAddr::V4(a) => u128::from(u32::from(*a)),
            IpAddr::V6(a) => u128::from(*a),
        }
    }
    let v4 = a.is_v4();
    let bits: u32 = if v4 { 32 } else { 128 };
    let all = if v4 { u128::from(u32::MAX) } else { u128::MAX };
    match a {
        Addr::Exact(ip) => Some((v4, n(ip), n(ip))),
        Addr::Range(x, y) if x.is_ipv4() == y.is_ipv4() && n(x) <= n(y) => Some((v4, n(x), n(y))),
        Addr::Range(..) => None,
        Addr::Cidr(_, 0) => None,
        Addr::Cidr(ip, l) if u32::from(*l) <= bits => {
            let host = if u32::from(*l) == bits { 0 } else { all >> l };
            let lo = n(ip) & !host & all;
            Some((v4, lo, lo | host))
        }
        Addr::Cidr(..) => None,
    }
}

fn num_ip(v4: bool, n: u128) -> IpAddr {
    if v4 {
        IpAddr::V4(std::net::Ipv4Addr::from(n as u32))
    } else {
        IpAddr::V6(std::net::Ipv6Addr::from(n))
    }
}

/// Sort ranges and join those that overlap or touch (a kernel interval set refuses overlaps).
pub fn merge_spans(mut spans: Vec<(u128, u128)>) -> Vec<(u128, u128)> {
    spans.sort_unstable();
    let mut out: Vec<(u128, u128)> = Vec::new();
    for (lo, hi) in spans {
        match out.last_mut() {
            Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
            _ => out.push((lo, hi)),
        }
    }
    out
}

/// A rule whose one source is a prefix, a range or a single address (any of them), with the source blanked.
type IntervalGroup = (Vec<Step>, usize, bool, String, Vec<(usize, u128, u128)>);

fn groups_for_intervals(block: &[RuleSpec]) -> Vec<IntervalGroup> {
    let mut groups: Vec<IntervalGroup> = Vec::new();
    for (idx, r) in block.iter().enumerate() {
        let mut found = r.steps.iter().enumerate().filter(|(_, s)| matches!(s, Step::Saddr(_)));
        let (Some((k, Step::Saddr(a))), None) = (found.next(), found.next()) else { continue };
        let Some((v4, lo, hi)) = addr_span(a) else { continue };
        let mut key = r.steps.clone();
        key[k] = Step::SaddrSet(String::new());
        match groups.iter_mut().find(|g| g.0 == key && g.1 == k && g.2 == v4 && g.3 == r.note) {
            Some(g) => g.4.push((idx, lo, hi)),
            None => groups.push((key, k, v4, r.note.clone(), vec![(idx, lo, hi)])),
        }
    }
    groups
}

/// Groups of `set_min` or more rules with at least one prefix or range among them become one rule and an interval set.
fn collapse_intervals(block: &[RuleSpec], set_min: usize, groups: &mut [IntervalGroup], skip: &mut [bool], put_at: &mut BTreeMap<usize, Vec<RuleSpec>>, sets: &mut Vec<AddrSet>) {
    for (key, k, v4, note, members) in groups.iter() {
        let has_span = members.iter().any(|(i, lo, hi)| lo != hi || !matches!(block[*i].steps[*k], Step::Saddr(Addr::Exact(_))));
        if members.len() < set_min || !has_span {
            continue;
        }
        let merged = merge_spans(members.iter().map(|(_, lo, hi)| (*lo, *hi)).collect());
        let mut reps = Vec::new();
        for chunk in merged.chunks(SET_MAX) {
            let name = format!("l{}", sets.len() + 1);
            let mut steps = key.clone();
            steps[*k] = Step::SaddrSet(name.clone());
            reps.push(RuleSpec { steps, note: note.clone() });
            sets.push(AddrSet { name, v4: *v4, members: Vec::new(), concat: Vec::new(), ranges: chunk.iter().map(|(lo, hi)| (num_ip(*v4, *lo), num_ip(*v4, *hi))).collect() });
        }
        for (m, _, _) in members {
            skip[*m] = true;
        }
        put_at.insert(members[0].0, reps);
    }
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
            Step::SaddrSet(n) => format!("saddr @{n}"),
            Step::SaddrDportSet(n) => format!("ip saddr . th dport @{n}"),
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
        let mut s = format!("table inet {TABLE} {{\n");
        for set in self.sets.iter().filter(|s| !s.concat.is_empty()) {
            s.push_str(&set.concat_text());
        }
        for set in self.sets.iter().filter(|s| s.concat.is_empty()) {
            s.push_str(&format!("  set {} {{\n    type {}\n    elements = {{ {} }}\n  }}\n", set.name, format!("{}{}", if set.v4 { "ipv4_addr" } else { "ipv6_addr" }, if set.ranges.is_empty() { "" } else { "\n    flags interval" }), set.members.iter().map(|m| m.to_string()).chain(set.ranges.iter().map(|(a, b)| if a == b { a.to_string() } else { format!("{a}-{b}") })).collect::<Vec<_>>().join(", ")));
        }
        s.push_str(&format!("  chain {CHAIN} {{\n    type filter hook input priority filter; policy drop;\n"));
        for r in &self.rules {
            let note: String = r.note.chars().filter(|c| !c.is_control()).collect();
            s.push_str(&format!("    {}  # {}\n", r.steps.iter().map(Step::text).collect::<Vec<_>>().join(" "), note));
        }
        s.push_str("  }\n}\n");
        s
    }
}

impl AddrSet {
    /// The `address . port` set in nft's own words (only for a set with [`AddrSet::concat`]).
    pub fn concat_text(&self) -> String {
        let fam = if self.v4 { "ipv4_addr" } else { "ipv6_addr" };
        let el: Vec<String> = self
            .concat
            .iter()
            .map(|((a, b), (p, q))| format!("{} . {}", if a == b { a.to_string() } else { format!("{a}-{b}") }, if p == q { p.to_string() } else { format!("{p}-{q}") }))
            .collect();
        format!("  set {} {{\n    type {fam} . inet_service\n    flags interval\n    elements = {{ {} }}\n  }}\n", self.name, el.join(", "))
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
    eval_counting(rs, p, &mut Vec::new())
}

/// [`eval`] that also counts like the kernel does: every rule whose `counter` step is reached (all steps before it matched) adds
/// one packet to its slot of `hits` (made as long as the rules). For tests that compare with what the kernel reports.
pub fn eval_counting(rs: &Ruleset, p: &Packet, hits: &mut Vec<u64>) -> bool {
    if hits.len() < rs.rules.len() {
        hits.resize(rs.rules.len(), 0);
    }
    'rules: for (ri, r) in rs.rules.iter().enumerate() {
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
                Step::SaddrSet(n) => rs.sets.iter().any(|set| &set.name == n && (set.members.contains(&p.src) || set.ranges.iter().any(|(lo, hi)| in_addr(&Addr::Range(*lo, *hi), p.src)))),
                Step::SaddrDportSet(n) => matches!(p.l4, 6 | 17) && rs.sets.iter().any(|set| &set.name == n && set.concat.iter().any(|((lo, hi), (plo, phi))| in_addr(&Addr::Range(*lo, *hi), p.src) && (*plo..=*phi).contains(&p.dport))),
                Step::Dport(a, b) => (*a..=*b).contains(&p.dport) && matches!(p.l4, 6 | 17),
                Step::IcmpType(a, b) | Step::Icmp6Type(a, b) => (*a..=*b).contains(&p.icmp_type),
                Step::Counter => {
                    hits[ri] += 1;
                    true
                }
                Step::Limit { .. } | Step::Log(_) => true,
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

/// What one rule (all the kernel rules that came from it, under one note) has let in or counted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleCount {
    pub note: String,
    pub packets: u64,
    pub bytes: u64,
}

/// Put the per-rule counters of the kernel (one entry per rule of the chain, in order, `None` for a rule without a counter) next to
/// the rules they belong to, summed by note. The "count blocked attempts" rule is left out (it is the blocked total). A list of a
/// different length (the table changed since it was built) gives nothing rather than a wrong answer.
pub fn rule_counts(rs: &Ruleset, per_rule: &[Option<(u64, u64)>]) -> Vec<RuleCount> {
    if per_rule.len() != rs.rules.len() {
        return Vec::new();
    }
    let mut out: Vec<RuleCount> = Vec::new();
    for (r, c) in rs.rules.iter().zip(per_rule) {
        let Some((pk, by)) = c else { continue };
        if r.note == BLOCKED_NOTE {
            continue;
        }
        match out.iter_mut().find(|x| x.note == r.note) {
            Some(x) => {
                x.packets = x.packets.saturating_add(*pk);
                x.bytes = x.bytes.saturating_add(*by);
            }
            None => out.push(RuleCount { note: r.note.clone(), packets: *pk, bytes: *by }),
        }
    }
    out
}

/// The id of a rule made on the Firewall page, from a counted note ("rule 3: office").
pub fn rule_id_of_note(note: &str) -> Option<u32> {
    note.strip_prefix("rule ")?.split([':', ' ']).next()?.parse().ok()
}
