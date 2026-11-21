//! The firewall, later (F-FW-10): filtering outgoing connections and forwarding rules. Pure: settings in, nft rule text out.
//!
//! This is the generator and the checks. It writes **nft's own words** (so `nft -c -f` can check them) for two tables, and nothing
//! here applies a rule: the kernel writer ([`crate::nl`]) does not yet know the output and forward hooks or NAT, so applying is
//! the remainder of the row. Nothing is ever applied by this module.
//!
//! * `table inet iyakki_fw_out`: an `output` chain (the person's allow and block rules in order, first match wins) and a `forward`
//!   chain (forwarding rules and the forwarded ports of port forwards). Both start with what must never be cut: replies to
//!   connections that exist, this machine talking to itself, cluster and container networks, DHCP and IPv6 neighbour discovery.
//! * `table ip iyakki_fw_nat`: a `prerouting` chain with one `dnat` rule per port forward (only when there is one).
//!
//! Safety rules, all checked: blocking by default needs at least one allow rule (or the machine would be cut off from everything);
//! every value is a number, an address or a short name (nothing typed becomes rule text); a port forward only to an address
//! of the local networks; at most [`MAX_RULES`] rules of each kind.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::model::Proto;
use crate::nl::tablegen::TableSpec;
use crate::validate::{check_comment, check_iface, parse_cidr, parse_ip, parse_ports, parse_range, PortRange};

pub const TABLE_OUT: &str = "iyakki_fw_out";
pub const TABLE_NAT: &str = "iyakki_fw_nat";
pub const MAX_RULES: usize = 200;

/// The interfaces of cluster and container networks that are never cut (the same list the input chain lets in).
const CLUSTER_IFACES: [&str; 8] = ["cni*", "flannel*", "veth*", "docker*", "podman*", "cali*", "cilium*", "kube-*"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Allow,
    Block,
}

/// One rule about connections this machine starts. Rules are tried in the order of the list; the first that matches decides.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutRule {
    pub id: u32,
    pub enabled: bool,
    pub verdict: Verdict,
    /// An address, `a-b` or `a/len`; empty means any destination.
    pub dest: String,
    pub proto: Proto,
    /// `443`, `80,443`, `8000-8100`; empty means every port.
    pub ports: String,
    /// Only packets that leave on this interface; empty means any.
    pub iface: String,
    pub note: String,
}

/// Lets traffic that arrives on `from_iface` leave on `to_iface` (this machine as a router or a share point).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardRule {
    pub id: u32,
    pub enabled: bool,
    pub from_iface: String,
    pub to_iface: String,
    /// Only to this destination; empty means any.
    pub dest: String,
    pub proto: Proto,
    pub ports: String,
    pub note: String,
}

/// Connections that arrive on `iface` for `port` of this machine are sent on to `to`:`to_port` (and allowed through `forward`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortForward {
    pub id: u32,
    pub enabled: bool,
    pub iface: String,
    /// `Tcp` or `Udp` (never `Both`: write two forwards).
    pub proto: Proto,
    pub port: u16,
    pub to: String,
    pub to_port: u16,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OutConfig {
    /// Off means no table at all.
    #[serde(default)]
    pub enabled: bool,
    /// The default for connections no rule matches: false lets them out (the usual), true blocks them.
    #[serde(default)]
    pub block_by_default: bool,
    #[serde(default)]
    pub rules: Vec<OutRule>,
    #[serde(default)]
    pub forwards: Vec<ForwardRule>,
    #[serde(default)]
    pub port_forwards: Vec<PortForward>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dest {
    Any,
    Ip(IpAddr),
    Range(IpAddr, IpAddr),
    Cidr(IpAddr, u8),
}

fn parse_dest(s: &str) -> Result<Dest, String> {
    let s = s.trim();
    if s.is_empty() {
        Ok(Dest::Any)
    } else if s.contains('/') {
        parse_cidr(s).map(|(a, l, _)| Dest::Cidr(a, l))
    } else if s.contains('-') && !s.contains(':') {
        parse_range(s).map(|(a, b, _)| Dest::Range(a, b))
    } else {
        parse_ip(s).map(Dest::Ip)
    }
}

fn dest_text(d: Dest) -> Option<String> {
    let fam = |a: &IpAddr| if a.is_ipv4() { "ip" } else { "ip6" };
    match d {
        Dest::Any => None,
        Dest::Ip(a) => Some(format!("{} daddr {a}", fam(&a))),
        Dest::Range(a, b) => Some(format!("{} daddr {a}-{b}", fam(&a))),
        Dest::Cidr(a, l) => Some(format!("{} daddr {a}/{l}", fam(&a))),
    }
}

fn ports_text(p: &[PortRange]) -> String {
    p.iter().map(|r| if r.lo == r.hi { r.lo.to_string() } else { format!("{}-{}", r.lo, r.hi) }).collect::<Vec<_>>().join(", ")
}

/// `meta l4proto tcp th dport 443`, for the protocol and ports of a rule (empty when it matches everything).
fn match_text(proto: Proto, ports: &[PortRange]) -> Result<String, String> {
    if !ports.is_empty() && proto == Proto::Both {
        // both protocols with ports: the same ports for tcp and udp
        return Ok(format!("meta l4proto {{ tcp, udp }} th dport {{ {} }}", ports_text(ports)));
    }
    let l4 = match proto {
        Proto::Tcp => "meta l4proto tcp",
        Proto::Udp => "meta l4proto udp",
        Proto::Both => return Ok(String::new()),
    };
    Ok(if ports.is_empty() { l4.to_string() } else { format!("{l4} th dport {{ {} }}", ports_text(ports)) })
}

fn iface_name(s: &str, what: &str) -> Result<Option<String>, String> {
    check_iface(s).map_err(|e| format!("{what}: {e}"))
}

fn note_ok(n: &str) -> Result<String, String> {
    check_comment(n)?;
    Ok(n.chars().filter(|c| !c.is_control() && *c != '"' && *c != '\\').collect())
}

fn local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_private() || a.is_link_local(),
        IpAddr::V6(a) => (a.segments()[0] & 0xfe00) == 0xfc00 || (a.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// Checks everything; the first problem is the answer, in words a person can act on.
pub fn check(cfg: &OutConfig) -> Result<(), String> {
    if cfg.rules.len() > MAX_RULES || cfg.forwards.len() > MAX_RULES || cfg.port_forwards.len() > MAX_RULES {
        return Err(format!("At most {MAX_RULES} rules of each kind."));
    }
    for r in &cfg.rules {
        let w = format!("Outgoing rule {}", r.id);
        parse_dest(&r.dest).map_err(|e| format!("{w}: {e}"))?;
        let ports = parse_ports(&r.ports).map_err(|e| format!("{w}: {e}"))?;
        let _ = ports;
        iface_name(&r.iface, &w)?;
        note_ok(&r.note).map_err(|e| format!("{w}: {e}"))?;
    }
    for f in &cfg.forwards {
        let w = format!("Forwarding rule {}", f.id);
        if f.from_iface.trim().is_empty() || f.to_iface.trim().is_empty() {
            return Err(format!("{w}: say which interface the traffic comes in on and which it goes out on."));
        }
        iface_name(&f.from_iface, &w)?;
        iface_name(&f.to_iface, &w)?;
        if f.from_iface.trim() == f.to_iface.trim() {
            return Err(format!("{w}: coming in and going out on the same interface forwards nothing."));
        }
        parse_dest(&f.dest).map_err(|e| format!("{w}: {e}"))?;
        parse_ports(&f.ports).map_err(|e| format!("{w}: {e}"))?;
        note_ok(&f.note).map_err(|e| format!("{w}: {e}"))?;
    }
    for p in &cfg.port_forwards {
        let w = format!("Port forward {}", p.id);
        if p.iface.trim().is_empty() {
            return Err(format!("{w}: say which interface the connections arrive on."));
        }
        iface_name(&p.iface, &w)?;
        if p.proto == Proto::Both {
            return Err(format!("{w}: choose TCP or UDP (add a second forward for the other)."));
        }
        if p.port == 0 || p.to_port == 0 {
            return Err(format!("{w}: ports are 1 to 65535."));
        }
        let to = parse_ip(&p.to).map_err(|e| format!("{w}: {e}"))?;
        if !local(to) {
            return Err(format!("{w}: connections are only forwarded to a computer on a local network (10.x, 172.16-31.x, 192.168.x, 169.254.x or an IPv6 private address), not to {to}."));
        }
        note_ok(&p.note).map_err(|e| format!("{w}: {e}"))?;
    }
    if cfg.enabled && cfg.block_by_default && !cfg.rules.iter().any(|r| r.enabled && r.verdict == Verdict::Allow) {
        return Err("Blocking outgoing connections by default needs at least one allow rule, or this computer could not reach anything (not even a name server).".into());
    }
    Ok(())
}

fn cluster_rule(dir: &str) -> String {
    CLUSTER_IFACES.iter().map(|n| format!("    {dir}name \"{n}\" accept  # cluster and container networks on this machine\n")).collect()
}

/// The nft script for `cfg`: empty when it is off. `Err` when the settings do not pass [`check`].
pub fn generate(cfg: &OutConfig) -> Result<String, String> {
    check(cfg)?;
    if !cfg.enabled {
        return Ok(String::new());
    }
    let mut s = format!("table inet {TABLE_OUT} {{\n  chain output {{\n    type filter hook output priority filter; policy {};\n", if cfg.block_by_default { "drop" } else { "accept" });
    s.push_str("    ct state established,related accept  # replies and anything that belongs to a connection\n    ct state invalid drop  # broken packets\n    oif \"lo\" accept  # this machine talking to itself\n");
    s.push_str(&cluster_rule("oif"));
    s.push_str("    meta nfproto ipv4 meta l4proto udp th dport 67 accept  # DHCP requests\n    meta nfproto ipv4 meta l4proto igmp accept  # group membership\n    meta nfproto ipv6 meta l4proto icmpv6 icmpv6 type { 133-137, 143 } accept  # IPv6 neighbour discovery and multicast listener\n    meta nfproto ipv6 meta l4proto udp th dport 547 accept  # DHCPv6 requests\n");
    for r in cfg.rules.iter().filter(|r| r.enabled) {
        let ports = parse_ports(&r.ports)?;
        let mut parts: Vec<String> = Vec::new();
        if let Some(i) = iface_name(&r.iface, "")? {
            parts.push(format!("oifname \"{i}\""));
        }
        let d = parse_dest(&r.dest)?;
        // a family match is needed before l4 matches of that family
        if let Some(t) = dest_text(d) {
            parts.push(t);
        }
        let m = match_text(r.proto, &ports)?;
        if !m.is_empty() {
            parts.push(m);
        }
        parts.push("counter".into());
        parts.push(if r.verdict == Verdict::Allow { "accept".into() } else { "reject".into() });
        let note = note_ok(&r.note)?;
        s.push_str(&format!("    {}  # outgoing rule {}{}\n", parts.join(" "), r.id, if note.is_empty() { String::new() } else { format!(": {note}") }));
    }
    s.push_str("  }\n");
    if !cfg.forwards.is_empty() || !cfg.port_forwards.is_empty() {
        s.push_str("  chain forward {\n    type filter hook forward priority filter; policy drop;\n    ct state established,related accept  # replies\n    ct state invalid drop  # broken packets\n");
        s.push_str(&cluster_rule("iif"));
        s.push_str(&cluster_rule("oif"));
        for f in cfg.forwards.iter().filter(|f| f.enabled) {
            let ports = parse_ports(&f.ports)?;
            let mut parts = vec![format!("iifname \"{}\"", f.from_iface.trim()), format!("oifname \"{}\"", f.to_iface.trim())];
            if let Some(t) = dest_text(parse_dest(&f.dest)?) {
                parts.push(t);
            }
            let m = match_text(f.proto, &ports)?;
            if !m.is_empty() {
                parts.push(m);
            }
            parts.push("counter accept".into());
            let note = note_ok(&f.note)?;
            s.push_str(&format!("    {}  # forwarding rule {}{}\n", parts.join(" "), f.id, if note.is_empty() { String::new() } else { format!(": {note}") }));
        }
        for p in cfg.port_forwards.iter().filter(|p| p.enabled) {
            let to = parse_ip(&p.to)?;
            let fam = if to.is_ipv4() { "ip" } else { "ip6" };
            let l4 = if p.proto == Proto::Udp { "udp" } else { "tcp" };
            s.push_str(&format!("    iifname \"{}\" {fam} daddr {to} meta l4proto {l4} th dport {} ct status dnat counter accept  # port forward {}\n", p.iface.trim(), p.to_port, p.id));
        }
        s.push_str("  }\n");
    }
    s.push_str("}\n");
    let v4: Vec<&PortForward> = cfg.port_forwards.iter().filter(|p| p.enabled && parse_ip(&p.to).is_ok_and(|a| a.is_ipv4())).collect();
    let v6: Vec<&PortForward> = cfg.port_forwards.iter().filter(|p| p.enabled && parse_ip(&p.to).is_ok_and(|a| a.is_ipv6())).collect();
    for (fam, list) in [("ip", v4), ("ip6", v6)] {
        if list.is_empty() {
            continue;
        }
        s.push_str(&format!("table {fam} {TABLE_NAT} {{\n  chain prerouting {{\n    type nat hook prerouting priority dstnat; policy accept;\n"));
        for p in list {
            let l4 = if p.proto == Proto::Udp { "udp" } else { "tcp" };
            let to = parse_ip(&p.to)?;
            let target = if to.is_ipv6() { format!("[{to}]:{}", p.to_port) } else { format!("{to}:{}", p.to_port) };
            s.push_str(&format!("    iifname \"{}\" {l4} dport {} counter dnat to {target}  # port forward {}\n", p.iface.trim(), p.port, p.id));
        }
        s.push_str("  }\n}\n");
    }
    Ok(s)
}

/// What the typed settings become for the netlink writer ([`crate::nl::tablegen`]): no text, no `nft` program. The filter table
/// (`inet iyakki_fw_out`: output chain, forward chain) and, when there are port forwards, the nat table (`inet iyakki_fw_nat`).
/// A rule with port ranges or both protocols becomes one kernel rule per protocol and range (the same outcome as the lists the
/// text generator writes); more than the writer's limit of rules is refused in words.
pub fn to_table_specs(cfg: &OutConfig) -> Result<Vec<TableSpec>, String> {
    use crate::nl::tablegen::{ChainKind, ChainSpec, Expr, Hook, IfName, CT_ESTABLISHED_RELATED, CT_INVALID};
    check(cfg)?;
    if !cfg.enabled {
        return Ok(Vec::new());
    }
    let exact = |n: &str| IfName::Exact(n.trim().to_string());
    let cluster = |out: bool| -> Vec<Vec<Expr>> {
        CLUSTER_IFACES
            .iter()
            .map(|n| {
                let name = IfName::Prefix(n.trim_end_matches('*').to_string());
                vec![if out { Expr::OifIs(name) } else { Expr::IifIs(name) }, Expr::Accept]
            })
            .collect()
    };
    // the destination, protocol and port steps of one rule, one list of steps per (protocol, port range)
    let matches_of = |dest: &str, proto: Proto, ports: &str| -> Result<Vec<Vec<Expr>>, String> {
        let mut head = Vec::new();
        match parse_dest(dest)? {
            Dest::Any => {}
            Dest::Ip(a) => head.push(Expr::Daddr(a, if a.is_ipv4() { 32 } else { 128 })),
            Dest::Range(a, b) => head.push(Expr::DaddrRange(a, b)),
            Dest::Cidr(a, l) => head.push(Expr::Daddr(a, l)),
        }
        let ports = parse_ports(ports)?;
        let protos: Vec<u8> = match proto {
            Proto::Tcp => vec![6],
            Proto::Udp => vec![17],
            Proto::Both if ports.is_empty() => Vec::new(),
            Proto::Both => vec![6, 17],
        };
        let mut out = Vec::new();
        if protos.is_empty() {
            out.push(head);
            return Ok(out);
        }
        for p in protos {
            if ports.is_empty() {
                let mut v = head.clone();
                v.push(Expr::L4(p));
                out.push(v);
            }
            for r in &ports {
                let mut v = head.clone();
                v.push(Expr::L4(p));
                v.push(if r.lo == r.hi { Expr::Dport(r.lo) } else { Expr::DportRange(r.lo, r.hi) });
                out.push(v);
            }
        }
        Ok(out)
    };
    let mut output: Vec<Vec<Expr>> = vec![
        vec![Expr::CtState(CT_ESTABLISHED_RELATED), Expr::Accept],
        vec![Expr::CtState(CT_INVALID), Expr::Drop],
        vec![Expr::OifIs(exact("lo")), Expr::Accept],
    ];
    output.extend(cluster(true));
    output.push(vec![Expr::Nfproto(true), Expr::L4(17), Expr::Dport(67), Expr::Accept]);
    output.push(vec![Expr::Nfproto(true), Expr::L4(2), Expr::Accept]);
    output.push(vec![Expr::Nfproto(false), Expr::L4(58), Expr::IcmpType(133, 137), Expr::Accept]);
    output.push(vec![Expr::Nfproto(false), Expr::L4(58), Expr::IcmpType(143, 143), Expr::Accept]);
    output.push(vec![Expr::Nfproto(false), Expr::L4(17), Expr::Dport(547), Expr::Accept]);
    for r in cfg.rules.iter().filter(|r| r.enabled) {
        for mut m in matches_of(&r.dest, r.proto, &r.ports)? {
            if let Some(i) = iface_name(&r.iface, "")? {
                m.insert(0, Expr::OifIs(exact(&i)));
            }
            m.push(Expr::Counter);
            m.push(if r.verdict == Verdict::Allow { Expr::Accept } else { Expr::Reject });
            output.push(m);
        }
    }
    let (output, mut out_sets) = merge_into_interval_sets(output);
    let output = merge_into_concat_sets(output, &mut out_sets);
    let mut chains = vec![ChainSpec { name: "output".into(), kind: ChainKind::Filter, hook: Hook::Output, priority: 0, policy_accept: !cfg.block_by_default, rules: output }];
    if !cfg.forwards.is_empty() || !cfg.port_forwards.is_empty() {
        let mut fwd: Vec<Vec<Expr>> = vec![vec![Expr::CtState(CT_ESTABLISHED_RELATED), Expr::Accept], vec![Expr::CtState(CT_INVALID), Expr::Drop]];
        fwd.extend(cluster(false));
        fwd.extend(cluster(true));
        for f in cfg.forwards.iter().filter(|f| f.enabled) {
            for mut m in matches_of(&f.dest, f.proto, &f.ports)? {
                m.insert(0, Expr::OifIs(exact(&f.to_iface)));
                m.insert(0, Expr::IifIs(exact(&f.from_iface)));
                m.push(Expr::Counter);
                m.push(Expr::Accept);
                fwd.push(m);
            }
        }
        for p in cfg.port_forwards.iter().filter(|p| p.enabled) {
            let to = parse_ip(&p.to)?;
            let l4 = if p.proto == Proto::Udp { 17 } else { 6 };
            fwd.push(vec![Expr::IifIs(exact(&p.iface)), Expr::Daddr(to, if to.is_ipv4() { 32 } else { 128 }), Expr::L4(l4), Expr::Dport(p.to_port), Expr::CtStatusDnat, Expr::Counter, Expr::Accept]);
        }
        chains.push(ChainSpec { name: "forward".into(), kind: ChainKind::Filter, hook: Hook::Forward, priority: 0, policy_accept: false, rules: fwd });
    }
    let mut specs = vec![TableSpec { name: TABLE_OUT.into(), sets: out_sets, chains }];
    let nat: Vec<Vec<Expr>> = cfg
        .port_forwards
        .iter()
        .filter(|p| p.enabled)
        .map(|p| {
            let to = parse_ip(&p.to)?;
            Ok(vec![Expr::IifIs(exact(&p.iface)), Expr::L4(if p.proto == Proto::Udp { 17 } else { 6 }), Expr::Dport(p.port), Expr::Counter, Expr::Dnat(to, p.to_port)])
        })
        .collect::<Result<_, String>>()?;
    if !nat.is_empty() {
        specs.push(TableSpec { name: TABLE_NAT.into(), sets: Vec::new(), chains: vec![ChainSpec { name: "prerouting".into(), kind: ChainKind::Nat, hook: Hook::Prerouting, priority: -100, policy_accept: true, rules: nat }] });
    }
    for t in &specs {
        t.check()?;
    }
    Ok(specs)
}

/// How many neighbouring rules that differ only in their destination are needed before they become one interval set.
pub const SET_MIN_RULES: usize = 4;

fn span_of(e: &crate::nl::tablegen::Expr) -> Option<(IpAddr, IpAddr)> {
    use crate::nl::tablegen::Expr;
    match e {
        Expr::DaddrRange(a, b) => Some((*a, *b)),
        Expr::Daddr(ip, p) => Some(match ip {
            IpAddr::V4(a) => {
                let bits = u32::from(*a);
                let mask = if *p == 0 { 0 } else { u32::MAX << (32 - u32::from(*p)) };
                (IpAddr::V4((bits & mask).into()), IpAddr::V4((bits | !mask).into()))
            }
            IpAddr::V6(a) => {
                let bits = u128::from(*a);
                let mask = if *p == 0 { 0 } else { u128::MAX << (128 - u32::from(*p)) };
                (IpAddr::V6((bits & mask).into()), IpAddr::V6((bits | !mask).into()))
            }
        }),
        _ => None,
    }
}

fn key_of(ip: &IpAddr) -> u128 {
    match ip {
        IpAddr::V4(a) => u128::from(u32::from(*a)),
        IpAddr::V6(a) => u128::from(*a),
    }
}

/// Union of address spans: sorted, overlapping and touching spans joined (the kernel wants an interval set without overlaps).
fn union_spans(mut v: Vec<(IpAddr, IpAddr)>) -> Vec<(IpAddr, IpAddr)> {
    v.sort_by_key(|(a, _)| key_of(a));
    let mut out: Vec<(IpAddr, IpAddr)> = Vec::new();
    for (a, b) in v {
        if let Some(last) = out.last_mut() {
            if key_of(&a) <= key_of(&last.1).saturating_add(1) {
                if key_of(&b) > key_of(&last.1) {
                    last.1 = b;
                }
                continue;
            }
        }
        out.push((a, b));
    }
    out
}

/// Runs of neighbouring rules that are the same except for one destination (address, range or network, one family) and have
/// [`SET_MIN_RULES`] or more members become one rule that looks the destination up in an interval set (F-FW-10). Only neighbours are
/// joined and the verdict is the same, so first-match order is kept. Returns the new rules and the sets they use.
pub fn merge_into_interval_sets(rules: Vec<Vec<crate::nl::tablegen::Expr>>) -> (Vec<Vec<crate::nl::tablegen::Expr>>, Vec<crate::nl::tablegen::SetSpec>) {
    use crate::nl::tablegen::{Expr, SetSpec, MAX_MEMBERS, MAX_SETS};
    // the rule without its destination step, the position of that step and the span
    let split = |r: &Vec<Expr>| -> Option<(Vec<Expr>, usize, (IpAddr, IpAddr))> {
        let mut found = None;
        for (i, e) in r.iter().enumerate() {
            if let Some(sp) = span_of(e) {
                if found.is_some() {
                    return None;
                }
                found = Some((i, sp));
            }
        }
        let (i, sp) = found?;
        // a rule that is itself a catch-all or has no verdict is left alone
        if !matches!(r.last(), Some(Expr::Accept | Expr::Reject | Expr::Drop)) {
            return None;
        }
        let mut rest = r.clone();
        rest.remove(i);
        Some((rest, i, sp))
    };
    let mut out: Vec<Vec<Expr>> = Vec::new();
    let mut sets: Vec<SetSpec> = Vec::new();
    let mut i = 0;
    while i < rules.len() {
        let Some((rest, pos, _)) = split(&rules[i]) else {
            out.push(rules[i].clone());
            i += 1;
            continue;
        };
        let mut j = i;
        let mut group: Vec<(IpAddr, IpAddr)> = Vec::new();
        while j < rules.len() {
            match split(&rules[j]) {
                Some((r2, p2, sp)) if r2 == rest && p2 == pos => {
                    group.push(sp);
                    j += 1;
                }
                _ => break,
            }
        }
        for v4 in [true, false] {
            let part: Vec<(IpAddr, IpAddr)> = group.iter().copied().filter(|(a, _)| a.is_ipv4() == v4).collect();
            let merged = union_spans(part.clone());
            if part.len() >= SET_MIN_RULES && sets.len() < MAX_SETS && merged.len() <= MAX_MEMBERS / 2 {
                let name = format!("out_dest{}_{}", if v4 { 4 } else { 6 }, sets.len());
                let mut r = rest.clone();
                r.insert(pos, Expr::DaddrSet(name.clone()));
                sets.push(SetSpec { name, v4, timeout: false, members: Vec::new(), ranges: merged, concat: Vec::new() });
                out.push(r);
            } else {
                for k in i..j {
                    if split(&rules[k]).map(|(_, _, (a, _))| a.is_ipv4() == v4).unwrap_or(false) {
                        out.push(rules[k].clone());
                    }
                }
            }
        }
        i = j;
    }
    (out, sets)
}

/// The destination port steps of a rule as a span, when the rule has exactly one.
fn port_span_of(e: &crate::nl::tablegen::Expr) -> Option<(u16, u16)> {
    use crate::nl::tablegen::Expr;
    match e {
        Expr::Dport(p) => Some((*p, *p)),
        Expr::DportRange(a, b) => Some((*a, *b)),
        _ => None,
    }
}

type ConcatElem = ((IpAddr, IpAddr), (u16, u16));

/// Runs of neighbouring rules that are the same except for the destination (address, range or network, one family) AND the
/// destination port (or port range) and have [`SET_MIN_RULES`] or more members become one rule that looks the address and the port
/// up together in an `address . port` interval set (F-FW-10, concatenation sets). Only neighbours with the same verdict are
/// joined, so first-match order is kept; a run whose elements overlap in both address and port is left as typed (the kernel's
/// set takes no overlapping elements). Runs the pass after [`merge_into_interval_sets`]; adds its sets to `sets`.
pub fn merge_into_concat_sets(rules: Vec<Vec<crate::nl::tablegen::Expr>>, sets: &mut Vec<crate::nl::tablegen::SetSpec>) -> Vec<Vec<crate::nl::tablegen::Expr>> {
    use crate::nl::tablegen::{Expr, SetSpec, MAX_MEMBERS, MAX_SETS};
    // the rule with the port step turned into a marker and the destination step removed, the marker's place, the element
    let split = |r: &Vec<Expr>| -> Option<(Vec<Expr>, usize, ConcatElem)> {
        let spans: Vec<(usize, (IpAddr, IpAddr))> = r.iter().enumerate().filter_map(|(i, e)| span_of(e).map(|s| (i, s))).collect();
        let ports: Vec<(usize, (u16, u16))> = r.iter().enumerate().filter_map(|(i, e)| port_span_of(e).map(|s| (i, s))).collect();
        let ([(si, sp)], [(pi, pp)]) = (spans.as_slice(), ports.as_slice()) else { return None };
        if !matches!(r.last(), Some(Expr::Accept | Expr::Reject | Expr::Drop)) || !matches!(r.get(pi.wrapping_sub(1)), Some(Expr::L4(6 | 17))) {
            return None;
        }
        let mut rest = r.clone();
        rest[*pi] = Expr::DaddrDportSet(String::new());
        rest.remove(*si);
        let at = if si < pi { pi - 1 } else { *pi };
        Some((rest, at, (*sp, *pp)))
    };
    let mut out: Vec<Vec<Expr>> = Vec::new();
    let mut i = 0;
    while i < rules.len() {
        let Some((rest, at, _)) = split(&rules[i]) else {
            out.push(rules[i].clone());
            i += 1;
            continue;
        };
        let mut j = i;
        let mut group: Vec<(usize, ConcatElem)> = Vec::new();
        while j < rules.len() {
            match split(&rules[j]) {
                Some((r2, a2, e)) if r2 == rest && a2 == at => {
                    group.push((j, e));
                    j += 1;
                }
                _ => break,
            }
        }
        for v4 in [true, false] {
            let part: Vec<&(usize, ConcatElem)> = group.iter().filter(|(_, ((a, _), _))| a.is_ipv4() == v4).collect();
            let mut el: Vec<ConcatElem> = part.iter().map(|(_, e)| *e).collect();
            el.sort_by_key(|((a, z), (p, q))| (key_of(a), key_of(z), *p, *q));
            el.dedup();
            let overlap = el.iter().enumerate().any(|(n, a)| el[n + 1..].iter().any(|b| key_of(&a.0 .0) <= key_of(&b.0 .1) && key_of(&b.0 .0) <= key_of(&a.0 .1) && a.1 .0 <= b.1 .1 && b.1 .0 <= a.1 .1));
            if part.len() >= SET_MIN_RULES && !overlap && sets.len() < MAX_SETS && el.len() <= MAX_MEMBERS / 2 {
                let name = format!("out_pair{}_{}", if v4 { 4 } else { 6 }, sets.len());
                let mut r = rest.clone();
                r[at] = Expr::DaddrDportSet(name.clone());
                // the destination step goes back out of the rule: the lookup names both
                sets.push(SetSpec { name, v4, timeout: false, members: Vec::new(), ranges: Vec::new(), concat: el });
                out.push(r);
            } else {
                for (k, _) in part {
                    out.push(rules[*k].clone());
                }
            }
        }
        i = j;
    }
    out
}

#[cfg(test)]
#[path = "outbound_tests.rs"]
mod tests;
