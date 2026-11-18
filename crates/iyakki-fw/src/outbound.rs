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

#[cfg(test)]
#[path = "outbound_tests.rs"]
mod tests;
