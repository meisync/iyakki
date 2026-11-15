//! Outbound-only network for a Linux-apps sandbox (F-LINAPP-13): the policy, the rule set and the plan, as pure functions.
//!
//! Today "network on" shares the host's network namespace, so a sandbox can reach services on the machine's loopback. The outbound-only
//! mode gives the sandbox its own network namespace joined to the host by a virtual pair of interfaces (`veth`), with address translation
//! out and a rule set that
//!
//! * never lets the sandbox reach the host itself (loopback, the host's own addresses, the pair's host end) or link-local addresses;
//! * by default does not reach private networks either (the person's LAN, the dev cluster) unless the allow-list names them;
//! * with an allow-list, reaches only what is listed (a name resolves when the rules are made).
//!
//! Everything here is typed values and numbers, so it is tested on any machine. Applying it (creating the pair, loading the table through the
//! Firewall's own netlink writer, moving one end into the sandbox's namespace) is the Linux-only step in `linuxapps_init.rs`; this module
//! produces the exact `ip` steps ([`setup_steps`], [`teardown_steps`]) and the table ([`table_spec`]). The policy is stored beside the sandbox and fails closed: a missing or damaged
//! file means [`Mode::Off`].

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;

use iyakki_fw::nl::tablegen::TableSpec;
use serde::{Deserialize, Serialize};

/// How the sandbox is connected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// An empty network (only `lo`). The default.
    #[default]
    Off,
    /// The host's network namespace (today's "on"). Can reach the host's loopback services.
    HostShared,
    /// Its own namespace, address translation out, the host unreachable.
    OutboundOnly,
}

impl Mode {
    pub fn id(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::HostShared => "host_shared",
            Mode::OutboundOnly => "outbound_only",
        }
    }
    pub fn parse(s: &str) -> Option<Mode> {
        [Mode::Off, Mode::HostShared, Mode::OutboundOnly].into_iter().find(|m| m.id() == s.trim())
    }
    /// The sentence the page shows.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Off => "Off: no network",
            Mode::HostShared => "On, shared with the computer: can reach any server and the computer's own services",
            Mode::OutboundOnly => "On, outbound only: reaches the internet, never the computer itself",
        }
    }
    /// The word on the page's button.
    pub fn short(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::HostShared => "shared with the computer",
            Mode::OutboundOnly => "outbound only",
        }
    }
    /// Does a sandbox in this mode still count as able to reach "any server" for the permission check? Only the shared mode is also able
    /// to reach the host; outbound only without an allow-list reaches any public server; with one, only the list.
    pub fn reaches_host(self) -> bool {
        self == Mode::HostShared
    }
}

/// Where an allow-list entry points.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    Net { addr: IpAddr, prefix: u8 },
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allow {
    pub target: Target,
    /// `None`: any port.
    pub port: Option<u16>,
}

impl Allow {
    /// `203.0.113.0/24`, `203.0.113.7:443`, `example.org`, `example.org:443`, `[2001:db8::1]:443`, `2001:db8::/32`.
    pub fn parse(s: &str) -> Result<Allow, String> {
        let s = s.trim();
        if s.is_empty() || s.len() > 253 || s.contains(char::is_whitespace) {
            return Err(format!("not an allow-list entry: `{s}`"));
        }
        let (host, port) = if let Some(rest) = s.strip_prefix('[') {
            let (h, after) = rest.split_once(']').ok_or_else(|| format!("missing `]` in `{s}`"))?;
            (h.to_string(), after.strip_prefix(':').map(str::to_string))
        } else if s.matches(':').count() > 1 {
            (s.to_string(), None) // a bare IPv6 address or prefix
        } else if let Some((h, p)) = s.rsplit_once(':') {
            (h.to_string(), Some(p.to_string()))
        } else {
            (s.to_string(), None)
        };
        let port = match port {
            None => None,
            Some(p) => Some(p.parse::<u16>().ok().filter(|p| *p != 0).ok_or_else(|| format!("`{p}` is not a port"))?),
        };
        if let Some((a, p)) = host.split_once('/') {
            let addr: IpAddr = a.parse().map_err(|_| format!("`{a}` is not an address"))?;
            let max = if addr.is_ipv4() { 32 } else { 128 };
            let prefix: u8 = p.parse().ok().filter(|p| *p <= max).ok_or_else(|| format!("`{p}` is not a prefix length"))?;
            if prefix == 0 {
                return Err("a prefix of /0 allows everything; leave the allow-list empty for that".into());
            }
            return Ok(Allow { target: Target::Net { addr: mask(addr, prefix), prefix }, port });
        }
        if let Ok(addr) = host.parse::<IpAddr>() {
            return Ok(Allow { target: Target::Net { addr, prefix: if addr.is_ipv4() { 32 } else { 128 } }, port });
        }
        let name = host.to_ascii_lowercase();
        let label_ok = |l: &str| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !name.contains('.') || !name.split('.').all(label_ok) || name.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return Err(format!("`{host}` is not an address, a prefix or a host name"));
        }
        Ok(Allow { target: Target::Name(name), port })
    }

    pub fn text(&self) -> String {
        let port = self.port.map(|p| format!(":{p}")).unwrap_or_default();
        match &self.target {
            Target::Net { addr, prefix } if *prefix == if addr.is_ipv4() { 32 } else { 128 } => match addr {
                IpAddr::V6(_) if self.port.is_some() => format!("[{addr}]{port}"),
                _ => format!("{addr}{port}"),
            },
            Target::Net { addr, prefix } => match addr {
                IpAddr::V6(_) if self.port.is_some() => format!("[{addr}/{prefix}]{port}"),
                _ => format!("{addr}/{prefix}{port}"),
            },
            Target::Name(n) => format!("{n}{port}"),
        }
    }
}

/// The policy of one sandbox.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NetPolicy {
    pub mode: Mode,
    /// Empty: any public address (private networks stay blocked). Otherwise only these.
    pub allow: Vec<Allow>,
    /// Name servers to use when the host's resolver file lists only loopback ones.
    pub dns: Vec<IpAddr>,
    /// The sandbox's slot (its pair of interfaces and its /30), chosen when outbound-only is switched on.
    #[serde(default)]
    pub slot: u16,
}

impl NetPolicy {
    pub fn parse_allow(entries: &[String]) -> Result<Vec<Allow>, String> {
        let mut v: Vec<Allow> = Vec::new();
        for e in entries {
            let a = Allow::parse(e)?;
            if !v.contains(&a) {
                v.push(a);
            }
        }
        if v.len() > 64 {
            return Err("at most 64 allow-list entries".into());
        }
        Ok(v)
    }

    /// Does this policy need the "reach any server" permission? Shared mode, or outbound only with an empty allow-list.
    pub fn reaches_any_server(&self) -> bool {
        match self.mode {
            Mode::Off => false,
            Mode::HostShared => true,
            Mode::OutboundOnly => self.allow.is_empty(),
        }
    }

    /// Stored beside the sandbox. A missing or damaged file reads as off.
    pub fn load(path: &Path) -> NetPolicy {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<NetPolicy>(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let tmp = path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("{}: {e}", path.display()))
    }
}

fn mask(addr: IpAddr, prefix: u8) -> IpAddr {
    match addr {
        IpAddr::V4(a) => {
            let m = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix as u32) };
            IpAddr::V4(Ipv4Addr::from(u32::from(a) & m))
        }
        IpAddr::V6(a) => {
            let m = if prefix == 0 { 0 } else { u128::MAX << (128 - prefix as u32) };
            IpAddr::V6(Ipv6Addr::from(u128::from(a) & m))
        }
    }
}

fn in_net(ip: IpAddr, net: IpAddr, prefix: u8) -> bool {
    match (ip, net) {
        (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_)) => mask(ip, prefix) == mask(net, prefix),
        _ => false,
    }
}

/// Why a destination is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deny {
    /// Loopback, unspecified, multicast, link-local or one of the host's own addresses: never reachable.
    Host,
    /// A private network and the allow-list does not name it.
    Private,
    /// An allow-list exists and does not list it.
    NotListed,
}

fn is_hard_blocked(ip: IpAddr, host_addrs: &[IpAddr]) -> bool {
    host_addrs.contains(&ip)
        || match ip {
            IpAddr::V4(a) => a.is_loopback() || a.is_unspecified() || a.is_multicast() || a.is_link_local() || a.is_broadcast() || a.octets()[0] == 0,
            IpAddr::V6(a) => a.is_loopback() || a.is_unspecified() || a.is_multicast() || (a.segments()[0] & 0xffc0) == 0xfe80,
        }
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            a.is_private() || (o[0] == 100 && (64..128).contains(&o[1])) /* CGNAT, tailnets */
        }
        IpAddr::V6(a) => (a.segments()[0] & 0xfe00) == 0xfc00, // unique local
    }
}

/// The rules in one function: may the sandbox send to `ip:port`? `resolved` holds the addresses the allow-list names resolved to when the
/// rule set was made. This is the model the rule text is generated from; the two are tested against each other.
pub fn decide(policy: &NetPolicy, host_addrs: &[IpAddr], resolved: &[(String, Vec<IpAddr>)], ip: IpAddr, port: u16) -> Result<(), Deny> {
    if policy.mode != Mode::OutboundOnly {
        return Err(Deny::Host);
    }
    if is_hard_blocked(ip, host_addrs) {
        return Err(Deny::Host);
    }
    let listed = policy.allow.iter().any(|a| {
        let port_ok = a.port.is_none_or(|p| p == port);
        port_ok
            && match &a.target {
                Target::Net { addr, prefix } => in_net(ip, *addr, *prefix),
                Target::Name(n) => resolved.iter().any(|(rn, ips)| rn == n && ips.contains(&ip)),
            }
    });
    if policy.allow.is_empty() {
        return if is_private(ip) { Err(Deny::Private) } else { Ok(()) };
    }
    if listed {
        Ok(())
    } else if is_private(ip) {
        Err(Deny::Private)
    } else {
        Err(Deny::NotListed)
    }
}

/// The addresses of the virtual pair. One `/30` per sandbox out of `10.213.0.0/16`: the host end `.1`, the sandbox `.2`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Veth {
    pub slot: u16,
    pub host_if: String,
    pub sandbox_if: String,
    pub host_ip: Ipv4Addr,
    pub sandbox_ip: Ipv4Addr,
}

impl Veth {
    pub const SLOTS: u32 = 16384;

    pub fn for_slot(slot: u16) -> Veth {
        let n = (slot as u32) * 4;
        Veth {
            slot,
            host_if: format!("ivh{slot:04x}"),
            sandbox_if: format!("ivs{slot:04x}"),
            host_ip: Ipv4Addr::new(10, 213, (n >> 8) as u8, (n & 0xff) as u8 + 1),
            sandbox_ip: Ipv4Addr::new(10, 213, (n >> 8) as u8, (n & 0xff) as u8 + 2),
        }
    }

    pub fn subnet(&self) -> String {
        let o = self.host_ip.octets();
        format!("10.213.{}.{}/30", o[2], o[3] - 1)
    }
}

/// A slot for a sandbox: a number from its name, moved on past the slots in use. `None` when every slot is taken.
pub fn pick_slot(name: &str, used: &[u16]) -> Option<u16> {
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    (0..Veth::SLOTS).map(|i| ((h + i) % Veth::SLOTS) as u16).find(|s| !used.contains(s))
}

/// The table of one sandbox: `inet iyakki_sbx_<slot>`.
pub fn table_name(veth: &Veth) -> String {
    format!("iyakki_sbx_{:04x}", veth.slot)
}

/// Networks that are never reachable from a sandbox (besides the computer's own addresses): this host, loopback, link-local, multicast.
pub const NEVER_V4: [([u8; 4], u8); 5] = [([127, 0, 0, 0], 8), ([0, 0, 0, 0], 8), ([169, 254, 0, 0], 16), ([224, 0, 0, 0], 4), ([255, 255, 255, 255], 32)];
/// Private ranges blocked unless the allow-list names them (the person's network, tailnets, the dev cluster).
pub const PRIVATE_V4: [([u8; 4], u8); 4] = [([10, 0, 0, 0], 8), ([172, 16, 0, 0], 12), ([192, 168, 0, 0], 16), ([100, 64, 0, 0], 10)];

fn full(ip: IpAddr) -> u8 {
    if ip.is_ipv4() { 32 } else { 128 }
}

/// The rules for one sandbox, as the typed table the Firewall's own netlink writer takes (the images ship no `nft` program; this is
/// the same route the VPN guard and internet sharing use). `resolved` holds what the allow-list's names resolved to now.
/// DNS to the sandbox's name servers is allowed whatever the list says. Pure; a test evaluates the rules and compares every verdict
/// with [`decide`].
pub fn table_spec(policy: &NetPolicy, veth: &Veth, host_addrs: &[IpAddr], resolved: &[(String, Vec<IpAddr>)], nameservers: &[IpAddr]) -> Result<TableSpec, String> {
    use iyakki_fw::nl::tablegen::{ChainKind, ChainSpec, Expr, Hook, IfName, CT_ESTABLISHED_RELATED};
    if policy.mode != Mode::OutboundOnly {
        return Err("the rule set is only for the outbound-only mode".into());
    }
    let down = || IfName::Exact(veth.host_if.clone());
    let mut fwd: Vec<Vec<Expr>> = vec![
        vec![Expr::OifIs(down()), Expr::CtState(CT_ESTABLISHED_RELATED), Expr::Accept],
        vec![Expr::OifIs(down()), Expr::Drop],
        vec![Expr::IifIs(down()), Expr::CtState(CT_ESTABLISHED_RELATED), Expr::Accept],
    ];
    for (a, bits) in NEVER_V4 {
        fwd.push(vec![Expr::IifIs(down()), Expr::Daddr(IpAddr::V4(a.into()), bits), Expr::Drop]);
    }
    for (a, bits) in [(Ipv6Addr::LOCALHOST, 128u8), (Ipv6Addr::UNSPECIFIED, 128), (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10), (Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8)] {
        fwd.push(vec![Expr::IifIs(down()), Expr::Daddr(IpAddr::V6(a), bits), Expr::Drop]);
    }
    for ip in host_addrs {
        fwd.push(vec![Expr::IifIs(down()), Expr::Daddr(*ip, full(*ip)), Expr::Drop]);
    }
    for ns in nameservers.iter().filter(|n| !is_hard_blocked(**n, host_addrs)) {
        for proto in [17u8, 6] {
            fwd.push(vec![Expr::IifIs(down()), Expr::Daddr(*ns, full(*ns)), Expr::L4(proto), Expr::Dport(53), Expr::Accept]);
        }
    }
    let allow_rule = |ip: IpAddr, prefix: u8, port: Option<u16>, fwd: &mut Vec<Vec<Expr>>| match port {
        Some(p) => {
            for proto in [6u8, 17] {
                fwd.push(vec![Expr::IifIs(down()), Expr::Daddr(ip, prefix), Expr::L4(proto), Expr::Dport(p), Expr::Accept]);
            }
        }
        None => fwd.push(vec![Expr::IifIs(down()), Expr::Daddr(ip, prefix), Expr::Accept]),
    };
    if policy.allow.is_empty() {
        for (a, bits) in PRIVATE_V4 {
            fwd.push(vec![Expr::IifIs(down()), Expr::Daddr(IpAddr::V4(a.into()), bits), Expr::Drop]);
        }
        fwd.push(vec![Expr::IifIs(down()), Expr::Daddr(IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0)), 7), Expr::Drop]);
        fwd.push(vec![Expr::IifIs(down()), Expr::Accept]);
    } else {
        for a in &policy.allow {
            match &a.target {
                // a block that starts inside the never-reachable set would only add a rule the drops above already beat
                Target::Net { addr, .. } if is_hard_blocked(*addr, host_addrs) => {}
                Target::Net { addr, prefix } => allow_rule(*addr, *prefix, a.port, &mut fwd),
                Target::Name(n) => {
                    let ips = resolved.iter().find(|(rn, _)| rn == n).map(|(_, v)| v.as_slice()).unwrap_or(&[]);
                    for ip in ips.iter().filter(|ip| !is_hard_blocked(**ip, host_addrs)) {
                        allow_rule(*ip, full(*ip), a.port, &mut fwd);
                    }
                }
            }
        }
        fwd.push(vec![Expr::IifIs(down()), Expr::Drop]);
    }
    let spec = TableSpec {
        name: table_name(veth),
        sets: vec![],
        chains: vec![
            // nothing from the sandbox reaches the computer's own services
            ChainSpec { name: "input".into(), kind: ChainKind::Filter, hook: Hook::Input, priority: -10, policy_accept: true, rules: vec![vec![Expr::IifIs(down()), Expr::Drop]] },
            ChainSpec { name: "forward".into(), kind: ChainKind::Filter, hook: Hook::Forward, priority: -10, policy_accept: true, rules: fwd },
            ChainSpec { name: "postrouting".into(), kind: ChainKind::Nat, hook: Hook::Postrouting, priority: 100, policy_accept: true, rules: vec![vec![Expr::Saddr(IpAddr::V4(veth.sandbox_ip), 30), Expr::OifNot(down()), Expr::Masquerade]] },
        ],
    };
    spec.check()?;
    Ok(spec)
}

/// One command of the set-up: run in the computer's network namespace, or inside the sandbox's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub inside: bool,
    pub argv: Vec<String>,
}

/// Joining the sandbox's namespace (the process `pid`) to the computer: make the pair, move one end in, address both, route out.
/// The executor runs `inside` steps with its own namespace switched to the sandbox's (`setns`), then back; only `ip` is needed.
pub fn setup_steps(veth: &Veth, pid: u32) -> Vec<Step> {
    let s = |inside: bool, a: &[&str]| Step { inside, argv: a.iter().map(|x| x.to_string()).collect() };
    let (hi, si, p) = (veth.host_if.as_str(), veth.sandbox_if.as_str(), pid.to_string());
    vec![
        s(false, &["ip", "link", "add", hi, "type", "veth", "peer", "name", si]),
        s(false, &["ip", "link", "set", si, "netns", &p]),
        s(false, &["ip", "addr", "add", &format!("{}/30", veth.host_ip), "dev", hi]),
        s(false, &["ip", "link", "set", hi, "up"]),
        s(true, &["ip", "addr", "add", &format!("{}/30", veth.sandbox_ip), "dev", si]),
        s(true, &["ip", "link", "set", si, "up"]),
        s(true, &["ip", "link", "set", "lo", "up"]),
        s(true, &["ip", "route", "add", "default", "via", &veth.host_ip.to_string()]),
    ]
}

/// Taking the pair down again (the other end goes with it); the table is removed through the netlink writer.
pub fn teardown_steps(veth: &Veth) -> Vec<Step> {
    vec![Step { inside: false, argv: vec!["ip".into(), "link".into(), "delete".into(), veth.host_if.clone()] }]
}

/// The addresses of the computer's own interfaces (what a sandbox must never reach). Empty if the system call fails.
#[cfg(unix)]
pub fn local_addresses() -> Vec<IpAddr> {
    let mut out = Vec::new();
    // SAFETY: getifaddrs fills a list that is only read, each pointer checked for null, and then freed with freeifaddrs.
    unsafe {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return out;
        }
        let mut cur = head;
        while !cur.is_null() {
            let a = (*cur).ifa_addr;
            if !a.is_null() {
                match i32::from((*a).sa_family) {
                    libc::AF_INET => {
                        let sin = a as *const libc::sockaddr_in;
                        out.push(IpAddr::V4(Ipv4Addr::from(u32::from_be((*sin).sin_addr.s_addr))));
                    }
                    libc::AF_INET6 => {
                        let sin6 = a as *const libc::sockaddr_in6;
                        out.push(IpAddr::V6(Ipv6Addr::from((*sin6).sin6_addr.s6_addr)));
                    }
                    _ => {}
                }
            }
            cur = (*cur).ifa_next;
        }
        libc::freeifaddrs(head);
    }
    out
}

/// The names of the computer's network interfaces (`getifaddrs`), each once.
#[cfg(unix)]
pub fn interface_names() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // SAFETY: getifaddrs fills a list that is only read, each pointer checked for null, and then freed with freeifaddrs.
    unsafe {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return out;
        }
        let mut cur = head;
        while !cur.is_null() {
            if !(*cur).ifa_name.is_null() {
                let n = std::ffi::CStr::from_ptr((*cur).ifa_name).to_string_lossy().into_owned();
                if !out.contains(&n) {
                    out.push(n);
                }
            }
            cur = (*cur).ifa_next;
        }
        libc::freeifaddrs(head);
    }
    out
}

/// Look the allow-list's names up now. A name that does not resolve gives no addresses (and so no access): it fails closed.
pub fn resolve_names(policy: &NetPolicy, lookup: &dyn Fn(&str) -> Vec<IpAddr>) -> Vec<(String, Vec<IpAddr>)> {
    policy.allow.iter().filter_map(|a| if let Target::Name(n) = &a.target { Some(n.clone()) } else { None }).collect::<std::collections::BTreeSet<_>>().into_iter().map(|n| {
        let ips = lookup(&n);
        (n, ips)
    }).collect()
}

/// The system's name lookup (`getaddrinfo`).
pub fn system_lookup(name: &str) -> Vec<IpAddr> {
    use std::net::ToSocketAddrs;
    (name, 0).to_socket_addrs().map(|i| i.map(|s| s.ip()).collect()).unwrap_or_default()
}

/// Does the account's host allow-list (`net_hosts`, `*` = every host) cover this policy? Open outbound, a network block or the shared mode
/// need `*`; a list of names needs each name listed.
pub fn host_gate(policy: &NetPolicy, net_hosts: &[String]) -> Result<(), String> {
    if net_hosts.iter().any(|h| h == "*") {
        return Ok(());
    }
    match policy.mode {
        Mode::Off => Ok(()),
        Mode::HostShared => Err("a sandbox sharing the computer's network can reach any server; only an account allowed every host may do that".into()),
        Mode::OutboundOnly if policy.allow.is_empty() => Err("outbound without an allow-list reaches any public server; only an account allowed every host may do that. Give an allow-list of host names".into()),
        Mode::OutboundOnly => {
            for a in &policy.allow {
                match &a.target {
                    Target::Name(n) if net_hosts.iter().any(|h| h == n) => {}
                    other => return Err(format!("{} is not in this account's list of allowed hosts", match other { Target::Name(n) => n.clone(), _ => a.text() })),
                }
            }
            Ok(())
        }
    }
}

/// The sandbox's `/etc/resolv.conf`: the host's name servers without the loopback ones (the host's stub resolver is not reachable from
/// its own namespace), else the policy's. `Err` when there is none left, so the person is told rather than left with a dead resolver.
pub fn resolv_for(host_resolv: &str, policy: &NetPolicy) -> Result<(String, Vec<IpAddr>), String> {
    let mut ns: Vec<IpAddr> = host_resolv
        .lines()
        .filter_map(|l| l.trim().strip_prefix("nameserver"))
        .filter_map(|r| r.trim().split('%').next().and_then(|a| a.parse::<IpAddr>().ok()))
        .filter(|a| !a.is_loopback() && !a.is_unspecified())
        .collect();
    if ns.is_empty() {
        ns = policy.dns.iter().copied().filter(|a| !a.is_loopback() && !a.is_unspecified()).collect();
    }
    if ns.is_empty() {
        return Err("the computer's resolver is a local one and no name server is set for this sandbox; add one (for example 9.9.9.9) to its network settings".into());
    }
    ns.truncate(3);
    let text = ns.iter().map(|a| format!("nameserver {a}\n")).collect::<String>();
    Ok((text, ns))
}

/// The lines the page and the audit log show for a policy.
pub fn describe(p: &NetPolicy) -> Vec<String> {
    let mut v = vec![p.mode.label().to_string()];
    if p.mode == Mode::OutboundOnly {
        if p.allow.is_empty() {
            v.push("Allowed: any public address; private networks and the computer itself are blocked".into());
        } else {
            v.push(format!("Allowed only: {}", p.allow.iter().map(Allow::text).collect::<Vec<_>>().join(", ")));
        }
    }
    v
}


// ------------------------------------------------------------------------------------------------ the guest self-test

/// The pieces of `iyakki.selftest=sandboxnet` (run in a QEMU guest as root; `dist/qemu/README.md`). It proves, on a real kernel, what the unit
/// tests cannot: the table is loaded, the pair and routes exist, an allowed destination is reached through the translation, a destination
/// that is not on the list and the computer's own address are refused, and tearing down leaves no pair, no table and the old forwarding
/// value. The network layer is exercised with a throw-away process in a new network namespace (no distribution root is needed); the
/// sandbox's file-system side is covered by the existing real-sandbox tests.
#[cfg(target_os = "linux")]
pub mod selftest {
    use super::*;
    use crate::qprintln;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    pub const SUBCOMMAND: &str = "selftest-netprobe";

    /// The probe process: new network namespace, say `ready`, wait for `go` on stdin, try each `label=address` and print `probe <label> ok|fail`.
    pub fn probe_main(args: &[String]) -> i32 {
        // SAFETY: unshare takes only the flag; this process has not started other threads.
        if unsafe { libc::unshare(libc::CLONE_NEWNET) } != 0 {
            qprintln!("error unshare: {}", std::io::Error::last_os_error());
            return 2;
        }
        qprintln!("ready");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        for a in args {
            let Some((label, addr)) = a.split_once('=') else { continue };
            let ok = addr.parse::<std::net::SocketAddr>().is_ok_and(|s| std::net::TcpStream::connect_timeout(&s, Duration::from_secs(3)).is_ok());
            qprintln!("probe {label} {}", if ok { "ok" } else { "fail" });
        }
        let _ = std::io::stdout().flush();
        0
    }

    fn table_there(name: &str) -> bool {
        iyakki_fw::nl::NfSocket::open(5).and_then(|mut s| s.table_exists(name)).unwrap_or(false)
    }

    /// Run the whole check. `allow` and `deny` are TCP ports a listener on the guest's gateway address (10.0.2.2, QEMU's user network) answers.
    /// Returns `(name, passed, detail)`.
    pub fn run(allow: u16, deny: u16) -> Vec<(String, bool, String)> {
        let mut out: Vec<(String, bool, String)> = Vec::new();
        let mut add = |n: &str, ok: bool, d: String| out.push((n.to_string(), ok, d));
        let Ok(exe) = std::env::current_exe() else { return vec![("start".into(), false, "no executable path".into())] };
        let listener = std::net::TcpListener::bind("0.0.0.0:0");
        let Ok(listener) = listener else { return vec![("host-listener".into(), false, "could not listen".into())] };
        let _ = listener.set_nonblocking(true);
        let lport = listener.local_addr().map(|a| a.port()).unwrap_or(0);

        let slot = pick_slot("selftest", &[]).unwrap_or(7);
        let veth = Veth::for_slot(slot);
        let policy = NetPolicy { mode: Mode::OutboundOnly, allow: NetPolicy::parse_allow(&[format!("10.0.2.2:{allow}")]).unwrap_or_default(), slot, ..Default::default() };
        let before_forward = crate::sharing::real::ip_forward_get().unwrap_or_default();
        let before_ifs = interface_names();
        let table = table_name(&veth);
        add("clean-start", !table_there(&table) && !before_ifs.iter().any(|n| n == &veth.host_if), format!("forward={before_forward}"));

        let Ok(spec_table) = table_spec(&policy, &veth, &local_addresses(), &[], &[]) else { return vec![("table-spec".into(), false, "could not build the table".into())] };
        let spec_path = format!("/run/iyakki-selftest-net-{slot}.json");
        if std::fs::write(&spec_path, serde_json::to_string(&spec_table).unwrap_or_default()).is_err() {
            return vec![("table-spec".into(), false, "could not write the rules".into())];
        }
        let mut spec = crate::linuxapps_init::Spec::new("/", "selftest", vec!["x".into()]);
        spec.veth = Some(slot);
        spec.net_spec = spec_path.clone();
        spec.net_resolv = "/run/iyakki-selftest-resolv".into();

        let targets = [format!("allowed=10.0.2.2:{allow}"), format!("denied=10.0.2.2:{deny}"), format!("host={}:{lport}", veth.host_ip)];
        let child = Command::new(exe).arg(SUBCOMMAND).args(&targets).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
        let Ok(mut child) = child else { return vec![("probe-start".into(), false, "could not start the probe".into())] };
        let mut reader = BufReader::new(child.stdout.take().expect("piped"));
        let mut first = String::new();
        let _ = reader.read_line(&mut first);
        add("probe-namespace", first.trim() == "ready", first.trim().to_string());
        let pid = child.id() as i32;

        let up = crate::linuxapps_init::net_up(&spec, pid);
        add("net-up", up.is_ok(), up.as_ref().err().cloned().unwrap_or_else(|| "ok".into()));
        let forward_before = up.ok().flatten();
        add("table-loaded", table_there(&table), table.clone());
        add("veth-host-end", interface_names().iter().any(|n| n == &veth.host_if), veth.host_if.clone());
        add("forwarding-on", crate::sharing::real::ip_forward_get().as_deref() == Ok("1"), "ip_forward=1".into());

        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(b"go\n");
            let _ = stdin.flush();
        }
        let mut seen: Vec<(String, bool)> = Vec::new();
        for l in reader.lines().map_while(Result::ok) {
            if let Some(rest) = l.strip_prefix("probe ") {
                let mut it = rest.split_whitespace();
                if let (Some(label), Some(v)) = (it.next(), it.next()) {
                    seen.push((label.to_string(), v == "ok"));
                }
            }
        }
        let got = |label: &str| seen.iter().find(|s| s.0 == label).map(|s| s.1);
        add("allowed-reachable", got("allowed") == Some(true), format!("10.0.2.2:{allow} -> {:?}", got("allowed")));
        add("not-listed-refused", got("denied") == Some(false), format!("10.0.2.2:{deny} -> {:?}", got("denied")));
        add("host-unreachable", got("host") == Some(false) && listener.accept().is_err(), format!("{}:{lport} -> {:?}", veth.host_ip, got("host")));
        let _ = child.wait();

        net_down_and_check(&mut add, &spec, forward_before, &before_forward, &veth, &table);
        let _ = std::fs::remove_file(spec_path);
        out
    }

    fn net_down_and_check(add: &mut dyn FnMut(&str, bool, String), spec: &crate::linuxapps_init::Spec, forward_before: Option<String>, before_forward: &str, veth: &Veth, table: &str) {
        crate::linuxapps_init::net_down(spec, forward_before);
        add("teardown-table", !table_there(table), table.to_string());
        add("teardown-veth", !interface_names().iter().any(|n| n.starts_with("ivh")), "no ivh interface left".into());
        let now = crate::sharing::real::ip_forward_get().unwrap_or_default();
        add("teardown-forwarding", now == before_forward, format!("before={before_forward} now={now}"));
        let _ = veth;
    }
}

#[cfg(test)]
#[path = "linuxapps_net_tests.rs"]
mod tests;
